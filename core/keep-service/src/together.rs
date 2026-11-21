// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together: people on different servers (Plex or Jellyfin) play the same title in sync,
//! through a room on the AlwyWatch server. Everyone signed in to that server sees the open rooms;
//! joining one asks its host, who accepts or declines (the same person on another device gets
//! in without asking).
//!
//! The room holds the title (by TMDB/IMDB/TVDB id, and season/episode, so each member plays
//! their own server's copy), whether it plays and the position at a server time. Members
//! long-poll for changes (`wait`), compute where playback should be from the server clock and
//! seek when they drift; every play, pause, seek or title change is sent with `update`.
//!
//! - `GET /v1/together` (signed in) → `Vec<`[`TogetherRoom`]`>`: the open rooms.
//! - `POST /v1/together` (signed in) → [`TogetherJoined`]: a new room, the caller its host.
//! - `POST /v1/together/{code}/knock` (signed in) → [`TogetherKnockState`]: asks to join.
//! - `GET /v1/together/{code}/knock/{id}/wait` → [`TogetherKnockState`], as soon as the host
//!   answered or after about 25 s (the asker's heartbeat: a request not polled for a minute goes).
//! - `DELETE /v1/together/{code}/knock/{id}` → 204: no longer asking.
//! - `POST /v1/together/{code}/requests/{id}` with [`TogetherAnswer`] (host) → [`TogetherState`].
//! - `PUT /v1/together/{code}` with [`TogetherUpdate`] → [`TogetherState`].
//! - `GET /v1/together/{code}/wait?after={seq}&member={id}` → [`TogetherState`], as soon as
//!   `seq` is past `after` or after about 25 s (it doubles as the member's heartbeat).
//! - `DELETE /v1/together/{code}/members/{id}` → 204.
//!
//! Rooms live in the server's memory: a member who stops polling for a minute leaves, an
//! empty room goes after five minutes, any room after twelve hours.

use serde::{Deserialize, Serialize};

macro_rules! record {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
        pub struct $name { $($body)* }
    };
}

/// How long `wait` holds a request without change.
pub const WAIT_SECS: u64 = 25;

record! {
    /// A title, as every server can find it.
    pub struct TogetherMedia {
        /// "movie" or "episode".
        pub kind: String,
        pub title: String,
        pub year: Option<i32>,
        pub tmdb: Option<String>,
        pub imdb: Option<String>,
        pub tvdb: Option<String>,
        /// Episodes: the show and its ids.
        pub series_title: Option<String>,
        pub series_tmdb: Option<String>,
        pub series_tvdb: Option<String>,
        pub season: Option<u32>,
        pub episode: Option<u32>,
    }
}

record! {
    pub struct TogetherMember {
        pub id: String,
        pub name: String,
    }
}

record! {
    pub struct TogetherState {
        pub code: String,
        /// Bumped on every change.
        pub seq: u64,
        pub media: TogetherMedia,
        pub playing: bool,
        /// Position at `at_ms` (server clock, Unix ms).
        pub position_ms: u64,
        pub at_ms: i64,
        /// Server clock when this answer was made (to estimate the offset).
        pub now_ms: i64,
        /// Member who made the last change (empty when it was the server).
        pub by: String,
        pub members: Vec<TogetherMember>,
        /// Member id of the host (who answers join requests; the longest member after they leave).
        #[serde(default)]
        pub host: String,
        /// People asking to join, for the host to answer.
        #[serde(default)]
        pub requests: Vec<TogetherMember>,
    }
}

record! {
    /// An open room, as listed to everyone signed in to the server.
    pub struct TogetherRoom {
        pub code: String,
        /// The host's name.
        pub host: String,
        pub media: TogetherMedia,
        /// Members' names.
        pub members: Vec<String>,
        pub playing: bool,
        pub position_ms: u64,
        pub at_ms: i64,
        pub now_ms: i64,
        /// The caller is in it (another of their devices).
        pub mine: bool,
    }
}

/// Where a join request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "lowercase")]
pub enum KnockStatus {
    Waiting,
    Accepted,
    Declined,
}

record! {
    pub struct TogetherKnockState {
        pub request_id: String,
        pub status: KnockStatus,
        /// Accepted: the asker's seat and the room.
        pub joined: Option<TogetherJoined>,
    }
}

record! {
    pub struct TogetherAnswer {
        /// The host's member id.
        pub member_id: String,
        pub accept: bool,
    }
}

record! {
    pub struct TogetherJoined {
        pub member_id: String,
        pub state: TogetherState,
    }
}

record! {
    pub struct TogetherCreate {
        pub media: TogetherMedia,
        pub position_ms: u64,
        pub playing: bool,
    }
}

record! {
    pub struct TogetherUpdate {
        pub member_id: String,
        pub playing: bool,
        pub position_ms: u64,
        /// A new title (next episode, another movie).
        pub media: Option<TogetherMedia>,
    }
}

impl TogetherState {
    /// Where playback should be now, on the server clock (`server_now_ms`).
    pub fn position_at(&self, server_now_ms: i64) -> u64 {
        if !self.playing {
            return self.position_ms;
        }
        let elapsed = (server_now_ms - self.at_ms).max(0) as u64;
        self.position_ms + elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_moves_only_while_playing() {
        let mut s = TogetherState {
            code: "1".into(),
            seq: 1,
            media: TogetherMedia {
                kind: "movie".into(),
                title: "X".into(),
                year: None,
                tmdb: None,
                imdb: None,
                tvdb: None,
                series_title: None,
                series_tmdb: None,
                series_tvdb: None,
                season: None,
                episode: None,
            },
            playing: true,
            position_ms: 10_000,
            at_ms: 1_000,
            now_ms: 1_000,
            by: String::new(),
            members: vec![],
            host: String::new(),
            requests: vec![],
        };
        assert_eq!(s.position_at(3_500), 12_500);
        s.playing = false;
        assert_eq!(s.position_at(3_500), 10_000);
    }
}
