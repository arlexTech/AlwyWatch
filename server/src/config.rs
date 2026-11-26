// SPDX-License-Identifier: AGPL-3.0-or-later
//! Server configuration: a TOML file (allow-list, URLs) plus environment variables for secrets.
//!
//! ```toml
//! listen = "0.0.0.0:8080"
//! data_dir = "/data"
//!
//! [[servers]]
//! id = "<Jellyfin server id or Plex machine identifier>"
//! kind = "jellyfin"            # or "plex"
//! name = "HomeJelly"
//! public_url = "https://jellyfin.example.org"
//! internal_url = "http://jellyfin:8096"
//!
//! [maintainerr]
//! url = "http://maintainerr:6246"
//! server = "<id of the server Maintainerr manages>"
//!
//! [tautulli]                   # optional: Plex "watched" detection
//! url = "http://tautulli:8181"
//!
//! [seerr]                      # optional: Seerr for users of the other media server type
//! url = "http://seerr:5055"
//! public_url = "https://seerr.example.org"
//! create_users = true          # create a local Seerr user when none matches
//! ```
//! Secrets (env): `MAINTAINERR_API_KEY` (optional), `TAUTULLI_API_KEY`,
//! `JELLYFIN_API_KEY_<SERVER ID>` (or `JELLYFIN_API_KEY` when there is one Jellyfin),
//! `WEBHOOK_SECRET` (path secret for /v1/hooks/*), `SEERR_API_KEY` (Seerr bridge).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use keep_service::ServerType;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConf {
    pub id: String,
    pub kind: ServerType,
    pub name: String,
    pub public_url: String,
    /// How the backend reaches it (Docker network); defaults to `public_url`.
    pub internal_url: Option<String>,
}

