// SPDX-License-Identifier: AGPL-3.0-or-later
//! Admin dashboard (`/dashboard`): what plays now on Plex and Jellyfin, and for each track
//! whether it plays as is or what the server converts, then bandwidth, CPU and RAM over the
//! last minutes and who watched what over the last days. One server, or all merged.

use std::cell::RefCell;
use std::collections::HashMap;

use i18n::{tr, trn};
use keep_service::dashboard::{
    DashServer, Live, Sample, Stats, Stream, StreamTrack, TopTitle, TopUser,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::keep::admin_error;
use crate::state;
use crate::ui::{ErrorText, Loading};

/// Live readings while the page is open.
const POLL_MS: i32 = 5000;

const LOCAL: &str = "#ffb224";
const REMOTE: &str = "#5b9cff";
const MOVIES: &str = "#3ec7b0";
const SHOWS: &str = "#e8689f";

thread_local! {
    /// Image reference → blob URL (posters do not change while the page is open).
    static IMAGES: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
}

fn remembered_server() -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("alwywatch.dashboard.server").ok().flatten())
        .unwrap_or_else(|| "all".into())
}

fn remember_server(id: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item("alwywatch.dashboard.server", id);
    }
}

fn tz_minutes() -> i32 {
    -(js_sys::Date::new_0().get_timezone_offset() as i32)
}

