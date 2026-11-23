// SPDX-License-Identifier: AGPL-3.0-or-later
//! The server's settings, edited by its admins in the web app (Settings → Admin → Server
//! settings), kept in `<data>/settings.json`. Only the port and the data folder come from the
//! environment. A server set up the old way (config.toml + .env) is imported once, on the first
//! start of a version with this file.

use std::path::{Path, PathBuf};

use keep_service::ServerType;
use serde::{Deserialize, Serialize};

/// What a secret reads as when sent to the apps, and what they send back to keep it.
pub const KEPT: &str = "\u{2022}\u{2022}\u{2022}\u{2022}";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stored {
    pub servers: Vec<StoredServer>,
    pub maintainerr: Option<StoredMaintainerr>,
    pub tautulli: Option<StoredService>,
    pub seerr: Option<StoredSeerr>,
    /// This site's address (`https://watch.example.org`): links in emails, passkeys, Pocket ID.
    pub public_url: Option<String>,
    /// Who may create an account: "invite" (default), "open" or "closed".
    pub registration: String,
    pub pocketid: Option<StoredPocketId>,
    pub smtp: Option<StoredSmtp>,
    /// Path secret of the webhook addresses (made automatically).
    pub webhook_secret: Option<String>,
    /// Reads everything, writes nothing to Maintainerr or Seerr.
    pub dry_run: bool,
    /// Minutes between two readings of Maintainerr.
    pub sync_minutes: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredServer {
    /// Jellyfin server id or Plex machine identifier.
    pub id: String,
    pub kind: Option<ServerType>,
    pub name: String,
    pub public_url: String,
    /// How this server reaches it (Docker network); the public address when empty.
    pub internal_url: Option<String>,
    /// Jellyfin: the API key AlwyWatch uses ("until watched", invitations).
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredMaintainerr {
    pub url: String,
    /// The media server Maintainerr is set up for (one of `servers`).
    pub server: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredService {
    pub url: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredSeerr {
    pub url: String,
    pub public_url: Option<String>,
    pub create_users: bool,
    pub api_key: Option<String>,
}

impl Default for StoredSeerr {
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
#[serde(default)]
pub struct StoredPocketId {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredSmtp {
    pub host: String,
    /// 465 for "force_tls", 587 for "starttls" when empty.
    pub port: Option<u16>,
    /// "force_tls", "starttls" or "off".
    pub security: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// The address emails are sent from.
    pub from: String,
    /// The sender's name ("AlwyWatch" when empty).
    pub from_name: Option<String>,
}

fn blank(s: &Option<String>) -> bool {
    s.as_deref().is_none_or(|v| v.trim().is_empty())
}

impl Stored {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.json")
    }

    pub fn read(path: &Path) -> Result<Option<Self>, String> {
        match std::fs::read_to_string(path) {
            Ok(t) => serde_json::from_str(&t)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Written whole, then renamed over the old file, readable by the server only.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// For the apps: every secret replaced by [`KEPT`] (or left out when not set).
    pub fn redacted(&self) -> Self {
        let hide = |s: &Option<String>| (!blank(s)).then(|| KEPT.to_owned());
        let mut r = self.clone();
        for s in &mut r.servers {
            s.api_key = hide(&s.api_key);
        }
        if let Some(m) = &mut r.maintainerr {
            m.api_key = hide(&m.api_key);
        }
        if let Some(t) = &mut r.tautulli {
            t.api_key = hide(&t.api_key);
        }
        if let Some(s) = &mut r.seerr {
            s.api_key = hide(&s.api_key);
        }
        if let Some(p) = &mut r.pocketid {
            p.client_secret = hide(&p.client_secret);
        }
        if let Some(m) = &mut r.smtp {
            m.password = hide(&m.password);
        }
        r.webhook_secret = hide(&r.webhook_secret);
        r
    }

    /// The apps' version of the settings, with [`KEPT`] where a secret stays as it is.
    pub fn merged_from(&self, mut new: Self) -> Self {
        fn keep(new: &mut Option<String>, old: Option<&String>) {
            if new.as_deref() == Some(KEPT) {
                *new = old.cloned();
            }
            if blank(new) {
                *new = None;
            }
        }
        for s in &mut new.servers {
            let old = self.servers.iter().find(|o| o.id == s.id);
            keep(&mut s.api_key, old.and_then(|o| o.api_key.as_ref()));
        }
        if let Some(m) = &mut new.maintainerr {
            keep(
                &mut m.api_key,
                self.maintainerr.as_ref().and_then(|o| o.api_key.as_ref()),
            );
        }
        if let Some(t) = &mut new.tautulli {
            keep(
                &mut t.api_key,
                self.tautulli.as_ref().and_then(|o| o.api_key.as_ref()),
            );
        }
        if let Some(s) = &mut new.seerr {
            keep(
                &mut s.api_key,
                self.seerr.as_ref().and_then(|o| o.api_key.as_ref()),
            );
        }
        if let Some(p) = &mut new.pocketid {
            keep(
                &mut p.client_secret,
                self.pocketid
                    .as_ref()
                    .and_then(|o| o.client_secret.as_ref()),
            );
        }
        if let Some(m) = &mut new.smtp {
            keep(
                &mut m.password,
                self.smtp.as_ref().and_then(|o| o.password.as_ref()),
            );
        }
        // Made by the server, never typed: the apps can't change it.
        new.webhook_secret = self.webhook_secret.clone();
        // A section left empty is a section turned off.
        if new
            .maintainerr
            .as_ref()
            .is_some_and(|m| m.url.trim().is_empty())
        {
            new.maintainerr = None;
        }
        if new
            .tautulli
            .as_ref()
            .is_some_and(|t| t.url.trim().is_empty())
        {
            new.tautulli = None;
        }
        if new.seerr.as_ref().is_some_and(|s| s.url.trim().is_empty()) {
            new.seerr = None;
        }
        if new
            .pocketid
            .as_ref()
            .is_some_and(|p| p.issuer.trim().is_empty())
        {
            new.pocketid = None;
        }
        if new.smtp.as_ref().is_some_and(|m| m.host.trim().is_empty()) {
            new.smtp = None;
        }
        if blank(&new.public_url) {
            new.public_url = None;
        }
        new
    }

    /// A webhook secret when there is none yet (webhooks then work as soon as they are set up
    /// on the media server's side).
    pub fn ensure_webhook_secret(&mut self) -> bool {
        if blank(&self.webhook_secret) {
            self.webhook_secret = Some(crate::accounts::random_hex(16));
            true
        } else {
            false
        }
    }

    /// From a server set up the old way: config.toml and the environment.
    pub fn import(toml_text: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Old {
            #[serde(default)]
            servers: Vec<crate::config::ServerConf>,
            #[serde(default)]
            maintainerr: Option<crate::config::MaintainerrConf>,
            tautulli: Option<crate::config::TautulliConf>,
            seerr: Option<crate::config::SeerrConf>,
            #[serde(default)]
            dry_run: bool,
            sync_minutes: Option<u64>,
        }
        let old: Old = toml::from_str(toml_text).map_err(|e| e.to_string())?;
        let env = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let jellyfin_key = |id: &str| {
            std::env::vars()
                .find(|(k, v)| {
                    k.strip_prefix("JELLYFIN_API_KEY_")
                        .is_some_and(|s| s.eq_ignore_ascii_case(id))
                        && !v.trim().is_empty()
                })
                .map(|(_, v)| v.trim().to_owned())
                .or(env("JELLYFIN_API_KEY"))
        };
        Ok(Self {
            servers: old
                .servers
                .into_iter()
                .map(|s| StoredServer {
                    api_key: (s.kind == ServerType::Jellyfin)
                        .then(|| jellyfin_key(&s.id))
                        .flatten(),
                    id: s.id,
                    kind: Some(s.kind),
                    name: s.name,
                    public_url: s.public_url,
                    internal_url: s.internal_url,
                })
                .collect(),
            maintainerr: old
                .maintainerr
                .filter(|m| !m.url.trim().is_empty())
                .map(|m| StoredMaintainerr {
                    url: m.url,
                    server: m.server,
                    api_key: env("MAINTAINERR_API_KEY"),
                }),
            tautulli: old.tautulli.map(|t| StoredService {
                url: t.url,
                api_key: env("TAUTULLI_API_KEY"),
            }),
            seerr: old.seerr.map(|s| StoredSeerr {
                url: s.url,
                public_url: s.public_url,
                create_users: s.create_users,
                api_key: env("SEERR_API_KEY"),
            }),
            public_url: env("ALWYWATCH_PUBLIC_URL"),
            registration: env("ALWYWATCH_REGISTRATION").unwrap_or_default(),
            pocketid: env("ALWYWATCH_POCKETID_ISSUER").map(|issuer| StoredPocketId {
                issuer,
                client_id: env("ALWYWATCH_POCKETID_CLIENT_ID").unwrap_or_default(),
                client_secret: env("ALWYWATCH_POCKETID_CLIENT_SECRET"),
            }),
            smtp: env("SMTP_HOST").map(|host| StoredSmtp {
                host,
                port: env("SMTP_PORT").and_then(|p| p.parse().ok()),
                security: env("SMTP_SECURITY").unwrap_or_else(|| {
                    if env("SMTP_SSL").as_deref() == Some("true") {
                        "force_tls".into()
                    } else {
                        String::new()
                    }
                }),
                username: env("SMTP_USERNAME"),
                password: env("SMTP_PASSWORD"),
                from: env("SMTP_FROM").unwrap_or_default(),
                from_name: env("SMTP_FROM_NAME"),
            }),
            webhook_secret: env("WEBHOOK_SECRET"),
            dry_run: old.dry_run,
            sync_minutes: old.sync_minutes,
        })
    }
}

/// Environment variables that used to hold settings: named at startup when still set, since
/// the app is now where they are changed.
pub const OLD_VARIABLES: [&str; 15] = [
    "MAINTAINERR_API_KEY",
    "TAUTULLI_API_KEY",
    "SEERR_API_KEY",
    "JELLYFIN_API_KEY",
    "WEBHOOK_SECRET",
    "ALWYWATCH_PUBLIC_URL",
    "ALWYWATCH_REGISTRATION",
    "ALWYWATCH_POCKETID_ISSUER",
    "ALWYWATCH_POCKETID_CLIENT_ID",
    "ALWYWATCH_POCKETID_CLIENT_SECRET",
    "SMTP_HOST",
    "SMTP_PORT",
    "SMTP_USERNAME",
    "SMTP_PASSWORD",
    "SMTP_FROM",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_leave_and_come_back_unchanged() {
        let s = Stored {
            servers: vec![StoredServer {
                id: "abc".into(),
                kind: Some(ServerType::Jellyfin),
                name: "HomeJelly".into(),
                public_url: "https://jellyfin.example.org".into(),
                internal_url: None,
                api_key: Some("jf-secret".into()),
            }],
            smtp: Some(StoredSmtp {
                host: "smtp.example.org".into(),
                password: Some("mail-secret".into()),
                from: "me@example.org".into(),
                ..Default::default()
            }),
            webhook_secret: Some("hook".into()),
            ..Default::default()
        };
        let shown = s.redacted();
        let text = serde_json::to_string(&shown).unwrap();
        assert!(
            !text.contains("jf-secret")
                && !text.contains("mail-secret")
                && !text.contains("\"hook\""),
            "{text}"
        );
        // Sent back as shown: nothing changes.
        assert_eq!(s.merged_from(shown.clone()), s);
        // A new key replaces the old one, an empty one removes it, and a section with no
        // address is off.
        let mut edit = shown;
        edit.servers[0].api_key = Some("new".into());
        edit.smtp.as_mut().unwrap().password = Some(String::new());
        edit.tautulli = Some(StoredService::default());
        let m = s.merged_from(edit);
        assert_eq!(m.servers[0].api_key.as_deref(), Some("new"));
        assert_eq!(m.smtp.unwrap().password, None);
        assert_eq!(m.tautulli, None);
        assert_eq!(m.webhook_secret.as_deref(), Some("hook"));
    }

    #[test]
    fn saved_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = Stored::path(dir.path());
        assert_eq!(Stored::read(&path).unwrap(), None);
        let mut s = Stored::default();
        assert!(s.ensure_webhook_secret());
        assert!(!s.ensure_webhook_secret());
        s.save(&path).unwrap();
        assert_eq!(Stored::read(&path).unwrap(), Some(s));
    }
}
