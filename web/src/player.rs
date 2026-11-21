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
    AnyServer, DeviceProfile, Item, ItemKind, MediaServer, MediaSource, PlaybackEvent,
    PlaybackInfo, ProgressReport, Segment, SegmentKind, ServerKind, StreamKind,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

use crate::prefs::{self, SubTrack};
use crate::state::{self, fmt_time};
use crate::together::{self, Room};

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

/// Codec names as servers write them, as the profile does.
fn codec_name(c: &str) -> String {
    match c.to_ascii_lowercase().as_str() {
        "avc" | "avc1" | "h.264" => "h264".into(),
        "h265" | "hev1" | "hvc1" | "h.265" => "hevc".into(),
        "mpeg4" => "mp4v".into(),
        other => other.to_owned(),
    }
}

/// Whether the browser plays this file as it is: container, video codec (and its bit depth)
/// and default audio codec. Plex leaves this to the client (it says yes to everything), and a
/// browser given an audio codec it can't decode often plays the picture silently, with no
/// error to fall back on.
pub fn can_direct(src: &MediaSource, p: &DeviceProfile) -> bool {
    let container_ok = src.container.as_deref().is_none_or(|c| {
        c.split(',').any(|c| {
            let c = c.trim().to_ascii_lowercase();
            let c = if c == "matroska" {
                "mkv".to_string()
            } else {
                c
            };
            p.containers.contains(&c)
        })
    });
    let streams = |k: StreamKind| src.streams.iter().filter(move |s| s.kind == k);
    let video_ok = streams(StreamKind::Video).next().is_none_or(|v| {
        let c = codec_name(&v.codec);
        p.video_codecs.contains(&c)
            && (v.bit_depth.unwrap_or(8) <= 8 || p.video_10bit_codecs.contains(&c))
            && p.max_width.is_none_or(|w| v.width.unwrap_or(0) <= w)
            && p.max_height.is_none_or(|h| v.height.unwrap_or(0) <= h)
    });
    let audio = src
        .default_audio_index
        .and_then(|i| streams(StreamKind::Audio).find(|a| a.index == i))
        .or_else(|| streams(StreamKind::Audio).find(|a| a.is_default))
        .or_else(|| streams(StreamKind::Audio).next());
    let audio_ok = audio.is_none_or(|a| p.audio_codecs.contains(&codec_name(&a.codec)));
    container_ok && video_ok && audio_ok
}

thread_local! {
    /// play() is refused without a click (autoplay rules): the controls are there, nothing to do.
    static IGNORE: Closure<dyn FnMut(JsValue)> = Closure::new(|_| {});
}

fn play(v: &web_sys::HtmlMediaElement) {
    if let Ok(p) = v.play() {
        IGNORE.with(|f| {
            let _ = p.catch(f);
        });
    }
}

/// The hls.js instance of the page, if any (destroyed when the player goes).
type HlsSlot = StoredValue<Option<JsValue>, LocalStorage>;

fn destroy_hls(slot: HlsSlot) {
    if let Some(h) = slot.try_update_value(Option::take).flatten()
        && let Ok(f) = js_sys::Reflect::get(&h, &"destroy".into())
        && let Some(f) = f.dyn_ref::<js_sys::Function>()
    {
        let _ = f.call0(&h);
    }
}

/// Plays an HLS URL: natively, or through hls.js (added to the page once). `failed` is called
/// when it can't (script missing, fatal stream error).
fn play_hls(
    video: &web_sys::HtmlVideoElement,
    url: String,
    slot: HlsSlot,
    failed: impl Fn() + Clone + 'static,
) {
    if native_hls() {
        video.set_src(&url);
        return;
    }
    let attach = {
        let video = video.clone();
        let failed = failed.clone();
        move || {
            // The player went while the script loaded.
            if slot.try_with_value(|_| ()).is_none() {
                return;
            }
            destroy_hls(slot);
            let Some(w) = web_sys::window() else { return };
            let Some(class) = js_sys::Reflect::get(&w, &"Hls".into())
                .ok()
                .and_then(|c| c.dyn_into::<js_sys::Function>().ok())
            else {
                return failed();
            };
            let Ok(hls) = js_sys::Reflect::construct(&class, &js_sys::Array::new()) else {
                return failed();
            };
            let call = |name: &str, a: &JsValue, b: Option<&JsValue>| {
                if let Ok(f) = js_sys::Reflect::get(&hls, &name.into())
                    && let Some(f) = f.dyn_ref::<js_sys::Function>()
                {
                    let _ = match b {
                        Some(b) => f.call2(&hls, a, b),
                        None => f.call1(&hls, a),
                    };
                }
            };
            // Only fatal errors end playback (hls.js recovers from the others itself).
            let on_error = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |_, data: JsValue| {
                if js_sys::Reflect::get(&data, &"fatal".into()).is_ok_and(|f| f.is_truthy()) {
                    failed();
                }
            })
            .into_js_value();
            call("on", &"hlsError".into(), Some(&on_error));
            call("loadSource", &url.clone().into(), None);
            call("attachMedia", &video.clone().into(), None);
            slot.set_value(Some(hls));
        }
    };
    let Some(w) = web_sys::window() else { return };
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
    let onerror = Closure::once_into_js(failed);
    let _ = script.add_event_listener_with_callback("error", onerror.unchecked_ref());
    if let Some(head) = doc.head() {
        let _ = head.append_child(&script);
    }
}

