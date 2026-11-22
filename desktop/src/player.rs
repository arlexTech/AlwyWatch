// SPDX-License-Identifier: AGPL-3.0-or-later
//! Playback session: one mpv instance driven from a dedicated thread, drawn inside AlwyWatch's
//! window with the app's own controls on top (see `video.rs`); in its own mpv window when the
//! app has no OpenGL. Progress is reported back through a callback, what the controls show
//! through `PlayRequest::view`.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use alwywatch_core::{
    HttpRequest, PlaybackEvent, Segment, SegmentKind, TrackInfo, TrackKind, TrackPref, TrackRules,
    choose_tracks, pick_track, playback_prompt,
};

use crate::mpv::{self, Mpv};

pub struct PlayRequest {
    pub media: HttpRequest,
    pub start_ms: u64,
    pub title: String,
    pub external_subs: Vec<ExternalSub>,
    /// Audio/subtitle choice for this title (shared with Android, see the core's `tracks`).
    pub tracks: TrackSetup,
    pub spdif: bool,
    /// Skippable intro/credits (from the server), for the skip and next-episode prompts.
    pub segments: Vec<Segment>,
    pub has_next: bool,
    pub has_previous: bool,
    pub auto_skip_intro: bool,
    pub auto_next: bool,
    /// Watching together: the room's play/pause/seek come in, ours go out.
    pub together: Option<TogetherLink>,
    /// Short messages shown on the video whatever plays (an invitation arrived).
    pub notes: Option<mpsc::Receiver<String>>,
    /// Draw inside the app's window and send what the controls show here; None: mpv's own
    /// window (also the fallback when the window can't host the video).
    pub view: Option<ViewLink>,
}

/// The in-window player's link to the app.
pub struct ViewLink {
    /// Called with the mpv instance once the video is in the window (the controls send their
    /// commands to it directly; mpv's client API is thread-safe).
    pub attached: Box<dyn Fn(Arc<Mpv>) + Send>,
    /// What the controls show, sent when it changes.
    pub show: Box<dyn Fn(PlayerView) + Send>,
}

/// What the in-window controls show.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerView {
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Buffered up to here.
    pub cache_ms: u64,
    pub paused: bool,
    /// "Skip intro" (empty: no button).
    pub skip: String,
    /// Seconds before the next episode starts (0: no countdown).
    pub countdown: u32,
    /// A short message ("Sam paused"), empty when none.
    pub note: String,
    pub has_next: bool,
    pub has_previous: bool,
    pub audio: Vec<TrackItem>,
    pub subtitles: Vec<TrackItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackItem {
    /// mpv's id ("no" for subtitles off).
    pub id: String,
    pub label: String,
    pub selected: bool,
}

/// From the app to a player in a watch together room.
pub enum ToPlayer {
    /// The room plays or is paused, at `position_ms` at the instant `at` (which may be a moment
    /// ahead: a start everyone presses play for at once). The player follows it continuously
    /// (slightly faster or slower for small drifts, a seek past 2 s: `together_correction`).
    /// `hold`: the room waits for players to load the title (stay paused at the position);
    /// `load`: this one too (say [`FromPlayer::Ready`] once enough is buffered); `waiting`:
    /// what to show meanwhile ("Waiting for Sam…").
    Sync {
        playing: bool,
        position_ms: u64,
        at: Instant,
        hold: bool,
        load: bool,
        waiting: Option<String>,
    },
    /// A short message on the video ("Sam paused").
    Note(String),
    /// Close the player (the room moved to a title found elsewhere).
    Quit,
}

/// From a player in a room to the app.
pub enum FromPlayer {
    /// The viewer played, paused or moved.
    User { playing: bool, position_ms: u64 },
    /// The title is loaded at the room's position (`READY_BUFFER_MS` ahead, or to its end).
    Ready,
}

/// What a player in a room last heard from it.
struct RoomSync {
    playing: bool,
    position_ms: u64,
    at: Instant,
    hold: bool,
    load: bool,
    waiting: Option<String>,
}

pub struct TogetherLink {
    pub rx: mpsc::Receiver<ToPlayer>,
    pub send: Box<dyn Fn(FromPlayer) + Send>,
}

