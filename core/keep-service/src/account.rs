// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch accounts (docs/keep-protocol.md "AlwyWatch accounts"): signing in without a media
//! server (username + password, passkeys, Pocket ID) and Quick Connect approvals. The types
//! are shared with `alwywatch-server`; the calls are on [`crate::KeepClient`], made with
//! [`crate::KeepClient::account`] for an account session.

use serde::{Deserialize, Serialize};

/// Account session tokens start with this.
pub const SESSION_PREFIX: &str = "aws_";

/// `GET /v1/account/options`: what the server offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountOptions {
    /// "invite", "open" or "closed".
    pub registration: String,
    pub passkeys: bool,
    pub pocketid: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedIn {
    /// The session token (`aws_…`): secret, kept in secure storage.
    pub token: String,
    pub account: AccountView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountView {
    pub id: String,
    pub username: Option<String>,
    pub name: String,
    /// The person's key (as in rooms and profiles).
    pub user_key: String,
    pub has_password: bool,
    #[serde(default)]
    pub passkeys: Vec<PasskeyView>,
    #[serde(default)]
    pub pocketid: bool,
    #[serde(default)]
    pub sessions: Vec<SessionView>,
    /// Media-server sign-ins linked to this person.
    #[serde(default)]
    pub linked: Vec<LinkedView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PasskeyView {
    pub id: String,
    pub name: String,
    pub created: i64,
    pub last_used: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub device: String,
    pub created: i64,
    pub last_seen: i64,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkedView {
    pub server_id: String,
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
    #[serde(default)]
    pub device: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub device: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PasswordChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteRequest {
    pub days: i64,
    pub uses: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    /// The web page that signs up with it.
    pub url: Option<String>,
    pub expires_in_days: i64,
    pub uses: i64,
}

/// `PUT /v1/transfer/{code}/approve`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approved {
    pub device: String,
}

/// A server the user added, kept in their synced settings under [`SERVERS_SETTING`] so a new
/// device signed in to the account offers them (no tokens: the user signs in on each).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedServer {
    /// "jellyfin" or "plex".
    pub kind: String,
    pub url: String,
    pub name: String,
}

/// Synced setting holding the user's servers (JSON list of [`SavedServer`]).
pub const SERVERS_SETTING: &str = "account.servers";

/// A user-facing reason for an account call's failure.
pub fn explain(e: &net::Error, signing_in: bool) -> String {
    match e {
        net::Error::Unauthorized if signing_in => "Wrong username or password.".into(),
        net::Error::Unauthorized => "This sign-in has ended. Sign in again.".into(),
        net::Error::NotFound => "Unknown or expired code.".into(),
        net::Error::Status { status: 409 } => "This username is already taken.".into(),
        net::Error::Status { status: 429 } => {
            "Too many attempts. Try again in a few minutes.".into()
        }
        net::Error::Status { status: 400 } => {
            "Check what you typed: usernames are 3 to 32 letters, digits, dots, dashes or underscores; passwords at least 8 characters.".into()
        }
        net::Error::Status { status: 501 } => "This server doesn't offer that.".into(),
        other => other.to_string(),
    }
}