#[derive(Clone)]
struct Loaded {
    srv: AnyServer,
    item: Item,
    info: PlaybackInfo,
    profile: DeviceProfile,
    /// Filled in after playback started (not worth waiting for).
    segments: Vec<Segment>,
    next: Option<Item>,
}

/// What playback needs (title and file, asked at the same time); the rest comes after.
async fn load(acc: &str, id: &str) -> Result<Loaded, String> {
    let c = state::ctx();
    let srv = c
        .server(acc)
        .ok_or("This account isn't signed in any more.")?;
    let profile = browser_profile();
    let (item, info) = state::join2(srv.item(id), srv.playback_info(id, &profile)).await;
    let fail = |e: media_api::Error| {
        c.failed(acc, &e);
        state::plain(&e)
    };
    let item = item.map_err(fail)?;
    let info = info.map_err(fail)?;
    if info.sources.is_empty() {
        return Err("The server has no file to play for this title.".into());
    }
    Ok(Loaded {
        srv,
        item,
        info,
        profile,
        segments: Vec::new(),
        next: None,
    })
}

/// Skippable parts and the next episode.
async fn extras(srv: &AnyServer, item: &Item) -> (Vec<Segment>, Option<Item>) {
    let next = async {
        let (ItemKind::Episode, Some(series), Some(season)) =
            (&item.kind, &item.series_id, &item.season_id)
        else {
            return None;
        };
        let eps = srv.episodes(series, season).await.unwrap_or_default();
        let at = eps.iter().position(|e| e.id == item.id);
        if let Some(n) = at.and_then(|i| eps.get(i + 1)) {
            return Some(n.clone());
        }
        // Last of the season: the first of the next one.
        let seasons = srv.seasons(series).await.unwrap_or_default();
        let here = seasons.iter().position(|s| s.id == *season)?;
        let s = seasons.get(here + 1)?;
        srv.episodes(series, &s.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .next()
    };
    let (segments, next) = state::join2(srv.segments(&item.id), next).await;
    (segments.unwrap_or_default(), next)
}

/// Subtitles a browser can draw: text ones (image subtitles need the server to burn them in).
fn text_subtitles(l: &Loaded) -> Vec<(i64, String)> {
    let Some(src) = l.info.sources.first() else {
        return Vec::new();
    };
    let jellyfin = l.srv.session().kind == ServerKind::Jellyfin;
    src.streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle)
        .filter(|s| {
            matches!(
                s.codec.to_lowercase().as_str(),
                "srt" | "subrip" | "vtt" | "webvtt" | "ass" | "ssa" | "mov_text"
            )
        })
        // Jellyfin converts any text track on request; Plex only hands out external files.
        .filter(|s| {
            jellyfin
                || (s.external.is_some()
                    && !matches!(s.codec.to_lowercase().as_str(), "ass" | "ssa"))
        })
        .map(|s| {
            let label = s
                .display_title
                .clone()
                .or(s.title.clone())
                .or(s.language.clone())
                .unwrap_or_else(|| "Subtitles".into());
            (s.index, label)
        })
        .collect()
}

