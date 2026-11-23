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
    /// "Forgot password" can email a link (the server has email set up).
    #[serde(default)]
    pub password_reset: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedIn {
    /// The session token (`aws_…`): secret, kept in secure storage.
    pub token: String,
    pub account: AccountView,
    /// The invitation also made a Jellyfin login with the same username and password: the app
    /// signs in to it right away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jellyfin: Option<NewJellyfinLogin>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewJellyfinLogin {
    pub server_id: String,
    pub name: String,
    /// The address the apps use.
    pub url: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountView {
    pub id: String,
    pub username: Option<String>,
    pub name: String,
    /// The person's key (as in rooms and profiles).
    pub user_key: String,
    pub has_password: bool,
    /// Where "forgot password" sends its link.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub passkeys: Vec<PasskeyView>,
    #[serde(default)]
    pub pocketid: bool,
    #[serde(default)]
    pub sessions: Vec<SessionView>,
    /// Media-server sign-ins linked to this person.
    #[serde(default)]
    pub linked: Vec<LinkedView>,
    /// Whether a Jellyfin/Plex sign-in of the person may act on the account (set its
    /// username and password, sign new devices in to it). The owner's choice
    /// (`PUT /v1/account/media-sign-in`); on unless turned off.
    #[serde(default = "yes")]
    pub media_sign_in: bool,
}

fn yes() -> bool {
    true
}

fn seven() -> i64 {
    7
}

fn one() -> i64 {
    1
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
    /// For "forgot password" (an invitation's email is offered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub device: String,
}

/// `POST /v1/account/password/forgot`: a username or an email address. Always answered the
/// same (nothing says which accounts exist).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForgotRequest {
    pub login: String,
    /// The email's language.
    #[serde(default)]
    pub lang: String,
}

/// `POST /v1/account/password/reset`: the emailed link's token and the new password. Signs
/// every device out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResetRequest {
    pub token: String,
    pub password: String,
}

/// `PUT /v1/account/email`: None removes it. `current`: the password, when the account has one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmailChange {
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
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
    #[serde(default = "seven")]
    pub days: i64,
    #[serde(default = "one")]
    pub uses: i64,
    /// Send it by email (one use), from the server's email settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Also make a Jellyfin login on this server (its id), with the same username and password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jellyfin: Option<String>,
    /// That login gets the same access (libraries, ratings…) as this Jellyfin user (its id);
    /// Jellyfin's defaults when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub like: Option<String>,
    /// A few words from the admin, in the email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The email's language (the admin's app language).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    /// The web page that signs up with it.
    pub url: Option<String>,
    pub expires_in_days: i64,
    pub uses: i64,
    /// Where it was emailed.
    #[serde(default)]
    pub sent_to: Option<String>,
}

/// `GET /v1/account/invites` (admins): what an invitation can do here, and the invitations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invitations {
    /// Emails can be sent (Server settings → Email).
    pub mail: bool,
    /// This site's address is set (needed for the links).
    pub public_url: bool,
    /// Jellyfin servers where an invitation can make a login (those with an API key).
    pub jellyfin: Vec<JellyfinTarget>,
    pub invites: Vec<InviteItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JellyfinTarget {
    pub server_id: String,
    pub name: String,
    /// Users whose access a new login can copy (not admins, not disabled).
    pub users: Vec<JellyfinUser>,
    /// The user copied last time, to offer first.
    #[serde(default)]
    pub last_like: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JellyfinUser {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteItem {
    pub id: i64,
    pub created_by: String,
    pub created: i64,
    pub expires: i64,
    pub uses_left: i64,
    pub email: Option<String>,
    /// The Jellyfin server name, when it also makes a login there.
    pub jellyfin: Option<String>,
    /// Usernames of the accounts made with it.
    pub used_by: Vec<String>,
}

/// `GET /v1/account/invites/check/{code}` (public): what the sign-up page shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteInfo {
    pub inviter: String,
    /// The Jellyfin server the account also gets a login on.
    pub jellyfin: Option<String>,
    /// Who it was sent to: offered as the account's email.
    pub email: Option<String>,
    pub expires: i64,
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
    use i18n::tr;

    match e {
        net::Error::Unauthorized if signing_in => tr!("Wrong username or password."),
        net::Error::Unauthorized => tr!("This sign-in has ended. Sign in again."),
        net::Error::NotFound => tr!("Unknown or expired code."),
        net::Error::Status { status: 409 } => tr!("This username is already taken."),
        net::Error::Status { status: 429 } => {
            tr!("Too many attempts. Try again in a few minutes.")
        }
        net::Error::Status { status: 400 } => {
            tr!(
                "Check what you typed: usernames are 3 to 32 letters, digits, dots, dashes or underscores; passwords at least 8 characters."
            )
        }
        net::Error::Status { status: 501 } => tr!("This server doesn't offer that."),
        other => other.to_string(),
    }
}
