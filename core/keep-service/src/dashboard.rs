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
    /// When `position_ms` was true (server time, Unix ms), when the media server says
    /// (Jellyfin); clients move a playing stream on from there.
    #[serde(default)]
    pub position_at_ms: Option<i64>,
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

impl Stream {
    /// Where this stream is now (ms), for a clock that moves it on while it plays.
    /// `now_ms`: the server time of the answer ([`Live::now_ms`]); `shown`: what the clock
    /// shows now (None the first time). With the media server's sample time (Jellyfin) the
    /// answer is exact; without it (Plex through Tautulli) a sample is a few seconds old, so a
    /// playing stream only goes back for a real seek, never for an old sample.
    pub fn position_now(&self, now_ms: i64, shown: Option<i64>) -> i64 {
        let playing = self.state == "playing";
        let end = if self.duration_ms > 0 {
            self.duration_ms
        } else {
            i64::MAX
        };
        let sampled = match self.position_at_ms {
            Some(at) if playing && now_ms > 0 => self.position_ms + (now_ms - at).clamp(0, 60_000),
            _ => self.position_ms,
        };
        let fresh = match shown {
            // Under a second and a half apart: the clock is right, no tiny jumps.
            Some(p) if playing && (p - sampled).abs() < 1_500 => p,
            // An old sample (no sample time): the clock is ahead, as it should be.
            Some(p)
                if playing
                    && self.position_at_ms.is_none()
                    && (0..15_000).contains(&(p - sampled)) =>
            {
                p
            }
            _ => sampled,
        };
        fresh.clamp(0, end)
    }
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
    /// Server time of this answer (Unix ms): with `position_at_ms`, where streams are now.
    #[serde(default)]
    pub now_ms: i64,
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

#[cfg(test)]
mod position_tests {
    use super::Stream;

    fn stream(state: &str, pos: i64, at: Option<i64>) -> Stream {
        Stream {
            state: state.into(),
            position_ms: pos,
            position_at_ms: at,
            duration_ms: 3_600_000,
            ..Default::default()
        }
    }

    #[test]
    fn a_sample_with_its_time_is_moved_on_to_now() {
        assert_eq!(
            stream("playing", 60_000, Some(1_000)).position_now(8_000, None),
            67_000
        );
        // Paused: where it is.
        assert_eq!(
            stream("paused", 60_000, Some(1_000)).position_now(8_000, None),
            60_000
        );
    }

    #[test]
    fn the_clock_stays_when_close_and_follows_a_seek() {
        let s = stream("playing", 60_000, Some(1_000));
        assert_eq!(s.position_now(8_000, Some(67_800)), 67_800);
        assert_eq!(s.position_now(8_000, Some(200_000)), 67_000);
    }

    #[test]
    fn an_old_sample_without_time_never_pulls_back_a_playing_clock() {
        let s = stream("playing", 60_000, None);
        assert_eq!(s.position_now(0, Some(66_000)), 66_000);
        // Ahead by more than any sample's age: a seek back.
        assert_eq!(s.position_now(0, Some(90_000)), 60_000);
        // Behind: a seek forward (or the first sample).
        assert_eq!(s.position_now(0, Some(30_000)), 60_000);
    }
}