fn blob_url(bytes: &[u8], mime: &str) -> Option<String> {
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

/// A poster or avatar through the AlwyWatch server (signed-in fetch, shown as a blob).
#[component]
fn Img(server: String, img: Option<String>, #[prop(into)] class: String) -> impl IntoView {
    let url = RwSignal::new(None::<String>);
    if let Some(img) = img {
        if img.starts_with("https://") {
            url.set(Some(img));
        } else {
            let key = format!("{server}|{img}");
            match IMAGES.with(|m| m.borrow().get(&key).cloned()) {
                Some(found) => url.set(found),
                None => {
                    IMAGES.with(|m| m.borrow_mut().insert(key.clone(), None));
                    spawn_local(async move {
                        let Some(k) = state::ctx().keep_client() else {
                            return;
                        };
                        let mut req = state::ctx()
                            .http
                            .get(k.admin_dashboard_image_url(&server, &img));
                        for (name, value) in k.auth_headers() {
                            req = req.header(name, value);
                        }
                        let Ok(res) = req.send().await else { return };
                        if !res.status().is_success() {
                            return;
                        }
                        let mime = res
                            .headers()
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            .filter(|t| t.starts_with("image/"))
                            .unwrap_or("image/jpeg")
                            .to_owned();
                        let Ok(bytes) = res.bytes().await else { return };
                        let blob = blob_url(&bytes, &mime);
                        IMAGES.with(|m| m.borrow_mut().insert(key, blob.clone()));
                        url.set(blob);
                    });
                }
            }
        }
    }
    view! {
        {move || match url.get() {
            Some(u) => view! { <img class=class.clone() src=u alt="" loading="lazy" /> }.into_any(),
            None => view! { <span class=format!("{class} img-empty")></span> }.into_any(),
        }}
    }
}

/// The mark of a server type, as the Android app draws it: a chevron in Plex's yellow, a
/// triangle in Jellyfin's purple-to-blue (simple shapes, not the brands' logo files).
#[component]
pub fn ServerMark(#[prop(into)] kind: String, #[prop(default = 22)] size: u32) -> impl IntoView {
    // The shape fills the middle 54 % of the round badge, as on Android.
    let p = |x: f64, y: f64| format!("{:.1},{:.1}", 23.0 + 54.0 * x, 23.0 + 54.0 * y);
    let shape = if kind == "plex" {
        let d = format!(
            "M{} L{} L{} L{} L{} L{} Z",
            p(0.22, 0.0),
            p(0.52, 0.0),
            p(0.86, 0.5),
            p(0.52, 1.0),
            p(0.22, 1.0),
            p(0.56, 0.5)
        );
        view! { <path d=d fill="#E5A00D" /> }.into_any()
    } else {
        let outer = format!("M{} L{} L{} Z", p(0.5, 0.0), p(1.0, 1.0), p(0.0, 1.0));
        let inner = format!("M{} L{} L{} Z", p(0.5, 0.42), p(0.72, 0.86), p(0.28, 0.86));
        view! {
            <defs>
                <linearGradient id="jf-mark" x1="0" y1="0" x2="1" y2="1">
                    <stop offset="0" stop-color="#AA5CC3" />
                    <stop offset="1" stop-color="#00A4DC" />
                </linearGradient>
            </defs>
            <path d=outer fill="url(#jf-mark)" />
            <path d=inner fill="#000000b8" />
        }
        .into_any()
    };
    let label = if kind == "plex" { "Plex" } else { "Jellyfin" };
    view! {
        <svg class="server-mark" width=size height=size viewBox="0 0 100 100" role="img" aria-label=label>
            <title>{label}</title>
            <circle cx="50" cy="50" r="50" fill="#000000b8" />
            {shape}
        </svg>
    }
}

fn clock(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn mbps(kbps: u32) -> String {
    if kbps >= 1000 {
        format!("{:.1} Mbps", kbps as f64 / 1000.0)
    } else {
        format!("{kbps} kbps")
    }
}

fn hours(seconds: u64) -> String {
    let h = seconds / 3600;
    let m = seconds / 60 % 60;
    match (h, m) {
        (0, m) => tr!("{n} min", n = m),
        (h, 0) => tr!("{n} h", n = h),
        (h, m) => tr!("{h} h {m}", h = h, m = format!("{m:02}")),
    }
}

fn decision_name(d: &str) -> String {
    match d {
        "transcode" => tr!("Transcode"),
        "direct_stream" => tr!("Direct stream"),
        _ => tr!("Direct play"),
    }
}

fn problem_text(p: &str) -> String {
    match p {
        "no_tautulli" => tr!("Plex is read through Tautulli: set it up in Server settings."),
        "no_key" => tr!("Needs this server's API key in Server settings."),
        "unreachable" => tr!("Unreachable right now."),
        other => other.to_owned(),
    }
}

/// One track line: what the file has, then whether it plays as is or what it becomes.
fn track_row(kind: String, t: Option<StreamTrack>) -> impl IntoView {
    t.map(|t| {
        let class = format!("decision {}", t.decision);
        let to = t.to.clone();
        view! {
            <div class="track">
                <span class="track-kind">{kind}</span>
                <span class="track-label">{t.label.clone()}</span>
                <span class=class>
                    {decision_name(&t.decision)}
                    {to.map(|to| view! { " → " <strong>{to}</strong> })}
                </span>
            </div>
        }
    })
}

thread_local! {
    /// Each stream's clock (position, set at Date.now()): moved on between answers.
    static CLOCKS: RefCell<HashMap<String, (i64, f64)>> = RefCell::new(HashMap::new());
}

fn stream_key(s: &Stream) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        s.server_id, s.user, s.device, s.title, s.subtitle
    )
}

fn shown(s: &Stream, (base, at): (i64, f64)) -> i64 {
    let p = if s.state == "playing" {
        base + (js_sys::Date::now() - at) as i64
    } else {
        base
    };
    if s.duration_ms > 0 {
        p.clamp(0, s.duration_ms)
    } else {
        p.max(0)
    }
}

/// A new answer: each stream's clock set again (see `Stream::position_now`).
fn set_clocks(l: &Live) {
    CLOCKS.with(|c| {
        let mut c = c.borrow_mut();
        for s in &l.streams {
            let k = stream_key(s);
            let was = c.get(&k).map(|v| shown(s, *v));
            c.insert(k, (s.position_now(l.now_ms, was), js_sys::Date::now()));
        }
    });
}

