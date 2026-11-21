// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together: people on different servers (Plex or Jellyfin) play the same title in sync,
//! through a private room on the AlwyWatch server.
//!
//! A room starts as a **lobby**: its host picks the title, invites people from
//! [`TogetherPerson`] (everyone who signed in to this AlwyWatch server, recent partners first),
//! and starts it when ready. Invited people see the room in their own list and join without
//! any approval step (or decline); the same user on another device joins directly too. Nobody
//! else can see that a room exists: every room call from them answers 404.
//!
//! The room holds the title (by TMDB/IMDB/TVDB id, and season/episode, so each member plays
//! their own server's copy), whether it plays and the position at a server time. Members
//! long-poll for changes (`wait`), compute where playback should be from the server clock and
//! seek when they drift; every play, pause, seek or title change is sent with `update`.
//!
//! - `GET /v1/together` → [`TogetherRooms`]: my rooms and invitations, with a `version`.
//! - `GET /v1/together/wait?after={version}` → [`TogetherRooms`], as soon as `version` differs
//!   from `after` or after about 25 s.
//! - `GET /v1/together/people` → `Vec<`[`TogetherPerson`]`>`: who can be invited.
//! - `POST /v1/together` with [`TogetherCreate`] → [`TogetherJoined`]: a new lobby, the caller
//!   its host.
//! - `POST /v1/together/{code}/invites` with [`TogetherInvite`] (host) → [`TogetherState`].
//! - `DELETE /v1/together/{code}/invites/{user_key}?member={id}` (host) → [`TogetherState`].
//! - `DELETE /v1/together/{code}/invites/me` (invited user) → 204: declined.
//! - `POST /v1/together/{code}/join` (invited user, or a member's other device) →
//!   [`TogetherJoined`].
//! - `PUT /v1/together/{code}` with [`TogetherUpdate`] → [`TogetherState`] (`started: true`
//!   from the host starts a lobby).
//! - `GET /v1/together/{code}/wait?after={seq}&member={id}` → [`TogetherState`], as soon as
//!   `seq` is past `after` or after about 25 s (it doubles as the member's heartbeat).
//! - `DELETE /v1/together/{code}/members/{id}` → 204.
//!
//! Rooms live in the server's memory: a member who stops polling for a minute is dropped, an
//! empty room goes after five minutes, any room after twelve hours. Full protocol:
//! `docs/keep-protocol.md`, app flows: `docs/together-apps.md`.

use serde::{Deserialize, Serialize};

macro_rules! record {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
        pub struct $name { $($body)* }
    };
}

/// How long `wait` (and the room list's `wait`) holds a request without change.
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
    /// One seat in a room (one device of one user).
    pub struct TogetherMember {
        /// Member id (secret to everyone but the room's members).
        pub id: String,
        pub name: String,
        /// The user behind this seat (several seats when they joined from several devices).
        #[serde(default)]
        pub user_key: String,
        #[serde(default)]
        pub avatar: crate::Avatar,
    }
}

record! {
    /// Someone invited to a room who is not in it (yet, or any more).
    pub struct TogetherInvitee {
        pub user_key: String,
        pub name: String,
        #[serde(default)]
        pub avatar: crate::Avatar,
    }
}

record! {
    pub struct TogetherState {
        pub code: String,
        /// Bumped on every change (playback, title, members, invitations, start).
        pub seq: u64,
        pub media: TogetherMedia,
        /// False while the room is a lobby; set once by the host, never goes back.
        #[serde(default)]
        pub started: bool,
        /// Always false in a lobby.
        pub playing: bool,
        /// Position at `at_ms` (server clock, Unix ms).
        pub position_ms: u64,
        pub at_ms: i64,
        /// Server clock when this answer was made (to estimate the offset).
        pub now_ms: i64,
        /// Member who made the last change (empty when it was the server: joins, leaves,
        /// invitations).
        pub by: String,
        /// Sorted by name.
        pub members: Vec<TogetherMember>,
        /// Member id of the host (the longest-present member after they leave). Every seat of
        /// the host's user has the host's rights: see [`TogetherState::is_host`].
        #[serde(default)]
        pub host: String,
        /// People invited who are not in the room, sorted by name.
        #[serde(default)]
        pub invited: Vec<TogetherInvitee>,
    }
}

