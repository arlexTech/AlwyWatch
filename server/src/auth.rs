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

/// How long a checked media token is trusted without asking its server again: a token revoked
/// on Jellyfin/Plex (signed out, user disabled) stops working here within this time.
const TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    /// The person's canonical user key (set per request by `api::Caller`; before that, the
    /// signed-in user's own key). All data is stored under it.
    pub user_key: String,
    /// The media-server user that signed in (`jellyfin:<server>:<id>` / `plex:<server>:<id>`).
    pub signed_in_key: String,
    /// User id on the signed-in media server.
    pub user_id: String,
    /// The name shown to others: the user's chosen display name, else `media_name` (filled in
    /// per request by `api::Caller`).
    pub name: String,
    /// The name from the media server.
    pub media_name: String,
    /// Filled in per request by `api::Caller`.
    pub avatar: keep_service::Avatar,
    pub server_id: String,
    pub kind: ServerType,
    /// AlwyWatch admin (per request: this sign-in's media-server rule, or a linked one's).
    pub is_admin: bool,
    /// Every sign-in of the person, this one included (filled in per request by `api::Caller`).
    pub identities: Vec<crate::db::IdentityRow>,
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
        // The token goes inside a quoted field of Jellyfin's header: a quote in it would add
        // fields of the caller's choosing (Jellyfin tokens are plain hex).
        if token.contains(['"', '\\']) || token.chars().any(char::is_control) {
            return Err(AuthError::Denied);
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
            signed_in_key: format!("jellyfin:{}:{}", server.id, me.id),
            identities: vec![],
            user_id: me.id,
            media_name: me.name.clone(),
            name: me.name,
            avatar: Default::default(),
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
        let name = if user.title.is_empty() {
            user.username
        } else {
            user.title
        };
        Ok(Identity {
            user_key: format!("plex:{}:{}", server.id, user.id),
            signed_in_key: format!("plex:{}:{}", server.id, user.id),
            identities: vec![],
            user_id: user.id.to_string(),
            media_name: name.clone(),
            name,
            avatar: Default::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_jellyfin_token_cannot_add_header_fields() {
        let server = ServerConf {
            id: "s1".into(),
            kind: ServerType::Jellyfin,
            name: "HomeJelly".into(),
            // Nothing listens there: a token that got through would fail as unreachable.
            public_url: "http://127.0.0.1:9".into(),
            internal_url: None,
        };
        let auth = Auth::new(net::api_client("test").unwrap(), "http://127.0.0.1:9");
        for t in ["abc\", UserId=\"x", "abc\\", "abc\r\nX: y"] {
            assert_eq!(auth.identify(&server, t).await, Err(AuthError::Denied));
        }
    }
}