fn position(s: &Stream) -> i64 {
    CLOCKS.with(|c| {
        c.borrow()
            .get(&stream_key(s))
            .map_or(s.position_ms, |v| shown(s, *v))
    })
}

fn stream_card(
    s: Stream,
    server: String,
    kind: String,
    merged: bool,
    tick: RwSignal<u64>,
) -> impl IntoView {
    let pos = {
        let s = s.clone();
        move || {
            tick.track();
            position(&s)
        }
    };
    let duration = s.duration_ms;
    let pct = {
        let pos = pos.clone();
        move || {
            if duration > 0 {
                (pos() * 100 / duration).clamp(0, 100)
            } else {
                0
            }
        }
    };
    let state = match s.state.as_str() {
        "paused" => tr!("Paused"),
        "buffering" => tr!("Buffering"),
        _ => tr!("Playing"),
    };
    let converted = [&s.video, &s.audio, &s.subtitle_track]
        .iter()
        .any(|t| t.as_ref().is_some_and(|t| t.decision == "transcode"));
    let where_ = if s.local { tr!("Local") } else { tr!("Remote") };
    let initial = s
        .user
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let sid = s.server_id.clone();
    let (pct_now, pct_bar) = (pct.clone(), pct);
    view! {
        <article class="stream" class:converted=converted>
            <div class="stream-head">
                <div class="poster-wrap">
                    <Img server=sid.clone() img=s.image.clone() class="stream-poster" />
                    <span class="poster-mark"><ServerMark kind=kind.clone() size=24 /></span>
                </div>
                <div class="stream-title">
                    <strong>{s.title.clone()}</strong>
                    <span class="muted small">{s.subtitle.clone()}</span>
                    <span class="muted small">{clock(s.duration_ms)}</span>
                    {merged.then(|| view! { <span class="server-tag">{server.clone()}</span> })}
                </div>
            </div>
            <div class="progress" role="progressbar" aria-valuenow=pct_now aria-valuemin="0" aria-valuemax="100">
                <span style=move || format!("width:{}%", pct_bar())></span>
            </div>
            <div class="stream-body">
                <div class="stream-player">
                    <strong>{s.product.clone()}" — "{s.device.clone()}</strong>
                    <span class="muted small">{state}" — "{move || clock(pos())}" / "{clock(s.duration_ms)}</span>
                    <span class="muted small">{where_}{(!s.ip.is_empty()).then(|| format!(" ({})", s.ip))}{(s.kbps > 0).then(|| format!(" — {}", mbps(s.kbps)))}</span>
                </div>
                {track_row(tr!("Video"), s.video.clone())}
                {track_row(tr!("Audio"), s.audio.clone())}
                {track_row(tr!("Subtitles"), s.subtitle_track.clone())}
            </div>
            <div class="stream-user">
                {match s.user_avatar.clone() {
                    Some(a) => view! { <Img server=sid.clone() img=Some(a) class="avatar-img" /> }.into_any(),
                    None => view! { <span class="avatar-letter">{initial}</span> }.into_any(),
                }}
                <span>{s.user.clone()}</span>
            </div>
        </article>
    }
}

