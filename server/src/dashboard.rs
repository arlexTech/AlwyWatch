// SPDX-License-Identifier: AGPL-3.0-or-later
//! The admin dashboard (`/v1/admin/dashboard/*`, see keep_service::dashboard): what plays now,
//! bandwidth and host load over the last 15 minutes, and plays over the last days.
//!
//! Sources, read only:
//! - Plex through Tautulli (`get_activity`, `get_history`): no Plex token or Plex Pass needed.
//! - Jellyfin through its API key: `GET /Sessions` now, `GET /System/ActivityLog/Entries`
//!   (VideoPlayback / VideoPlaybackStopped, kept about a month) for history, `GET /Items?Ids=`
//!   for what those were.
//! - CPU and RAM from `/proc` of the host AlwyWatch runs on (a container sees the host's).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use keep_service::ServerType;
use keep_service::dashboard::{
    DashServer, DayPlays, Live, Sample, ServerSince, Stats, Stream, StreamTrack, TopTitle, TopUser,
};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::ServerConf;
use crate::keep::State;

/// Live graphs keep this much.
const KEEP_SECS: i64 = 15 * 60;
/// A live reading younger than this is reused (several admins, several tabs).
const FRESH: Duration = Duration::from_secs(4);
/// Stats are recomputed at most this often.
const STATS_TTL: Duration = Duration::from_secs(300);
/// Between readings while nobody looks (the graphs then have a point every 30 s).
pub const IDLE_EVERY: Duration = Duration::from_secs(30);

/// (server or "all", days, viewer's UTC offset in minutes).
type StatsKey = (String, u32, i32);

/// (unix seconds, per server (local, remote kbps), host CPU %, host RAM %).
type Reading = (i64, HashMap<String, (u32, u32)>, Option<f32>, Option<f32>);

#[derive(Default)]
pub struct Dash {
    live: Mutex<LiveState>,
    stats: Mutex<HashMap<StatsKey, (Instant, Stats)>>,
}

#[derive(Default)]
struct LiveState {
    at: Option<Instant>,
    /// Per server: the streams of the last reading (None: unreachable).
    streams: HashMap<String, Option<Vec<Stream>>>,
    /// (t, per server (local, remote kbps), cpu, ram)
    samples: VecDeque<Reading>,
    cpu_prev: Option<(u64, u64)>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- JSON helpers (Tautulli
// mixes strings and numbers for the same field)

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn n(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(x)) => x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)),
        Some(Value::String(x)) => x.trim().parse::<f64>().ok().map(|f| f as i64),
        Some(Value::Bool(b)) => Some(*b as i64),
        _ => None,
    }
    .unwrap_or(0)
}

fn nonempty(x: String) -> Option<String> {
    (!x.trim().is_empty()).then_some(x)
}

fn upper(x: &str) -> String {
    x.to_uppercase()
}

// ---------------------------------------------------------------- host

/// (busy, total) jiffies of all CPUs.
fn cpu_times() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    let v: Vec<u64> = line
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() < 4 {
        return None;
    }
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    let total: u64 = v.iter().take(8).sum();
    Some((total - idle, total))
}

/// (used %, total MB).
fn memory() -> Option<(f32, u64)> {
    let m = std::fs::read_to_string("/proc/meminfo").ok()?;
    let get = |k: &str| {
        m.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|x| x.parse::<u64>().ok())
    };
    let total = get("MemTotal:")?;
    let avail = get("MemAvailable:")?;
    (total > 0).then(|| {
        (
            (total.saturating_sub(avail)) as f32 * 100.0 / total as f32,
            total / 1024,
        )
    })
}

fn cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- Plex (Tautulli)

fn decision(d: &str) -> String {
    match d.trim() {
        "transcode" => "transcode",
        "copy" => "direct_stream",
        _ => "direct_play",
    }
    .into()
}

fn channels(layout: &str, count: &str) -> String {
    let l = layout.split('(').next().unwrap_or("").trim();
    if !l.is_empty() {
        return l.to_owned();
    }
    match count {
        "1" => "Mono".into(),
        "2" => "Stereo".into(),
        "6" => "5.1".into(),
        "8" => "7.1".into(),
        _ => String::new(),
    }
}

fn paren(head: &str, inner: &[String]) -> String {
    let inner: Vec<&str> = inner
        .iter()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .collect();
    match (head.trim(), inner.is_empty()) {
        ("", true) => String::new(),
        ("", false) => inner.join(" "),
        (h, true) => h.to_owned(),
        (h, false) => format!("{h} ({})", inner.join(" ")),
    }
}

