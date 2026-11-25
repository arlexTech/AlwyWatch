// SPDX-License-Identifier: AGPL-3.0-or-later
//! The player: the browser's video element (its own controls, keyboard and fullscreen), with
//! skip intro/credits, next episode and progress sync on top.
//!
//! The original file plays directly whenever the browser says it can; if it then fails (an
//! audio codec it can't decode, a container it only claimed), the server's HLS transcode takes
//! over at the same position. HLS plays through hls.js (loaded only then, from this site),
//! natively only where there is no MediaSource (iPhone).

use i18n::{tr, trn};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use media_api::{
    AnyServer, DeviceProfile, Item, ItemKind, MediaServer, MediaSource, MediaStream, PlayMethod,
    PlaybackEvent, PlaybackInfo, ProgressReport, Quality, Segment, SegmentKind, ServerKind,
    StreamKind, SubtitleSelection, TrackSelection,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

use crate::prefs::{self, SubTrack};
use crate::profile::Avatar;
use crate::state::{self, fmt_time};
use crate::together::{self, Room};

/// −10 s / +30 s, like the apps' defaults.
const SEEK_BACK_S: u32 = 10;
const SEEK_FORWARD_S: u32 = 30;
/// How long the bubble stays, and presses on its side keep adding to it.
const SEEK_BUBBLE_MS: u64 = 900;

/// The "+30" / "−10" bubble: its side, the seconds added up, one count per press.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SeekBubble {
    forward: bool,
    seconds: u32,
    n: u32,
    at: f64,
    fading: bool,
}

impl SeekBubble {
    /// After a press at `now`: adds up while presses on the same side follow each other.
    fn next(prev: Option<&Self>, forward: bool, step: u32, now: f64) -> Self {
        let follows = prev.filter(|p| p.forward == forward && now - p.at < SEEK_BUBBLE_MS as f64);
        SeekBubble {
            forward,
            seconds: follows.map_or(0, |p| p.seconds) + step,
            n: prev.map_or(0, |p| p.n) + 1,
            at: now,
            fading: false,
        }
    }
}

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
        // As the Android app. None let Jellyfin size a conversion from the audio's bitrate
        // alone: 416×234 video for a 1080p film.
        max_bitrate: Some(200_000_000),
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

/// HLS through the browser itself only where hls.js can't run (iPhone Safari: no
/// MediaSource). Chrome now says "maybe" for HLS too, but its own player stalled on Jellyfin's
/// conversions (the playlist loaded, never a segment, no error): hls.js wherever it can.
fn native_hls() -> bool {
    let mse = web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"MediaSource".into()).ok())
        .is_some_and(|m| !m.is_undefined());
    !mse && test_video()
        .is_some_and(|v| !v.can_play_type("application/vnd.apple.mpegurl").is_empty())
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
        .ok_or_else(|| tr!("This account isn't signed in any more."))?;
    let profile = browser_profile();
    let (item, info) = state::join2(srv.item(id), srv.playback_info(id, &profile)).await;
    let fail = |e: media_api::Error| {
        c.failed(acc, &e);
        state::plain(&e)
    };
    let item = item.map_err(fail)?;
    let info = info.map_err(fail)?;
    if info.sources.is_empty() {
        return Err(tr!("The server has no file to play for this title."));
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
        .map(|s| (s.index, sub_label(s)))
        .collect()
}

fn sub_label(s: &MediaStream) -> String {
    s.display_title
        .clone()
        .or(s.title.clone())
        .or(s.language.clone())
        .unwrap_or_else(|| tr!("Subtitles"))
}

/// The subtitles offered: those the page draws, or every one while the server converts (it
/// burns in those the page can't draw).
fn menu_subtitles(l: &Loaded, converting: bool) -> Vec<(i64, String)> {
    if !converting {
        return text_subtitles(l);
    }
    let Some(src) = l.info.sources.first() else {
        return Vec::new();
    };
    src.streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle)
        .map(|s| (s.index, sub_label(s)))
        .collect()
}