/// (name, colour, value of a sample, None: no point)
type Series = (String, &'static str, fn(&Sample) -> Option<f64>);

/// A line/area chart of one or two series over time, with a hover readout.
#[component]
fn TimeChart(
    #[prop(into)] title: Signal<String>,
    samples: Signal<Vec<Sample>>,
    series: Vec<Series>,
    /// Fixed top of the scale (percentages), else the data's maximum.
    #[prop(optional)]
    max: Option<f64>,
    format: fn(f64) -> String,
) -> impl IntoView {
    const W: f64 = 600.0;
    const H: f64 = 160.0;
    let hover = RwSignal::new(None::<usize>);
    let series = StoredValue::new(series);
    let scale = Memo::new(move |_| {
        let top = max.unwrap_or_else(|| {
            samples.with(|v| {
                v.iter()
                    .flat_map(|s| {
                        series.with_value(|ser| {
                            ser.iter().filter_map(|(_, _, f)| f(s)).collect::<Vec<_>>()
                        })
                    })
                    .fold(0.0, f64::max)
            })
        });
        if top <= 0.0 {
            1.0
        } else {
            top * if max.is_some() { 1.0 } else { 1.15 }
        }
    });
    let x_of = move |i: usize, n: usize| {
        if n <= 1 {
            W
        } else {
            i as f64 * W / (n - 1) as f64
        }
    };
    let paths = move || {
        let top = scale.get();
        samples.with(|v| {
            let n = v.len();
            series.with_value(|ser| {
                ser.iter()
                    .map(|(_, color, f)| {
                        let mut d = String::new();
                        let mut first = true;
                        for (i, s) in v.iter().enumerate() {
                            match f(s) {
                                Some(y) => {
                                    let y = H - (y / top).min(1.0) * H;
                                    d.push_str(&format!("{}{:.1},{:.1} ", if first { "M" } else { "L" }, x_of(i, n), y));
                                    first = false;
                                }
                                None => first = true,
                            }
                        }
                        view! { <path d=d fill="none" stroke=*color stroke-width="2" vector-effect="non-scaling-stroke" stroke-linejoin="round" /> }
                    })
                    .collect_view()
            })
        })
    };
    let on_move = move |e: web_sys::PointerEvent| {
        use wasm_bindgen::JsCast;
        let Some(el) = e
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let r = el.get_bounding_client_rect();
        let n = samples.with(Vec::len);
        if n == 0 || r.width() <= 0.0 {
            return;
        }
        let f = ((e.client_x() as f64 - r.left()) / r.width()).clamp(0.0, 1.0);
        hover.set(Some((f * (n - 1) as f64).round() as usize));
    };
    let readout = move || {
        let i = hover.get()?;
        samples.with(|v| {
            let s = v.get(i)?;
            let when = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(s.t as f64 * 1000.0));
            let time = String::from(when.to_locale_time_string(i18n::language()));
            let left = x_of(i, v.len()) / W * 100.0;
            let rows = series.with_value(|ser| {
                ser.iter()
                    .map(|(name, color, f)| {
                        let val = f(s).map(format).unwrap_or_else(|| "—".into());
                        view! { <div><span class="dot" style=format!("background:{color}")></span>{name.clone()}": "<strong>{val}</strong></div> }
                    })
                    .collect_view()
            });
            Some(view! {
                <div class="cross" style=format!("left:{left:.2}%")></div>
                <div class="tip" style=format!("left:clamp(0px, calc({left:.2}% - 70px), calc(100% - 150px))")>
                    <div class="muted small">{time}</div>
                    {rows}
                </div>
            })
        })
    };
    let last = move || {
        samples.with(|v| {
            let s = v.last()?;
            Some(series.with_value(|ser| {
                ser.iter()
                    .map(|(name, color, f)| {
                        let val = f(s).map(format).unwrap_or_else(|| "—".into());
                        view! { <span class="legend-item"><span class="dot" style=format!("background:{color}")></span>{name.clone()}" "<strong>{val}</strong></span> }
                    })
                    .collect_view()
            }))
        })
    };
    let span = move || {
        samples.with(|v| match (v.first(), v.last()) {
            (Some(a), Some(b)) if b.t > a.t => tr!(
                "last {n} min",
                n = ((b.t - a.t) as f64 / 60.0).round() as i64
            ),
            _ => String::new(),
        })
    };
    view! {
        <section class="dash-panel">
            <div class="panel-head"><h2>{title}</h2><span class="muted small">{span}</span></div>
            <div class="legend">{last}</div>
            <div class="chart" on:pointermove=on_move on:pointerleave=move |_| hover.set(None)>
                <div class="grid-label top">{move || format(scale.get())}</div>
                <div class="grid-label mid">{move || format(scale.get() / 2.0)}</div>
                <svg viewBox=format!("0 0 {W} {H}") preserveAspectRatio="none" aria-hidden="true">
                    <line x1="0" x2=W y1=H / 2.0 y2=H / 2.0 class="gridline" vector-effect="non-scaling-stroke" />
                    <line x1="0" x2=W y1=H y2=H class="baseline" vector-effect="non-scaling-stroke" />
                    {paths}
                </svg>
                {readout}
            </div>
        </section>
    }
}

