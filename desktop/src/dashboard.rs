// SPDX-License-Identifier: AGPL-3.0-or-later
//! Admin dashboard (Settings → Admin → Dashboard): what plays now on Plex and Jellyfin with each
//! track direct or converted, bandwidth/CPU/RAM over the last minutes, and who watched what.
//! Polled every 5 s while the page is shown, like the web app's /dashboard.

use std::rc::Rc;
use std::time::Duration;

use i18n::{tr, trn};
use keep_service::dashboard::{Live, Sample, Stats, Stream, StreamTrack};
use slint::{Color, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use super::{App, async_sleep, sstr};
use crate::{DashChart, DashRow, DashServerChip, DashStream, DashTitle, DashTrack, DashUser, Page};

const POLL: Duration = Duration::from_secs(5);
/// Chart box (the Slint paths use the same viewbox).
const W: f64 = 600.0;
const H: f64 = 160.0;

#[derive(Default)]
pub struct Dash {
    server: String,
    days: u32,
    /// Bumped on every opening: an older polling loop sees it and stops.
    generation: u64,
    live: Option<Live>,
    stats: Option<Stats>,
}

fn rgb(hex: u32) -> Color {
    Color::from_rgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn mbps(kbps: u64) -> String {
    if kbps >= 1000 {
        format!("{:.1} Mbps", kbps as f64 / 1000.0)
    } else {
        format!("{kbps} kbps")
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

fn hours(seconds: u64) -> String {
    let (h, m) = (seconds / 3600, seconds / 60 % 60);
    match (h, m) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

fn decision(t: &StreamTrack) -> String {
    let name = match t.decision.as_str() {
        "transcode" => tr!("Transcode"),
        "direct_stream" => tr!("Direct stream"),
        _ => tr!("Direct play"),
    };
    match &t.to {
        Some(to) => format!("{name} → {to}"),
        None => name,
    }
}

fn converted(s: &Stream) -> bool {
    [&s.video, &s.audio, &s.subtitle_track]
        .iter()
        .any(|t| t.as_ref().is_some_and(|t| t.decision == "transcode"))
}

fn problem_text(p: &str) -> String {
    match p {
        "no_tautulli" => tr!("Plex is read through Tautulli: set it up in Server settings."),
        "no_key" => tr!("Needs this server's API key in Server settings."),
        "unreachable" => tr!("Unreachable right now."),
        other => other.to_owned(),
    }
}

/// SVG path commands of one series in the chart box (gaps where a value is missing).
fn line(samples: &[Sample], top: f64, f: impl Fn(&Sample) -> Option<f64>) -> String {
    let n = samples.len();
    let mut d = String::new();
    let mut first = true;
    for (i, s) in samples.iter().enumerate() {
        match f(s) {
            Some(v) => {
                let x = if n <= 1 {
                    W
                } else {
                    i as f64 * W / (n - 1) as f64
                };
                let y = H - (v / top).min(1.0) * H;
                d.push_str(&format!("{}{x:.1} {y:.1} ", if first { "M" } else { "L" }));
                first = false;
            }
            None => first = true,
        }
    }
    d
}

impl App {
    pub(super) fn dash_open(self: &Rc<Self>) {
        let generation = {
            let mut d = self.dash.borrow_mut();
            if d.server.is_empty() {
                d.server = self
                    .core
                    .get_setting("dashboard.server".into())
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "all".into());
                d.days = 30;
            }
            d.generation += 1;
            d.generation
        };
        let ui = self.ui();
        ui.set_dash_server(sstr(&self.dash.borrow().server));
        ui.set_dash_days(self.dash.borrow().days as i32);
        self.dash_load_stats();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            loop {
                let server = {
                    let Some(t) = this.upgrade() else { return };
                    if t.dash.borrow().generation != generation
                        || t.ui().get_page() != Page::Dashboard
                    {
                        return;
                    }
                    t.dash.borrow().server.clone()
                };
                let r = core.keep_admin_dashboard_live(server.clone()).await;
                let Some(t) = this.upgrade() else { return };
                if t.dash.borrow().server == server {
                    match r {
                        Ok(l) => {
                            t.dash.borrow_mut().live = Some(l);
                            t.ui().set_dash_message(Default::default());
                            t.dash_render_live();
                        }
                        Err(e) => t.ui().set_dash_message(sstr(e.to_string())),
                    }
                }
                drop(t);
                async_sleep(POLL).await;
            }
        })
        .ok();
    }

    pub(super) fn dash_pick(self: &Rc<Self>, server: String) {
        self.core
            .set_setting("dashboard.server".into(), server.clone())
            .ok();
        {
            let mut d = self.dash.borrow_mut();
            d.server = server;
            d.live = None;
            d.stats = None;
        }
        self.dash_open();
    }

    pub(super) fn dash_days(self: &Rc<Self>, days: i32) {
        self.dash.borrow_mut().days = days.clamp(1, 90) as u32;
        self.ui().set_dash_days(days);
        self.dash_load_stats();
    }

    fn dash_load_stats(self: &Rc<Self>) {
        let (server, days) = {
            let d = self.dash.borrow();
            (d.server.clone(), d.days)
        };
        self.ui().set_dash_stats_message(sstr(tr!("Loading…")));
        let tz = crate::localtime::utc_offset_minutes();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .keep_admin_dashboard_stats(server.clone(), days, tz)
                .await;
            let Some(t) = this.upgrade() else { return };
            let current = {
                let d = t.dash.borrow();
                d.server == server && d.days == days
            };
            if !current {
                return;
            }
            match r {
                Ok(s) => {
                    t.dash.borrow_mut().stats = Some(s);
                    t.ui().set_dash_stats_message(Default::default());
                    t.dash_render_stats();
                }
                Err(e) => t.ui().set_dash_stats_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// The image of a dashboard reference, or empty while it loads (the page is redrawn when it
    /// arrives).
    fn dash_image(self: &Rc<Self>, server: &str, img: Option<&str>) -> Image {
        let Some(img) = img else {
            return Image::default();
        };
        let key = format!("dash|{server}|{img}");
        if let Some(i) = self.images.borrow().get(&key) {
            return i.clone();
        }
        self.images
            .borrow_mut()
            .insert(key.clone(), Image::default());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        let (server, img) = (server.to_owned(), img.to_owned());
        slint::spawn_local(async move {
            let Ok(bytes) = core.keep_admin_dashboard_image(server, img).await else {
                return;
            };
            let decoded = core
                .runtime()
                .spawn_blocking(move || {
                    let i = image::load_from_memory(&bytes).ok()?.into_rgba8();
                    let (w, h) = i.dimensions();
                    Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        i.as_raw(),
                        w,
                        h,
                    ))
                })
                .await
                .ok()
                .flatten();
            let (Some(t), Some(buf)) = (this.upgrade(), decoded) else {
                return;
            };
            t.images.borrow_mut().insert(key, Image::from_rgba8(buf));
            t.dash_render_live();
            t.dash_render_stats();
        })
        .ok();
        Image::default()
    }

    fn dash_render_live(self: &Rc<Self>) {
        let Some(l) = self.dash.borrow().live.clone() else {
            return;
        };
        let ui = self.ui();
        if ui.get_page() != Page::Dashboard {
            return;
        }
        let selected = self.dash.borrow().server.clone();
        let merged = selected == "all";
        ui.set_dash_servers(ModelRc::new(VecModel::from(
            l.servers
                .iter()
                .map(|s| DashServerChip {
                    id: sstr(&s.id),
                    name: sstr(&s.name),
                    kind: sstr(&s.kind),
                })
                .collect::<Vec<_>>(),
        )));
        let problems: Vec<String> = l
            .servers
            .iter()
            .filter(|s| merged || s.id == selected)
            .filter_map(|s| {
                Some(format!(
                    "{}: {}",
                    s.name,
                    problem_text(s.problem.as_deref()?)
                ))
            })
            .collect();
        ui.set_dash_message(sstr(problems.join("\n")));
        let n = l.streams.len();
        let conv = l.streams.iter().filter(|s| converted(s)).count();
        let total: u64 = l.streams.iter().map(|s| s.kbps as u64).sum();
        ui.set_dash_summary(sstr(if n == 0 {
            tr!("Nothing is playing.")
        } else {
            format!(
                "{} · {} · {}",
                trn!("{n} stream", "{n} streams", n),
                if conv == 0 {
                    tr!("all direct")
                } else {
                    trn!("{n} converted", "{n} converted", conv)
                },
                mbps(total)
            )
        }));
        let cards: Vec<DashStream> = l
            .streams
            .iter()
            .map(|s| {
                let server = l.servers.iter().find(|x| x.id == s.server_id);
                let tracks: Vec<DashTrack> = [
                    (tr!("Video"), &s.video),
                    (tr!("Audio"), &s.audio),
                    (tr!("Subtitles"), &s.subtitle_track),
                ]
                .into_iter()
                .filter_map(|(kind, t)| {
                    let t = t.as_ref()?;
                    Some(DashTrack {
                        kind: sstr(kind),
                        label: sstr(&t.label),
                        decision: sstr(decision(t)),
                        code: sstr(&t.decision),
                    })
                })
                .collect();
                let state = match s.state.as_str() {
                    "paused" => tr!("Paused"),
                    "buffering" => tr!("Buffering"),
                    _ => tr!("Playing"),
                };
                let mut place = if s.local { tr!("Local") } else { tr!("Remote") };
                if !s.ip.is_empty() {
                    place.push_str(&format!(" ({})", s.ip));
                }
                if s.kbps > 0 {
                    place.push_str(&format!(" — {}", mbps(s.kbps as u64)));
                }
                let avatar = self.dash_image(&s.server_id, s.user_avatar.as_deref());
                DashStream {
                    title: sstr(&s.title),
                    subtitle: sstr(format!("{}\n{}", s.subtitle, clock(s.duration_ms)).trim()),
                    server: sstr(if merged {
                        server.map(|x| x.name.as_str()).unwrap_or_default()
                    } else {
                        ""
                    }),
                    kind: sstr(server.map(|x| x.kind.as_str()).unwrap_or_default()),
                    poster: self.dash_image(&s.server_id, s.image.as_deref()),
                    progress: if s.duration_ms > 0 {
                        (s.position_ms as f32 / s.duration_ms as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                    player: sstr(format!("{} — {}", s.product, s.device)),
                    state: sstr(format!(
                        "{state} — {} / {}",
                        clock(s.position_ms),
                        clock(s.duration_ms)
                    )),
                    r#where: sstr(place),
                    tracks: ModelRc::new(VecModel::from(tracks)),
                    user: sstr(&s.user),
                    has_avatar: s.user_avatar.is_some(),
                    avatar,
                    converted: converted(s),
                    filler: false,
                }
            })
            .collect();
        let rows: Vec<DashRow> = cards
            .chunks(3)
            .map(|c| {
                let mut row = c.to_vec();
                row.resize_with(3, || DashStream {
                    filler: true,
                    ..Default::default()
                });
                DashRow {
                    cards: ModelRc::new(VecModel::from(row)),
                }
            })
            .collect();
        ui.set_dash_streams(ModelRc::new(VecModel::from(cards)));
        ui.set_dash_rows(ModelRc::new(VecModel::from(rows)));

        let samples = &l.samples;
        let span = match (samples.first(), samples.last()) {
            (Some(a), Some(b)) if b.t > a.t => {
                tr!(
                    "last {n} min",
                    n = ((b.t - a.t) as f64 / 60.0).round() as i64
                )
            }
            _ => String::new(),
        };
        let last = samples.last();
        let bw_top = samples
            .iter()
            .map(|s| s.local_kbps.max(s.remote_kbps) as f64)
            .fold(0.0, f64::max);
        let bw_top = if bw_top <= 0.0 { 1.0 } else { bw_top * 1.15 };
        let pct = |v: Option<f32>| v.map(|v| format!("{v:.0} %")).unwrap_or_else(|| "—".into());
        let ram_title = if l.ram_total_mb > 0 {
            format!("{} · {:.0} GB", tr!("RAM"), l.ram_total_mb as f64 / 1024.0)
        } else {
            tr!("RAM")
        };
        let charts = vec![
            DashChart {
                title: sstr(tr!("Bandwidth").to_uppercase()),
                note: sstr(&span),
                legend_a: sstr(format!(
                    "{} {}",
                    tr!("Remote"),
                    last.map(|s| mbps(s.remote_kbps as u64)).unwrap_or_default()
                )),
                path_a: sstr(line(samples, bw_top, |s| Some(s.remote_kbps as f64))),
                color_a: rgb(0x5b9cff),
                legend_b: sstr(format!(
                    "{} {}",
                    tr!("Local"),
                    last.map(|s| mbps(s.local_kbps as u64)).unwrap_or_default()
                )),
                path_b: sstr(line(samples, bw_top, |s| Some(s.local_kbps as f64))),
                color_b: rgb(0xffb224),
                top: sstr(mbps(bw_top as u64)),
            },
            DashChart {
                title: sstr(tr!("CPU").to_uppercase()),
                note: sstr(&span),
                legend_a: sstr(format!(
                    "{} {}",
                    tr!("Server"),
                    pct(last.and_then(|s| s.cpu))
                )),
                path_a: sstr(line(samples, 100.0, |s| s.cpu.map(f64::from))),
                color_a: rgb(0xffb224),
                legend_b: Default::default(),
                path_b: Default::default(),
                color_b: rgb(0xffb224),
                top: sstr("100 %"),
            },
            DashChart {
                title: sstr(ram_title.to_uppercase()),
                note: sstr(&span),
                legend_a: sstr(format!(
                    "{} {}",
                    tr!("Server"),
                    pct(last.and_then(|s| s.ram))
                )),
                path_a: sstr(line(samples, 100.0, |s| s.ram.map(f64::from))),
                color_a: rgb(0x5b9cff),
                legend_b: Default::default(),
                path_b: Default::default(),
                color_b: rgb(0x5b9cff),
                top: sstr("100 %"),
            },
        ];
        ui.set_dash_charts(ModelRc::new(VecModel::from(charts)));
    }

    fn dash_render_stats(self: &Rc<Self>) {
        let Some(st) = self.dash.borrow().stats.clone() else {
            return;
        };
        let ui = self.ui();
        if ui.get_page() != Page::Dashboard {
            return;
        }
        let names: std::collections::HashMap<String, String> = self
            .dash
            .borrow()
            .live
            .as_ref()
            .map(|l| {
                l.servers
                    .iter()
                    .map(|s| (s.id.clone(), s.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        ui.set_dash_since(sstr(
            st.since
                .iter()
                .map(|s| {
                    tr!(
                        "{server}: history since {date}",
                        server = names.get(&s.server_id).cloned().unwrap_or_default(),
                        date = crate::localtime::date(s.since)
                    )
                })
                .collect::<Vec<_>>()
                .join(" · "),
        ));
        let users: Vec<DashUser> = st
            .top_users
            .iter()
            .map(|u| DashUser {
                name: sstr(&u.name),
                plays: sstr(format!(
                    "{} · {}",
                    trn!("{n} play", "{n} plays", u.plays),
                    hours(u.seconds)
                )),
                tv: sstr(hours(u.episode_seconds)),
                movies: sstr(hours(u.movie_seconds)),
                has_avatar: u.avatar.is_some(),
                avatar: self.dash_image(&u.server_id, u.avatar.as_deref()),
            })
            .collect();
        ui.set_dash_users(ModelRc::new(VecModel::from(users)));
        let days = &st.history;
        let top = days
            .iter()
            .map(|d| d.movies + d.episodes)
            .max()
            .unwrap_or(0)
            .max(1) as f64
            * 1.1;
        let n = days.len().max(1) as f64;
        let slot = W / n;
        let gap = if slot > 6.0 { 2.0 } else { 0.0 };
        let (mut shows, mut movies) = (String::new(), String::new());
        for (i, d) in days.iter().enumerate() {
            let x = i as f64 * slot + gap / 2.0;
            let w = slot - gap;
            let eh = d.episodes as f64 / top * H;
            let mh = d.movies as f64 / top * H;
            if eh > 0.0 {
                shows.push_str(&format!(
                    "M{x:.1} {:.1} h{w:.1} v{eh:.1} h{:.1} Z ",
                    H - eh,
                    -w
                ));
            }
            if mh > 0.0 {
                movies.push_str(&format!(
                    "M{x:.1} {:.1} h{w:.1} v{mh:.1} h{:.1} Z ",
                    H - eh - mh,
                    -w
                ));
            }
        }
        ui.set_dash_history_shows(sstr(shows));
        ui.set_dash_history_movies(sstr(movies));
        ui.set_dash_history_legend(sstr(format!(
            "{} {}  ·  {} {}",
            tr!("TV"),
            days.iter().map(|d| d.episodes).sum::<u32>(),
            tr!("Movies"),
            days.iter().map(|d| d.movies).sum::<u32>()
        )));
        ui.set_dash_history_note(sstr(if st.direct_plays + st.transcodes > 0 {
            tr!(
                "{direct} direct play · {transcodes} converted (Plex)",
                direct = st.direct_plays,
                transcodes = st.transcodes
            )
        } else {
            String::new()
        }));
        ui.set_dash_history_first(sstr(
            days.first().map(|d| d.date.as_str()).unwrap_or_default(),
        ));
        ui.set_dash_history_last(sstr(
            days.last().map(|d| d.date.as_str()).unwrap_or_default(),
        ));
        let titles = |list: &[keep_service::dashboard::TopTitle]| -> Vec<DashTitle> {
            list.iter()
                .map(|t| DashTitle {
                    title: sstr(&t.title),
                    detail: sstr(format!(
                        "{} · {}",
                        trn!("{n} play", "{n} plays", t.plays),
                        trn!("{n} person", "{n} people", t.users)
                    )),
                    poster: self.dash_image(&t.server_id, t.image.as_deref()),
                })
                .collect()
        };
        ui.set_dash_movies(ModelRc::new(VecModel::from(titles(&st.top_movies))));
        ui.set_dash_shows(ModelRc::new(VecModel::from(titles(&st.top_shows))));
    }
}