/// SubRip to WebVTT: a header, and dots in the times.
fn to_vtt(text: &str) -> String {
    if text.trim_start().starts_with("WEBVTT") {
        return text.to_owned();
    }
    let mut out = String::from("WEBVTT\n\n");
    for line in text.replace("\r\n", "\n").lines() {
        if line.contains("-->") {
            out.push_str(&line.replace(',', "."));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Takes the subtitle tracks off the video, freeing their files.
fn clear_tracks(video: &web_sys::HtmlVideoElement) {
    while let Ok(Some(old)) = video.query_selector("track") {
        if let Some(src) = old.get_attribute("src")
            && src.starts_with("blob:")
        {
            let _ = web_sys::Url::revoke_object_url(&src);
        }
        old.remove();
    }
}

/// Shows the subtitle stream `index` (or none): fetched as text, given to the video as a
/// track from this page (no cross-origin rules on the video itself). `want` is the choice
/// now: a quicker later choice wins over this one.
async fn show_subtitle(
    l: &Loaded,
    video: &web_sys::HtmlVideoElement,
    index: i64,
    want: RwSignal<i64>,
) {
    clear_tracks(video);
    let Some(src) = l.info.sources.first() else {
        return;
    };
    let Some(stream) = src
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Subtitle && s.index == index)
    else {
        return;
    };
    let session = l.srv.session();
    let url = match (session.kind, &stream.external) {
        (ServerKind::Jellyfin, _) => state::with_token(
            &format!(
                "{}/Videos/{}/{}/Subtitles/{}/Stream.vtt",
                session.base_url, l.item.id, src.id, index
            ),
            session,
        ),
        (_, Some(ext)) => state::with_token(&ext.url, session),
        _ => return,
    };
    let Ok(resp) = state::ctx().http.get(url).send().await else {
        return;
    };
    let Ok(text) = resp.text().await else { return };
    if want.try_get_untracked() != Some(index) {
        return;
    }
    clear_tracks(video);
    let parts = js_sys::Array::of1(&JsValue::from_str(&to_vtt(&text)));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type("text/vtt");
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag) else {
        return;
    };
    let Ok(blob_url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Ok(track) = doc.create_element("track") else {
        return;
    };
    let _ = track.set_attribute("kind", "subtitles");
    let _ = track.set_attribute(
        "label",
        stream.display_title.as_deref().unwrap_or("Subtitles"),
    );
    let _ = track.set_attribute("src", &blob_url);
    let _ = track.set_attribute("default", "");
    let _ = video.append_child(&track);
    // "default" only applies to tracks present at load: show this one explicitly.
    if let Ok(tracks) = js_sys::Reflect::get(video, &"textTracks".into()) {
        let n = js_sys::Reflect::get(&tracks, &"length".into())
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as u32;
        for i in 0..n {
            if let Ok(t) = js_sys::Reflect::get_u32(&tracks, i) {
                let mode = if i + 1 == n { "showing" } else { "disabled" };
                let _ = js_sys::Reflect::set(&t, &"mode".into(), &mode.into());
            }
        }
    }
}

/// The subtitle stream the settings choose at start, among those the browser can show.
fn start_subtitle(l: &Loaded, prefs: &prefs::PlayerPrefs) -> Option<i64> {
    let src = l.info.sources.first()?;
    let drawable = text_subtitles(l);
    let subs: Vec<SubTrack> = src
        .streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle && drawable.iter().any(|(i, _)| *i == s.index))
        .map(|s| SubTrack {
            index: s.index,
            language: s.language.clone(),
            forced: s.is_forced,
        })
        .collect();
    let audio = src
        .streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio)
        .find(|s| s.is_default)
        .or_else(|| src.streams.iter().find(|s| s.kind == StreamKind::Audio))
        .and_then(|s| s.language.clone());
    let lang = if prefs.sub_lang.is_empty() {
        prefs::browser_language()
    } else {
        prefs.sub_lang.clone()
    };
    prefs::pick_subtitle(&subs, audio.as_deref(), &prefs.sub_mode, &lang)
}

/// Out of the player: back where the viewer came from, else to the title's page (a player
/// opened from a link has nothing to go back to in this site).
fn leave(navigate: &impl Fn(&str, leptos_router::NavigateOptions), acc: &str, id: &str) {
    let history = web_sys::window().and_then(|w| w.history().ok());
    match history {
        Some(h) if h.length().unwrap_or(0) > 1 => {
            let _ = h.back();
        }
        _ => navigate(
            &format!("/item/{acc}/{id}"),
            leptos_router::NavigateOptions {
                replace: true,
                ..Default::default()
            },
        ),
    }
}