fn pct(v: f64) -> String {
    format!("{v:.0} %")
}

fn kbps_fmt(v: f64) -> String {
    mbps(v.round() as u32)
}

#[component]
fn LivePanels(
    live: RwSignal<Option<Live>>,
    merged: Signal<bool>,
    tick: RwSignal<u64>,
) -> impl IntoView {
    let samples = Signal::derive(move || {
        live.with(|l| l.as_ref().map(|l| l.samples.clone()).unwrap_or_default())
    });
    let ram_total = move || live.with(|l| l.as_ref().map_or(0, |l| l.ram_total_mb));
    let streams = move || {
        let l = live.get()?;
        let n = l.streams.len();
        let transcodes = l
            .streams
            .iter()
            .filter(|s| {
                [&s.video, &s.audio, &s.subtitle_track]
                    .iter()
                    .any(|t| t.as_ref().is_some_and(|t| t.decision == "transcode"))
            })
            .count();
        let total: u32 = l.streams.iter().map(|s| s.kbps).sum();
        let summary = if n == 0 {
            tr!("Nothing is playing.")
        } else {
            format!(
                "{} · {} · {}",
                trn!("{n} stream", "{n} streams", n),
                if transcodes == 0 {
                    tr!("all direct")
                } else {
                    trn!("{n} converted", "{n} converted", transcodes)
                },
                mbps(total)
            )
        };
        let m = merged.get();
        let cards: Vec<_> = l
            .streams
            .into_iter()
            .map(|s| {
                let found = l.servers.iter().find(|x| x.id == s.server_id);
                let name = found.map(|x| x.name.clone()).unwrap_or_default();
                let kind = found.map(|x| x.kind.clone()).unwrap_or_default();
                stream_card(s, name, kind, m, tick)
            })
            .collect();
        Some(view! {
            <section class="dash-panel wide">
                <div class="panel-head"><h2>{tr!("Now playing")}</h2><span class="muted small">{summary}</span></div>
                <div class="streams">
                    {cards}
                </div>
            </section>
        })
    };
    view! {
        {streams}
        <div class="dash-grid">
            <TimeChart title=tr!("Bandwidth") samples=samples format=kbps_fmt
                series=vec![
                    (tr!("Remote"), REMOTE, |s: &Sample| Some(s.remote_kbps as f64)),
                    (tr!("Local"), LOCAL, |s: &Sample| Some(s.local_kbps as f64)),
                ] />
            <TimeChart title=tr!("CPU") samples=samples format=pct max=100.0
                series=vec![(tr!("Server"), LOCAL, |s: &Sample| s.cpu.map(f64::from))] />
            <TimeChart title=Signal::derive(move || {
                    let t = ram_total();
                    if t > 0 { format!("{} · {:.0} GB", tr!("RAM"), t as f64 / 1024.0) } else { tr!("RAM") }
                }) samples=samples format=pct max=100.0
                series=vec![(tr!("Server"), REMOTE, |s: &Sample| s.ram.map(f64::from))] />
        </div>
    }
}

