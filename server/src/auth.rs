// SPDX-License-Identifier: AGPL-3.0-or-later
//! Validates the caller's own Jellyfin/Plex token against an allow-listed server.
//!
//! Results are cached for 10 minutes under a keyed hash of (server, token) (`RandomState`, so
//! keys are not predictable); tokens themselves are never stored or logged.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use keep_service::ServerType;
use net::reqwest::Client;
use serde::Deserialize;

use crate::config::ServerConf;

const TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub user_key: String,
    pub user_id: String,
    pub name: String,
    pub server_id: String,
    pub kind: ServerType,
    pub is_admin: bool,
}

#[derive(Debug, PartialEq)]
pub enum AuthError {
    /// Bad or expired token, or no access to this server.
    Denied,
    /// The media server could not be reached: try again.
    Unavailable,
}

pub struct Auth {
    http: Client,
    plex_tv: String,
    hasher: RandomState,
    cache: Mutex<HashMap<u64, (Identity, Instant)>>,
}

impl Auth {
    pub fn new(http: Client, plex_tv: &str) -> Self {
        Self {
            http,
            plex_tv: plex_tv.trim_end_matches('/').to_owned(),
            hasher: RandomState::new(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn identify(&self, server: &ServerConf, token: &str) -> Result<Identity, AuthError> {
        let key = self.hasher.hash_one((&server.id, token));
        if let Some((id, at)) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            && at.elapsed() < TTL
        {
            return Ok(id.clone());
        }
        let id = match server.kind {
            ServerType::Jellyfin => self.jellyfin(server, token).await?,
            ServerType::Plex => self.plex(server, token).await?,
        };
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, (_, at)| at.elapsed() < TTL);
        cache.insert(key, (id.clone(), Instant::now()));
        Ok(id)
    }

    async fn jellyfin(&self, server: &ServerConf, token: &str) -> Result<Identity, AuthError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Me {
            id: String,
            name: String,
            #[serde(default)]
            policy: Option<Policy>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Policy {
            #[serde(default)]
            is_administrator: bool,
        }
        let req = self
            .http
            .get(format!("{}/Users/Me", server.internal()))
            .header(
                "Authorization",
                format!("MediaBrowser Client=\"AlwyWatch-Server\", Device=\"server\", DeviceId=\"alwywatch-server\", Version=\"{}\", Token=\"{token}\"", env!("CARGO_PKG_VERSION")),
            )
            .header("Accept", "application/json");
        let me: Me = net::send_json(req).await.map_err(map)?;
        Ok(Identity {
            user_key: format!("jellyfin:{}:{}", server.id, me.id),
            user_id: me.id,
            name: me.name,
            server_id: server.id.clone(),
            kind: ServerType::Jellyfin,
            is_admin: me.policy.is_some_and(|p| p.is_administrator),
        })
    }

    async fn plex(&self, server: &ServerConf, token: &str) -> Result<Identity, AuthError> {
        #[derive(Deserialize)]
        struct User {
            id: u64,
            #[serde(default)]
            username: String,
            #[serde(default)]
            title: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Resource {
            client_identifier: String,
            #[serde(default)]
            owned: bool,
        }
        let get = |path: &str| {
            self.http
                .get(format!("{}{path}", self.plex_tv))
                .header("X-Plex-Token", token)
                .header("X-Plex-Client-Identifier", "alwywatch-server")
                .header("X-Plex-Product", "AlwyWatch Server")
                .header("Accept", "application/json")
        };
        let user: User = net::send_json(get("/api/v2/user")).await.map_err(map)?;
        let resources: Vec<Resource> = net::send_json(get("/api/v2/resources?includeHttps=1"))
            .await
            .map_err(map)?;
        // Access to the allow-listed server is required; its owner is the admin.
        let res = resources
            .iter()
            .find(|r| r.client_identifier == server.id)
            .ok_or(AuthError::Denied)?;
        Ok(Identity {
            user_key: format!("plex:{}:{}", server.id, user.id),
            user_id: user.id.to_string(),
            name: if user.title.is_empty() {
                user.username
            } else {
                user.title
            },
            server_id: server.id.clone(),
            kind: ServerType::Plex,
            is_admin: res.owned,
        })
    }
}

fn map(e: net::Error) -> AuthError {
    if e.is_transient() {
        AuthError::Unavailable
    } else {
        AuthError::Denied
    }
}