/// The player page: one per title (rebuilt when the next episode starts), and the watch
/// together room, which outlives the titles and is left when the page closes.
#[component]
pub fn Player() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let room = Room::new();
    let (code, member, start_together) =
        query.with_untracked(|q| (q.get("room"), q.get("member"), q.get("together").is_some()));
    if let (Some(code), Some(member)) = (code, member) {
        room.open(code, member, None);
    }
    on_cleanup(move || room.leave("", true));
    // Notes go away after a few seconds.
    Effect::new(move |_| {
        let Some(n) = room.note.get() else { return };
        spawn_local(async move {
            state::sleep(5000).await;
            if room.note.try_get_untracked().flatten().as_deref() == Some(n.as_str()) {
                room.note.try_set(None);
            }
        });
    });
    let title = Memo::new(move |_| {
        params.with(|p| {
            (
                p.get("acc").unwrap_or_default(),
                p.get("id").unwrap_or_default(),
            )
        })
    });
    let first = StoredValue::new(true);
    move || {
        let (acc, id) = title.get();
        let start_ms: u64 = query
            .with_untracked(|q| q.get("t").and_then(|t| t.parse().ok()))
            .unwrap_or(0);
        let start_together = start_together && first.get_value();
        first.set_value(false);
        view! { <PlayerView acc=acc id=id start_ms=start_ms room=room start_together=start_together /> }
    }
}