fn user_card(u: TopUser) -> impl IntoView {
    let initial = u
        .name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    view! {
        <div class="top-user">
            <div class="top-user-head">
                {match u.avatar.clone() {
                    Some(a) => view! { <Img server=u.server_id.clone() img=Some(a) class="avatar-img big" /> }.into_any(),
                    None => view! { <span class="avatar-letter big">{initial}</span> }.into_any(),
                }}
                <div>
                    <strong>{u.name.clone()}</strong>
                    <div class="muted small">{trn!("{n} play", "{n} plays", u.plays)}</div>
                    <div class="muted small">{hours(u.seconds)}</div>
                </div>
            </div>
            <div class="kv"><span>{tr!("TV")}</span><span>{hours(u.episode_seconds)}</span></div>
            <div class="kv"><span>{tr!("Movies")}</span><span>{hours(u.movie_seconds)}</span></div>
        </div>
    }
}

fn title_list(title: String, list: Vec<TopTitle>) -> impl IntoView {
    view! {
        <div class="top-list">
            <h3>{title}</h3>
            {if list.is_empty() {
                view! { <p class="muted small">{tr!("Nothing yet.")}</p> }.into_any()
            } else {
                list.into_iter().map(|t| view! {
                    <div class="top-title">
                        <Img server=t.server_id.clone() img=t.image.clone() class="mini-poster" />
                        <div>
                            <strong>{t.title.clone()}</strong>
                            <div class="muted small">{trn!("{n} play", "{n} plays", t.plays)}</div>
                            <div class="muted small">{trn!("{n} person", "{n} people", t.users)}</div>
                        </div>
                    </div>
                }).collect_view().into_any()
            }}
        </div>
    }
}

/// Plays per day, movies and episodes stacked, with a hover readout.
fn history_chart(stats: &Stats) -> impl IntoView {
    let days = stats.history.clone();
    let top = days
        .iter()
        .map(|d| d.movies + d.episodes)
        .max()
        .unwrap_or(0)
        .max(1) as f64
        * 1.1;
    let hover = RwSignal::new(None::<usize>);
    let n = days.len().max(1);
    let days2 = days.clone();
    let bars = days
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let w = 100.0 / n as f64;
            let ep = d.episodes as f64 / top * 100.0;
            let mv = d.movies as f64 / top * 100.0;
            view! {
                <div class="hbar" style=format!("left:{:.3}%;width:{:.3}%", i as f64 * w, w)
                    on:pointerenter=move |_| hover.set(Some(i))>
                    <span class="seg" style=format!("height:{ep:.2}%;background:{SHOWS}")></span>
                    <span class="seg" style=format!("height:{mv:.2}%;background:{MOVIES}")></span>
                </div>
            }
        })
        .collect_view();
    let first = days.first().map(|d| d.date.clone()).unwrap_or_default();
    let last = days.last().map(|d| d.date.clone()).unwrap_or_default();
    let tip = move || {
        let i = hover.get()?;
        let d = days2.get(i)?;
        let left = (i as f64 + 0.5) * 100.0 / n as f64;
        Some(view! {
            <div class="tip" style=format!("left:clamp(0px, calc({left:.2}% - 70px), calc(100% - 150px))")>
                <div class="muted small">{d.date.clone()}</div>
                <div><span class="dot" style=format!("background:{SHOWS}")></span>{tr!("TV")}": "<strong>{d.episodes}</strong>" ("{hours(d.episode_seconds)}")"</div>
                <div><span class="dot" style=format!("background:{MOVIES}")></span>{tr!("Movies")}": "<strong>{d.movies}</strong>" ("{hours(d.movie_seconds)}")"</div>
            </div>
        })
    };
    let total_ep: u32 = days.iter().map(|d| d.episodes).sum();
    let total_mv: u32 = days.iter().map(|d| d.movies).sum();
    view! {
        <div class="legend">
            <span class="legend-item"><span class="dot" style=format!("background:{SHOWS}")></span>{tr!("TV")}" "<strong>{total_ep}</strong></span>
            <span class="legend-item"><span class="dot" style=format!("background:{MOVIES}")></span>{tr!("Movies")}" "<strong>{total_mv}</strong></span>
        </div>
        <div class="chart bars" on:pointerleave=move |_| hover.set(None)>
            <div class="grid-label top">{format!("{:.0}", top)}</div>
            {bars}
            {tip}
        </div>
        <div class="axis-dates muted small"><span>{first}</span><span>{last}</span></div>
    }
}