/// One Tautulli `get_activity` session.
pub fn tautulli_stream(server_id: &str, v: &Value) -> Stream {
    let movie = s(v, "media_type") == "movie";
    let (title, subtitle) = if movie {
        (s(v, "title"), s(v, "year"))
    } else if s(v, "media_type") == "episode" {
        (
            s(v, "grandparent_title"),
            format!(
                "S{} · E{} {}",
                s(v, "parent_media_index"),
                s(v, "media_index"),
                s(v, "title")
            ),
        )
    } else {
        (s(v, "full_title"), String::new())
    };
    let thumb = if movie {
        s(v, "thumb")
    } else {
        nonempty(s(v, "grandparent_thumb")).unwrap_or_else(|| s(v, "thumb"))
    };
    let profile = s(v, "video_profile");
    let video = nonempty(s(v, "video_codec")).map(|codec| {
        let mut inner = vec![upper(&codec)];
        if !profile.is_empty() {
            inner.push(title_case(&profile));
        }
        if s(v, "video_dynamic_range") != "SDR" {
            inner.push(s(v, "video_dynamic_range"));
        }
        let d = decision(&s(v, "stream_video_decision"));
        StreamTrack {
            label: paren(&s(v, "video_full_resolution"), &inner),
            to: (d == "transcode").then(|| {
                paren(
                    &s(v, "stream_video_full_resolution"),
                    &[upper(&s(v, "stream_video_codec"))],
                )
            }),
            decision: d,
        }
    });
    let audio = nonempty(s(v, "audio_codec")).map(|codec| {
        let mut inner = vec![
            upper(&codec),
            channels(&s(v, "audio_channel_layout"), &s(v, "audio_channels")),
        ];
        if n(v, "audio_atmos") == 1 {
            inner.push("+ Atmos".into());
        }
        let d = decision(&s(v, "stream_audio_decision"));
        StreamTrack {
            label: paren(&s(v, "audio_language"), &inner),
            to: (d == "transcode").then(|| {
                paren(
                    "",
                    &[
                        upper(&s(v, "stream_audio_codec")),
                        channels(
                            &s(v, "stream_audio_channel_layout"),
                            &s(v, "stream_audio_channels"),
                        ),
                    ],
                )
            }),
            decision: d,
        }
    });
    let subtitle_track = (n(v, "subtitles") == 1)
        .then(|| nonempty(s(v, "subtitle_codec")))
        .flatten()
        .map(|codec| {
            let forced = if n(v, "subtitle_forced") == 1 {
                " Forced"
            } else {
                ""
            };
            let d = s(v, "stream_subtitle_decision");
            StreamTrack {
                label: paren(
                    &format!("{}{forced}", s(v, "subtitle_language")),
                    &[upper(&codec)],
                ),
                decision: if d == "burn" {
                    "transcode".into()
                } else {
                    decision(&d)
                },
                to: (d == "burn").then(|| "burned in".into()),
            }
        });
    let state = match s(v, "state").as_str() {
        "paused" => "paused",
        "buffering" => "buffering",
        _ => "playing",
    };
    Stream {
        server_id: server_id.into(),
        user: nonempty(s(v, "friendly_name")).unwrap_or_else(|| s(v, "user")),
        user_key: nonempty(s(v, "user_id")).map(|id| format!("plex:{server_id}:{id}")),
        user_avatar: nonempty(s(v, "user_thumb")).filter(|u| u.starts_with("https://")),
        title,
        subtitle,
        image: nonempty(thumb).map(|t| format!("plex:{t}")),
        duration_ms: n(v, "duration"),
        position_ms: n(v, "view_offset"),
        state: state.into(),
        product: s(v, "product"),
        device: nonempty(s(v, "player")).unwrap_or_else(|| s(v, "device")),
        local: s(v, "location") == "lan" || n(v, "local") == 1,
        ip: s(v, "ip_address"),
        kbps: n(v, "bandwidth").max(0) as u32,
        video,
        audio,
        subtitle_track,
    }
}

fn title_case(x: &str) -> String {
    x.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------- Jellyfin

fn ticks_ms(v: &Value, k: &str) -> i64 {
    n(v, k) / 10_000
}

fn private_ip(ip: &str) -> bool {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => a.is_private() || a.is_loopback() || a.is_link_local(),
        Ok(std::net::IpAddr::V6(a)) => {
            a.is_loopback()
                || (a.segments()[0] & 0xfe00) == 0xfc00
                || a.to_ipv4_mapped().is_some_and(|m| m.is_private())
        }
        Err(_) => false,
    }
}

fn jf_decision(direct: Option<bool>, method: &str) -> String {
    match (direct, method) {
        (Some(false), _) => "transcode".into(),
        (Some(true), _) | (None, "DirectStream") => "direct_stream".into(),
        (None, "Transcode") => "transcode".into(),
        _ => "direct_play".into(),
    }
}

