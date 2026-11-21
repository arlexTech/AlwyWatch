// SPDX-License-Identifier: AGPL-3.0-or-later
//! The player: the browser's video element (its own controls, keyboard and fullscreen), with
//! skip intro/credits, next episode and progress sync on top.
//!
//! The original file plays directly whenever the browser says it can; if it then fails (an
//! audio codec it can't decode, a container it only claimed), the server's HLS transcode takes
//! over at the same position. HLS plays natively in Safari, through hls.js elsewhere (loaded
//! only then, from this site).

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use media_api::{
    AnyServer, DeviceProfile, Item, ItemKind, MediaServer, PlaybackEvent, PlaybackInfo,
    ProgressReport, Segment, SegmentKind,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

use crate::state::{self, fmt_time};

fn test_video() -> Option<web_sys::HtmlMediaElement> {
    web_sys::window()?
        .document()?
        .create_element("video")
        .ok()?
        .dyn_into()
        .ok()
}

/// What this browser decodes, asked to the browser itself.
fn browser_profile() -> DeviceProfile {
    let v = test_video();
    let can = |t: &str| v.as_ref().is_some_and(|v| !v.can_play_type(t).is_empty());
    let ua = web_sys::window()
        .and_then(|w| w.navigator().user_agent().ok())
        .unwrap_or_default();
    let mut video = vec!["h264".to_string()];
    let mut ten_bit = Vec::new();
    if can(r#"video/mp4; codecs="hvc1.1.6.L120.90""#)
        || can(r#"video/mp4; codecs="hev1.1.6.L120.90""#)
    {
        video.push("hevc".into());
        if can(r#"video/mp4; codecs="hvc1.2.4.L153.B0""#) {
            ten_bit.push("hevc".into());
        }
    }
    if can(r#"video/webm; codecs="vp09.00.10.08""#) {
        video.push("vp9".into());
    }
    if can(r#"video/mp4; codecs="av01.0.08M.08""#) {
        video.push("av1".into());
        if can(r#"video/mp4; codecs="av01.0.08M.10""#) {
            ten_bit.push("av1".into());
        }
    }
    let mut audio = vec!["aac".to_string(), "mp3".to_string()];
    for (codec, t) in [
        ("opus", r#"audio/webm; codecs="opus""#),
        ("vorbis", r#"audio/webm; codecs="vorbis""#),
        ("flac", "audio/flac"),
        ("ac3", r#"audio/mp4; codecs="ac-3""#),
        ("eac3", r#"audio/mp4; codecs="ec-3""#),
    ] {
        if can(t) {
            audio.push(codec.into());
        }
    }
    let mut containers: Vec<String> = ["mp4", "m4v", "mov", "webm"].map(String::from).into();
    // Chromium and Firefox play Matroska though few admit it in canPlayType.
    if can("video/x-matroska")
        || can("video/mkv")
        || ua.contains("Chrome/")
        || ua.contains("Firefox/")
    {
        containers.push("mkv".into());
    }
    DeviceProfile {
        max_bitrate: None,
        containers,
        video_codecs: video,
        video_10bit_codecs: ten_bit,
        audio_codecs: audio,
        subtitle_codecs: vec!["vtt".into(), "srt".into(), "subrip".into()],
        max_width: None,
        max_height: None,
        hdr: vec![],
    }
}

fn native_hls() -> bool {
    test_video().is_some_and(|v| !v.can_play_type("application/vnd.apple.mpegurl").is_empty())
}

/// Plays an HLS URL: natively, or through hls.js (added to the page once).
fn play_hls(video: &web_sys::HtmlVideoElement, url: String) {
    if native_hls() {
        video.set_src(&url);
        return;
    }
    let attach = {
        let video = video.clone();
        move || {
            let w = web_sys::window().expect("window");
            let Ok(hls_class) = js_sys::Reflect::get(&w, &"Hls".into()) else {
                return;
            };
            let class: &js_sys::Function = hls_class.unchecked_ref();
            let Ok(hls) = js_sys::Reflect::construct(class, &js_sys::Array::new()) else {
                return;
            };
            let call = |name: &str, arg: &JsValue| {
                if let Ok(f) = js_sys::Reflect::get(&hls, &name.into()) {
                    let _ = f.unchecked_into::<js_sys::Function>().call1(&hls, arg);
                }
            };
            call("loadSource", &url.clone().into());
            call("attachMedia", &video.clone().into());
        }
    };
    let w = web_sys::window().expect("window");
    if js_sys::Reflect::get(&w, &"Hls".into()).is_ok_and(|h| !h.is_undefined()) {
        attach();
        return;
    }
    let Some(doc) = w.document() else { return };
    let Ok(script) = doc.create_element("script") else {
        return;
    };
    let _ = script.set_attribute("src", "/vendor/hls.light.min.js");
    let onload = Closure::once_into_js(attach);
    let _ = script.add_event_listener_with_callback("load", onload.unchecked_ref());
    if let Some(head) = doc.head() {
        let _ = head.append_child(&script);
    }
}

#[derive(Clone)]
struct Loaded {
    srv: AnyServer,
    item: Item,
    info: PlaybackInfo,
    segments: Vec<Segment>,
    next: Option<Item>,
}

async fn load(acc: &str, id: &str) -> Result<Loaded, String> {
    let srv = state::ctx().server(acc).ok_or("Account not found")?;
    let item = srv.item(id).await.map_err(|e| e.to_string())?;
    let info = srv
        .playback_info(id, &browser_profile())
        .await
        .map_err(|e| e.to_string())?;
    let segments = srv.segments(id).await.unwrap_or_default();
    let next = match (&item.kind, &item.series_id, &item.season_id) {
        (ItemKind::Episode, Some(series), Some(season)) => {
            let eps = srv.episodes(series, season).await.unwrap_or_default();
            let at = eps.iter().position(|e| e.id == item.id);
            match at.and_then(|i| eps.get(i + 1)) {
                Some(n) => Some(n.clone()),
                // Last of the season: the first of the next one.
                None => {
                    let seasons = srv.seasons(series).await.unwrap_or_default();
                    let here = seasons.iter().position(|s| s.id == *season);
                    match here.and_then(|i| seasons.get(i + 1)) {
                        Some(s) => srv
                            .episodes(series, &s.id)
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .next(),
                        None => None,
                    }
                }
            }
        }
        _ => None,
    };
    Ok(Loaded {
        srv,
        item,
        info,
        segments,
        next,
    })
}

#[component]
pub fn Player() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let navigate = use_navigate();
    let acc = params.read_untracked().get("acc").unwrap_or_default();
    let id = params.read_untracked().get("id").unwrap_or_default();
    let start_ms: u64 = query
        .read_untracked()
        .get("t")
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    let video = NodeRef::<leptos::html::Video>::new();
    let loaded = RwSignal::new(None::<Result<Loaded, String>>);
    let pos = RwSignal::new(0u64);
    let transcoding = RwSignal::new(false);
    let last_report = StoredValue::new(0f64);
    let stopped = StoredValue::new(false);
    {
        let (acc, id) = (acc.clone(), id.clone());
        spawn_local(async move { loaded.set(Some(load(&acc, &id).await)) });
    }

    let report = move |event: PlaybackEvent| {
        let Some(Ok(l)) = loaded.get_untracked() else {
            return;
        };
        let Some(v) = video.get_untracked() else {
            return;
        };
        let src = l.info.sources.first();
        let r = ProgressReport {
            item_id: l.item.id.clone(),
            media_source_id: src.map(|s| s.id.clone()),
            play_session_id: l.info.play_session_id.clone(),
            position_ms: (v.current_time() * 1000.0) as u64,
            duration_ms: v
                .duration()
                .is_finite()
                .then(|| (v.duration() * 1000.0) as u64),
            event,
            audio_index: None,
            subtitle_index: None,
        };
        spawn_local(async move {
            let _ = l.srv.report_progress(&r).await;
        });
    };

    // Starts the source once both the data and the element are there.
    Effect::new(move |_| {
        let (Some(Some(Ok(l))), Some(v)) = (loaded.get().map(Some), video.get()) else {
            return;
        };
        let Some(src) = l.info.sources.first() else {
            return;
        };
        let session = l.srv.session().clone();
        if src.supports_direct_play && !transcoding.get_untracked() {
            v.set_src(&state::with_token(&src.direct.url, &session));
        } else if let Some(t) = &src.transcode {
            transcoding.set(true);
            play_hls(&v, state::with_token(&t.url, &session));
        }
        let _ = v.play();
    });

    // The original file failed: same position, through the server.
    let on_error = move |_| {
        if transcoding.get_untracked() {
            return;
        }
        let (Some(Ok(l)), Some(v)) = (loaded.get_untracked(), video.get_untracked()) else {
            return;
        };
        let Some(t) = l.info.sources.first().and_then(|s| s.transcode.clone()) else {
            return;
        };
        transcoding.set(true);
        play_hls(&v, state::with_token(&t.url, l.srv.session()));
        let _ = v.play();
    };

    let on_meta = move |_| {
        if let Some(v) = video.get_untracked() {
            let at = if transcoding.get_untracked() && pos.get_untracked() > 0 {
                pos.get_untracked()
            } else {
                start_ms
            };
            if at > 0 {
                v.set_current_time(at as f64 / 1000.0);
            }
        }
    };

    let on_time = move |_| {
        let Some(v) = video.get_untracked() else {
            return;
        };
        let t = v.current_time();
        pos.set((t * 1000.0) as u64);
        if (t - last_report.get_value()).abs() >= 10.0 {
            last_report.set_value(t);
            report(PlaybackEvent::Progress);
        }
    };

    // Which segment we're in, if skippable.
    let current = Memo::new(move |_| {
        let p = pos.get();
        loaded.with(|l| match l {
            Some(Ok(l)) => l
                .segments
                .iter()
                .find(|s| p >= s.start_ms && p + 500 < s.end_ms)
                .cloned(),
            _ => None,
        })
    });
    let skip = move |_| {
        if let (Some(s), Some(v)) = (current.get_untracked(), video.get_untracked()) {
            v.set_current_time(s.end_ms as f64 / 1000.0);
        }
    };

    let nav2 = navigate.clone();
    let acc2 = acc.clone();
    let go_next = move || {
        let next = loaded.with_untracked(|l| {
            l.as_ref()
                .and_then(|l| l.as_ref().ok())
                .and_then(|l| l.next.clone())
        });
        stopped.set_value(true);
        report(PlaybackEvent::Stop);
        match next {
            Some(n) => nav2(&format!("/play/{}/{}?t=0", acc2, n.id), Default::default()),
            None => {
                if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
                    let _ = h.back();
                }
            }
        }
    };
    let go_next2 = go_next.clone();
    let back = move |_| {
        if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
            let _ = h.back();
        }
    };
    on_cleanup(move || {
        if !stopped.get_value() {
            report(PlaybackEvent::Stop);
        }
    });

    view! {
        <div class="player">
            <video node_ref=video controls autoplay playsinline
                on:loadedmetadata=on_meta on:timeupdate=on_time on:error=on_error
                on:playing=move |_| report(PlaybackEvent::Start)
                on:pause=move |_| report(PlaybackEvent::Pause)
                on:ended=move |_| go_next() />
            <div class="player-top">
                <button class="icon" on:click=back aria-label="Back">"←"</button>
                {move || match loaded.get() {
                    Some(Ok(l)) => {
                        let i = &l.item;
                        let sub = (i.kind == ItemKind::Episode).then(|| format!("S{} E{} · {}",
                            i.season_number.unwrap_or(0), i.episode_number.unwrap_or(0), i.title));
                        let title = i.series_title.clone().filter(|_| i.kind == ItemKind::Episode).unwrap_or(i.title.clone());
                        view! { <div><div class="ptitle">{title}</div><div class="muted">{sub}</div></div> }.into_any()
                    }
                    Some(Err(e)) => view! { <div class="error">{e}</div> }.into_any(),
                    None => view! { <div class="muted">"Loading…"</div> }.into_any(),
                }}
                {move || transcoding.get().then(|| view! { <span class="pill">"Converted by the server"</span> })}
            </div>
            {move || current.get().map(|s| {
                let has_next = loaded.with(|l| matches!(l, Some(Ok(l)) if l.next.is_some()));
                let go_next = go_next2.clone();
                match s.kind {
                    SegmentKind::Credits if has_next => view! {
                        <button class="skip" on:click=move |_| go_next()>"Next episode"</button>
                    }.into_any(),
                    k => {
                        let label = match k {
                            SegmentKind::Intro => "Skip intro",
                            SegmentKind::Credits => "Skip credits",
                            SegmentKind::Recap => "Skip recap",
                            SegmentKind::Preview => "Skip preview",
                            SegmentKind::Commercial => "Skip ad",
                        };
                        view! { <button class="skip" on:click=skip>{label}" · "{fmt_time(s.end_ms)}</button> }.into_any()
                    }
                }
            })}
        </div>
    }
}
