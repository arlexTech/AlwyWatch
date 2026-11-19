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
use std::path::PathBuf;

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

#[derive(Debug, Clone, Deserialize)]
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
}

#[derive(Debug, Clone, Default)]
pub struct Secrets {
    pub maintainerr_api_key: Option<String>,
    pub tautulli_api_key: Option<String>,
    pub webhook_secret: Option<String>,
    pub seerr_api_key: Option<String>,
    jellyfin_keys: Vec<(String, String)>,
    jellyfin_default: Option<String>,
}

impl Secrets {
    pub fn jellyfin_key(&self, server_id: &str) -> Option<&str> {
        self.jellyfin_keys
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(server_id))
            .map(|(_, k)| k.as_str())
            .or(self.jellyfin_default.as_deref())
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
    pub fn parse(toml_text: &str) -> Result<Self, String> {
        let mut c: Config = toml::from_str(toml_text).map_err(|e| e.to_string())?;
        if !c.servers.iter().any(|s| s.id == c.maintainerr.server) {
            return Err(format!(
                "maintainerr.server '{}' is not in [[servers]]",
                c.maintainerr.server
            ));
        }
        c.secrets = Secrets::from_env();
        Ok(c)
    }

    pub fn load() -> Result<Self, String> {
        let path = std::env::var("ALWYWATCH_CONFIG").unwrap_or_else(|_| "/data/config.toml".into());
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let mut c = Self::parse(&text)?;
        if let Ok(l) = std::env::var("ALWYWATCH_LISTEN")
            && let Ok(a) = l.parse()
        {
            c.listen = a;
        }
        if let Ok(d) = std::env::var("ALWYWATCH_DATA") {
            c.data_dir = d.into();
        }
        if std::env::var("ALWYWATCH_DRY_RUN").is_ok_and(|v| v == "1" || v == "true") {
            c.dry_run = true;
        }
        Ok(c)
    }

    pub fn server(&self, id: &str) -> Option<&ServerConf> {
        self.servers.iter().find(|s| s.id == id)
    }
}

impl Secrets {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let jellyfin_keys = std::env::vars()
            .filter_map(|(k, v)| {
                k.strip_prefix("JELLYFIN_API_KEY_")
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
        }
    }
}