/// One Jellyfin session with something playing.
pub fn jellyfin_stream(server_id: &str, v: &Value) -> Option<Stream> {
    let item = v.get("NowPlayingItem")?;
    let play = v.get("PlayState").cloned().unwrap_or(Value::Null);
    let tr = v.get("TranscodingInfo");
    let method = s(&play, "PlayMethod");
    let episode = s(item, "Type") == "Episode";
    let (title, subtitle) = if episode {
        (
            s(item, "SeriesName"),
            format!(
                "S{} · E{} {}",
                n(item, "ParentIndexNumber"),
                n(item, "IndexNumber"),
                s(item, "Name")
            ),
        )
    } else {
        (s(item, "Name"), s(item, "ProductionYear"))
    };
    let streams = item
        .get("MediaStreams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let pick = |kind: &str, index_key: &str| -> Option<Value> {
        let idx = play.get(index_key).and_then(Value::as_i64);
        streams
            .iter()
            .filter(|x| s(x, "Type") == kind)
            .find(|x| idx.is_none_or(|i| x.get("Index").and_then(Value::as_i64) == Some(i)))
            .cloned()
            .or_else(|| {
                (kind == "Video")
                    .then(|| streams.iter().find(|x| s(x, "Type") == kind).cloned())
                    .flatten()
            })
    };
    let direct = |k: &str| tr.and_then(|t| t.get(k)).and_then(Value::as_bool);
    let video = pick("Video", "__none").map(|x| {
        let h = n(&x, "Height");
        let w = n(&x, "Width");
        let res = resolution(w, h);
        let mut inner = vec![upper(&s(&x, "Codec"))];
        if let Some(p) = nonempty(s(&x, "Profile")) {
            inner.push(p);
        }
        let range = s(&x, "VideoRangeType");
        if !range.is_empty() && range != "SDR" {
            inner.push(range);
        }
        let d = jf_decision(direct("IsVideoDirect"), &method);
        StreamTrack {
            label: paren(&res, &inner),
            to: (d == "transcode").then(|| {
                let t = tr.cloned().unwrap_or(Value::Null);
                paren(
                    &resolution(n(&t, "Width"), n(&t, "Height")),
                    &[upper(&s(&t, "VideoCodec"))],
                )
            }),
            decision: d,
        }
    });
    let audio = pick("Audio", "AudioStreamIndex").map(|x| {
        let d = jf_decision(direct("IsAudioDirect"), &method);
        StreamTrack {
            label: paren(
                &language(&s(&x, "Language")),
                &[upper(&s(&x, "Codec")), s(&x, "ChannelLayout")],
            ),
            to: (d == "transcode")
                .then(|| upper(&s(&tr.cloned().unwrap_or(Value::Null), "AudioCodec"))),
            decision: d,
        }
    });
    let sub_index = play.get("SubtitleStreamIndex").and_then(Value::as_i64);
    let subtitle_track = sub_index.filter(|i| *i >= 0).and_then(|_| {
        pick("Subtitle", "SubtitleStreamIndex").map(|x| {
            let forced = if x.get("IsForced").and_then(Value::as_bool) == Some(true) {
                " Forced"
            } else {
                ""
            };
            let burned = tr
                .and_then(|t| t.get("SubtitleDeliveryMethod"))
                .and_then(Value::as_str)
                == Some("Encode");
            StreamTrack {
                label: paren(
                    &format!("{}{forced}", language(&s(&x, "Language"))),
                    &[upper(&s(&x, "Codec"))],
                ),
                decision: if burned { "transcode" } else { "direct_play" }.into(),
                to: burned.then(|| "burned in".into()),
            }
        })
    });
    let kbps = match tr.map(|t| n(t, "Bitrate")) {
        Some(b) if b > 0 => b / 1000,
        _ => {
            streams
                .iter()
                .filter(|x| matches!(s(x, "Type").as_str(), "Video" | "Audio"))
                .map(|x| n(x, "BitRate"))
                .max()
                .unwrap_or(0)
                .max(n(item, "Bitrate"))
                / 1000
        }
    };
    let ip = s(v, "RemoteEndPoint");
    let image_id = if episode {
        nonempty(s(item, "SeriesId")).unwrap_or_else(|| s(item, "Id"))
    } else {
        s(item, "Id")
    };
    let state = if play.get("IsPaused").and_then(Value::as_bool) == Some(true) {
        "paused"
    } else {
        "playing"
    };
    let user_id = s(v, "UserId");
    Some(Stream {
        server_id: server_id.into(),
        user: s(v, "UserName"),
        user_key: nonempty(user_id.clone()).map(|id| format!("jellyfin:{server_id}:{id}")),
        user_avatar: nonempty(s(v, "UserPrimaryImageTag"))
            .and(nonempty(user_id))
            .map(|id| format!("jf:user:{id}")),
        title,
        subtitle,
        image: nonempty(image_id).map(|id| format!("jf:item:{id}")),
        duration_ms: ticks_ms(item, "RunTimeTicks"),
        position_ms: ticks_ms(&play, "PositionTicks"),
        state: state.into(),
        product: s(v, "Client"),
        device: s(v, "DeviceName"),
        local: private_ip(&ip),
        ip,
        kbps: kbps.max(0) as u32,
        video,
        audio,
        subtitle_track,
    })
}

fn resolution(w: i64, h: i64) -> String {
    // By width first: a 1920×800 scope picture is still "1080p".
    let p = match (w, h) {
        (w, _) if w >= 3200 => "4K",
        (w, h) if w >= 1800 || h >= 1000 => "1080p",
        (w, h) if w >= 1200 || h >= 700 => "720p",
        (w, h) if w >= 900 || h >= 540 => "576p",
        (0, 0) => "",
        _ => "SD",
    };
    p.into()
}

fn language(code: &str) -> String {
    let name = match code {
        "eng" | "en" => "English",
        "fre" | "fra" | "fr" => "French",
        "ger" | "deu" | "de" => "German",
        "spa" | "es" => "Spanish",
        "ita" | "it" => "Italian",
        "por" | "pt" => "Portuguese",
        "jpn" | "ja" => "Japanese",
        "kor" | "ko" => "Korean",
        "chi" | "zho" | "zh" => "Chinese",
        "dut" | "nld" | "nl" => "Dutch",
        "rus" | "ru" => "Russian",
        "ara" | "ar" => "Arabic",
        "und" | "" => "",
        other => other,
    };
    name.into()
}

// ---------------------------------------------------------------- plays (history)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movie,
    Episode,
    Other,
}

/// One play from any server, as the stats need it.
#[derive(Debug, Clone)]
pub struct Play {
    pub server_id: String,
    /// The AlwyWatch-style user key (`kind:server:id`).
    pub user_key: String,
    pub user_name: String,
    pub avatar: Option<String>,
    pub kind: Kind,
    /// The movie, or the show of an episode.
    pub title: String,
    pub image: Option<String>,
    /// Unix seconds.
    pub start: i64,
    pub seconds: u64,
    /// None: not known (Jellyfin's log does not say).
    pub transcode: Option<bool>,
}