#[component]
fn PlayerView(
    acc: String,
    id: String,
    start_ms: u64,
    room: Room,
    start_together: bool,
) -> impl IntoView {
    let navigate = use_navigate();
    let prefs = prefs::load();
    let (auto_skip, auto_next, countdown_s) =
        (prefs.auto_skip_intro, prefs.auto_next, prefs.countdown_s);
    // The video element outlives this page a little: its "pause" and "timeupdate" (sent when
    // it leaves the page) would find this page's state gone. Every handler checks this first.
    let alive = StoredValue::new(());
    let alive = move || alive.try_get_value().is_some();
    // Shown subtitle stream (-1: none), picked from the settings at start.
    let sub_pick = RwSignal::new(-1i64);
    // Next episode countdown: where the credits started showing, and whether it was cancelled.
    let credits_from = RwSignal::new(None::<u64>);
    let cancelled = RwSignal::new(false);
    let auto_skipped = StoredValue::new(None::<u64>);
    let video = NodeRef::<leptos::html::Video>::new();
    let loaded = RwSignal::new(None::<Result<Loaded, String>>);
    // Playback itself failed (after loading): in plain words.
    let failure = RwSignal::new(None::<String>);
    let pos = RwSignal::new(0u64);
    let transcoding = RwSignal::new(false);
    let hls: HlsSlot = StoredValue::new_local(None);
    let last_report = StoredValue::new(0f64);
    let start_sent = StoredValue::new(false);
    let stopped = StoredValue::new(false);
    {
        let (acc, id) = (acc.clone(), id.clone());
        spawn_local(async move {
            let l = load(&acc, &id).await;
            let more = l.as_ref().ok().map(|l| (l.srv.clone(), l.item.clone()));
            loaded.set(Some(l));
            if let Some((srv, item)) = more {
                let (segments, next) = extras(&srv, &item).await;
                loaded.update(|l| {
                    if let Some(Ok(l)) = l {
                        l.segments = segments;
                        l.next = next;
                    }
                });
            }
        });
    }
    let cant_play = move || {
        failure.try_set(Some(
            "This video can't be played in this browser, even converted by the server.".into(),
        ));
    };

    let report = move |event: PlaybackEvent| {
        let Some(v) = video.get_untracked() else {
            return;
        };
        let event = match event {
            PlaybackEvent::Progress if v.paused() => PlaybackEvent::Pause,
            e => e,
        };
        let sub = sub_pick.get_untracked();
        let Some((srv, r)) = loaded.with_untracked(|l| {
            let Some(Ok(l)) = l else { return None };
            let src = l.info.sources.first();
            Some((
                l.srv.clone(),
                ProgressReport {
                    item_id: l.item.id.clone(),
                    media_source_id: src.map(|s| s.id.clone()),
                    play_session_id: l.info.play_session_id.clone(),
                    position_ms: (v.current_time() * 1000.0) as u64,
                    duration_ms: v
                        .duration()
                        .is_finite()
                        .then(|| (v.duration() * 1000.0) as u64),
                    event,
                    audio_index: src.and_then(|s| s.default_audio_index),
                    subtitle_index: (sub >= 0).then_some(sub),
                },
            ))
        }) else {
            return;
        };
        spawn_local(async move {
            let _ = srv.report_progress(&r).await;
        });
    };
    // Stop goes once (leaving, next episode, the room moving on), and nothing after it.
    let stop = move || {
        if stopped.try_get_value() == Some(false) {
            stopped.set_value(true);
            report(PlaybackEvent::Stop);
        }
    };

    // Starts the source once both the data and the element are there (once: the extras
    // arriving later don't restart it).
    let ready = Memo::new(move |_| loaded.with(|l| matches!(l, Some(Ok(_)))));
    Effect::new(move |_| {
        let (true, Some(v)) = (ready.get(), video.get()) else {
            return;
        };
        let Some(Ok(l)) = loaded.get_untracked() else {
            return;
        };
        let Some(src) = l.info.sources.first() else {
            return;
        };
        let session = l.srv.session().clone();
        if src.supports_direct_play && can_direct(src, &l.profile) {
            v.set_src(&state::media_url(&src.direct, &session));
        } else if let Some(t) = &src.transcode {
            transcoding.set(true);
            play_hls(&v, state::media_url(t, &session), hls, cant_play);
        } else {
            return cant_play();
        }
        play(&v);
        // The settings' subtitles, once per title.
        if sub_pick.get_untracked() < 0
            && let Some(i) = start_subtitle(&l, &prefs)
        {
            sub_pick.set(i);
            spawn_local(async move { show_subtitle(&l, &v, i, sub_pick).await });
        }
    });

    // The original file failed: same position, through the server (once; if that fails too,
    // say so).
    let on_error = move |_| {
        if !alive() {
            return;
        }
        let (Some(Ok(l)), Some(v)) = (loaded.get_untracked(), video.get_untracked()) else {
            return;
        };
        // An emptied source (leaving) is no failure.
        if v.get_attribute("src").is_some_and(|s| s.is_empty()) {
            return;
        }
        if transcoding.get_untracked() {
            return cant_play();
        }
        let Some(t) = l.info.sources.first().and_then(|s| s.transcode.clone()) else {
            return cant_play();
        };
        transcoding.set(true);
        play_hls(&v, state::media_url(&t, l.srv.session()), hls, cant_play);
        play(&v);
    };

    on_cleanup(move || {
        stop();
        destroy_hls(hls);
        // Stop downloading: a removed video keeps its connection until collected.
        if let Some(v) = video.get_untracked() {
            clear_tracks(&v);
            let _ = v.pause();
            v.set_src("");
            let _ = v.remove_attribute("src");
            v.load();
        }
    });

    // This title isn't the room's (the room moved to one we don't have): don't send.
    let off_title = StoredValue::new(false);
    let started = StoredValue::new(false);
    let on_meta = move |_| {
        if !alive() {
            return;
        }
        let Some(v) = video.get_untracked() else {
            return;
        };
        let first = !started.get_value();
        started.set_value(true);
        let at = if transcoding.get_untracked() && pos.get_untracked() > 0 {
            pos.get_untracked()
        } else {
            start_ms
        };
        let Some((srv, item)) = loaded.with_untracked(|l| match l {
            Some(Ok(l)) => Some((l.srv.clone(), l.item.clone())),
            _ => None,
        }) else {
            return;
        };
        // In a room: a title chosen here is announced, one chosen by the room follows it.
        if room.active() && first && room.announce.get_value() {
            room.announce.set_value(false);
            room.send(Some((srv.clone(), item.clone())), true, at);
        } else if room.active()
            && room.plays(&item)
            && let Some((exp, playing)) = room.expected()
        {
            v.set_current_time(exp as f64 / 1000.0);
            if !playing {
                let _ = v.pause();
            }
            return;
        }
        if at > 0 {
            v.set_current_time(at as f64 / 1000.0);
        }
        if first && start_together {
            room.create(srv, item, at, true);
        }
    };

    // Plays or pauses like the room and seeks when more than 1.5 s apart.
    let sync = move || {
        let (Some(v), Some((exp, playing))) = (video.get_untracked(), room.expected()) else {
            return;
        };
        if v.ready_state() == 0 {
            return;
        }
        if playing && v.paused() {
            play(&v);
        } else if !playing && !v.paused() {
            let _ = v.pause();
        }
        if together::drifted((v.current_time() * 1000.0) as u64, exp) {
            v.set_current_time(exp as f64 / 1000.0);
        }
    };
    // Our own play, pause or seek: tell the room (its own changes, once applied, match it).
    let local_action = move || {
        if !room.active() || room.busy() || off_title.get_value() {
            return;
        }
        let (Some(v), Some((exp, room_playing))) = (video.get_untracked(), room.expected()) else {
            return;
        };
        if v.ended() || v.ready_state() == 0 {
            return;
        }
        let playing = !v.paused();
        let at = (v.current_time() * 1000.0) as u64;
        if together::local_change(playing, at, room_playing, exp) {
            room.send(None, playing, at);
        }
    };
    // Play pressed while the room plays (after the browser refused to start by itself, say):
    // catch up with the room rather than pull everyone back to where this player stood.
    let on_play = move || {
        if !alive() {
            return;
        }
        if room.active() && !off_title.get_value() && room.expected().is_some_and(|(_, p)| p) {
            sync();
        } else {
            local_action();
        }
    };

    let on_time = move |_| {
        if !alive() {
            return;
        }
        let Some(v) = video.get_untracked() else {
            return;
        };
        let t = v.current_time();
        let p = (t * 1000.0) as u64;
        pos.set(p);
        let seg = loaded.with_untracked(|l| match l {
            Some(Ok(l)) => l
                .segments
                .iter()
                .find(|s| p >= s.start_ms && p + 500 < s.end_ms)
                .cloned(),
            _ => None,
        });
        match &seg {
            // Intros and recaps go by themselves when asked (once: seeking back replays them).
            Some(s)
                if auto_skip
                    && matches!(s.kind, SegmentKind::Intro | SegmentKind::Recap)
                    && auto_skipped.get_value() != Some(s.start_ms) =>
            {
                auto_skipped.set_value(Some(s.start_ms));
                v.set_current_time(s.end_ms as f64 / 1000.0);
            }
            Some(s) if s.kind == SegmentKind::Credits => {
                if credits_from.get_untracked().is_none() {
                    credits_from.set(Some(p));
                }
            }
            _ => {
                if credits_from.get_untracked().is_some() {
                    credits_from.set(None);
                    cancelled.set(false);
                }
            }
        }
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
    let id2 = id.clone();
    let go_next = move || {
        if !alive() || stopped.get_value() {
            return;
        }
        let next = loaded.with_untracked(|l| {
            l.as_ref()
                .and_then(|l| l.as_ref().ok())
                .and_then(|l| l.next.clone())
        });
        stop();
        match next {
            Some(n) => {
                let href = match (room.code.get_untracked(), room.member.get_untracked()) {
                    (Some(code), Some(member)) => {
                        room.announce.set_value(true);
                        together::play_href(&acc2, &n.id, 0, &code, &member)
                    }
                    _ => format!("/play/{}/{}?t=0", acc2, n.id),
                };
                nav2(&href, Default::default())
            }
            // Watching together: stay with the others.
            None if room.active() => {}
            None => leave(&nav2, &acc2, &id2),
        }
    };
    let go_next2 = go_next.clone();
    let has_next = move || loaded.with(|l| matches!(l, Some(Ok(l)) if l.next.is_some()));
    // Seconds before the next episode starts by itself (credits shown, setting on).
    let countdown = Memo::new(move |_| {
        if !auto_next || cancelled.get() || !has_next() {
            return None;
        }
        credits_from
            .get()
            .map(|from| prefs::countdown_left(countdown_s, from, pos.get()))
    });
    let go_next3 = go_next.clone();
    Effect::new(move |_| {
        if countdown.get() == Some(0) {
            cancelled.set(true);
            go_next3();
        }
    });
    let (nav4, acc4, id4) = (navigate.clone(), acc.clone(), id.clone());
    let back = move |_| leave(&nav4, &acc4, &id4);

    // Others' changes (not the one left over from the previous title).
    let nav3 = navigate.clone();
    Effect::new(move |prev: Option<()>| {
        let incoming = room.incoming.get();
        if prev.is_none() {
            return;
        }
        match incoming {
            Some((false, _)) => sync(),
            // The room moved to another title: find it on our servers and play it.
            Some((true, s)) => {
                let nav = nav3.clone();
                spawn_local(async move {
                    match together::find(&s.media).await {
                        None => {
                            room.note.try_set(Some(format!(
                                "{} isn't on your servers",
                                together::media_label(&s.media)
                            )));
                            off_title.try_update_value(|o| *o = true);
                            if let Some(v) = video.try_get_untracked().flatten() {
                                let _ = v.pause();
                            }
                        }
                        Some((acc, item)) => {
                            if !alive() {
                                return;
                            }
                            let (Some(code), Some(member)) = (
                                room.code.try_get_untracked().flatten(),
                                room.member.try_get_untracked().flatten(),
                            ) else {
                                return;
                            };
                            stop();
                            room.announce.set_value(false);
                            let at = room.expected().map_or(0, |(p, _)| p);
                            nav(
                                &together::play_href(&acc, &item, at, &code, &member),
                                Default::default(),
                            );
                        }
                    }
                });
            }
            None => {}
        }
    });

    view! {
        <div class="player">
            <video node_ref=video controls autoplay playsinline
                on:loadedmetadata=on_meta on:timeupdate=on_time on:error=on_error
                on:playing=move |_| {
                    // Start once; later "playing" (after a pause or buffering) is a resume.
                    if !alive() { return }
                    let first = !start_sent.get_value();
                    start_sent.set_value(true);
                    report(if first { PlaybackEvent::Start } else { PlaybackEvent::Progress });
                }
                on:play=move |_| on_play()
                on:seeked=move |_| if alive() { local_action() }
                on:pause=move |_| if alive() && !stopped.get_value() { report(PlaybackEvent::Pause); local_action() }
                on:ended=move |_| go_next() />
            <div class="player-top">
                <button class="icon" on:click=back.clone() aria-label="Back">"←"</button>
                {move || match loaded.get() {
                    Some(Ok(l)) => {
                        let i = &l.item;
                        let sub = (i.kind == ItemKind::Episode).then(|| format!("S{} E{} · {}",
                            i.season_number.unwrap_or(0), i.episode_number.unwrap_or(0), i.title));
                        let title = i.series_title.clone().filter(|_| i.kind == ItemKind::Episode).unwrap_or(i.title.clone());
                        view! { <div><div class="ptitle">{title}</div><div class="muted">{sub}</div></div> }.into_any()
                    }
                    Some(Err(_)) => ().into_any(),
                    None => view! { <div class="muted">"Loading…"</div> }.into_any(),
                }}
                {move || transcoding.get().then(|| view! { <span class="pill">"Converted by the server"</span> })}
                {move || match room.state.get() {
                    Some(s) => {
                        let names = s.members.iter().map(|m| m.name.clone()).collect::<Vec<_>>().join(", ");
                        view! {
                            <span class="pill together" title=names.clone()>{format!("{} watching · {}", s.members.len(), names)}</span>
                            <button class="small" on:click=move |_| room.leave("You left watch together", true)>"Leave"</button>
                        }.into_any()
                    }
                    None => {
                        let can = state::ctx().keep.get().is_some() && loaded.with(|l| matches!(l, Some(Ok(l))
                            if matches!(l.item.kind, ItemKind::Movie | ItemKind::Episode)));
                        (can && !room.active()).then(|| view! {
                            <button class="small" on:click=move |_| {
                                let (Some(Ok(l)), Some(v)) = (loaded.get_untracked(), video.get_untracked()) else { return };
                                room.create(l.srv.clone(), l.item.clone(), (v.current_time() * 1000.0) as u64, !v.paused());
                            }>"Watch together"</button>
                        }).into_any()
                    }
                }}
                {move || {
                    let subs = loaded.with(|l| match l {
                        Some(Ok(l)) => text_subtitles(l),
                        _ => Vec::new(),
                    });
                    (!subs.is_empty()).then(|| view! {
                        <select class="chip subs" aria-label="Subtitles" prop:value=move || sub_pick.get().to_string() on:change=move |ev| {
                            let pick: i64 = event_target_value(&ev).parse().unwrap_or(-1);
                            sub_pick.set(pick);
                            let (Some(Ok(l)), Some(v)) = (loaded.get_untracked(), video.get_untracked()) else { return };
                            spawn_local(async move { show_subtitle(&l, &v, pick, sub_pick).await });
                        }>
                            <option value="-1">"Subtitles off"</option>
                            {subs.into_iter().map(|(i, label)| view! { <option value=i.to_string()>{label}</option> }).collect_view()}
                        </select>
                    })
                }}
            </div>
            {move || {
                let e = failure.get().or_else(|| loaded.with(|l| l.as_ref().and_then(|l| l.as_ref().err().cloned())));
                e.map(|e| view! {
                    <div class="player-msg" role="alert">
                        <p>{e}</p>
                        <button class="primary" on:click=back.clone()>"Back"</button>
                    </div>
                })
            }}
            {move || room.note.get().map(|n| view! { <div class="toast" role="status">{n}</div> })}
            {move || room.request().map(|r| {
                let (a, b) = (r.id.clone(), r.id.clone());
                view! {
                    <div class="knock">
                        <div><strong>{r.name.clone()}</strong>" wants to watch with you"</div>
                        <div class="actions tight">
                            <button class="primary small" on:click=move |_| room.answer(a.clone(), true)>"Let in"</button>
                            <button class="small" on:click=move |_| room.answer(b.clone(), false)>"Not now"</button>
                        </div>
                    </div>
                }
            })}
            {move || current.get().map(|s| {
                let has_next = loaded.with(|l| matches!(l, Some(Ok(l)) if l.next.is_some()));
                let go_next = go_next2.clone();
                match s.kind {
                    SegmentKind::Credits if has_next => view! {
                        <div class="skip-row">
                            {move || countdown.get().map(|_| view! {
                                <button class="small" on:click=move |_| cancelled.set(true)>"Cancel"</button>
                            })}
                            <button class="skip" on:click={
                                let go_next = go_next.clone();
                                move |_| go_next()
                            }>
                                {move || match countdown.get() {
                                    Some(n) => format!("Next episode in {n} s"),
                                    None => "Next episode".to_string(),
                                }}
                            </button>
                        </div>
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

#[cfg(test)]
mod tests {
    use super::*;
    use media_api::{HttpRequest, MediaStream};

    fn stream(kind: StreamKind, index: i64, codec: &str, bits: Option<u32>) -> MediaStream {
        MediaStream {
            index,
            kind,
            codec: codec.into(),
            profile: None,
            language: None,
            title: None,
            display_title: None,
            channels: None,
            channel_layout: None,
            bit_depth: bits,
            width: Some(1920),
            height: Some(1080),
            hdr: None,
            dv_profile: None,
            is_default: false,
            is_forced: false,
            is_external: false,
            external: None,
        }
    }

    fn source(container: &str, streams: Vec<MediaStream>, audio: Option<i64>) -> MediaSource {
        let req = HttpRequest {
            url: String::new(),
            headers: vec![],
        };
        MediaSource {
            id: "1".into(),
            container: Some(container.into()),
            size: None,
            bitrate: None,
            duration_ms: None,
            file_name: None,
            streams,
            direct: req.clone(),
            transcode: Some(req),
            supports_direct_play: true,
            default_audio_index: audio,
            default_subtitle_index: None,
        }
    }

    fn chrome() -> DeviceProfile {
        DeviceProfile {
            max_bitrate: None,
            containers: ["mp4", "m4v", "mov", "webm", "mkv"]
                .map(String::from)
                .into(),
            video_codecs: ["h264", "vp9", "av1"].map(String::from).into(),
            video_10bit_codecs: vec!["av1".into()],
            audio_codecs: ["aac", "mp3", "opus", "flac"].map(String::from).into(),
            subtitle_codecs: vec![],
            max_width: None,
            max_height: None,
            hdr: vec![],
        }
    }

    #[test]
    fn direct_play_only_what_the_browser_decodes() {
        let p = chrome();
        let v = |c, b| stream(StreamKind::Video, 0, c, b);
        let a = |i, c| stream(StreamKind::Audio, i, c, None);
        // H.264 + AAC in Matroska: plays.
        assert!(can_direct(
            &source("mkv", vec![v("h264", Some(8)), a(1, "aac")], Some(1)),
            &p
        ));
        // Jellyfin's container list, and "avc" naming.
        assert!(can_direct(
            &source("mov,mp4,m4a", vec![v("avc", None), a(1, "aac")], None),
            &p
        ));
        // DTS default audio (would play silently): converted, even with an AAC track after it.
        assert!(!can_direct(
            &source(
                "mkv",
                vec![v("h264", None), a(1, "dca"), a(2, "aac")],
                Some(1)
            ),
            &p
        ));
        // HEVC here, 10-bit H.264, AVI: converted.
        assert!(!can_direct(
            &source("mkv", vec![v("hevc", None), a(1, "aac")], None),
            &p
        ));
        assert!(!can_direct(
            &source("mkv", vec![v("h264", Some(10)), a(1, "aac")], None),
            &p
        ));
        assert!(can_direct(
            &source("mkv", vec![v("av1", Some(10)), a(1, "opus")], None),
            &p
        ));
        assert!(!can_direct(
            &source("avi", vec![v("h264", None), a(1, "mp3")], None),
            &p
        ));
        let mut small = p.clone();
        small.max_width = Some(1280);
        assert!(!can_direct(
            &source("mp4", vec![v("h264", None)], None),
            &small
        ));
    }
}
