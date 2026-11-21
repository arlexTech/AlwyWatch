// SPDX-License-Identifier: AGPL-3.0-or-later
//! Playback session: one mpv instance in its own native window (best hardware decoding and HDR
//! path), driven from a dedicated thread. Progress is reported back through a callback.

use std::sync::mpsc;
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
}

/// From the app to a player in a watch together room.
pub enum ToPlayer {
    /// Play or pause like the room, at this position (seek when more than 1.5 s apart).
    Sync { playing: bool, position_ms: u64 },
    /// A short message on the video ("Sam paused").
    Note(String),
    /// Someone asks the host to join: (request id, name); None when nobody waits.
    Request(Option<(String, String)>),
    /// Close the player (the room moved to a title found elsewhere).
    Quit,
}

/// From a player in a room to the app.
pub enum FromPlayer {
    /// The viewer played, paused or moved.
    User { playing: bool, position_ms: u64 },
    /// The host answered a join request.
    Answer { request_id: String, accept: bool },
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

fn run(req: PlayRequest, report: &dyn Fn(Progress)) -> Result<(), String> {
    let m = Mpv::new()?;
    // Video: hardware decoding when safe (VAAPI/NVDEC on Linux, D3D11VA on Windows), the modern
    // renderer, and a bounded demuxer cache so RAM stays predictable for 4K remuxes.
    for (k, v) in [
        ("vo", "gpu-next"),
        ("hwdec", "auto-safe"),
        ("force-window", "immediate"),
        ("fullscreen", "yes"),
        ("keep-open", "no"),
        ("input-default-bindings", "yes"),
        ("input-vo-keyboard", "yes"),
        ("osc", "yes"),
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
         c script-message alwy cancel\n\
         y script-message alwy yes\nY script-message alwy yes\n\
         n script-message alwy no\nN script-message alwy no",
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
    let mut request: Option<(String, String)> = None;
    let mut started = false;
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
        let event = m.wait_event(if together.is_some() { 0.1 } else { 0.5 });
        if let Some(t) = together {
            while let Ok(msg) = t.rx.try_recv() {
                match msg {
                    ToPlayer::Sync {
                        playing,
                        position_ms,
                    } => {
                        quiet_until = Instant::now() + Duration::from_millis(1500);
                        let _ = m.command(&["set", "pause", if playing { "no" } else { "yes" }]);
                        paused = !playing;
                        let pos = m
                            .get_f64("time-pos")
                            .map(|s| (s.max(0.0) * 1000.0) as u64)
                            .unwrap_or(0);
                        if pos.abs_diff(position_ms) > 1500 {
                            seek_to(&m, position_ms);
                        }
                    }
                    ToPlayer::Note(t) => note = Some((t, Instant::now())),
                    ToPlayer::Request(r) => request = r,
                    ToPlayer::Quit => {
                        let _ = m.command(&["quit"]);
                    }
                }
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
            mpv::EVENT_PLAYBACK_RESTART if Instant::now() >= quiet_until => send_user(&m),
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
                    Some(answer @ ("yes" | "no")) => {
                        if let (Some(t), Some((id, _))) = (together, request.take()) {
                            t.send.as_ref()(FromPlayer::Answer {
                                request_id: id,
                                accept: answer == "yes",
                            });
                        }
                    }
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
        // Watching together: who asks to join (until answered), then short notes.
        if let Some((_, name)) = &request {
            let t = format!("{name} wants to watch with you   ·   Y let them in   ·   N not now");
            let _ = m.command(&["show-text", &t, "900"]);
            continue;
        }
        if let Some((t, at)) = &note {
            if at.elapsed() < Duration::from_secs(4) {
                let _ = m.command(&["show-text", t, "900"]);
                continue;
            }
            note = None;
        }
        // Skip intro / next episode prompts, shared with the other players (core).
        let Some(dur) = last_pos.duration_ms else {
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
            continue;
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
            Some(format!(
                "Next episode in {} s   ·   > now   ·   C to keep watching",
                left.as_secs() + 1
            ))
        } else {
            if !prompt.credits {
                countdown = None;
            }
            if let Some(seg) = &prompt.skip {
                Some(format!("Press S to skip {}", skip_name(seg.kind)))
            } else if prompt.next_episode {
                Some("Press > for the next episode".to_owned())
            } else {
                None
            }
        };
        if let Some(t) = text {
            // Refreshed every loop (0.5 s), so it disappears soon after the prompt ends.
            let _ = m.command(&["show-text", &t, "900"]);
        }
    }
    Ok(())
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