/// Unix seconds + offset (minutes east) → `YYYY-MM-DD`.
pub fn day_of(t: i64, tz_minutes: i32) -> String {
    let days = (t + tz_minutes as i64 * 60).div_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// The people behind user keys: (person key, display name), from AlwyWatch's sign-ins.
pub type People = HashMap<String, (String, String)>;

/// Everything the stats page shows, from plays of the last `days` days up to `until`.
pub fn aggregate(plays: &[Play], people: &People, days: u32, until: i64, tz_minutes: i32) -> Stats {
    let from = until - days as i64 * 86_400;
    let plays: Vec<&Play> = plays
        .iter()
        .filter(|p| p.start >= from && p.start <= until && p.kind != Kind::Other)
        .collect();
    let mut history: Vec<DayPlays> = (0..days)
        .rev()
        .map(|i| DayPlays {
            date: day_of(until - i as i64 * 86_400, tz_minutes),
            ..Default::default()
        })
        .collect();
    let index: HashMap<String, usize> = history
        .iter()
        .enumerate()
        .map(|(i, d)| (d.date.clone(), i))
        .collect();
    let mut users: HashMap<String, TopUser> = HashMap::new();
    let mut titles: HashMap<(bool, String), (TopTitle, HashSet<String>)> = HashMap::new();
    let (mut direct, mut transcodes) = (0, 0);
    for p in &plays {
        let movie = p.kind == Kind::Movie;
        if let Some(&i) = index.get(&day_of(p.start, tz_minutes)) {
            let d = &mut history[i];
            if movie {
                d.movies += 1;
                d.movie_seconds += p.seconds;
            } else {
                d.episodes += 1;
                d.episode_seconds += p.seconds;
            }
        }
        match p.transcode {
            Some(true) => transcodes += 1,
            Some(false) => direct += 1,
            None => {}
        }
        let (person, name) = people
            .get(&p.user_key)
            .cloned()
            .unwrap_or_else(|| (p.user_key.clone(), p.user_name.clone()));
        let u = users.entry(person.clone()).or_insert_with(|| TopUser {
            name,
            user_key: people.contains_key(&p.user_key).then(|| person.clone()),
            server_id: p.server_id.clone(),
            avatar: p.avatar.clone(),
            ..Default::default()
        });
        if u.avatar.is_none() {
            u.avatar = p.avatar.clone();
        }
        u.plays += 1;
        u.seconds += p.seconds;
        if movie {
            u.movie_plays += 1;
            u.movie_seconds += p.seconds;
        } else {
            u.episode_plays += 1;
            u.episode_seconds += p.seconds;
        }
        let key = (movie, p.title.trim().to_lowercase());
        let (t, who) = titles.entry(key).or_insert_with(|| {
            (
                TopTitle {
                    title: p.title.clone(),
                    server_id: p.server_id.clone(),
                    image: p.image.clone(),
                    ..Default::default()
                },
                HashSet::new(),
            )
        });
        if t.image.is_none() {
            t.image = p.image.clone();
            t.server_id = p.server_id.clone();
        }
        t.plays += 1;
        t.seconds += p.seconds;
        who.insert(person);
    }
    let mut top_users: Vec<TopUser> = users.into_values().collect();
    top_users.sort_by(|a, b| b.seconds.cmp(&a.seconds).then(a.name.cmp(&b.name)));
    let top = |movie: bool| {
        let mut v: Vec<TopTitle> = titles
            .iter()
            .filter(|((m, _), _)| *m == movie)
            .map(|(_, (t, who))| TopTitle {
                users: who.len() as u32,
                ..t.clone()
            })
            .collect();
        v.sort_by(|a, b| {
            b.plays
                .cmp(&a.plays)
                .then(b.users.cmp(&a.users))
                .then(a.title.cmp(&b.title))
        });
        v.truncate(10);
        v
    };
    let top_movies = top(true);
    let top_shows = top(false);
    Stats {
        days,
        top_users,
        top_movies,
        top_shows,
        history,
        direct_plays: direct,
        transcodes,
        since: Vec::new(),
    }
}

/// Jellyfin activity log entries (newest first, as Jellyfin gives them) → plays with the
/// item id: a start, ended by the same user's next stop of the same item (else 0 s).
pub fn jellyfin_plays(entries: &[Value]) -> Vec<(String, String, i64, u64)> {
    let parse = |e: &Value| parse_iso(&s(e, "Date"));
    let mut sorted: Vec<&Value> = entries
        .iter()
        .filter(|e| {
            matches!(
                s(e, "Type").as_str(),
                "VideoPlayback" | "VideoPlaybackStopped"
            )
        })
        .collect();
    sorted.sort_by_key(|e| parse(e));
    let mut open: HashMap<(String, String), i64> = HashMap::new();
    let mut out = Vec::new();
    for e in sorted {
        let key = (s(e, "UserId"), s(e, "ItemId"));
        let Some(t) = parse(e) else { continue };
        if key.1.is_empty() {
            continue;
        }
        if s(e, "Type") == "VideoPlayback" {
            if let Some(start) = open.insert(key.clone(), t) {
                out.push((key.0.clone(), key.1.clone(), start, 0));
            }
        } else if let Some(start) = open.remove(&key) {
            out.push((key.0, key.1, start, (t - start).clamp(0, 12 * 3600) as u64));
        }
    }
    for ((u, i), start) in open {
        out.push((u, i, start, 0));
    }
    out
}

/// `2026-11-20T07:17:28.6041526Z` → unix seconds.
pub fn parse_iso(x: &str) -> Option<i64> {
    let b = x.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: usize, z: usize| x.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // days_from_civil
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se)
}

// ---------------------------------------------------------------- fetching

impl State {
    fn tautulli(&self) -> Option<(&str, &str)> {
        let t = self.config.tautulli.as_ref()?;
        let key = self.config.secrets.tautulli_api_key.as_deref()?;
        Some((t.url.trim_end_matches('/'), key))
    }

