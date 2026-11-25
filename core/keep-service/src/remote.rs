// SPDX-License-Identifier: AGPL-3.0-or-later
//! Play on the TV: a phone starts a title on the same person's AlwyWatch TV and controls it.
//!
//! While AlwyWatch is open on a TV with a profile, the TV long-polls `listen`: that is both its
//! presence (the person's phones list it) and how it receives commands. It reports what it
//! plays with `report`; the phone shows it in its remote and long-polls `state`. Only the
//! same person (any of their linked sign-ins) sees or controls their TVs; for everyone else a
//! device answers 404, like an unknown one.
//!
//! - `GET /v1/remote/listen?device={id}&name={name}&after={seq}` (TV) → [`RemoteCommands`]:
//!   commands past `after`, as soon as there are any or after about 25 s. `after=0` answers at
//!   once with the current `last_seq` and no commands (nothing sent before the TV was there).
//! - `PUT /v1/remote/devices/{id}/state` (TV) with [`RemoteReport`] → 204.
//! - `DELETE /v1/remote/devices/{id}` (TV, leaving) → 204.
//! - `GET /v1/remote/devices` (phone) → `Vec<`[`RemoteDevice`]`>`: my TVs there now.
//! - `POST /v1/remote/devices/{id}/commands` (phone) with [`RemoteCommand`] (`seq` ignored) →
//!   204; 404 when the TV is gone.
//! - `GET /v1/remote/devices/{id}/state?after={version}` (phone) → [`RemoteState`], as soon as
//!   `version` differs from `after` or after about 25 s; 404 when the TV is gone.
//!
//! Everything lives in the server's memory: a TV that stops listening for a minute is gone.
//! Full protocol: `docs/keep-protocol.md` ("Play on the TV").

use serde::{Deserialize, Serialize};

use crate::together::TogetherMedia;

macro_rules! record {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
        pub struct $name { $($body)* }
    };
}

/// How long `listen` and `state` hold a request without change.
pub const WAIT_SECS: u64 = 25;
/// A TV that has not listened for this long is gone.
pub const DEVICE_TTL_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum RemoteAction {
    /// Start `media` (at `position_ms` when given, else where the TV's server says).
    Play,
    Pause,
    Resume,
    /// Go to `position_ms`.
    Seek,
    /// Close the player.
    Stop,
    /// Audio track `index` (an index of [`RemoteState::audio`]).
    Audio,
    /// Subtitle track `index` (of [`RemoteState::subtitles`]); none: subtitles off.
    Subtitles,
}

record! {
    /// What the phone asks the TV to do.
    pub struct RemoteCommand {
        /// Set by the server (ignored when sending).
        #[serde(default)]
        pub seq: u64,
        pub action: RemoteAction,
        /// Play: the title as every server can find it (the TV may use another server).
        #[serde(default)]
        pub media: Option<TogetherMedia>,
        /// Play: the title on the phone's server; the TV uses it directly when it is signed in
        /// to the same server.
        #[serde(default)]
        pub server_id: Option<String>,
        #[serde(default)]
        pub item_id: Option<String>,
        #[serde(default)]
        pub position_ms: Option<i64>,
        #[serde(default)]
        pub index: Option<i32>,
    }
}

record! {
    /// The commands a TV has not seen yet.
    pub struct RemoteCommands {
        /// Pass it as `after` to the next `listen`.
        pub last_seq: u64,
        pub commands: Vec<RemoteCommand>,
    }
}

record! {
    /// An audio or subtitle track as the TV's player names it.
    pub struct RemoteTrack {
        pub index: i32,
        pub label: String,
    }
}

/// What a TV plays, as it reports it. An empty `title`: nothing plays.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct RemoteReport {
    #[serde(default)]
    pub title: String,
    /// "S1 · E3 · Name" for an episode.
    #[serde(default)]
    pub subtitle: Option<String>,
    /// The title on the TV's server (the phone shows its artwork when it has that server).
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub playing: bool,
    /// Still loading (or buffering): the position is not moving.
    #[serde(default)]
    pub loading: bool,
    #[serde(default)]
    pub position_ms: i64,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub audio: Vec<RemoteTrack>,
    #[serde(default)]
    pub subtitles: Vec<RemoteTrack>,
    #[serde(default)]
    pub audio_index: Option<i32>,
    #[serde(default)]
    pub subtitle_index: Option<i32>,
    /// The last Play could not be done (shown on the phone as it is).
    #[serde(default)]
    pub error: Option<String>,
}

record! {
    /// A TV's last report, as phones get it.
    pub struct RemoteState {
        /// Changes with every report (never 0, a client's "nothing yet").
        pub version: u64,
        /// Server time (Unix ms) of the report: `position_ms` was true then.
        pub at_ms: i64,
        /// Server time now: with `at_ms`, where a playing title is.
        pub now_ms: i64,
        pub report: RemoteReport,
    }
}

record! {
    /// One of my TVs, there now.
    pub struct RemoteDevice {
        pub id: String,
        pub name: String,
        /// What it plays (empty: nothing).
        pub title: String,
    }
}

impl RemoteState {
    /// Where playback is now (ms), moving on from the report while it plays.
    pub fn position_now(&self) -> i64 {
        let r = &self.report;
        let moved = if r.playing && !r.loading {
            (self.now_ms - self.at_ms).max(0)
        } else {
            0
        };
        let p = r.position_ms + moved;
        if r.duration_ms > 0 {
            p.min(r.duration_ms)
        } else {
            p
        }
    }
}

/// A device id or name as the server keeps it: no control characters, bounded length.
pub fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_moves_only_while_playing() {
        let mut s = RemoteState {
            version: 1,
            at_ms: 1_000,
            now_ms: 4_000,
            report: RemoteReport {
                title: "T".into(),
                subtitle: None,
                server_id: None,
                item_id: None,
                playing: true,
                loading: false,
                position_ms: 10_000,
                duration_ms: 12_000,
                audio: vec![],
                subtitles: vec![],
                audio_index: None,
                subtitle_index: None,
                error: None,
            },
        };
        assert_eq!(s.position_now(), 12_000);
        s.report.duration_ms = 0;
        assert_eq!(s.position_now(), 13_000);
        s.report.playing = false;
        assert_eq!(s.position_now(), 10_000);
    }

    #[test]
    fn a_command_needs_only_its_action() {
        let c: RemoteCommand = serde_json::from_str(r#"{"action":"pause"}"#).unwrap();
        assert_eq!(c.action, RemoteAction::Pause);
        assert_eq!(c.seq, 0);
        let r: RemoteReport = serde_json::from_str("{}").unwrap();
        assert!(r.title.is_empty());
    }
}
