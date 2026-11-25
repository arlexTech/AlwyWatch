// SPDX-License-Identifier: AGPL-3.0-or-later
//! The admin dashboard: what is playing now, bandwidth and host load over the last minutes,
//! and who watched what over the last days, per media server or merged.
//!
//! - `GET /v1/admin/dashboard/live?server={id|all}` → [`Live`] (cheap: polled every few
//!   seconds while the page is open).
//! - `GET /v1/admin/dashboard/stats?server={id|all}&days={n}` → [`Stats`] (cached a few
//!   minutes on the server).
//! - `GET /v1/admin/dashboard/image?server={id}&img={ref}` → the poster or avatar bytes
//!   (`img` is a [`Stream::image`], [`TopTitle::image`] or `*_avatar` value as given).
//!
//! Plex comes through Tautulli, Jellyfin through its API key (sessions) and activity log
//! (history). CPU and RAM are the AlwyWatch server's host.

use serde::{Deserialize, Serialize};

/// One media server the dashboard can show.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct DashServer {
    pub id: String,
    pub name: String,
    /// "plex" or "jellyfin".
    pub kind: String,
    /// Why it shows nothing: "no_tautulli" (Plex needs Tautulli), "no_key" (Jellyfin needs
    /// its API key) or "unreachable".
    #[serde(default)]
    pub problem: Option<String>,
}

/// One track of a stream and what the server does with it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct StreamTrack {
    /// "1080p (HEVC Main 10)", "English (EAC3 5.1)", "French Forced (SRT)".
    pub label: String,
    /// "direct_play", "direct_stream" (copied into another container) or "transcode".
    pub decision: String,
    /// What it is converted to, when converted ("720p H264").
    #[serde(default)]
    pub to: Option<String>,
}

/// Something playing right now.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Stream {
    pub server_id: String,
    pub user: String,
    /// The AlwyWatch user key when this person signed in to AlwyWatch.
    #[serde(default)]
    pub user_key: Option<String>,
    /// An image reference for `/dashboard/image`, or a public https URL.
    #[serde(default)]
    pub user_avatar: Option<String>,
    /// The movie or the show.
    pub title: String,
    /// "S1 · E8 Episode name", or the year of a movie.
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub image: Option<String>,
    pub duration_ms: i64,
    pub position_ms: i64,
    /// "playing", "paused" or "buffering".
    pub state: String,
    /// The app ("Plex for Android (TV)", "AlwyWatch").
    pub product: String,
    /// The device's name ("salon").
    pub device: String,
    pub local: bool,
    #[serde(default)]
    pub ip: String,
    /// What the stream takes now, in kbit/s (0 when unknown).
    pub kbps: u32,
    #[serde(default)]
    pub video: Option<StreamTrack>,
    #[serde(default)]
    pub audio: Option<StreamTrack>,
    #[serde(default)]
    pub subtitle_track: Option<StreamTrack>,
}

/// One point of the live graphs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Sample {
    /// Unix seconds.
    pub t: i64,
    pub local_kbps: u32,
    pub remote_kbps: u32,
    /// Host CPU, 0–100 (None when the host does not say).
    #[serde(default)]
    pub cpu: Option<f32>,
    /// Host memory in use, 0–100.
    #[serde(default)]
    pub ram: Option<f32>,
}

/// `GET /v1/admin/dashboard/live`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Live {
    pub servers: Vec<DashServer>,
    pub streams: Vec<Stream>,
    /// Oldest first, every few seconds, the last 15 minutes at most.
    pub samples: Vec<Sample>,
    /// Host memory size in MB (0 when unknown).
    #[serde(default)]
    pub ram_total_mb: u64,
    #[serde(default)]
    pub cpus: u32,
}

/// A person's watching over the period.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TopUser {
    pub name: String,
    #[serde(default)]
    pub user_key: Option<String>,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub avatar: Option<String>,
    pub plays: u32,
    /// Seconds watched, all kinds.
    pub seconds: u64,
    pub movie_seconds: u64,
    pub episode_seconds: u64,
    pub movie_plays: u32,
    pub episode_plays: u32,
}

/// A movie or show by plays.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TopTitle {
    pub title: String,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub image: Option<String>,
    pub plays: u32,
    /// Distinct people.
    pub users: u32,
    pub seconds: u64,
}

/// Plays on one day (local date of the AlwyWatch server, `YYYY-MM-DD`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct DayPlays {
    pub date: String,
    pub movies: u32,
    pub episodes: u32,
    #[serde(default)]
    pub movie_seconds: u64,
    #[serde(default)]
    pub episode_seconds: u64,
}

/// `GET /v1/admin/dashboard/stats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Stats {
    pub days: u32,
    pub top_users: Vec<TopUser>,
    pub top_movies: Vec<TopTitle>,
    pub top_shows: Vec<TopTitle>,
    /// One entry per day of the period, oldest first, empty days included.
    pub history: Vec<DayPlays>,
    /// Plays that were direct play / a conversion, over the period.
    #[serde(default)]
    pub direct_plays: u32,
    #[serde(default)]
    pub transcodes: u32,
    /// Since when the history goes back per server, when shorter than the period (Jellyfin's
    /// activity log keeps about a month): (server id, unix seconds).
    #[serde(default)]
    pub since: Vec<ServerSince>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct ServerSince {
    pub server_id: String,
    pub since: i64,
}