/// What the server's conversion must carry for subtitle stream `pick` (-1: none): burned in
/// when the page can't draw it (image subtitles; Plex's embedded ones, which it doesn't hand
/// out), else none (the page draws it). Always explicit, so the server adds none of its own.
fn server_subtitle(l: &Loaded, pick: i64) -> SubtitleSelection {
    if pick < 0 || text_subtitles(l).iter().any(|(i, _)| *i == pick) {
        SubtitleSelection::Off
    } else {
        SubtitleSelection::Stream { index: pick }
    }
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
        &stream
            .display_title
            .clone()
            .unwrap_or_else(|| tr!("Subtitles")),
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

/// The subtitle stream the settings choose at start, among those offered.
fn start_subtitle(l: &Loaded, prefs: &prefs::PlayerPrefs, converting: bool) -> Option<i64> {
    let src = l.info.sources.first()?;
    let drawable = menu_subtitles(l, converting);
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
        // Only when a page of this app came first (the browser's history also counts other
        // sites: a player link opened in a used tab would go back to one of them).
        Some(h) if state::came_from_app() => {
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

/// The player page: one per title (rebuilt when the next episode starts). In a watch
/// together room (`?room=`, the app's seat in it), it follows the room; the seat outlives the
/// page (back to the Room page keeps it).
#[component]
pub fn Player() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let room = state::ctx().room;
    let title = Memo::new(move |_| {
        params.with(|p| {
            (
                p.get("acc").unwrap_or_default(),
                p.get("id").unwrap_or_default(),
            )
        })
    });
    move || {
        let (acc, id) = title.get();
        let (start_ms, room_code) = query.with_untracked(|q| {
            (
                q.get("t").and_then(|t| t.parse::<u64>().ok()).unwrap_or(0),
                q.get("room"),
            )
        });
        view! { <PlayerView acc=acc id=id start_ms=start_ms room=room room_code=room_code /> }
    }
}

#[component]
fn PlayerView(
    acc: String,
    id: String,
    start_ms: u64,
    room: Room,
    room_code: Option<String>,
) -> impl IntoView {
    let navigate = use_navigate();
    // Playing in the room this tab has a seat in (not after leaving it).
    let room_code = StoredValue::new(room_code);
    let joined = move || {
        room_code
            .try_with_value(|c| c.as_deref().is_some_and(|c| room.is(c)))
            .unwrap_or(false)
    };
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
    // The audio track picked in the menu (None: the file's default). Browsers can't switch
    // tracks of a file they play themselves: another one is asked of the server's conversion.
    let audio_pick = RwSignal::new(None::<i64>);
    let hls: HlsSlot = StoredValue::new_local(None);
    let last_report = StoredValue::new(0f64);
    let start_sent = StoredValue::new(false);
    let stopped = StoredValue::new(false);
    // The quality menu: the file as the server has it, the size picked (None: the original
    // when the browser plays it), and a switch in progress (resumes where it was).
    let original = RwSignal::new(None::<PlaybackInfo>);
    let quality = RwSignal::new(None::<Quality>);
    let quality_switch = StoredValue::new(false);
    // ←/→ and double taps on the picture: −10 s / +30 s, added up in a bubble on that side.
    let bubble = RwSignal::new(None::<SeekBubble>);
    let bubble_shape = Memo::new(move |_| bubble.with(|b| b.map(|b| (b.forward, b.seconds, b.n))));
    let seek_by = move |forward: bool| {
        let Some(v) = video.get_untracked() else {
            return;
        };
        let secs = if forward { SEEK_FORWARD_S } else { SEEK_BACK_S };
        let dur = v.duration();
        let step = f64::from(secs);
        let mut to = v.current_time() + if forward { step } else { -step };
        if dur.is_finite() {
            to = to.min(dur - 0.5);
        }
        v.set_current_time(to.max(0.0));
        let now = js_sys::Date::now();
        let next = bubble.with_untracked(|b| SeekBubble::next(b.as_ref(), forward, secs, now));
        let n = next.n;
        bubble.set(Some(next));
        set_timeout(
            move || {
                // The player may have gone meanwhile.
                if bubble.try_with_untracked(|b| b.as_ref().is_some_and(|b| b.n == n)) == Some(true)
                {
                    bubble.update(|b| {
                        if let Some(b) = b {
                            b.fading = true;
                        }
                    });
                }
            },
            std::time::Duration::from_millis(SEEK_BUBBLE_MS - 250),
        );
    };
    // Fullscreen on the whole player (the video's own button would leave out Skip intro, the
    // next-episode countdown, subtitles and Back).
    let shell = NodeRef::<leptos::html::Div>::new();
    let fullscreen = move || {
        let doc = leptos::prelude::document();
        if doc.fullscreen_element().is_some() {
            doc.exit_fullscreen();
        } else if let Some(p) = shell.get_untracked() {
            let _ = p.request_fullscreen();
        }
    };
    let keys = window_event_listener(leptos::ev::keydown, move |ev| {
        let typing = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
            .is_some_and(|e| matches!(e.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT"));
        if typing || ev.alt_key() || ev.ctrl_key() || ev.meta_key() {
            return;
        }
        let forward = match ev.key().as_str() {
            "ArrowRight" => true,
            "ArrowLeft" => false,
            // Wherever the focus is on the page (the video's own keys need it focused).
            k @ (" " | "k" | "K" | "m" | "M") => {
                let Some(v) = video.get_untracked() else {
                    return;
                };
                // A focused button or the video itself already answers Space.
                let on_control = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
                    .is_some_and(|e| matches!(e.tag_name().as_str(), "BUTTON" | "VIDEO" | "A"));
                if k == " " && on_control {
                    return;
                }
                ev.prevent_default();
                if k.eq_ignore_ascii_case("m") {
                    v.set_muted(!v.muted());
                } else if v.paused() {
                    let _ = v.play();
                } else {
                    let _ = v.pause();
                }
                return;
            }
            "f" | "F" => {
                ev.prevent_default();
                fullscreen();
                return;
            }
            _ => return,
        };
        // Instead of the browser's own 5 s.
        ev.prevent_default();
        seek_by(forward);
    });
    on_cleanup(move || keys.remove());
    // Touch: two taps on the same half of the picture seek, and each tap that follows soon
    // after repeats it. The second tap's click is cancelled (no native seek or controls).
    let last_tap = StoredValue::new((0f64, false));
    let on_touch_end = move |ev: web_sys::TouchEvent| {
        let (Some(v), Some(t)) = (video.get_untracked(), ev.changed_touches().get(0)) else {
            return;
        };
        let r = v.get_bounding_client_rect();
        let (x, y) = (
            f64::from(t.client_x()) - r.left(),
            f64::from(t.client_y()) - r.top(),
        );
        // The bottom strip is the browser's control bar; the middle third pauses there.
        if y > r.height() - 72.0 || (x > r.width() / 3.0 && x < r.width() * 2.0 / 3.0) {
            return;
        }
        let forward = x > r.width() / 2.0;
        let now = js_sys::Date::now();
        let (at, side) = last_tap.get_value();
        let following = bubble.with_untracked(|b| {
            b.as_ref()
                .is_some_and(|b| b.forward == forward && now - b.at < SEEK_BUBBLE_MS as f64)
        });
        if following || (side == forward && now - at < 300.0) {
            ev.prevent_default();
            seek_by(forward);
            last_tap.set_value((0.0, forward));
        } else {
            last_tap.set_value((now, forward));
        }
    };
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
        failure.try_set(Some(tr!(
            "This video can't be played in this browser, even converted by the server."
        )));
    };

    // The subtitles the current conversion was asked with (Default: none asked).
    let burned = StoredValue::new(SubtitleSelection::Default);
    let build_report = move |event: PlaybackEvent| -> Option<(AnyServer, ProgressReport)> {
        let v = video.get_untracked()?;
        let event = match event {
            PlaybackEvent::Progress if v.paused() => PlaybackEvent::Pause,
            e => e,
        };
        let sub = sub_pick.get_untracked();
        let play_method = if transcoding.get_untracked() {
            PlayMethod::Transcode
        } else {
            PlayMethod::Direct
        };
        loaded.with_untracked(|l| {
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
                    audio_index: audio_pick
                        .try_get_untracked()
                        .flatten()
                        .or(src.and_then(|s| s.default_audio_index)),
                    subtitle_index: (sub >= 0).then_some(sub),
                    play_method,
                },
            ))
        })
    };
    let report = move |event: PlaybackEvent| {
        let Some((srv, r)) = build_report(event) else {
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

    // Plays the server's conversion carrying subtitle `pick` (-1: none; burned in when the page
    // can't draw it). `restart`: one is playing with other subtitles; it is reported stopped
    // first (Plex ends a session reported stopped, Jellyfin its conversion), then the new one
    // starts where it was (`pos`, see `on_meta`) and reports a fresh start.
    let convert = move |v: web_sys::HtmlVideoElement, pick: i64, restart: bool| {
        let Some(Ok(l)) = loaded.get_untracked() else {
            return;
        };
        let Some(fallback) = l
            .info
            .sources
            .first()
            .map(|s| (s.id.clone(), s.transcode.clone()))
        else {
            return;
        };
        // Jellyfin lists no conversion for a file it thinks plays as is (the browser disagreed,
        // or failed on it): one is asked for at full size (the server copies what it can).
        let force = fallback.1.is_none();
        let sel = TrackSelection {
            source_id: Some(fallback.0),
            audio_index: audio_pick.get_untracked(),
            subtitle: server_subtitle(&l, pick),
            quality: quality
                .get_untracked()
                .or(force.then_some(media_api::quality::FULL_SIZE)),
            reencode: false,
        };
        let stop_old = restart.then(|| build_report(PlaybackEvent::Stop)).flatten();
        transcoding.set(true);
        spawn_local(async move {
            if let Some((srv, r)) = stop_old {
                let _ = srv.report_progress(&r).await;
            }
            let info = l
                .srv
                .playback_info_with(&l.item.id, &l.profile, &sel)
                .await
                .ok()
                .filter(|i| i.sources.first().is_some_and(|s| s.transcode.is_some()));
            // Gone, or another pick since: that one plays.
            if !alive() || sub_pick.try_get_untracked() != Some(pick) {
                return;
            }
            let t = match &info {
                Some(i) => i.sources.first().and_then(|s| s.transcode.clone()),
                None => fallback.1,
            };
            let Some(t) = t else { return cant_play() };
            burned.set_value(if info.is_some() {
                sel.subtitle
            } else {
                SubtitleSelection::Default
            });
            if let Some(info) = info {
                loaded.update(|l| {
                    if let Some(Ok(l)) = l {
                        l.info = info;
                    }
                });
            }
            if restart {
                start_sent.set_value(false);
            }
            play_hls(&v, state::media_url(&t, l.srv.session()), hls, cant_play);
            play(&v);
            // Drawn by the page (not burned in).
            match loaded.get_untracked() {
                Some(Ok(l))
                    if pick >= 0
                        && burned.get_value() != (SubtitleSelection::Stream { index: pick }) =>
                {
                    show_subtitle(&l, &v, pick, sub_pick).await
                }
                _ => clear_tracks(&v),
            }
        });
    };

    // The browser said it plays the file, but some don't play without saying so: a picture it
    // can't decode stays black, a sound it can't decode stays silent, a file it chokes on never
    // starts, all with no error event. Checked a few seconds in: then the server converts it,
    // at the same place, as if the file had failed.
    let direct_try = StoredValue::new(0u32);
    let watch_direct = move |v: web_sys::HtmlVideoElement| {
        direct_try.update_value(|n| *n += 1);
        let mine = direct_try.get_value();
        spawn_local(async move {
            for wait in [8_000, 12_000] {
                state::sleep(wait).await;
                if !alive()
                    || transcoding.get_untracked()
                    || direct_try.try_get_value() != Some(mine)
                {
                    return;
                }
                let (has_video, has_audio) = loaded.with_untracked(|l| match l {
                    Some(Ok(l)) => l.info.sources.first().map_or((false, false), |s| {
                        (
                            s.streams.iter().any(|x| x.kind == StreamKind::Video),
                            s.streams.iter().any(|x| x.kind == StreamKind::Audio),
                        )
                    }),
                    _ => (false, false),
                });
                // Paused (autoplay refused, or the viewer paused): nothing to judge.
                let trying = !v.paused();
                let stuck = trying && v.ready_state() < 2;
                let no_picture = has_video && v.ready_state() >= 1 && v.video_width() == 0;
                let prop = |name: &str| {
                    js_sys::Reflect::get(&v, &name.into())
                        .ok()
                        .filter(|x| !x.is_undefined())
                };
                let no_sound = has_audio
                    && trying
                    && v.current_time() > 3.0
                    && (prop("webkitAudioDecodedByteCount").and_then(|x| x.as_f64()) == Some(0.0)
                        || prop("mozHasAudio").and_then(|x| x.as_bool()) == Some(false));
                if stuck || no_picture || no_sound {
                    return convert(v, sub_pick.get_untracked(), true);
                }
            }
        });
    };

    // A size picked in the menu: the conversion restarts at it, here; "Original" goes back to
    // the file itself when the browser plays it (else to the server's own conversion).
    let pick_quality = move |q: Option<Quality>| {
        let (Some(v), Some(info), Some(Ok(l))) = (
            video.get_untracked(),
            original.get_untracked(),
            loaded.get_untracked(),
        ) else {
            return;
        };
        if quality.get_untracked() == q {
            return;
        }
        quality.set(q);
        pos.set((v.current_time() * 1000.0) as u64);
        let Some(src) = info.sources.first().cloned() else {
            return;
        };
        if q.is_none() && src.supports_direct_play && can_direct(&src, &l.profile) {
            if transcoding.get_untracked() {
                if let Some((srv, r)) = build_report(PlaybackEvent::Stop) {
                    spawn_local(async move {
                        let _ = srv.report_progress(&r).await;
                    });
                }
                start_sent.set_value(false);
            }
            destroy_hls(hls);
            transcoding.set(false);
            quality_switch.set_value(true);
            burned.set_value(SubtitleSelection::Default);
            loaded.update(|x| {
                if let Some(Ok(x)) = x {
                    x.info = info;
                }
            });
            v.set_src(&state::media_url(&src.direct, l.srv.session()));
            play(&v);
            watch_direct(v.clone());
            let pick = sub_pick.get_untracked();
            if pick >= 0 {
                spawn_local(async move { show_subtitle(&l, &v, pick, sub_pick).await });
            }
        } else {
            convert(v, sub_pick.get_untracked(), true);
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
        original.set(Some(l.info.clone()));
        let session = l.srv.session().clone();
        if src.supports_direct_play && can_direct(src, &l.profile) {
            v.set_src(&state::media_url(&src.direct, &session));
            play(&v);
            watch_direct(v.clone());
            // The settings' subtitles, once per title.
            if sub_pick.get_untracked() < 0
                && let Some(i) = start_subtitle(&l, &prefs, false)
            {
                sub_pick.set(i);
                spawn_local(async move { show_subtitle(&l, &v, i, sub_pick).await });
            }
        } else {
            // Converted: the server must know the subtitles before it starts (it would add
            // its own pick otherwise).
            let pick = start_subtitle(&l, &prefs, true).unwrap_or(-1);
            sub_pick.set(pick);
            convert(v, pick, false);
        }
    });

    // The original file failed: same position, through the server (once; if that fails too,
    // say so).
    let on_error = move |_| {
        if !alive() {
            return;
        }
        let (Some(Ok(_)), Some(v)) = (loaded.get_untracked(), video.get_untracked()) else {
            return;
        };
        // An emptied source (leaving) is no failure.
        if v.get_attribute("src").is_some_and(|s| s.is_empty()) {
            return;
        }
        if transcoding.get_untracked() {
            return cant_play();
        }
        convert(v, sub_pick.get_untracked(), false);
    };

    // Leaving the player of a room: the host takes everyone back to the waiting room (to pick
    // something else; nobody leaves). Not when the page makes way for the room's next title.
    let switching = StoredValue::new(false);
    on_cleanup(move || {
        if switching.try_get_value() == Some(false)
            && joined()
            && room.hosting_untracked()
            && room
                .state
                .with_untracked(|s| s.as_ref().is_some_and(|s| s.started))
        {
            room.lobby(room.expected().map_or(0, |(p, _)| p));
        }
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
    // "Watch together" pressed while watching alone: the room dialog, where it starts from.
    let dialog = RwSignal::new(false);
    let dialog_at = RwSignal::new(0u64);
    let picking = RwSignal::new(false);
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
        let switched = quality_switch
            .try_update_value(std::mem::take)
            .unwrap_or(false);
        let at = if (transcoding.get_untracked() || switched) && pos.get_untracked() > 0 {
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
        if joined() && first && room.announce.get_value() {
            room.announce.set_value(false);
            room.send(Some((srv.clone(), item.clone())), true, at);
        } else if joined()
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
    };

    // Stays in step with the room every half second: slightly faster or slower for small drifts,
    // a seek past 2 s (`together_correction`, shared with the apps). Not right after our own
    // change (the room hasn't echoed it yet) nor while a seek lands.
    let own_hold = StoredValue::new(0.0f64);
    let seek_hold = StoredValue::new(0.0f64);
    // False once a speed change didn't close the gap: seeks only.
    let rate_works = StoredValue::new(true);
    let rate_check = StoredValue::new((0.0f64, 0u64));
    // A start: paused at the room's position until every player has loaded the title, then
    // play at the instant the room says (the same for everyone, on the server's clock).
    let loaded_since = StoredValue::new(None::<f64>);
    let start_timer = StoredValue::new(0i64);
    let wait_start = move |v: web_sys::HtmlVideoElement, phase: together::Phase, at: u64| {
        if !v.paused() {
            let _ = v.pause();
        }
        if (v.playback_rate() - 1.0).abs() > 0.0001 {
            v.set_playback_rate(1.0);
        }
        let cur = (v.current_time() * 1000.0) as u64;
        if !v.seeking() && v.ready_state() >= 1 && cur.abs_diff(at) > 300 {
            v.set_current_time(at as f64 / 1000.0);
        }
        match &phase {
            together::Phase::StartsIn(ms) => {
                let key = room
                    .state
                    .with_untracked(|s| s.as_ref().map_or(0, |s| s.at_ms));
                if start_timer.get_value() != key {
                    start_timer.set_value(key);
                    let delay = std::time::Duration::from_millis((*ms).max(0) as u64);
                    set_timeout(
                        move || {
                            if !alive() || !joined() {
                                return;
                            }
                            if let (Some(v), Some((_, true))) =
                                (video.get_untracked(), room.expected())
                            {
                                play(&v);
                            }
                            room.waiting.try_set(None);
                        },
                        delay,
                    );
                }
            }
            together::Phase::Loading { me: true, .. } if !room.ready_sent() => {
                let here = v.ready_state() >= 3 && !v.seeking() && cur.abs_diff(at) <= 500;
                let now = together::now_ms();
                if !here {
                    loaded_since.set_value(None);
                } else if loaded_since.get_value().is_none() {
                    loaded_since.set_value(Some(now));
                }
                let t = v.current_time();
                let b = v.buffered();
                let end = (0..b.length())
                    .filter_map(|i| Some((b.start(i).ok()?, b.end(i).ok()?)))
                    .find(|(s, e)| *s <= t + 0.5 && t <= *e)
                    .map_or(t, |(_, e)| e);
                let d = v.duration();
                let enough = (end - t) * 1000.0 >= keep_service::together::READY_BUFFER_MS as f64
                    || (d.is_finite() && end >= d - 1.0)
                    || loaded_since.get_value().is_some_and(|s| now - s > 8_000.0);
                if here && enough {
                    room.ready();
                }
            }
            _ => {}
        }
        let text = together::waiting_text(&phase, room.ready_sent());
        if room.waiting.get_untracked() != text {
            room.waiting.set(text);
        }
    };
    let follow = move || {
        if !alive() || !joined() || room.busy() || off_title.get_value() {
            return;
        }
        if let (Some(v), Some((phase, at, _))) = (video.get_untracked(), room.phase())
            && phase != together::Phase::Live
        {
            return wait_start(v, phase, at);
        }
        if room.waiting.get_untracked().is_some() {
            room.waiting.set(None);
        }
        let now = together::now_ms();
        if now < own_hold.get_value() || now < seek_hold.get_value() {
            return;
        }
        let (Some(v), Some((exp, playing))) = (video.get_untracked(), room.expected()) else {
            return;
        };
        // HAVE_FUTURE_DATA; and only while playing/paused like the room.
        if v.ready_state() < 3 || v.paused() == playing || v.ended() {
            return;
        }
        let at = (v.current_time() * 1000.0) as u64;
        let current = v.playback_rate();
        let c = keep_service::together::together_correction(
            exp,
            at,
            playing,
            current,
            rate_works.get_value(),
        );
        if let Some(to) = c.seek_to_ms {
            v.set_current_time(to as f64 / 1000.0);
            seek_hold.set_value(now + 2_000.0);
        }
        let drift = at.abs_diff(exp);
        if (c.rate - current).abs() > 0.0001 {
            if (current - 1.0).abs() < 0.0001 {
                rate_check.set_value((now + 6_000.0, drift));
            }
            v.set_playback_rate(c.rate);
        } else if (current - 1.0).abs() > 0.0001 {
            let (when, before) = rate_check.get_value();
            if now >= when {
                if drift * 10 > before * 8 {
                    rate_works.set_value(false);
                    v.set_playback_rate(1.0);
                }
                rate_check.set_value((now + 6_000.0, drift));
            }
        }
    };
    if let Ok(h) = set_interval_with_handle(follow, std::time::Duration::from_millis(500)) {
        on_cleanup(move || h.clear());
    }

    // Plays or pauses like the room, then follows its position.
    let sync = move || {
        if !joined() {
            return;
        }
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
        let _ = exp;
        own_hold.set_value(0.0);
        follow();
    };
    // Our own play, pause or seek: tell the room (its own changes, once applied, match it).
    let local_action = move || {
        if !joined() || room.busy() || off_title.get_value() {
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
            own_hold.set_value(together::now_ms() + 2_000.0);
            room.send(None, playing, at);
        }
    };
    // Play pressed while the room plays (after the browser refused to start by itself, say):
    // catch up with the room rather than pull everyone back to where this player stood.
    let on_play = move || {
        if !alive() {
            return;
        }
        if joined() && !off_title.get_value() && room.expected().is_some_and(|(_, p)| p) {
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
                let href = match room.code.get_untracked().filter(|_| joined()) {
                    Some(code) => {
                        switching.set_value(true);
                        room.announce.set_value(true);
                        together::play_href(&acc2, &n.id, 0, &code)
                    }
                    None => format!("/play/{}/{}?t={}", acc2, n.id, n.user.position_ms),
                };
                // In place of the one that ended: Back leaves the player instead of replaying it.
                nav2(
                    &href,
                    leptos_router::NavigateOptions {
                        replace: true,
                        ..Default::default()
                    },
                )
            }
            // Watching together: stay with the others.
            None if joined() => {}
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
    let nav5 = navigate.clone();
    let (nav6, acc6, id6) = (navigate.clone(), acc.clone(), id.clone());

    // Others' changes (not the one left over from the previous title).
    let nav3 = navigate.clone();
    Effect::new(move |prev: Option<()>| {
        let incoming = room.incoming.get();
        if prev.is_none() || !joined() {
            return;
        }
        match incoming {
            // The host went back to the waiting room: so do we (the room stays).
            Some((false, s)) if !s.started => {
                if !room.hosting_untracked() {
                    switching.set_value(true);
                    nav3(&together::room_href(&s.code), Default::default());
                }
            }
            Some((false, _)) => sync(),
            // The room moved to another title: find it on our servers and play it.
            Some((true, s)) => {
                let nav = nav3.clone();
                spawn_local(async move {
                    match together::find(&s.media).await {
                        None => {
                            room.note.try_set(Some(tr!(
                                "{title} isn't on your servers",
                                title = together::media_label(&s.media)
                            )));
                            off_title.try_update_value(|o| *o = true);
                            if let Some(v) = video.try_get_untracked().flatten() {
                                let _ = v.pause();
                            }
                        }
                        Some((acc, item)) => {
                            if !alive() || !joined() {
                                return;
                            }
                            let Some(code) = room.code.try_get_untracked().flatten() else {
                                return;
                            };
                            stop();
                            let _ = switching.try_set_value(true);
                            room.announce.set_value(false);
                            let at = room.expected().map_or(0, |(p, _)| p);
                            nav(
                                &together::play_href(&acc, &item, at, &code),
                                Default::default(),
                            );
                        }
                    }
                });
            }
            None => {}
        }
    });

    // The tab closing or the page going away without the app's own Back: Stop all the same.
    let gone = window_event_listener(leptos::ev::pagehide, move |_| stop());
    on_cleanup(move || gone.remove());
    // The bar over the picture fades after a few still seconds while playing (a move, a tap
    // or a key brings it back; paused, it stays).
    let idle = RwSignal::new(false);
    let wake_seq = StoredValue::new(0u32);
    let wake = move || {
        idle.set(false);
        wake_seq.update_value(|n| *n += 1);
        let mine = wake_seq.get_value();
        spawn_local(async move {
            state::sleep(3000).await;
            if wake_seq.try_get_value() == Some(mine)
                && video.get_untracked().is_some_and(|v| !v.paused())
            {
                idle.try_set(true);
            }
        });
    };
    view! {
        <div class="player" node_ref=shell class:idle=move || idle.get()
            on:mousemove=move |_| wake() on:touchstart=move |_| wake() on:keydown=move |_| wake()
            on:focusin=move |_| wake()>
            <video node_ref=video controls autoplay playsinline controlslist="nofullscreen"
                on:dblclick=move |_| fullscreen() on:touchend=on_touch_end
                on:loadedmetadata=on_meta on:timeupdate=on_time on:error=on_error
                on:playing=move |_| {
                    // Start once; later "playing" (after a pause or buffering) is a resume.
                    if !alive() { return }
                    let first = !start_sent.get_value();
                    start_sent.set_value(true);
                    report(if first { PlaybackEvent::Start } else { PlaybackEvent::Progress });
                }
                on:play=move |_| { on_play(); wake() }
                on:seeked=move |_| if alive() {
                    local_action();
                    // Closing the tab right after a seek keeps the new position.
                    if start_sent.get_value() && !stopped.get_value() { report(PlaybackEvent::Progress) }
                }
                on:pause=move |_| { idle.set(false); if alive() && !stopped.get_value() { report(PlaybackEvent::Pause); local_action() } }
                on:ended=move |_| {
                    // The next episode only with the setting on and the countdown not cancelled.
                    if auto_next && !cancelled.get_untracked() {
                        go_next()
                    } else if alive() && !joined() {
                        stop();
                        leave(&nav6, &acc6, &id6);
                    }
                } />
            // Re-created on each press (the arrow turns again); fading only changes a class.
            {move || bubble_shape.get().map(|(forward, seconds, _)| view! {
                <div class="seek-bubble" class:forward=forward aria-hidden="true"
                    class:fading=move || bubble.with(|b| b.is_some_and(|b| b.fading))>
                    <svg viewBox="0 -960 960 960"><path d=crate::icons::REPLAY /></svg>
                    <span>{if forward { format!("+{seconds}") } else { format!("−{seconds}") }}</span>
                </div>
            })}
            <div class="player-top">
                <button class="pbtn" on:click=back.clone() aria-label=tr!("Back") title=tr!("Back")><crate::icons::Icon path=crate::icons::ARROW_BACK /></button>
                {move || match loaded.get() {
                    Some(Ok(l)) => {
                        let i = &l.item;
                        let sub = (i.kind == ItemKind::Episode).then(|| tr!("S{season} E{episode} · {title}",
                            season = i.season_number.unwrap_or(0), episode = i.episode_number.unwrap_or(0), title = i.title));
                        let title = i.series_title.clone().filter(|_| i.kind == ItemKind::Episode).unwrap_or(i.title.clone());
                        view! { <div class="ptitles"><div class="ptitle">{title}</div><div class="psub">{sub}</div></div> }.into_any()
                    }
                    Some(Err(_)) => ().into_any(),
                    None => view! { <div class="ptitles psub">{tr!("Loading…")}</div> }.into_any(),
                }}
                <div class="pspacer" />
                // Quality: what plays now, and a menu to pick another size.
                {move || {
                    let src = original.get().and_then(|i| i.sources.into_iter().next());
                    // Any server file: the server converts on request (Jellyfin lists no
                    // conversion for a file it thinks plays as is).
                    match src {
                        Some(src) => {
                            let converted = transcoding.get();
                            let chosen = quality.get();
                            let now = media_api::quality::quality_label(&src, converted, chosen.filter(|_| converted));
                            let choices = media_api::quality::quality_choices(&src);
                            let picks: Vec<Option<Quality>> = choices.iter().map(|c| c.quality).collect();
                            view! {
                                <span class="pill" title=format!("{}: {now}", tr!("Playing now"))>{now.clone()}</span>
                                <select class="chip small quality" aria-label=tr!("Quality")
                                    on:change=move |ev| {
                                        if let Some(q) = event_target_value(&ev).parse::<usize>().ok().and_then(|i| picks.get(i)) {
                                            pick_quality(*q);
                                        }
                                    }>
                                    {choices.into_iter().enumerate().map(|(i, c)| view! {
                                        <option value=i.to_string() selected=c.quality == chosen>{c.label}</option>
                                    }).collect_view()}
                                </select>
                            }.into_any()
                        }
                        None => transcoding.get().then(|| view! { <span class="pill">{tr!("Converted by the server")}</span> }).into_any(),
                    }
                }}
                {move || match room.state.get().filter(|_| { room.code.track(); joined() }) {
                    Some(s) => {
                        let people = together::group_members(&s);
                        let names = people.iter().map(|p| p.member.name.clone()).collect::<Vec<_>>().join(", ");
                        let shown = people.len();
                        view! {
                            <span class="pill together" title=names.clone()>
                                <span class="faces">
                                    {people.into_iter().take(4).map(|p| view! {
                                        <Avatar user_key=p.member.user_key.clone() name=p.member.name.clone() avatar=p.member.avatar.clone() size=20 />
                                    }).collect_view()}
                                </span>
                                {trn!("{n} watching · {names}", "{n} watching · {names}", shown, names = names)}
                            </span>
                            <button class="small" on:click=move |_| room.leave(&tr!("You left watch together"), true)>{tr!("Leave")}</button>
                        }.into_any()
                    }
                    None => {
                        let can = state::ctx().keep.get().is_some() && loaded.with(|l| matches!(l, Some(Ok(l))
                            if matches!(l.item.kind, ItemKind::Movie | ItemKind::Episode)));
                        let _ = &nav5;
                        (can && room.code.get().is_none()).then(|| view! {
                            <button class="small" on:click=move |_| {
                                let Some(Ok(l)) = loaded.get_untracked() else { return };
                                // Paused, the room over the video: invite, then start from here.
                                if let Some(v) = video.get_untracked() {
                                    let _ = v.pause();
                                    dialog_at.set((v.current_time() * 1000.0) as u64);
                                }
                                dialog.set(true);
                                spawn_local(async move {
                                    if let Err(e) = together::create_room(l.srv.clone(), l.item.clone()).await {
                                        dialog.try_set(false);
                                        room.note.try_set(Some(tr!("Watch together unavailable: {error}", error = e)));
                                    } else if !dialog.try_get_untracked().unwrap_or(false) {
                                        // Cancelled meanwhile.
                                        room.leave("", true);
                                    }
                                });
                            }>{tr!("Watch together")}</button>
                        }).into_any()
                    }
                }}
                {move || {
                    let tracks = loaded.with(|l| match l {
                        Some(Ok(l)) => l.info.sources.first().map(|src| {
                            let list: Vec<_> = src.streams.iter().filter(|s| s.kind == StreamKind::Audio)
                                .map(|s| (s.index, s.display_title.clone().or(s.title.clone()).or(s.language.clone())
                                    .unwrap_or_else(|| tr!("Track {number}", number = s.index))))
                                .collect();
                            (list, src.default_audio_index)
                        }),
                        _ => None,
                    });
                    let (list, default) = tracks?;
                    (list.len() > 1).then(|| view! {
                        <select class="chip subs" aria-label=tr!("Audio") title=tr!("Audio")
                            prop:value=move || audio_pick.get().or(default).map(|i| i.to_string()).unwrap_or_default()
                            on:change=move |ev| {
                                let Ok(i) = event_target_value(&ev).parse::<i64>() else { return };
                                audio_pick.set(Some(i));
                                let Some(v) = video.get_untracked() else { return };
                                // At the same place, from the server's conversion with this track.
                                convert(v, sub_pick.get_untracked(), true);
                            }>
                            {list.into_iter().map(|(i, label)| view! { <option value=i.to_string()>{label}</option> }).collect_view()}
                        </select>
                    })
                }}
                {move || {
                    let converting = transcoding.get();
                    // Every one: those the page can't draw (PGS, VobSub) are burned in by the
                    // server's conversion when picked.
                    let _ = converting;
                    let subs = loaded.with(|l| match l {
                        Some(Ok(l)) => menu_subtitles(l, true),
                        _ => Vec::new(),
                    });
                    (!subs.is_empty()).then(|| view! {
                        <select class="chip subs" aria-label=tr!("Subtitles") title=tr!("Subtitles") prop:value=move || sub_pick.get().to_string() on:change=move |ev| {
                            let pick: i64 = event_target_value(&ev).parse().unwrap_or(-1);
                            sub_pick.set(pick);
                            let (Some(Ok(l)), Some(v)) = (loaded.get_untracked(), video.get_untracked()) else { return };
                            // Converted and the server's part changes (burn another one, or stop
                            // burning): ask it again, at the same place.
                            if transcoding.get_untracked() && server_subtitle(&l, pick) != burned.get_value() {
                                return convert(v, pick, true);
                            }
                            // Playing the file itself, and this one only the server can draw.
                            if !transcoding.get_untracked() && pick >= 0 && !text_subtitles(&l).iter().any(|(i, _)| *i == pick) {
                                return convert(v, pick, true);
                            }
                            spawn_local(async move { show_subtitle(&l, &v, pick, sub_pick).await });
                        }>
                            <option value="-1">{tr!("Subtitles off")}</option>
                            {subs.into_iter().map(|(i, label)| view! { <option value=i.to_string()>{label}</option> }).collect_view()}
                        </select>
                    })
                }}
                <button class="pbtn" on:click=move |_| fullscreen() aria-label=tr!("Full screen") title=tr!("Full screen (F)")><crate::icons::Icon path=crate::icons::FULLSCREEN /></button>
            </div>
            {move || {
                let e = failure.get().or_else(|| loaded.with(|l| l.as_ref().and_then(|l| l.as_ref().err().cloned())));
                e.map(|e| view! {
                    <div class="player-msg" role="alert">
                        <p>{e}</p>
                        <button class="primary" on:click=back.clone()>{tr!("Back")}</button>
                    </div>
                })
            }}
            {move || room.waiting.get().filter(|_| joined()).or_else(|| room.note.get()).map(|n| view! { <div class="toast" role="status">{n}</div> })}
            {move || dialog.get().then(|| {
                let start = move |_| {
                    let Some((code, me)) = room.ids() else { return };
                    let Some(k) = state::ctx().keep_client() else { return };
                    let from = video.get_untracked().map_or(dialog_at.get_untracked(), |v| (v.current_time() * 1000.0) as u64);
                    spawn_local(async move {
                        match k.together_start(&code, &me, from).await {
                            Ok(s) => {
                                if !alive() {
                                    return;
                                }
                                room.handled_round.set_value(s.round);
                                room_code.set_value(Some(code));
                                dialog.set(false);
                                room.on_state(s);
                                follow();
                            }
                            Err(e) => {
                                room.note.try_set(Some(together::plain_together(&e)));
                            }
                        }
                    });
                };
                let cancel = move |_| {
                    dialog.set(false);
                    picking.set(false);
                    room.leave("", true);
                    if let Some(v) = video.get_untracked() {
                        play(&v);
                    }
                };
                view! {
                    <div class="dialog-back">
                        <div class="dialog room-dialog" role="dialog" aria-modal="true" aria-labelledby="room-dialog-title">
                            <p class="muted small">{tr!("WATCH TOGETHER")}</p>
                            <h2 id="room-dialog-title">{move || room.state.with(|s| s.as_ref().map(|s| together::media_label(&s.media))).unwrap_or_else(|| tr!("Opening a room…"))}</h2>
                            <p class="muted">{move || tr!(
                                "Paused for now. Invite people, then start: everyone starts at {time} at the same moment, once all of them have loaded it.",
                                time = fmt_time(dialog_at.get())
                            )}</p>
                            {move || room.state.get().map(|s| {
                                let people = together::group_members(&s);
                                view! {
                                    {people.into_iter().map(|p| view! {
                                        <div class="person-row">
                                            <Avatar user_key=p.member.user_key.clone() name=p.member.name.clone() avatar=p.member.avatar.clone() size=32 />
                                            <span class="pname">{p.member.name.clone()}</span>
                                            <span class="muted small">{tr!("In the room")}</span>
                                        </div>
                                    }).collect_view()}
                                    {s.invited.iter().map(|i| view! {
                                        <div class="person-row">
                                            <Avatar user_key=i.user_key.clone() name=i.name.clone() avatar=i.avatar.clone() size=32 />
                                            <span class="pname">{i.name.clone()}</span>
                                            <span class="muted small">{tr!("Invited")}</span>
                                        </div>
                                    }).collect_view()}
                                }
                            })}
                            <div class="actions">
                                <button disabled=move || room.state.with(|s| s.is_none()) on:click=move |_| picking.set(true)>{tr!("Invite people")}</button>
                                <button class="primary" disabled=move || room.state.with(|s| s.is_none()) on:click=start>
                                    {move || tr!("Start at {time}", time = fmt_time(dialog_at.get()))}
                                </button>
                                <button class="subtle" on:click=cancel>{tr!("Cancel")}</button>
                            </div>
                        </div>
                    </div>
                    {move || picking.get().then(|| view! { <together::PeoplePicker room=room on_close=move || picking.set(false) /> })}
                }
            })}
            <Show when=move || state::ctx().keep.get().is_some()>
                <together::InviteBanner compact=true />
            </Show>
            {move || current.get().map(|s| {
                let has_next = loaded.with(|l| matches!(l, Some(Ok(l)) if l.next.is_some()));
                let go_next = go_next2.clone();
                match s.kind {
                    SegmentKind::Credits if has_next => view! {
                        <div class="skip-row">
                            {move || countdown.get().map(|_| view! {
                                <button class="small" on:click=move |_| cancelled.set(true)>{tr!("Cancel")}</button>
                            })}
                            <button class="skip" on:click={
                                let go_next = go_next.clone();
                                move |_| go_next()
                            }>
                                {move || match countdown.get() {
                                    Some(n) => tr!("Next episode in {seconds} s", seconds = n),
                                    None => tr!("Next episode"),
                                }}
                            </button>
                        </div>
                    }.into_any(),
                    k => {
                        let label = match k {
                            SegmentKind::Intro => tr!("Skip intro"),
                            SegmentKind::Credits => tr!("Skip credits"),
                            SegmentKind::Recap => tr!("Skip recap"),
                            SegmentKind::Preview => tr!("Skip preview"),
                            SegmentKind::Commercial => tr!("Skip ad"),

                        };
                        view! { <button class="skip" on:click=skip>{label}</button> }.into_any()
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

#[cfg(test)]
mod seek_tests {
    use super::*;

    #[test]
    fn seek_presses_add_up_on_one_side_only() {
        let a = SeekBubble::next(None, true, 30, 0.0);
        let b = SeekBubble::next(Some(&a), true, 30, 500.0);
        assert_eq!((b.seconds, b.n), (60, 2));
        // The other side, or too late: starts again.
        assert_eq!(SeekBubble::next(Some(&b), false, 10, 600.0).seconds, 10);
        assert_eq!(SeekBubble::next(Some(&b), true, 30, 2_000.0).seconds, 30);
    }
}
