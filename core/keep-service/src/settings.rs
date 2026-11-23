// SPDX-License-Identifier: AGPL-3.0-or-later
//! The server's settings as its admins edit them in the apps (Settings → Admin → Server
//! settings), and the first-start setup (alwywatch-server: `server_settings.rs`). Same shapes
//! as alwywatch-server's `settings::Stored`; secrets read [`KEPT`] and are sent back as they
//! are to keep them, empty to remove them, or a new value to replace them.

use serde::{Deserialize, Serialize};

use crate::ServerType;

/// What a secret reads as, and what keeps it when sent back.
pub const KEPT: &str = "\u{2022}\u{2022}\u{2022}\u{2022}";

/// `GET /v1/setup` (public).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct SetupState {
    /// No media server yet: the apps show the setup page.
    pub needed: bool,
}

/// The whole settings (`settings` of `GET /v1/admin/settings`, the body of the `PUT`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct ServerSettings {
    pub servers: Vec<SettingsServer>,
    pub maintainerr: Option<SettingsMaintainerr>,
    pub tautulli: Option<SettingsService>,
    pub seerr: Option<SettingsSeerr>,
    /// This site's address: links in emails, passkeys, Pocket ID.
    pub public_url: Option<String>,
    /// "invite" (also when empty), "open" or "closed".
    pub registration: String,
    pub pocketid: Option<SettingsPocketId>,
    pub smtp: Option<SettingsSmtp>,
    /// Read only (made by the server).
    pub webhook_secret: Option<String>,
    /// Reads everything, writes nothing to Maintainerr or Seerr.
    pub dry_run: bool,
    pub sync_minutes: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct SettingsServer {
    /// Jellyfin server id or Plex machine identifier.
    pub id: String,
    pub kind: Option<ServerType>,
    pub name: String,
    pub public_url: String,
    /// How the AlwyWatch server reaches it (Docker network); the public address when empty.
    pub internal_url: Option<String>,
    /// Jellyfin: AlwyWatch's own API key there.
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct SettingsMaintainerr {
    pub url: String,
    /// The media server Maintainerr manages (one of `servers`, by id).
    pub server: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct SettingsService {
    pub url: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct SettingsSeerr {
    pub url: String,
    /// Seerr's own address for people (links), when published.
    pub public_url: Option<String>,
    pub create_users: bool,
    pub api_key: Option<String>,
}

impl Default for SettingsSeerr {
    fn default() -> Self {
        Self {
            url: String::new(),
            public_url: None,
            create_users: true,
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct SettingsPocketId {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct SettingsSmtp {
    pub host: String,
    /// 465 for "force_tls", 587 for "starttls" when empty.
    pub port: Option<u16>,
    /// "" (automatic), "force_tls", "starttls" or "off".
    pub security: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// The address emails are sent from.
    pub from: String,
    /// The sender's name ("AlwyWatch" when empty).
    pub from_name: Option<String>,
}

/// A webhook address to paste in Jellyfin, Plex or Tautulli (sent as `[kind, url]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Webhook {
    /// "jellyfin", "plex" or "tautulli".
    pub kind: String,
    pub url: String,
}

/// `GET /v1/admin/settings`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct SettingsView {
    pub settings: ServerSettings,
    /// Only once this site's address is set.
    pub webhooks: Vec<Webhook>,
    /// Why Maintainerr isn't used although it is set up.
    pub maintainerr_problem: Option<String>,
}

/// `POST /v1/admin/settings/test`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct SettingsTest {
    /// "server", "maintainerr", "tautulli", "seerr", "email" or "pocketid".
    pub what: String,
    /// The settings being edited (not saved yet).
    pub settings: ServerSettings,
    /// "server": which one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    /// "email": also send a test email there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_to: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
}

/// Adds a Jellyfin server (or renews AlwyWatch's key there) with an administrator's login,
/// used once and not kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AddJellyfin {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal_url: Option<String>,
    pub username: String,
    pub password: String,
}

/// Adds a Plex server owned by the person signed in to plex.tv with `token`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AddPlex {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal_url: Option<String>,
    pub token: String,
}

/// The answer to adding a server.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(default)]
pub struct AddedServer {
    /// The server's own name.
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_servers_answer() {
        let v: SettingsView = serde_json::from_str(
            r#"{"settings":{"servers":[{"id":"a","kind":"jellyfin","name":"HomeJelly",
            "public_url":"https://jellyfin.example.org","internal_url":null,"api_key":"••••"}],
            "maintainerr":null,"tautulli":null,"seerr":{"url":"http://seerr:5055",
            "public_url":null,"create_users":false,"api_key":null},"public_url":null,
            "registration":"invite","pocketid":null,"smtp":null,"webhook_secret":"••••",
            "dry_run":false,"sync_minutes":null},
            "webhooks":[["jellyfin","https://watch.example.org/v1/hooks/jellyfin/x"]],
            "maintainerr_problem":null}"#,
        )
        .unwrap();
        assert_eq!(v.settings.servers[0].kind, Some(ServerType::Jellyfin));
        assert_eq!(v.settings.servers[0].api_key.as_deref(), Some(KEPT));
        assert!(!v.settings.seerr.unwrap().create_users);
        assert_eq!(v.webhooks[0].kind, "jellyfin");
    }
}