    async fn tautulli_get(&self, params: &[(&str, &str)]) -> Option<Value> {
        let (url, key) = self.tautulli()?;
        let res = self
            .http
            .get(format!("{url}/api/v2"))
            .query(&[("apikey", key)])
            .query(params)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            // The API key is in the query string: never let the URL reach a log.
            .map_err(|e| e.without_url())
            .ok()?;
        let v: Value = res.json().await.map_err(|e| e.without_url()).ok()?;
        v.get("response")?.get("data").cloned()
    }

    async fn jellyfin_get(&self, server: &ServerConf, path: &str) -> Option<Value> {
        let key = self.config.secrets.jellyfin_key(&server.id)?;
        let res = self
            .http
            .get(format!("{}{path}", server.internal()))
            .header("Authorization", format!("MediaBrowser Token=\"{key}\""))
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .ok()?;
        if !res.status().is_success() {
            return None;
        }
        res.json().await.ok()
    }

    /// Whether the dashboard can read this server, else why not.
    fn dash_problem(&self, server: &ServerConf) -> Option<String> {
        match server.kind {
            ServerType::Plex if self.tautulli().is_none() => Some("no_tautulli".into()),
            ServerType::Jellyfin if self.config.secrets.jellyfin_key(&server.id).is_none() => {
                Some("no_key".into())
            }
            _ => None,
        }
    }

    fn dash_servers(&self) -> Vec<DashServer> {
        self.config
            .servers
            .iter()
            .map(|x| DashServer {
                id: x.id.clone(),
                name: x.name.clone(),
                kind: match x.kind {
                    ServerType::Plex => "plex",
                    ServerType::Jellyfin => "jellyfin",
                }
                .into(),
                problem: self.dash_problem(x),
            })
            .collect()
    }

    /// The Plex server Tautulli watches: the only Plex one (Tautulli follows one server).
    fn tautulli_server(&self) -> Option<&ServerConf> {
        let mut plex = self
            .config
            .servers
            .iter()
            .filter(|x| x.kind == ServerType::Plex);
        let first = plex.next()?;
        plex.next().is_none().then_some(first)
    }

    async fn streams_of(&self, server: &ServerConf) -> Option<Vec<Stream>> {
        self.dash_problem(server).is_none().then_some(())?;
        match server.kind {
            ServerType::Plex => {
                if self.tautulli_server().map(|x| &x.id) != Some(&server.id) {
                    return None;
                }
                let d = self.tautulli_get(&[("cmd", "get_activity")]).await?;
                Some(
                    d.get("sessions")
                        .and_then(Value::as_array)
                        .map(|v| v.iter().map(|x| tautulli_stream(&server.id, x)).collect())
                        .unwrap_or_default(),
                )
            }
            ServerType::Jellyfin => {
                let d = self
                    .jellyfin_get(server, "/Sessions?activeWithinSeconds=600")
                    .await?;
                Some(
                    d.as_array()
                        .map(|v| {
                            v.iter()
                                .filter_map(|x| jellyfin_stream(&server.id, x))
                                .collect()
                        })
                        .unwrap_or_default(),
                )
            }
        }
    }

    /// Reads every server and the host now, unless a reading is fresh. Called by the live
    /// endpoint and every [`IDLE_EVERY`] by the background task.
    pub async fn dash_refresh(&self) {
        let mut live = self.dashboard.live.lock().await;
        if live.at.is_some_and(|a| a.elapsed() < FRESH) {
            return;
        }
        let reads = self
            .config
            .servers
            .iter()
            .map(|x| async move { (x.id.clone(), self.streams_of(x).await) });
        let results = futures_util::future::join_all(reads).await;
        let t = now();
        let mut bw = HashMap::new();
        for (id, streams) in &results {
            let (mut l, mut r) = (0, 0);
            for x in streams.iter().flatten() {
                if x.local {
                    l += x.kbps;
                } else {
                    r += x.kbps;
                }
            }
            bw.insert(id.clone(), (l, r));
        }
        let cpu = cpu_times().and_then(|(busy, total)| {
            let prev = live.cpu_prev.replace((busy, total));
            let (pb, pt) = prev?;
            (total > pt).then(|| (busy.saturating_sub(pb)) as f32 * 100.0 / (total - pt) as f32)
        });
        let ram = memory().map(|m| m.0);
        live.samples.push_back((t, bw, cpu, ram));
        while live.samples.front().is_some_and(|s| s.0 < t - KEEP_SECS) {
            live.samples.pop_front();
        }
        live.streams = results.into_iter().collect();
        live.at = Some(Instant::now());
    }

    pub async fn dash_live(&self, server: &str) -> Live {
        self.dash_refresh().await;
        let live = self.dashboard.live.lock().await;
        let all = server == "all";
        let wanted = |id: &str| all || id == server;
        let mut streams: Vec<Stream> = live
            .streams
            .iter()
            .filter(|(id, _)| wanted(id))
            .flat_map(|(_, v)| v.iter().flatten().cloned())
            .collect();
        streams.sort_by(|a, b| a.user.cmp(&b.user).then(a.title.cmp(&b.title)));
        let samples = live
            .samples
            .iter()
            .map(|(t, bw, cpu, ram)| {
                let (l, r) = bw
                    .iter()
                    .filter(|(id, _)| wanted(id))
                    .fold((0, 0), |a, (_, v)| (a.0 + v.0, a.1 + v.1));
                Sample {
                    t: *t,
                    local_kbps: l,
                    remote_kbps: r,
                    cpu: *cpu,
                    ram: *ram,
                }
            })
            .collect();
        let mut servers = self.dash_servers();
        for x in &mut servers {
            if x.problem.is_none() && matches!(live.streams.get(&x.id), Some(None)) {
                x.problem = Some("unreachable".into());
            }
        }
        Live {
            servers,
            streams,
            samples,
            ram_total_mb: memory().map(|m| m.1).unwrap_or(0),
            cpus: cpus(),
        }
    }