#[component]
fn StatsPanels(server: Signal<String>, servers: Signal<Vec<DashServer>>) -> impl IntoView {
    let days = RwSignal::new(30u32);
    let stats = RwSignal::new(None::<Result<Stats, String>>);
    Effect::new(move |_| {
        let (asked, d) = (server.get(), days.get());
        stats.set(None);
        spawn_local(async move {
            let Some(k) = state::ctx().keep_client() else {
                return;
            };
            let r = k
                .admin_dashboard_stats(&asked, d, tz_minutes())
                .await
                .map_err(|e| admin_error(&e));
            // Only if still asked for (and still on the page: try_ when it was left).
            if server.try_get_untracked().as_ref() == Some(&asked)
                && days.try_get_untracked() == Some(d)
            {
                stats.set(Some(r));
            }
        });
    });
    view! {
        <div class="panel-head stats-head">
            <h2>{tr!("Over the last days")}</h2>
            <div class="chips">
                {[7u32, 30, 90].into_iter().map(|d| view! {
                    <button class="chip small" class:selected=move || days.get() == d on:click=move |_| days.set(d)>
                        {trn!("{n} day", "{n} days", d)}
                    </button>
                }).collect_view()}
            </div>
        </div>
        {move || match stats.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
            Some(Ok(st)) => {
                let names: HashMap<String, String> = servers.get().into_iter().map(|s| (s.id, s.name)).collect();
                let since = st.since.iter().map(|s| {
                    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(s.since as f64 * 1000.0));
                    let date = String::from(d.to_locale_date_string(i18n::language(), &wasm_bindgen::JsValue::UNDEFINED));
                    tr!("{server}: history since {date}", server = names.get(&s.server_id).cloned().unwrap_or_default(), date = date)
                }).collect::<Vec<_>>();
                let decided = st.direct_plays + st.transcodes;
                view! {
                    {(!since.is_empty()).then(|| view! { <p class="muted small">{since.join(" · ")}</p> })}
                    <section class="dash-panel wide">
                        <div class="panel-head"><h2>{tr!("Top users")}</h2></div>
                        <div class="top-users">
                            {if st.top_users.is_empty() {
                                view! { <p class="muted small">{tr!("Nothing yet.")}</p> }.into_any()
                            } else {
                                st.top_users.clone().into_iter().map(user_card).collect_view().into_any()
                            }}
                        </div>
                    </section>
                    <section class="dash-panel wide">
                        <div class="panel-head">
                            <h2>{tr!("Play history")}</h2>
                            {(decided > 0).then(|| view! {
                                <span class="muted small">{tr!("{direct} direct play · {transcodes} converted (Plex)", direct = st.direct_plays, transcodes = st.transcodes)}</span>
                            })}
                        </div>
                        {history_chart(&st)}
                    </section>
                    <section class="dash-panel wide">
                        <div class="panel-head"><h2>{tr!("Top played")}</h2></div>
                        <div class="top-lists">
                            {title_list(tr!("Movies"), st.top_movies.clone())}
                            {title_list(tr!("TV"), st.top_shows.clone())}
                        </div>
                    </section>
                }.into_any()
            }
        }}
    }
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let server = RwSignal::new(remembered_server());
    // The last good reading stays on screen when a poll fails (a note says so).
    let live = RwSignal::new(None::<Live>);
    // Every second: the streams' positions move on.
    let tick = RwSignal::new(0u64);
    let error = RwSignal::new(None::<String>);
    // Kept apart from the readings: a failed poll or a new pick leaves the chips in place.
    let servers = RwSignal::new(Vec::<DashServer>::new());
    // Leaving the page disposes these signals: the loop reads them with try_ and stops.
    let alive = RwSignal::new(true);
    on_cleanup(move || alive.set(false));
    let open = move || alive.try_get_untracked().unwrap_or(false);
    // Poll while the page is open and visible.
    spawn_local(async move {
        let mut asked = String::new();
        while open() {
            let hidden = web_sys::window()
                .and_then(|w| w.document())
                .is_some_and(|d| d.hidden());
            let Some(now) = server.try_get_untracked() else {
                return;
            };
            if !hidden || asked != now {
                let Some(k) = state::ctx().keep_client() else {
                    error.set(Some(tr!("Sign in with an admin account first.")));
                    return;
                };
                let r = k.admin_dashboard_live(&now).await;
                if !open() {
                    return;
                }
                if server.try_get_untracked().as_ref() == Some(&now) {
                    match r {
                        Ok(l) => {
                            servers.update(|list| {
                                if now == "all" {
                                    *list = l.servers.clone();
                                } else {
                                    // One server's reading: refresh its entry, keep the others.
                                    for s in &l.servers {
                                        match list.iter_mut().find(|x| x.id == s.id) {
                                            Some(x) => *x = s.clone(),
                                            None => list.push(s.clone()),
                                        }
                                    }
                                }
                            });
                            set_clocks(&l);
                            live.set(Some(l));
                            error.set(None);
                        }
                        Err(e) => error.set(Some(match e {
                            net::Error::Network { .. } => tr!("Can't reach the server right now."),
                            e => admin_error(&e),
                        })),
                    }
                    asked = now;
                }
            }
            // Wake early when the server choice changes.
            for i in 0..(POLL_MS / 250) {
                state::sleep(250).await;
                if i % 4 == 3 {
                    let _ = tick.try_update(|t| *t += 1);
                }
                if !open() || server.try_get_untracked().as_ref() != Some(&asked) {
                    break;
                }
            }
        }
    });
    let merged = Signal::derive(move || server.get() == "all");
    let pick = move |id: String| {
        remember_server(&id);
        // Another server's readings: not shown under this one's chip.
        live.set(None);
        error.set(None);
        server.set(id);
    };
    view! {
        <h1>{tr!("Dashboard")}</h1>
        <div class="chips" role="tablist">
            <button class="chip" role="tab" class:selected=move || server.get() == "all" on:click=move |_| pick("all".into())>{tr!("All servers")}</button>
            {move || servers.get().into_iter().map(|s| {
                let id = s.id.clone();
                let id2 = s.id.clone();
                view! {
                    <button class="chip with-mark" role="tab" class:selected=move || server.get() == id on:click=move |_| pick(id2.clone())><ServerMark kind=s.kind.clone() size=20 />{s.name.clone()}</button>
                }
            }).collect_view()}
        </div>
        {move || servers.get().into_iter().filter(|s| s.problem.is_some() && (server.get() == "all" || server.get() == s.id)).map(|s| view! {
            <p class="banner" role="status">{format!("{}: {}", s.name, problem_text(s.problem.as_deref().unwrap_or_default()))}</p>
        }).collect_view()}
        {move || match (live.with(Option::is_some), error.get()) {
            (false, None) => view! { <Loading /> }.into_any(),
            (false, Some(e)) => view! { <ErrorText text=e /> }.into_any(),
            // Still the last reading: a small note, not an empty page.
            (true, Some(e)) => view! { <p class="muted small" role="status">{e}</p> }.into_any(),
            (true, None) => ().into_any(),
        }}
        <Show when=move || live.with(Option::is_some)>
            <LivePanels live=live merged=merged tick=tick />
        </Show>
        <StatsPanels server=server.into() servers=servers.into() />
    }
}