record! {
    /// A room as the caller's list shows it: only rooms they are in or invited to.
    pub struct TogetherRoom {
        pub code: String,
        /// The host's display name.
        pub host: String,
        #[serde(default)]
        pub host_avatar: crate::Avatar,
        pub media: TogetherMedia,
        /// Members' names (sorted; a user on two devices appears twice).
        pub members: Vec<String>,
        pub started: bool,
        pub playing: bool,
        pub position_ms: u64,
        pub at_ms: i64,
        pub now_ms: i64,
        /// The caller is in it (on any of their devices).
        pub mine: bool,
        /// The caller is the host's user.
        pub hosting: bool,
        /// The caller is invited and not in it: offer Join / Not now.
        pub invited: bool,
    }
}

record! {
    /// The caller's rooms and invitations.
    pub struct TogetherRooms {
        /// Changes whenever this list changes in a way the caller should see (invited,
        /// uninvited, room started or closed, title, members). Opaque; below 2^53.
        pub version: u64,
        /// Invitations first, then the caller's rooms; newest first within each.
        pub rooms: Vec<TogetherRoom>,
    }
}

record! {
    /// Someone the caller can invite.
    pub struct TogetherPerson {
        pub user_key: String,
        /// Display name (chosen, else the media-server name).
        pub name: String,
        /// The media-server name (names are not unique: show it next to `name`).
        pub media_name: String,
        pub avatar: crate::Avatar,
        /// Display name of the media server they sign in with (tells two "alex" apart).
        pub server_name: String,
        /// Last time (unix seconds) the caller and this person were in a started room together.
        pub last_together_at: Option<i64>,
    }
}

record! {
    pub struct TogetherJoined {
        pub member_id: String,
        pub state: TogetherState,
    }
}

record! {
    /// A new lobby with this title.
    pub struct TogetherCreate {
        pub media: TogetherMedia,
    }
}

record! {
    /// The host invites people (by `TogetherPerson::user_key`).
    pub struct TogetherInvite {
        /// One of the host user's member ids.
        pub member_id: String,
        /// 1 to 20 users.
        pub user_keys: Vec<String>,
    }
}

record! {
    pub struct TogetherUpdate {
        pub member_id: String,
        pub playing: bool,
        pub position_ms: u64,
        /// A new title (next episode, another movie).
        pub media: Option<TogetherMedia>,
        /// True: start the lobby (host only). False changes nothing (a room never goes back).
        #[serde(default)]
        pub started: bool,
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

    /// Whether this seat has the host's rights (invite, start, change the lobby): it belongs to
    /// the same user as the host's seat.
    pub fn is_host(&self, member_id: &str) -> bool {
        let user = |id: &str| {
            self.members
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.user_key.as_str())
        };
        if !member_id.is_empty() && member_id == self.host {
            return true;
        }
        match (user(member_id), user(&self.host)) {
            (Some(a), Some(b)) => !a.is_empty() && a == b,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> TogetherState {
        TogetherState {
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
            started: true,
            playing: true,
            position_ms: 10_000,
            at_ms: 1_000,
            now_ms: 1_000,
            by: String::new(),
            members: vec![],
            host: String::new(),
            invited: vec![],
        }
    }

    #[test]
    fn position_moves_only_while_playing() {
        let mut s = state();
        assert_eq!(s.position_at(3_500), 12_500);
        s.playing = false;
        assert_eq!(s.position_at(3_500), 10_000);
    }

    #[test]
    fn every_seat_of_the_host_user_is_host() {
        let mut s = state();
        let m = |id: &str, user: &str| TogetherMember {
            id: id.into(),
            name: user.into(),
            user_key: user.into(),
            avatar: Default::default(),
        };
        s.members = vec![m("a1", "ana"), m("a2", "ana"), m("b", "bo")];
        s.host = "a1".into();
        assert!(s.is_host("a1"));
        assert!(s.is_host("a2"));
        assert!(!s.is_host("b"));
        assert!(!s.is_host("unknown"));
    }

    #[test]
    fn updates_without_started_do_not_start() {
        let u: TogetherUpdate = serde_json::from_str(
            r#"{"member_id":"a","playing":true,"position_ms":3,"media":null}"#,
        )
        .unwrap();
        assert!(!u.started);
    }
}
