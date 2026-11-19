// SPDX-License-Identifier: AGPL-3.0-or-later
//! Playback session: one mpv instance in its own native window (best hardware decoding and HDR
//! path), driven from a dedicated thread. Progress is reported back through a callback.

use std::time::{Duration, Instant};

use alwywatch_core::{HttpRequest, PlaybackEvent};

use crate::mpv::{self, Mpv};

pub struct PlayRequest {
    pub media: HttpRequest,
    pub start_ms: u64,
    pub title: String,
    pub external_subs: Vec<String>,
    pub spdif: bool,
}

pub struct Progress {
    pub event: PlaybackEvent,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    /// Set on the final Stop when the file played to the end.
    pub finished: bool,
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
    m.command(&["loadfile", &req.media.url])?;

    let snapshot = |event, finished| Progress {
        event,
        position_ms: m
            .get_f64("time-pos")
            .map(|s| (s.max(0.0) * 1000.0) as u64)
            .unwrap_or(0),
        duration_ms: m.get_f64("duration").map(|s| (s * 1000.0) as u64),
        finished,
    };
    let mut last = Instant::now();
    let mut paused = false;
    let mut last_pos = Progress {
        event: PlaybackEvent::Stop,
        position_ms: req.start_ms,
        duration_ms: None,
        finished: false,
    };
    loop {
        let (ev, reason) = m.wait_event(0.5);
        match ev {
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
            _ => {}
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
    }
    Ok(())
}
