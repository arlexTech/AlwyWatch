// SPDX-License-Identifier: AGPL-3.0-or-later
//! Linked identities: one AlwyWatch account for a person's Plex and Jellyfin users.
//!
//! - `GET /v1/me/links` → `[LinkedIdentity]`: the sign-ins of the caller's account.
//! - `POST /v1/me/links` with [`LinkRequest`] (a second served server and that server's token
//!   for the same person) → `[LinkedIdentity]`: both sign-ins are now one account.
//! - `DELETE /v1/me/links/{user_key}` → `[LinkedIdentity]`: that sign-in is its own (new,
//!   empty) account again. The canonical sign-in cannot be removed (409).
//!
//! - `POST /v1/account/server-sign-in` (account session) with [`ServerSignInRequest`] →
//!   [`ServerSignIn`]: signs this device in to a linked server without typing anything
//!   (Jellyfin: the server approves the device's Quick Connect code; Plex: it hands over that
//!   user's server token).
//!
//! `GET /v1/me` (and every answer that is a `Me`) also carries `identities`; read it with
//! [`crate::KeepClient::me_with_identities`]. Details and merge rules: `docs/keep-protocol.md`,
//! "Linked identities"; app flows: `docs/identity-apps.md`.

use serde::{Deserialize, Serialize};

use crate::{Me, ServerType};

macro_rules! record {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
        pub struct $name { $($body)* }
    };
}

record! {
    /// One media-server user of the caller's AlwyWatch account.
    pub struct LinkedIdentity {
        /// `jellyfin:<server>:<user id>` or `plex:<server>:<plex user id>`.
        pub user_key: String,
        pub server_id: String,
        /// Allow-list name of the server (its id when it is no longer served).
        pub server_name: String,
        pub kind: ServerType,
        /// The user's name on that media server.
        pub media_name: String,
        /// The account's data is stored under this one (`Me.user_key`); cannot be unlinked.
        pub canonical: bool,
        /// The sign-in this request was made with.
        #[serde(default)]
        pub current: bool,
    }
}

/// `POST /v1/me/links` body: the second sign-in, proven by its own token (validated exactly
/// like a sign-in, never stored nor logged). `Debug` never shows the token.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkRequest {
    /// An allow-listed server id (`/v1/servers`).
    pub server_id: String,
    /// Jellyfin: the access token; Plex: the plex.tv account token.
    pub token: String,
}

impl std::fmt::Debug for LinkRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkRequest")
            .field("server_id", &self.server_id)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// `POST /v1/account/server-sign-in` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerSignInRequest {
    /// A served server one of my sign-ins is on.
    pub server_id: String,
    /// Jellyfin: the Quick Connect code this device just started (the server approves it as
    /// my user there). Plex: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// `POST /v1/account/server-sign-in` answer. `Debug` never shows the token.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerSignIn {
    pub kind: ServerType,
    pub server_id: String,
    pub server_name: String,
    /// The server's public address.
    pub url: String,
    pub user_id: String,
    pub user_name: String,
    pub is_admin: bool,
    /// Plex: my token for this server (to use like any Plex server token). Jellyfin: none,
    /// the Quick Connect code was approved: poll it as usual.
    #[serde(default)]
    pub token: Option<String>,
}

impl std::fmt::Debug for ServerSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerSignIn")
            .field("kind", &self.kind)
            .field("server_id", &self.server_id)
            .field("user_id", &self.user_id)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// `GET /v1/me` read with its `identities` (kept out of [`Me`] so existing code building a
/// `Me` keeps compiling).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeWithIdentities {
    #[serde(flatten)]
    pub me: Me,
    /// Every sign-in of the account, the canonical one included (one entry: nothing linked).
    #[serde(default)]
    pub identities: Vec<LinkedIdentity>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = r#"{"user_key":"jellyfin:j:1","name":"Ann","media_name":"ann",
        "name_is_custom":false,"avatar":{"preset":null,"custom_version":null},"profile_set":true,
        "server_id":"p","is_admin":false,
        "rights":{"can_keep_forever":true,"max_forever":null,"max_keep_days":null},
        "forever_count":2,
        "identities":[
          {"user_key":"jellyfin:j:1","server_id":"j","server_name":"Fin","kind":"jellyfin",
           "media_name":"ann","canonical":true,"current":false},
          {"user_key":"plex:p:9","server_id":"p","server_name":"Px","kind":"plex",
           "media_name":"Ann P","canonical":false,"current":true}]}"#;

    #[test]
    fn me_still_decodes_with_identities_and_they_can_be_read() {
        let me: Me = serde_json::from_str(ME).unwrap();
        assert_eq!(me.user_key, "jellyfin:j:1");
        let full: MeWithIdentities = serde_json::from_str(ME).unwrap();
        assert_eq!(full.me, me);
        assert_eq!(full.identities.len(), 2);
        assert!(full.identities[0].canonical && full.identities[1].current);
        assert_eq!(full.identities[1].kind, ServerType::Plex);
    }

    #[test]
    fn older_servers_without_identities_or_current() {
        let old = ME
            .split(",\n        \"identities\"")
            .next()
            .unwrap()
            .to_owned()
            + "}";
        let full: MeWithIdentities = serde_json::from_str(&old).unwrap();
        assert!(full.identities.is_empty());
        let i: LinkedIdentity = serde_json::from_str(
            r#"{"user_key":"k","server_id":"s","server_name":"S","kind":"plex","media_name":"m","canonical":true}"#,
        )
        .unwrap();
        assert!(!i.current);
    }

    #[test]
    fn link_request_never_shows_its_token() {
        let r = LinkRequest {
            server_id: "s".into(),
            token: "secret-token".into(),
        };
        let shown = format!("{r:?}");
        assert!(!shown.contains("secret-token") && shown.contains("redacted"));
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            serde_json::json!({"server_id": "s", "token": "secret-token"})
        );
    }
}