    async fn plex_plays(&self, server: &ServerConf, after: i64) -> Option<Vec<Play>> {
        if self.tautulli_server().map(|x| &x.id) != Some(&server.id) {
            return None;
        }
        let after_day = day_of(after - 86_400, 0);
        let d = self
            .tautulli_get(&[
                ("cmd", "get_history"),
                ("after", &after_day),
                ("length", "20000"),
                ("order_column", "date"),
                ("order_dir", "desc"),
            ])
            .await?;
        let rows = d.get("data")?.as_array()?;
        Some(
            rows.iter()
                .map(|r| {
                    let kind = match s(r, "media_type").as_str() {
                        "movie" => Kind::Movie,
                        "episode" => Kind::Episode,
                        _ => Kind::Other,
                    };
                    let (title, thumb) = if kind == Kind::Episode {
                        (
                            s(r, "grandparent_title"),
                            nonempty(s(r, "grandparent_rating_key"))
                                .map(|k| format!("/library/metadata/{k}/thumb")),
                        )
                    } else {
                        (s(r, "title"), nonempty(s(r, "thumb")))
                    };
                    let user_id = s(r, "user_id");
                    Play {
                        server_id: server.id.clone(),
                        user_key: format!("plex:{}:{user_id}", server.id),
                        user_name: nonempty(s(r, "friendly_name")).unwrap_or_else(|| s(r, "user")),
                        avatar: nonempty(s(r, "user_thumb")).filter(|u| u.starts_with("https://")),
                        kind,
                        title,
                        image: thumb.map(|t| format!("plex:{t}")),
                        start: n(r, "started").max(n(r, "date")),
                        seconds: n(r, "play_duration").max(0) as u64,
                        transcode: Some(s(r, "transcode_decision") == "transcode"),
                    }
                })
                .collect(),
        )
    }

    /// Plays from the activity log, and since when the log goes back.
    async fn jellyfin_history(&self, server: &ServerConf, after: i64) -> Option<(Vec<Play>, i64)> {
        let min = format!("{}T00:00:00Z", day_of(after - 86_400, 0));
        let mut entries: Vec<Value> = Vec::new();
        let mut start = 0;
        loop {
            let page = self
                .jellyfin_get(
                    server,
                    &format!(
                        "/System/ActivityLog/Entries?startIndex={start}&limit=1000&minDate={min}&hasUserId=true"
                    ),
                )
                .await?;
            let items = page
                .get("Items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = n(&page, "TotalRecordCount");
            let got = items.len();
            entries.extend(items);
            start += got;
            if got == 0 || start as i64 >= total || start >= 20_000 {
                break;
            }
        }
        // The oldest entry of any kind: the log reaches back that far.
        let oldest = entries
            .iter()
            .filter_map(|e| parse_iso(&s(e, "Date")))
            .min()
            .unwrap_or(after);
        let raw = jellyfin_plays(&entries);
        let ids: Vec<String> = raw
            .iter()
            .map(|r| r.1.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let mut items: HashMap<String, Value> = HashMap::new();
        for chunk in ids.chunks(100) {
            if let Some(v) = self
                .jellyfin_get(
                    server,
                    &format!(
                        "/Items?Ids={}&Fields=SeriesPrimaryImage&EnableUserData=false",
                        chunk.join(",")
                    ),
                )
                .await
            {
                for it in v
                    .get("Items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    items.insert(s(it, "Id"), it.clone());
                }
            }
        }
        let names: HashMap<String, (String, bool)> = self
            .jellyfin_get(server, "/Users")
            .await
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|u| {
                (
                    s(u, "Id"),
                    (s(u, "Name"), nonempty(s(u, "PrimaryImageTag")).is_some()),
                )
            })
            .collect();
        let plays = raw
            .into_iter()
            .filter_map(|(user, item, start, seconds)| {
                let it = items.get(&item)?;
                let kind = match s(it, "Type").as_str() {
                    "Movie" => Kind::Movie,
                    "Episode" => Kind::Episode,
                    _ => Kind::Other,
                };
                let (title, image) = if kind == Kind::Episode {
                    (s(it, "SeriesName"), nonempty(s(it, "SeriesId")))
                } else {
                    (s(it, "Name"), Some(item.clone()))
                };
                let (name, has_image) = names.get(&user).cloned().unwrap_or_default();
                // A play is the time watched, never longer than the title.
                let runtime = (n(it, "RunTimeTicks") / 10_000_000).max(0) as u64;
                Some(Play {
                    server_id: server.id.clone(),
                    user_key: format!("jellyfin:{}:{user}", server.id),
                    user_name: name,
                    avatar: has_image.then(|| format!("jf:user:{user}")),
                    kind,
                    title,
                    image: image.map(|i| format!("jf:item:{i}")),
                    start,
                    seconds: if runtime > 0 {
                        seconds.min(runtime)
                    } else {
                        seconds
                    },
                    transcode: None,
                })
            })
            .collect();
        Some((plays, oldest))
    }

    fn dash_people(&self) -> People {
        let names: HashMap<String, String> = self
            .db
            .users()
            .unwrap_or_default()
            .into_iter()
            .map(|u| (u.user_key, u.name))
            .collect();
        let mut out = People::new();
        for (person, ids) in self.db.all_identities().unwrap_or_default() {
            let name = names.get(&person).cloned();
            for i in ids {
                let n = name.clone().unwrap_or_else(|| i.media_name.clone());
                out.insert(i.user_key, (person.clone(), n));
            }
        }
        out
    }

    pub async fn dash_stats(&self, server: &str, days: u32, tz: i32) -> Stats {
        let days = days.clamp(1, 90);
        let key = (server.to_owned(), days, tz);
        if let Some((at, s)) = self.dashboard.stats.lock().await.get(&key)
            && at.elapsed() < STATS_TTL
        {
            return s.clone();
        }
        let until = now();
        let after = until - days as i64 * 86_400;
        let wanted: Vec<&ServerConf> = self
            .config
            .servers
            .iter()
            .filter(|x| server == "all" || x.id == server)
            .collect();
        let reads = wanted.iter().map(|x| async move {
            match x.kind {
                ServerType::Plex => (self.plex_plays(x, after).await.unwrap_or_default(), None),
                ServerType::Jellyfin => match self.jellyfin_history(x, after).await {
                    Some((p, oldest)) => {
                        (p, (oldest > after + 86_400).then(|| (x.id.clone(), oldest)))
                    }
                    None => (Vec::new(), None),
                },
            }
        });
        let mut plays = Vec::new();
        let mut since = Vec::new();
        for (p, s) in futures_util::future::join_all(reads).await {
            plays.extend(p);
            since.extend(s.map(|(server_id, since)| ServerSince { server_id, since }));
        }
        let mut stats = aggregate(&plays, &self.dash_people(), days, until, tz);
        stats.since = since;
        let mut cache = self.dashboard.stats.lock().await;
        cache.retain(|_, (at, _)| at.elapsed() < STATS_TTL);
        cache.insert(key, (Instant::now(), stats.clone()));
        stats
    }

    /// The bytes of a dashboard image reference (`plex:/library/…`, `jf:item:…`,
    /// `jf:user:…`), with its content type. Only these shapes: nothing else is fetched.
    pub async fn dash_image(&self, server: &str, img: &str) -> Option<(String, Vec<u8>)> {
        let conf = self.config.server(server)?;
        let ok_id = |x: &str| {
            !x.is_empty() && x.len() <= 64 && x.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        };
        let req = if let Some(path) = img.strip_prefix("plex:") {
            let safe = path.starts_with("/library/metadata/")
                && path.len() < 120
                && path.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/');
            if conf.kind != ServerType::Plex || !safe {
                return None;
            }
            let (url, key) = self.tautulli()?;
            self.http.get(format!("{url}/api/v2")).query(&[
                ("apikey", key),
                ("cmd", "pms_image_proxy"),
                ("img", path),
                ("width", "300"),
                ("height", "450"),
                ("fallback", "poster"),
            ])
        } else if let Some(rest) = img.strip_prefix("jf:") {
            if conf.kind != ServerType::Jellyfin {
                return None;
            }
            let (what, id) = rest.split_once(':')?;
            if !ok_id(id) {
                return None;
            }
            let path = match what {
                "item" => format!("/Items/{id}/Images/Primary?fillHeight=450&quality=85"),
                "user" => format!("/Users/{id}/Images/Primary?fillHeight=96&quality=85"),
                _ => return None,
            };
            let key = self.config.secrets.jellyfin_key(&conf.id)?;
            self.http
                .get(format!("{}{path}", conf.internal()))
                .header("Authorization", format!("MediaBrowser Token=\"{key}\""))
        } else {
            return None;
        };
        let res = req
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| e.without_url())
            .ok()?;
        if !res.status().is_success() {
            return None;
        }
        let mime = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .filter(|t| t.starts_with("image/"))
            .unwrap_or("image/jpeg")
            .to_owned();
        let bytes = res.bytes().await.ok()?;
        (bytes.len() <= 4 << 20).then(|| (mime, bytes.to_vec()))
    }
}

