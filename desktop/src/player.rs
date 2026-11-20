// SPDX-License-Identifier: AGPL-3.0-or-later
//! Playback session: one mpv instance in its own native window (best hardware decoding and HDR
//! path), driven from a dedicated thread. Progress is reported back through a callback.

use std::time::{Duration, Instant};

use alwywatch_core::{HttpRequest, PlaybackEvent, Segment, SegmentKind, playback_prompt};

use crate::mpv::{self, Mpv};

pub struct PlayRequest {
    pub media: HttpRequest,
    pub start_ms: u64,
    pub title: String,
    pub external_subs: Vec<String>,
    pub spdif: bool,
    /// Skippable intro/credits (from the server), for the skip and next-episode prompts.
    pub segments: Vec<Segment>,
    pub has_next: bool,
    pub has_previous: bool,
    pub auto_skip_intro: bool,
    pub auto_next: bool,
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
            if let Err(e) = run(req, &report) {
                eprintln!("playback failed: {e}");
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
    // One header per call: the Jellyfin header contains commas that list-splitting would break.
    for h in &req.media.headers {
        m.set_option(
            "http-header-fields-append",
            &format!("{}: {}", h.name, h.value),
        )?;
    }
    for s in &req.external_subs {
        m.set_option("sub-files-append", s)?;
    }
    m.initialize()?;
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
    };
    let mut last = Instant::now();
    let mut paused = false;
    let mut last_pos = Progress {
        event: PlaybackEvent::Stop,
        position_ms: req.start_ms,
        duration_ms: None,
        finished: false,
        go: None,
    };
    let mut skipped: Vec<u64> = Vec::new();
    let mut countdown: Option<Instant> = None;
    let mut cancelled = false;
    let mut go: Option<Go> = None;
    loop {
        let event = m.wait_event(0.5);
        let reason = event.reason;
        match event.id {
            mpv::EVENT_FILE_LOADED => report(snapshot(PlaybackEvent::Start, false)),
            mpv::EVENT_END_FILE => {
                let finished = reason == Some(mpv::END_FILE_EOF);
                if reason == Some(mpv::END_FILE_ERROR) {
                    eprintln!("mpv could not play the file");
                }
                report(Progress {
                    finished,
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
        let is_paused = m.get_flag("pause").unwrap_or(false);
        if is_paused != paused {
            paused = is_paused;
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