/// A subtitle file next to the media, added with its language so the choice can find it.
pub struct ExternalSub {
    pub url: String,
    pub title: Option<String>,
    pub language: Option<String>,
}

pub struct TrackSetup {
    pub pref: Option<TrackPref>,
    pub rules: TrackRules,
    /// Called when the viewer switches audio or subtitles: the movie's or show's new choice.
    pub save: Box<dyn Fn(TrackPref) + Send>,
}

/// Where to go after this one, when the viewer asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Go {
    Next,
    Previous,
}

/// Seconds of the credits countdown before the next episode starts by itself.
const COUNTDOWN: Duration = Duration::from_secs(10);

pub struct Progress {
    pub event: PlaybackEvent,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    /// Set on the final Stop when the file played to the end.
    pub finished: bool,
    /// Set on the final Stop when the viewer asked for the next/previous episode.
    pub go: Option<Go>,
    /// Set on the final Stop when playback could not start or the file could not be played.
    pub error: Option<String>,
}

/// Starts playback on a new thread. `report` is called for start, every 10 s, pause and stop.
pub fn spawn(req: PlayRequest, report: impl Fn(Progress) + Send + 'static) -> Result<(), String> {
    if !mpv::available() {
        return Err(
            "libmpv is not installed. Install mpv (Linux: your distribution's mpv/libmpv package) \
             or put libmpv-2.dll next to AlwyWatch.exe (Windows)."
                .into(),
        );
    }
    std::thread::Builder::new()
        .name("mpv".into())
        .spawn(move || {
            let start_ms = req.start_ms;
            if let Err(e) = run(req, &report) {
                eprintln!("playback failed: {e}");
                report(Progress {
                    event: PlaybackEvent::Stop,
                    position_ms: start_ms,
                    duration_ms: None,
                    finished: false,
                    go: None,
                    error: Some(format!("Playback failed: {e}")),
                });
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A configured, initialised mpv: in the app's window when `embed` and the window can host it.
fn open(req: &PlayRequest, embed: bool) -> Result<(Arc<Mpv>, bool), String> {
    let m = Arc::new(Mpv::new()?);
    let window: &[(&str, &str)] = if embed {
        // Drawn by the app (render API): no window, no mpv keys or on-screen controls.
        &[
            ("vo", "libmpv"),
            ("input-default-bindings", "no"),
            ("osc", "no"),
            ("osd-bar", "no"),
        ]
    } else {
        &[
            ("vo", "gpu-next"),
            ("force-window", "immediate"),
            ("fullscreen", "yes"),
            ("input-default-bindings", "yes"),
            ("input-vo-keyboard", "yes"),
            ("osc", "yes"),
        ]
    };
    for (k, v) in window {
        m.set_option(k, v)?;
    }
    // Video: hardware decoding when safe (VAAPI/NVDEC on Linux, D3D11VA on Windows) and a
    // bounded demuxer cache so RAM stays predictable for 4K remuxes.
    for (k, v) in [
        ("hwdec", "auto-safe"),
        ("keep-open", "no"),
        ("cache", "yes"),
        ("demuxer-max-bytes", "64MiB"),
        ("demuxer-max-back-bytes", "32MiB"),
        (
            "user-agent",
            concat!("AlwyWatch/", env!("CARGO_PKG_VERSION")),
        ),
        ("title", &req.title),
    ] {
        m.set_option(k, v)?;
    }
    if req.spdif {
        m.set_option("audio-spdif", "ac3,eac3,dts,dts-hd,truehd")?;
    }
    if req.start_ms > 0 {
        m.set_option("start", &format!("{:.3}", req.start_ms as f64 / 1000.0))?;
    }
    m.initialize()?;
    if embed && let Err(e) = crate::video::attach(m.clone()) {
        eprintln!("video in the window unavailable ({e}): using mpv's window");
        drop(m);
        return open(req, false);
    }
    Ok((m, embed))
}

fn run(req: PlayRequest, report: &dyn Fn(Progress)) -> Result<(), String> {
    let (m, embedded) = open(&req, req.view.is_some())?;
    let view = req.view.as_ref().filter(|_| embedded);
    if let Some(v) = view {
        (v.attached)(m.clone());
    }
    let mut shown = PlayerView::default();
    // One header per call: the Jellyfin header contains commas that list-splitting would break.
    // (`http-header-fields-append` is a command-line form only: libmpv rejects it as an option.)
    for h in &req.media.headers {
        m.command(&[
            "change-list",
            "http-header-fields",
            "append",
            &format!("{}: {}", h.name, h.value),
        ])?;
    }
    // Our keys, on top of mpv's defaults: S skips, > and < (mpv's playlist keys) and the media
    // keys change episode, C cancels the credits countdown.
    m.command(&[
        "define-section",
        "alwywatch",
        "s script-message alwy skip\nS script-message alwy skip\n\
         > script-message alwy next\nNEXT script-message alwy next\n\
         < script-message alwy previous\nPREV script-message alwy previous\n\
         c script-message alwy cancel",
        "force",
    ])?;
    m.command(&["enable-section", "alwywatch"])?;
    m.command(&["loadfile", &req.media.url])?;

    let snapshot = |event, finished| Progress {
        event,
        position_ms: m
            .get_f64("time-pos")
            .map(|s| (s.max(0.0) * 1000.0) as u64)
            .unwrap_or(0),
        duration_ms: m.get_f64("duration").map(|s| (s * 1000.0) as u64),
        finished,
        go: None,
        error: None,
    };
    let mut last = Instant::now();
    let mut paused = false;
    let mut last_pos = Progress {
        event: PlaybackEvent::Stop,
        position_ms: req.start_ms,
        duration_ms: None,
        finished: false,
        go: None,
        error: None,
    };
    let mut skipped: Vec<u64> = Vec::new();
    let mut countdown: Option<Instant> = None;
    let mut cancelled = false;
    let mut go: Option<Go> = None;
    let mut tracks = Tracks::default();
    let mut pref = req.tracks.pref.clone();
    let mut applied: (Option<String>, Option<String>) = (None, None);
    // Watching together: changes we make for the room are not the viewer's to send back.
    let together = req.together.as_ref();
    let mut quiet_until = Instant::now() + Duration::from_secs(3);
    let mut started = false;
    // The room's last state: followed continuously.
    let mut room: Option<RoomSync> = None;
    // Loading for the room: since when the player could start, and whether we said so.
    let mut loaded_since: Option<Instant> = None;
    let mut ready_sent = false;
    let mut next_check = Instant::now();
    let mut rate = 1.0_f64;
    let mut note: Option<(String, Instant)> = None;
    let send_user = |m: &Mpv| {
        if let Some(t) = together {
            t.send.as_ref()(FromPlayer::User {
                playing: !m.get_flag("pause").unwrap_or(false),
                position_ms: m
                    .get_f64("time-pos")
                    .map(|s| (s.max(0.0) * 1000.0) as u64)
                    .unwrap_or(0),
            });
        }
    };
    loop {
        // A start everyone presses play for at once: wake up right then.
        let until_start = room
            .as_ref()
            .filter(|r| r.playing && !r.hold)
            .and_then(|r| r.at.checked_duration_since(Instant::now()))
            .map(|d| d.as_secs_f64());
        let tick = if together.is_some() || view.is_some() {
            0.1
        } else {
            0.5
        };
        let event = m.wait_event(until_start.map_or(tick, |u| u.min(tick)));
        if let Some(t) = together {
            while let Ok(msg) = t.rx.try_recv() {
                match msg {
                    ToPlayer::Sync {
                        playing,
                        position_ms,
                        at,
                        hold,
                        load,
                        waiting,
                    } => {
                        // Paused until a start that's a moment ahead, or while loading.
                        let play = playing && !hold && Instant::now() >= at;
                        if paused == play {
                            quiet_until = Instant::now() + Duration::from_millis(1500);
                            let _ = m.command(&["set", "pause", if play { "no" } else { "yes" }]);
                            paused = !play;
                        }
                        if load && !room.as_ref().is_some_and(|r| r.load) {
                            ready_sent = false;
                            loaded_since = None;
                        }
                        if (hold || at > Instant::now()) && (rate - 1.0).abs() > 0.0001 {
                            rate = 1.0;
                            let _ = m.command(&["set", "speed", "1"]);
                        }
                        room = Some(RoomSync {
                            playing,
                            position_ms,
                            at,
                            hold,
                            load,
                            waiting,
                        });
                        next_check = Instant::now();
                    }
                    ToPlayer::Note(t) => note = Some((t, Instant::now())),
                    ToPlayer::Quit => {
                        let _ = m.command(&["quit"]);
                    }
                }
            }
        }
        // The start everyone heard: play right at its instant.
        if let Some(r) = &room
            && r.playing
            && !r.hold
            && paused
            && Instant::now() >= r.at
        {
            quiet_until = Instant::now() + Duration::from_millis(1500);
            let _ = m.command(&["set", "pause", "no"]);
            paused = false;
            next_check = Instant::now() + Duration::from_millis(500);
        }
        // Loading for the room: paused at its position until enough is buffered.
        if let (Some(r), Some(t)) = (&room, together)
            && (r.hold || Instant::now() < r.at)
        {
            let pos = m.get_f64("time-pos").map(|s| (s.max(0.0) * 1000.0) as u64);
            let seeking = m.get_flag("seeking").unwrap_or(true);
            if started
                && !seeking
                && Instant::now() >= next_check
                && pos.is_some_and(|p| p.abs_diff(r.position_ms) > 300)
            {
                quiet_until = Instant::now() + Duration::from_millis(1500);
                seek_to(&m, r.position_ms);
                next_check = Instant::now() + Duration::from_millis(500);
            }
            if r.load && !ready_sent {
                let here =
                    started && !seeking && pos.is_some_and(|p| p.abs_diff(r.position_ms) <= 500);
                if !here {
                    loaded_since = None;
                } else if loaded_since.is_none() {
                    loaded_since = Some(Instant::now());
                }
                let ahead = m.get_f64("demuxer-cache-duration").unwrap_or(0.0);
                let to_end = match (m.get_f64("demuxer-cache-time"), m.get_f64("duration")) {
                    (Some(c), Some(d)) => c >= d - 1.0,
                    _ => false,
                };
                let enough = ahead * 1000.0 >= alwywatch_core::together_ready_buffer_ms() as f64
                    || to_end
                    || loaded_since.is_some_and(|t| t.elapsed() > Duration::from_secs(8));
                if here && enough {
                    ready_sent = true;
                    t.send.as_ref()(FromPlayer::Ready);
                }
            }
        }
        if let Some(r) = &room
            && started
            && !r.hold
            && Instant::now() >= r.at
            // Only while playing/paused like the room (the room's echo of a change of ours
            // comes first).
            && paused != r.playing
            && Instant::now() >= next_check
        {
            let (playing, position_ms, at) = (r.playing, r.position_ms, r.at);
            next_check = Instant::now() + Duration::from_millis(500);
            let expected = position_ms
                + if playing {
                    at.elapsed().as_millis() as u64
                } else {
                    0
                };
            let actual = m
                .get_f64("time-pos")
                .map_or(expected, |s| (s.max(0.0) * 1000.0) as u64);
            // Passthrough audio can't change speed: seeks only.
            let c =
                alwywatch_core::together_correction(expected, actual, playing, rate, !req.spdif);
            if let Some(to) = c.seek_to_ms {
                quiet_until = Instant::now() + Duration::from_millis(1500);
                seek_to(&m, to);
                // Let the seek land (buffering) before judging again.
                next_check = Instant::now() + Duration::from_secs(2);
            }
            if (c.rate - rate).abs() > 0.0001 {
                rate = c.rate;
                let _ = m.command(&["set", "speed", &format!("{rate:.3}")]);
            }
        }
        if let Some(rx) = &req.notes {
            while let Ok(t) = rx.try_recv() {
                note = Some((t, Instant::now()));
            }
        }
        let reason = event.reason;
        match event.id {
            mpv::EVENT_FILE_LOADED => {
                for s in &req.external_subs {
                    let _ = m.command(&[
                        "sub-add",
                        &s.url,
                        "auto",
                        s.title.as_deref().unwrap_or(""),
                        s.language.as_deref().unwrap_or(""),
                    ]);
                }
                tracks = read_tracks(&m);
                let rules = TrackRules {
                    default_audio: selected(&m, "aid")
                        .and_then(|id| tracks.find(TrackKind::Audio, &id)),
                    ..req.tracks.rules.clone()
                };
                let choice = choose_tracks(pref.clone(), rules, tracks.infos.clone());
                if let Some(a) = choice.audio {
                    let _ = m.command(&["set", "aid", &tracks.mpv_ids[a as usize]]);
                }
                let sid = choice
                    .subtitle
                    .map_or("no", |s| tracks.mpv_ids[s as usize].as_str());
                let _ = m.command(&["set", "sid", sid]);
                applied = (selected(&m, "aid"), selected(&m, "sid"));
                report(snapshot(PlaybackEvent::Start, false));
            }
            // The first one is the start itself, not the viewer moving.
            mpv::EVENT_PLAYBACK_RESTART if !started => started = true,
            mpv::EVENT_PLAYBACK_RESTART if Instant::now() >= quiet_until => {
                send_user(&m);
                // Our own seek: the room hasn't echoed it yet, don't pull back meanwhile.
                next_check = Instant::now() + Duration::from_secs(2);
            }
            mpv::EVENT_END_FILE => {
                let finished = reason == Some(mpv::END_FILE_EOF);
                let error = (reason == Some(mpv::END_FILE_ERROR))
                    .then(|| "The player could not play this file.".to_owned());
                report(Progress {
                    finished,
                    error,
                    ..last_pos
                });
                break;
            }
            mpv::EVENT_SHUTDOWN => {
                report(last_pos);
                break;
            }
            mpv::EVENT_CLIENT_MESSAGE
                if event.message.first().map(String::as_str) == Some("alwy") =>
            {
                let pos = m
                    .get_f64("time-pos")
                    .map(|s| (s.max(0.0) * 1000.0) as u64)
                    .unwrap_or(0);
                let dur = m.get_f64("duration").map(|s| (s * 1000.0) as u64);
                let prompt = playback_prompt(req.segments.clone(), pos, dur, req.has_next);
                match event.message.get(1).map(String::as_str) {
                    Some("skip") => {
                        if let Some(seg) = prompt.skip {
                            seek_to(&m, seg.end_ms);
                        }
                    }
                    Some("next") if req.has_next => go = Some(Go::Next),
                    Some("previous") if req.has_previous => go = Some(Go::Previous),
                    Some("cancel") => {
                        cancelled = true;
                        countdown = None;
                        let _ = m.command(&["show-text", "", "1"]);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if let Some(g) = go {
            report(Progress {
                go: Some(g),
                ..snapshot(PlaybackEvent::Stop, false)
            });
            break;
        }
        // Keep the latest position: after END_FILE the properties are gone.
        let now = snapshot(PlaybackEvent::Stop, false);
        if now.duration_ms.is_some() {
            last_pos = now;
        }
        // The viewer switched audio or subtitles (mpv's # and j keys): remember it for the
        // movie or the show.
        if !tracks.infos.is_empty() {
            let now = (selected(&m, "aid"), selected(&m, "sid"));
            for (kind, before, after) in [
                (TrackKind::Audio, &applied.0, &now.0),
                (TrackKind::Subtitle, &applied.1, &now.1),
            ] {
                if before != after {
                    let picked = after.as_deref().and_then(|id| tracks.find(kind, id));
                    if after.is_none() || picked.is_some() {
                        let p = pick_track(pref.clone(), kind, tracks.infos.clone(), picked);
                        (req.tracks.save)(p.clone());
                        pref = Some(p);
                    }
                }
            }
            applied = now;
        }
        let is_paused = m.get_flag("pause").unwrap_or(false);
        if is_paused != paused {
            paused = is_paused;
            if Instant::now() >= quiet_until {
                send_user(&m);
                next_check = Instant::now() + Duration::from_secs(2);
            }
            report(snapshot(
                if paused {
                    PlaybackEvent::Pause
                } else {
                    PlaybackEvent::Progress
                },
                false,
            ));
        }
        if !paused && last.elapsed() >= Duration::from_secs(10) {
            last = Instant::now();
            report(snapshot(PlaybackEvent::Progress, false));
        }
        // Short notes: the room ("Sam paused"), an invitation (longer, to be read).
        let mut view_note = String::new();
        // The room waits (loading, a start a moment ahead): said as long as it lasts.
        let waiting = room
            .as_ref()
            .filter(|r| r.hold || Instant::now() < r.at)
            .and_then(|r| r.waiting.clone());
        if let Some(w) = waiting {
            if view.is_some() {
                view_note = w;
            } else {
                let _ = m.command(&["show-text", &w, "300"]);
            }
        } else if let Some((t, at)) = &note {
            if at.elapsed() < Duration::from_secs(6) {
                if view.is_some() {
                    view_note = t.clone();
                } else {
                    let _ = m.command(&["show-text", t, "900"]);
                    continue;
                }
            } else {
                note = None;
            }
        }
        let mut view_skip = String::new();
        let mut view_countdown = 0;
        // Skip intro / next episode prompts, shared with the other players (core).
        let Some(dur) = last_pos.duration_ms else {
            if let Some(v) = view {
                push_view(
                    &m,
                    v,
                    &req,
                    &mut shown,
                    view_note,
                    view_skip,
                    view_countdown,
                );
            }
            continue;
        };
        let pos = last_pos.position_ms;
        let prompt = playback_prompt(req.segments.clone(), pos, Some(dur), req.has_next);
        if let Some(seg) = &prompt.skip
            && req.auto_skip_intro
            && matches!(seg.kind, SegmentKind::Intro | SegmentKind::Recap)
            && !skipped.contains(&seg.start_ms)
        {
            skipped.push(seg.start_ms);
            seek_to(&m, seg.end_ms);
        }
        let text = if prompt.credits && req.auto_next && !cancelled && !paused {
            let since = *countdown.get_or_insert_with(Instant::now);
            let left = COUNTDOWN.saturating_sub(since.elapsed());
            if left.is_zero() {
                report(Progress {
                    go: Some(Go::Next),
                    ..snapshot(PlaybackEvent::Stop, false)
                });
                break;
            }
            view_countdown = left.as_secs() as u32 + 1;
            Some(format!(
                "Next episode in {} s   ·   > now   ·   C to keep watching",
                view_countdown
            ))
        } else {
            if !prompt.credits {
                countdown = None;
            }
            // A button on the in-window player; nothing in mpv's window (S still skips there).
            if let Some(seg) = &prompt.skip {
                view_skip = format!("Skip {}", skip_name(seg.kind));
            }
            None
        };
        if let Some(v) = view {
            push_view(
                &m,
                v,
                &req,
                &mut shown,
                view_note,
                view_skip,
                view_countdown,
            );
        } else if let Some(t) = text {
            // Refreshed every loop (0.5 s), so it disappears soon after the prompt ends.
            let _ = m.command(&["show-text", &t, "900"]);
        }
    }
    Ok(())
}

/// Sends what the controls show when it changed.
fn push_view(
    m: &Mpv,
    v: &ViewLink,
    req: &PlayRequest,
    shown: &mut PlayerView,
    note: String,
    skip: String,
    countdown: u32,
) {
    let ms = |p: &str| m.get_f64(p).map_or(0, |s| (s.max(0.0) * 1000.0) as u64);
    let position_ms = ms("time-pos");
    let tracks = |kind: &str| {
        let count: usize = m
            .get_string("track-list/count")
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let mut out = Vec::new();
        if kind == "sub" {
            out.push(TrackItem {
                id: "no".into(),
                label: "Off".into(),
                selected: matches!(m.get_string("sid").as_deref(), Some("no" | "false") | None),
            });
        }
        for i in 0..count {
            let prop = |n: &str| {
                m.get_string(&format!("track-list/{i}/{n}"))
                    .filter(|v| !v.is_empty())
            };
            if prop("type").as_deref() != Some(kind) {
                continue;
            }
            let Some(id) = prop("id") else { continue };
            let lang = prop("lang").map(|l| language_name(&l));
            let mut parts: Vec<String> = lang.iter().cloned().collect();
            if let Some(t) = prop("title").filter(|t| Some(t) != lang.as_ref()) {
                parts.push(t);
            }
            if kind == "audio"
                && let Some(c) = prop("codec")
            {
                parts.push(c.to_uppercase());
            }
            if m.get_flag(&format!("track-list/{i}/forced"))
                .unwrap_or(false)
            {
                parts.push("forced".into());
            }
            if parts.is_empty() {
                parts.push(format!("Track {id}"));
            }
            out.push(TrackItem {
                id,
                label: parts.join(" · "),
                selected: m
                    .get_flag(&format!("track-list/{i}/selected"))
                    .unwrap_or(false),
            });
        }
        out
    };
    let now = PlayerView {
        // Steps of a quarter second: fewer UI updates, smooth enough for the bar and clock.
        position_ms: position_ms / 250 * 250,
        duration_ms: ms("duration"),
        cache_ms: ms("demuxer-cache-time") / 1000 * 1000,
        paused: m.get_flag("pause").unwrap_or(false),
        skip,
        countdown,
        note,
        has_next: req.has_next,
        has_previous: req.has_previous,
        audio: tracks("audio"),
        subtitles: tracks("sub"),
    };
    if now != *shown {
        (v.show)(now.clone());
        *shown = now;
    }
}

fn language_name(code: &str) -> String {
    let name = match code.to_ascii_lowercase().as_str() {
        "fre" | "fra" | "fr" => "French",
        "eng" | "en" => "English",
        "ger" | "deu" | "de" => "German",
        "spa" | "es" => "Spanish",
        "ita" | "it" => "Italian",
        "jpn" | "ja" => "Japanese",
        "dut" | "nld" | "nl" => "Dutch",
        "por" | "pt" => "Portuguese",
        "kor" | "ko" => "Korean",
        "chi" | "zho" | "zh" => "Chinese",
        "rus" | "ru" => "Russian",
        "ara" | "ar" => "Arabic",
        "pol" | "pl" => "Polish",
        "und" => "Unknown language",
        _ => return code.to_owned(),
    };
    name.to_owned()
}

/// mpv's audio and subtitle tracks, numbered for the core, with their mpv ids.
#[derive(Default)]
struct Tracks {
    infos: Vec<TrackInfo>,
    mpv_ids: Vec<String>,
}

impl Tracks {
    fn find(&self, kind: TrackKind, mpv_id: &str) -> Option<u32> {
        self.infos
            .iter()
            .find(|t| t.kind == kind && self.mpv_ids[t.id as usize] == mpv_id)
            .map(|t| t.id)
    }
}

fn read_tracks(m: &Mpv) -> Tracks {
    let mut out = Tracks::default();
    let count: usize = m
        .get_string("track-list/count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    for i in 0..count {
        let prop = |name: &str| {
            m.get_string(&format!("track-list/{i}/{name}"))
                .filter(|v| !v.is_empty())
        };
        let kind = match prop("type").as_deref() {
            Some("audio") => TrackKind::Audio,
            Some("sub") => TrackKind::Subtitle,
            _ => continue,
        };
        let Some(id) = prop("id") else { continue };
        out.infos.push(TrackInfo {
            id: out.infos.len() as u32,
            kind,
            language: prop("lang"),
            title: prop("title"),
            forced: m
                .get_flag(&format!("track-list/{i}/forced"))
                .unwrap_or(false),
        });
        out.mpv_ids.push(id);
    }
    out
}

/// The selected track id of "aid"/"sid"; None when off ("no") or not decided yet.
fn selected(m: &Mpv, prop: &str) -> Option<String> {
    m.get_string(prop)
        .filter(|v| !matches!(v.as_str(), "no" | "auto" | "false" | ""))
}

fn seek_to(m: &Mpv, ms: u64) {
    let _ = m.command(&["seek", &format!("{:.3}", ms as f64 / 1000.0), "absolute"]);
}

fn skip_name(kind: SegmentKind) -> &'static str {
    match kind {
        SegmentKind::Intro => "the intro",
        SegmentKind::Credits => "the credits",
        SegmentKind::Recap => "the recap",
        SegmentKind::Preview => "the preview",
        SegmentKind::Commercial => "this",
    }
}