impl Dash {
    /// Whether a reading is due while nobody looks (the background task).
    pub async fn idle_due(&self) -> bool {
        self.live
            .lock()
            .await
            .at
            .is_none_or(|a| a.elapsed() >= IDLE_EVERY - Duration::from_secs(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dates() {
        assert_eq!(day_of(0, 0), "1970-01-01");
        assert_eq!(
            parse_iso("2026-11-20T07:17:28.6041526Z"),
            Some(1_795_159_048)
        );
        assert_eq!(day_of(1_795_159_048, 0), "2026-11-20");
        // 23:30 UTC is the next day in Brussels (UTC+1).
        assert_eq!(
            day_of(parse_iso("2026-11-20T23:30:00Z").unwrap(), 60),
            "2026-11-21"
        );
    }

    #[test]
    fn a_tautulli_session_reads_like_plex_shows_it() {
        let v = json!({
            "media_type": "episode", "grandparent_title": "The Show", "parent_media_index": "1",
            "media_index": "8", "title": "Ep", "grandparent_thumb": "/library/metadata/1/thumb/2",
            "user": "u", "friendly_name": "Someone", "user_id": 42, "user_thumb": "https://plex.tv/users/x/avatar",
            "duration": "2592000", "view_offset": "2417000", "state": "playing",
            "product": "Plex for Android (TV)", "player": "salon", "location": "wan", "local": 0,
            "ip_address": "203.0.113.4", "bandwidth": "4104",
            "video_codec": "hevc", "video_profile": "main 10", "video_full_resolution": "1080p",
            "video_dynamic_range": "SDR", "stream_video_decision": "direct play",
            "audio_codec": "eac3", "audio_channel_layout": "5.1(side)", "audio_language": "English",
            "audio_atmos": 1, "stream_audio_decision": "direct play",
            "subtitles": 1, "subtitle_codec": "srt", "subtitle_language": "French", "subtitle_forced": 1,
            "stream_subtitle_decision": "direct play"
        });
        let x = tautulli_stream("p", &v);
        assert_eq!(x.title, "The Show");
        assert_eq!(x.subtitle, "S1 · E8 Ep");
        assert_eq!(x.video.unwrap().label, "1080p (HEVC Main 10)");
        assert_eq!(x.audio.unwrap().label, "English (EAC3 5.1 + Atmos)");
        assert_eq!(x.subtitle_track.unwrap().label, "French Forced (SRT)");
        assert_eq!(x.user_key.as_deref(), Some("plex:p:42"));
        assert_eq!(x.image.as_deref(), Some("plex:/library/metadata/1/thumb/2"));
        assert!(!x.local);
        assert_eq!(x.kbps, 4104);
    }

    #[test]
    fn a_jellyfin_session_with_a_conversion() {
        let v = json!({
            "UserName": "alex", "UserId": "abc", "Client": "AlwyWatch", "DeviceName": "Chromecast",
            "RemoteEndPoint": "192.168.1.20",
            "NowPlayingItem": {"Type": "Episode", "Name": "Nine", "SeriesName": "Show", "SeriesId": "def",
                "Id": "0a", "ParentIndexNumber": 1, "IndexNumber": 9, "RunTimeTicks": 14_400_000_000i64,
                "MediaStreams": [
                    {"Type": "Video", "Index": 0, "Codec": "hevc", "Profile": "Main 10", "Width": 1920, "Height": 1080, "BitRate": 4_000_000},
                    {"Type": "Audio", "Index": 1, "Codec": "aac", "Language": "jpn", "ChannelLayout": "stereo"},
                    {"Type": "Audio", "Index": 2, "Codec": "eac3", "Language": "fre", "ChannelLayout": "5.1"}
                ]},
            "PlayState": {"PositionTicks": 2_315_310_000i64, "IsPaused": false, "PlayMethod": "Transcode", "AudioStreamIndex": 2},
            "TranscodingInfo": {"IsVideoDirect": false, "IsAudioDirect": true, "VideoCodec": "h264", "Width": 1280, "Height": 720, "Bitrate": 3_000_000}
        });
        let x = jellyfin_stream("j", &v).unwrap();
        assert_eq!(
            (x.title.as_str(), x.subtitle.as_str()),
            ("Show", "S1 · E9 Nine")
        );
        let vid = x.video.unwrap();
        assert_eq!(vid.label, "1080p (HEVC Main 10)");
        assert_eq!(
            (vid.decision.as_str(), vid.to.as_deref()),
            ("transcode", Some("720p (H264)"))
        );
        let a = x.audio.unwrap();
        assert_eq!(
            (a.label.as_str(), a.decision.as_str()),
            ("French (EAC3 5.1)", "direct_stream")
        );
        assert!(x.local);
        assert_eq!(x.kbps, 3000);
        assert_eq!(x.position_ms, 231_531);
        assert_eq!(x.image.as_deref(), Some("jf:item:def"));
        // Nothing playing: no stream.
        assert!(jellyfin_stream("j", &json!({"UserName": "x"})).is_none());
    }

    #[test]
    fn activity_log_pairs_starts_and_stops() {
        let e = |t: &str, ty: &str, item: &str| json!({"Date": t, "Type": ty, "UserId": "u", "ItemId": item});
        let entries = vec![
            e("2026-11-20T10:30:00Z", "VideoPlaybackStopped", "a"),
            e("2026-11-20T10:00:00Z", "VideoPlayback", "a"),
            e("2026-11-20T11:00:00Z", "VideoPlayback", "b"),
            json!({"Date": "2026-11-20T09:00:00Z", "Type": "SessionStarted", "UserId": "u"}),
        ];
        let mut p = jellyfin_plays(&entries);
        p.sort_by_key(|x| x.2);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].1.as_str(), p[0].3), ("a", 1800));
        assert_eq!((p[1].1.as_str(), p[1].3), ("b", 0));
    }