impl ServerConf {
    pub fn internal(&self) -> &str {
        self.internal_url
            .as_deref()
            .unwrap_or(&self.public_url)
            .trim_end_matches('/')
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MaintainerrConf {
    pub url: String,
    /// Allow-listed server id whose item ids Maintainerr uses.
    pub server: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TautulliConf {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeerrConf {
    pub url: String,
    /// Shown to users (web UI link); never used by the backend.
    pub public_url: Option<String>,
    #[serde(default = "yes")]
    pub create_users: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    #[serde(default = "default_data")]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub servers: Vec<ServerConf>,
    /// Absent: no Leaving soon and no keep choices; accounts, the web app, watch together,
    /// device sign-in and the Seerr bridge work on their own (see [`Config::keep_on`]).
    #[serde(default)]
    pub maintainerr: MaintainerrConf,
    pub tautulli: Option<TautulliConf>,
    pub seerr: Option<SeerrConf>,
    /// Reads everything, writes nothing: Maintainerr writes and Seerr changes are logged
    /// instead, and votes go to a separate database (`keep-dryrun.sqlite`). Also
    /// `ALWYWATCH_DRY_RUN=1`.
    #[serde(default)]
    pub dry_run: bool,
    /// Minutes between Maintainerr syncs.
    #[serde(default = "default_sync")]
    pub sync_minutes: u64,
    #[serde(skip)]
    pub secrets: Secrets,
    /// AlwyWatch accounts (from the environment, see [`AccountsConf::from_env`]).
    #[serde(skip)]
    pub accounts: AccountsConf,
}

/// Who may create an AlwyWatch account without a media-server sign-in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Registration {
    /// With an invitation from an admin (default).
    #[default]
    Invite,
    Open,
    Closed,
}

#[derive(Debug, Clone, Default)]
pub struct AccountsConf {
    /// This server's public address (`ALWYWATCH_PUBLIC_URL`): needed for passkeys and Pocket
    /// ID (both are tied to it).
    pub rp: Option<crate::passkey::RelyingParty>,
    pub pocketid: Option<crate::oidc::PocketId>,
    pub registration: Registration,
    /// Invitation emails (Settings → Admin → Server settings → Email).
    pub mail: Option<crate::mail::Mailer>,
}

impl AccountsConf {
    #[cfg(test)]
    /// A value that is set but wrong is an error, not a silent fallback: passkeys quietly off
    /// or sign-up quietly on "invite" is found out much later, by someone else.
    pub fn from_env() -> Result<Self, String> {
        let get = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let rp = match get("ALWYWATCH_PUBLIC_URL") {
            None => None,
            Some(url) => Some(crate::passkey::RelyingParty::from_url(&url).ok_or(format!(
                "ALWYWATCH_PUBLIC_URL must be this site's address like https://watch.example.org \
                 (https://, the host, no path), got '{url}'"
            ))?),
        };
        let pocketid = rp
            .as_ref()
            .and_then(|r| crate::oidc::PocketId::from_env(&r.origin));
        const POCKETID: [&str; 3] = [
            "ALWYWATCH_POCKETID_ISSUER",
            "ALWYWATCH_POCKETID_CLIENT_ID",
            "ALWYWATCH_POCKETID_CLIENT_SECRET",
        ];
        let set = POCKETID.iter().filter(|k| get(k).is_some()).count();
        if set > 0 && pocketid.is_none() {
            return Err(if rp.is_none() {
                "ALWYWATCH_POCKETID_* is set but ALWYWATCH_PUBLIC_URL is not: Pocket ID needs \
                 this site's address"
                    .to_owned()
            } else {
                format!("Pocket ID needs all three of {}", POCKETID.join(", "))
            });
        }
        let registration = match get("ALWYWATCH_REGISTRATION").as_deref() {
            None | Some("invite") => Registration::Invite,
            Some("open") => Registration::Open,
            Some("closed") => Registration::Closed,
            Some(v) => {
                return Err(format!(
                    "ALWYWATCH_REGISTRATION must be invite, open or closed, got '{v}'"
                ));
            }
        };
        Ok(Self {
            rp,
            pocketid,
            registration,
            mail: None,
        })
    }

    pub fn from_stored(st: &crate::settings::Stored) -> Result<Self, String> {
        let rp = match st
            .public_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            None => None,
            Some(url) => Some(crate::passkey::RelyingParty::from_url(url).ok_or(format!(
                "This site's address must look like https://watch.example.org (https://, the \
                 host, no path), got '{url}'"
            ))?),
        };
        let pocketid = match (&st.pocketid, &rp) {
            (None, _) => None,
            (Some(_), None) => {
                return Err("Pocket ID needs this site's address (https://…) first".into());
            }
            (Some(p), Some(rp)) => {
                let secret = p.client_secret.clone().unwrap_or_default();
                if p.client_id.trim().is_empty() || secret.trim().is_empty() {
                    return Err("Pocket ID needs its client id and client secret".into());
                }
                Some(crate::oidc::PocketId::new(
                    &p.issuer,
                    p.client_id.trim(),
                    secret.trim(),
                    &rp.origin,
                ))
            }
        };
        let registration = match st.registration.trim() {
            "" | "invite" => Registration::Invite,
            "open" => Registration::Open,
            "closed" => Registration::Closed,
            v => return Err(format!("sign-up must be invite, open or closed, got '{v}'")),
        };
        let mail = match &st.smtp {
            None => None,
            Some(m) => Some(crate::mail::Mailer::from_stored(m)?),
        };
        Ok(Self {
            rp,
            pocketid,
            registration,
            mail,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Secrets {
    pub maintainerr_api_key: Option<String>,
    pub tautulli_api_key: Option<String>,
    pub webhook_secret: Option<String>,
    pub seerr_api_key: Option<String>,
    jellyfin_keys: Vec<(String, String)>,
    jellyfin_default: Option<String>,
    /// Plex servers: the owner's plex.tv token, saved when the owner adds the server (to sign
    /// users' new devices in, see `server_sign_in`).
    plex_tokens: Vec<(String, String)>,
}

impl Secrets {
    pub fn jellyfin_key(&self, server_id: &str) -> Option<&str> {
        self.jellyfin_keys
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(server_id))
            .map(|(_, k)| k.as_str())
            .or(self.jellyfin_default.as_deref())
    }

    pub fn plex_owner_token(&self, server_id: &str) -> Option<&str> {
        self.plex_tokens
            .iter()
            .find(|(id, _)| id == server_id)
            .map(|(_, k)| k.as_str())
    }
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 8080))
}

fn default_data() -> PathBuf {
    PathBuf::from("/data")
}

fn default_sync() -> u64 {
    10
}

impl Config {
    #[cfg(test)]
    pub fn parse(toml_text: &str) -> Result<Self, String> {
        let mut c: Config = toml::from_str(toml_text).map_err(|e| e.to_string())?;
        if c.keep_on() && !c.servers.iter().any(|s| s.id == c.maintainerr.server) {
            return Err(format!(
                "[maintainerr] server = '{}' must be the id of one [[servers]] entry (the media \
                 server Maintainerr is set up for); ids in this file: {}",
                c.maintainerr.server,
                c.servers
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        c.secrets = Secrets::from_env();
        c.accounts = AccountsConf::from_env()?;
        Ok(c)
    }

    /// The server from its saved settings; `listen` and `data_dir` come from the environment.
    pub fn from_stored(
        st: &crate::settings::Stored,
        listen: SocketAddr,
        data_dir: PathBuf,
    ) -> Result<Self, String> {
        let mut servers = Vec::new();
        for x in &st.servers {
            let kind = x.kind.ok_or(format!(
                "the server '{}' has no type (Jellyfin or Plex)",
                x.name
            ))?;
            if x.id.trim().is_empty() || x.public_url.trim().is_empty() {
                return Err(format!("the server '{}' needs its id and address", x.name));
            }
            servers.push(ServerConf {
                id: x.id.trim().to_owned(),
                kind,
                name: x.name.trim().to_owned(),
                public_url: x.public_url.trim().trim_end_matches('/').to_owned(),
                internal_url: x
                    .internal_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .map(|u| u.trim_end_matches('/').to_owned()),
            });
        }
        let maintainerr = st
            .maintainerr
            .as_ref()
            .map(|m| MaintainerrConf {
                url: m.url.trim().trim_end_matches('/').to_owned(),
                server: m.server.trim().to_owned(),
            })
            .unwrap_or_default();
        let c = Config {
            listen,
            data_dir,
            servers,
            maintainerr,
            tautulli: st.tautulli.as_ref().map(|t| TautulliConf {
                url: t.url.trim().trim_end_matches('/').to_owned(),
            }),
            seerr: st.seerr.as_ref().map(|x| SeerrConf {
                url: x.url.trim().trim_end_matches('/').to_owned(),
                public_url: x.public_url.clone().filter(|u| !u.trim().is_empty()),
                create_users: x.create_users,
            }),
            dry_run: st.dry_run
                || std::env::var("ALWYWATCH_DRY_RUN").is_ok_and(|v| v == "1" || v == "true"),
            sync_minutes: st.sync_minutes.unwrap_or_else(default_sync).max(1),
            secrets: Secrets::from_stored(st),
            accounts: AccountsConf::from_stored(st)?,
        };
        if c.keep_on() && !c.servers.iter().any(|s| s.id == c.maintainerr.server) {
            return Err(
                "Maintainerr: choose the media server it is set up for (one of your servers)"
                    .into(),
            );
        }
        Ok(c)
    }

    /// The port and the data folder (`ALWYWATCH_LISTEN`, `ALWYWATCH_DATA`).
    pub fn env_basics() -> Result<(SocketAddr, PathBuf), String> {
        let listen = match std::env::var("ALWYWATCH_LISTEN") {
            Ok(l) => l.parse().map_err(|_| {
                format!("ALWYWATCH_LISTEN must be an address like 0.0.0.0:8080, got '{l}'")
            })?,
            Err(_) => default_listen(),
        };
        let data = std::env::var("ALWYWATCH_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_data());
        Ok((listen, data))
    }

    /// The saved settings, imported once from config.toml and the environment for a server set
    /// up before settings moved into the app; empty for a new server (setup in the web app).
    pub fn load_stored(data_dir: &Path) -> Result<crate::settings::Stored, String> {
        use crate::settings::Stored;
        let path = Stored::path(data_dir);
        if let Some(st) = Stored::read(&path)? {
            let old: Vec<&str> = crate::settings::OLD_VARIABLES
                .into_iter()
                .filter(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()))
                .collect();
            if !old.is_empty() {
                eprintln!(
                    "[settings] ignored: {} (settings are changed in the web app now: Settings → \
                     Admin → Server settings; these variables can be removed)",
                    old.join(", ")
                );
            }
            return Ok(st);
        }
        let toml_path = std::env::var("ALWYWATCH_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_dir.join("config.toml"));
        let mut st = match std::fs::read_to_string(&toml_path) {
            Ok(text) => {
                let st =
                    Stored::import(&text).map_err(|e| format!("{}: {e}", toml_path.display()))?;
                eprintln!(
                    "[settings] imported {} and the environment into {}: from now on, settings are \
                     changed in the web app (Settings → Admin → Server settings)",
                    toml_path.display(),
                    path.display()
                );
                st
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            // Unreadable is not "new": a set-up server must never fall back to setup mode.
            Err(e) => return Err(format!("{}: {e}", toml_path.display())),
        };
        st.ensure_webhook_secret();
        if !st.servers.is_empty() {
            st.save(&path)?;
        }
        Ok(st)
    }

    /// Whether Maintainerr is configured (Leaving soon and keep choices).
    pub fn keep_on(&self) -> bool {
        !self.maintainerr.url.trim().is_empty()
    }

    /// One line at startup: what is on and what is off, so a forgotten variable shows.
    pub fn summary(&self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        let a = &self.accounts;
        format!(
            "{} media server(s); Leaving soon (Maintainerr)={}; accounts: sign-up={}, passkeys={}, Pocket ID={}; \
             Seerr bridge={}; Tautulli={}; Jellyfin key={}; webhooks={}",
            self.servers.len(),
            on(self.keep_on()),
            match a.registration {
                Registration::Invite => "invite",
                Registration::Open => "open",
                Registration::Closed => "closed",
            },
            on(a.rp.is_some()),
            on(a.pocketid.is_some()),
            on(self.seerr.is_some() && self.secrets.seerr_api_key.is_some()),
            on(self.tautulli.is_some() && self.secrets.tautulli_api_key.is_some()),
            on(self
                .servers
                .iter()
                .any(|s| self.secrets.jellyfin_key(&s.id).is_some())),
            on(self.secrets.webhook_secret.is_some()),
        )
    }

    pub fn server(&self, id: &str) -> Option<&ServerConf> {
        self.servers.iter().find(|s| s.id == id)
    }
}

impl Secrets {
    #[cfg(test)]
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let jellyfin_keys = std::env::vars()
            .filter_map(|(k, v)| {
                k.strip_prefix("JELLYFIN_API_KEY_")
                    .filter(|_| !v.trim().is_empty())
                    .map(|id| (id.to_owned(), v))
            })
            .collect();
        Self {
            maintainerr_api_key: get("MAINTAINERR_API_KEY"),
            tautulli_api_key: get("TAUTULLI_API_KEY"),
            webhook_secret: get("WEBHOOK_SECRET"),
            seerr_api_key: get("SEERR_API_KEY"),
            jellyfin_keys,
            jellyfin_default: get("JELLYFIN_API_KEY"),
            plex_tokens: vec![],
        }
    }

    pub fn from_stored(st: &crate::settings::Stored) -> Self {
        let clean = |v: &Option<String>| v.clone().filter(|v| !v.trim().is_empty());
        Self {
            maintainerr_api_key: st.maintainerr.as_ref().and_then(|m| clean(&m.api_key)),
            tautulli_api_key: st.tautulli.as_ref().and_then(|t| clean(&t.api_key)),
            webhook_secret: clean(&st.webhook_secret),
            seerr_api_key: st.seerr.as_ref().and_then(|x| clean(&x.api_key)),
            jellyfin_keys: st
                .servers
                .iter()
                .filter(|x| x.kind != Some(keep_service::ServerType::Plex))
                .filter_map(|x| clean(&x.api_key).map(|k| (x.id.clone(), k)))
                .collect(),
            jellyfin_default: None,
            plex_tokens: st
                .servers
                .iter()
                .filter(|x| x.kind == Some(keep_service::ServerType::Plex))
                .filter_map(|x| clean(&x.api_key).map(|k| (x.id.clone(), k)))
                .collect(),
        }
    }

    #[cfg(test)]
    pub fn for_tests(jellyfin: &str, tautulli: &str) -> Self {
        Self {
            maintainerr_api_key: None,
            tautulli_api_key: Some(tautulli.into()),
            webhook_secret: Some("hook".into()),
            seerr_api_key: Some("seerr-key".into()),
            jellyfin_keys: vec![],
            jellyfin_default: Some(jellyfin.into()),
            plex_tokens: vec![],
        }
    }

    #[cfg(test)]
    pub fn with_plex_token(mut self, server_id: &str, token: &str) -> Self {
        self.plex_tokens.push((server_id.into(), token.into()));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = r#"
[[servers]]
id = "abc"
kind = "jellyfin"
name = "HomeJelly"
public_url = "https://jellyfin.example.org"
"#;

    #[test]
    fn maintainerr_is_optional() {
        // On its own: the web app, accounts, rooms and device sign-in, no Leaving soon.
        let c = Config::parse(SERVER).unwrap();
        assert!(!c.keep_on());
        assert!(c.summary().contains("Leaving soon (Maintainerr)=off"));
        let with = format!("{SERVER}\n[maintainerr]\nurl = \"http://m:6246\"\nserver = \"abc\"\n");
        assert!(Config::parse(&with).unwrap().keep_on());
    }

    #[test]
    fn a_maintainerr_for_an_unknown_server_says_which_ids_exist() {
        let bad = format!("{SERVER}\n[maintainerr]\nurl = \"http://m:6246\"\nserver = \"nope\"\n");
        let e = Config::parse(&bad).unwrap_err();
        assert!(e.contains("'nope'") && e.contains("abc"), "{e}");
    }
}