    #[test]
    fn stats_merge_people_across_servers() {
        let play = |server: &str, user: &str, kind, title: &str, start, seconds| Play {
            server_id: server.into(),
            user_key: user.into(),
            user_name: user.into(),
            avatar: None,
            kind,
            title: title.into(),
            image: None,
            start,
            seconds,
            transcode: Some(false),
        };
        let until = 1_795_600_000;
        let plays = vec![
            play("p", "plex:p:1", Kind::Episode, "Show", until - 100, 1200),
            play(
                "j",
                "jellyfin:j:a",
                Kind::Episode,
                "show",
                until - 90_000,
                600,
            ),
            play("j", "jellyfin:j:b", Kind::Movie, "Film", until - 200, 6000),
            play(
                "j",
                "jellyfin:j:b",
                Kind::Movie,
                "Old",
                until - 40 * 86_400,
                6000,
            ),
        ];
        let mut people = People::new();
        people.insert("plex:p:1".into(), ("plex:p:1".into(), "Alex".into()));
        people.insert("jellyfin:j:a".into(), ("plex:p:1".into(), "Alex".into()));
        let st = aggregate(&plays, &people, 7, until, 0);
        assert_eq!(st.history.len(), 7);
        assert_eq!(st.history.iter().map(|d| d.episodes).sum::<u32>(), 2);
        assert_eq!(st.top_users.len(), 2);
        let alex = st.top_users.iter().find(|u| u.name == "Alex").unwrap();
        assert_eq!((alex.plays, alex.episode_seconds), (2, 1800));
        assert_eq!(st.top_shows[0].plays, 2);
        assert_eq!(st.top_shows[0].users, 1);
        assert_eq!(st.top_movies.len(), 1, "the old play is outside the period");
        assert_eq!(st.direct_plays, 3);
    }
}
