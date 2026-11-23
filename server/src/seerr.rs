// SPDX-License-Identifier: AGPL-3.0-or-later
//! Seerr bridge: Seerr for users whose media server is not the one Seerr runs on.
//!
//! Seerr serves one media server type at a time, so with Seerr in Plex mode a Jellyfin user
//! cannot sign in to it (and the other way round after a switch). The bridge authenticates the
//! caller like every keep call (their own Jellyfin/Plex token), links them to a Seerr user and
//! forwards an allow-listed subset of the Seerr API with Seerr's API key acting as that user
//! (`X-API-User`, Seerr's own mechanism). Seerr keeps enforcing that user's permissions, quotas
//! and auto-approval, so the operator manages them in Seerr as usual.
//!
//! Linking: an existing Seerr user with the same Plex id / Jellyfin user id is reused (so a Plex
//! user going through the bridge sees their usual requests); otherwise a local Seerr user is
//! created once (`create_users`), with a random password that is never stored.

use axum::body::Bytes;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use keep_service::{SeerrBridgeInfo, ServerType};
use net::reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use crate::auth::Identity;
use crate::config::SeerrConf;
use crate::db::Db;

#[derive(Debug, PartialEq)]
pub enum BridgeError {
    /// Not configured (no `[seerr]` or no `SEERR_API_KEY`).
    Disabled,
    /// No matching Seerr user and creating one is turned off.
    NoAccount,
    /// Read-only mode refuses changes.
    DryRun,
    /// Seerr unreachable or answered unexpectedly.
    Unavailable(String),
}

pub struct Bridge {
    http: Client,
    url: String,
    public_url: Option<String>,
    create_users: bool,
    api_key: String,
    /// Read-only: no Seerr user creation, no requests or cancellations.
    dry_run: bool,
    /// Serialises linking so two first calls do not create two users.
    link_lock: tokio::sync::Mutex<()>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeerrUserDto {
    id: i64,
    #[serde(default)]
    plex_id: Option<i64>,
    #[serde(default)]
    jellyfin_user_id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    permissions: u64,
}

fn same_jellyfin_id(a: &str, b: &str) -> bool {
    let n = |s: &str| s.replace('-', "").to_ascii_lowercase();
    n(a) == n(b)
}

/// Email of a user the bridge creates (Seerr requires a unique one; it is never mailed since
/// an explicit password is set).
fn synthetic_email(id: &Identity) -> String {
    synthetic_email_of(id.kind, &id.user_id)
}

fn synthetic_email_of(kind: ServerType, user_id: &str) -> String {
    let kind = match kind {
        ServerType::Jellyfin => "jellyfin",
        ServerType::Plex => "plex",
    };
    format!("{kind}-{}@alwywatch.local", user_id.to_ascii_lowercase())
}

/// The Seerr endpoints the apps use. Everything else (settings, users, admin) is refused here,
/// on top of Seerr's own permission checks for the acting user.
pub fn allowed(method: &Method, path: &str) -> bool {
    let num = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let segs: Vec<&str> = path.split('/').collect();
    match (method.as_str(), segs.as_slice()) {
        ("GET", ["auth", "me"] | ["search"] | ["request"] | ["settings", "public"]) => true,
        ("GET", ["movie" | "tv" | "collection", id]) => num(id),
        ("GET", ["tv", id, "season", n]) => num(id) && num(n),
        ("GET", ["request", id]) => num(id),
        ("GET", ["discover", "movies" | "tv" | "trending"]) => true,
        ("POST", ["request"]) => true,
        ("DELETE", ["request", id]) => num(id),
        _ => false,
    }
}

impl Bridge {
    pub fn new(
        http: Client,
        conf: Option<&SeerrConf>,
        api_key: Option<&str>,
        dry_run: bool,
    ) -> Option<Self> {
        let (conf, api_key) = (conf?, api_key?);
        Some(Self {
            http,
            url: conf.url.trim_end_matches('/').to_owned(),
            public_url: conf.public_url.clone(),
            create_users: conf.create_users,
            api_key: api_key.to_owned(),
            dry_run,
            link_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn req(
        &self,
        method: Method,
        path_and_query: &str,
        act_as: Option<i64>,
    ) -> net::reqwest::RequestBuilder {
        let mut r = self
            .http
            .request(method, format!("{}/api/v1/{path_and_query}", self.url))
            .header("X-Api-Key", &self.api_key)
            .header(header::ACCEPT, "application/json");
        if let Some(u) = act_as {
            r = r.header("X-API-User", u.to_string());
        }
        r
    }

    fn unavailable(e: impl ToString) -> BridgeError {
        BridgeError::Unavailable(e.to_string())
    }

    /// The caller's Seerr user id, linking or creating it on first use.
    /// Seerr's users (as the API key's admin), for linking by hand.
    pub async fn users(&self) -> Result<Vec<keep_service::SeerrUser>, BridgeError> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            results: Vec<SeerrUserDto>,
        }
        let page: Page = net::send_json(self.req(Method::GET, "user?take=5000&skip=0", None))
            .await
            .map_err(Self::unavailable)?;
        Ok(page
            .results
            .into_iter()
            .map(|u| keep_service::SeerrUser {
                id: u.id,
                name: u
                    .display_name
                    .filter(|n| !n.is_empty())
                    .or(u.email)
                    .unwrap_or_else(|| format!("user {}", u.id)),
            })
            .collect())
    }

    /// Whether this Seerr user may manage Seerr (admin, settings, users or others' requests):
    /// requests made as them would carry those rights.
    pub async fn is_manager(&self, user: i64) -> Result<bool, BridgeError> {
        // Seerr's ADMIN, MANAGE_SETTINGS, MANAGE_USERS, MANAGE_REQUESTS.
        const MANAGE: u64 = 2 | 4 | 8 | 16;
        let u: SeerrUserDto = net::send_json(self.req(Method::GET, &format!("user/{user}"), None))
            .await
            .map_err(Self::unavailable)?;
        Ok(u.permissions & MANAGE != 0)
    }

    pub async fn link(&self, db: &Db, id: &Identity) -> Result<i64, BridgeError> {
        if let Some(u) = db.seerr_link(&id.user_key).map_err(Self::unavailable)? {
            return Ok(u);
        }
        let _guard = self.link_lock.lock().await;
        if let Some(u) = db.seerr_link(&id.user_key).map_err(Self::unavailable)? {
            return Ok(u);
        }
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            results: Vec<SeerrUserDto>,
        }
        // As the API key's own user (the Seerr admin), so plexId/email are visible.
        let page: Page = net::send_json(self.req(Method::GET, "user?take=5000&skip=0", None))
            .await
            .map_err(Self::unavailable)?;
        // Any sign-in of the person may already have a Seerr user (this one first).
        let mut ids: Vec<(ServerType, &str)> = vec![(id.kind, id.user_id.as_str())];
        for i in &id.identities {
            if i.user_key != id.signed_in_key {
                ids.push((i.kind, i.user_id.as_str()));
            }
        }
        let email = synthetic_email(id);
        let by_media_id = ids.iter().find_map(|(kind, user_id)| {
            page.results.iter().find(|u| match kind {
                ServerType::Plex => u.plex_id.is_some_and(|p| p.to_string() == *user_id),
                ServerType::Jellyfin => u
                    .jellyfin_user_id
                    .as_deref()
                    .is_some_and(|j| same_jellyfin_id(j, user_id)),
            })
        });
        let found = by_media_id.or_else(|| {
            ids.iter().find_map(|(kind, user_id)| {
                let email = synthetic_email_of(*kind, user_id);
                page.results.iter().find(|u| {
                    u.email
                        .as_deref()
                        .is_some_and(|e| e.eq_ignore_ascii_case(&email))
                })
            })
        });
        let (seerr_id, created) = match found {
            Some(u) => (u.id, false),
            None if !self.create_users || self.dry_run => {
                if self.dry_run {
                    eprintln!("[dry-run] would create a Seerr user for {}", id.user_key);
                }
                return Err(BridgeError::NoAccount);
            }
            None => {
                let password = random_password();
                let body =
                    json!({ "email": email, "username": id.media_name, "password": password });
                let u: SeerrUserDto =
                    net::send_json(self.req(Method::POST, "user", None).json(&body))
                        .await
                        .map_err(Self::unavailable)?;
                (u.id, true)
            }
        };
        db.set_seerr_link(&id.user_key, seerr_id, created)
            .map_err(Self::unavailable)?;
        db.log(
            &id.user_key,
            "seerr-link",
            &format!(
                "seerr user {seerr_id}{}",
                if created { " (created)" } else { "" }
            ),
        );
        Ok(seerr_id)
    }

    /// `GET /v1/seerr`: links the caller and describes their Seerr account.
    pub async fn info(&self, db: &Db, id: &Identity) -> Result<SeerrBridgeInfo, BridgeError> {
        for attempt in 0..2 {
            let seerr_id = self.link(db, id).await?;
            match net::send_json::<SeerrUserDto>(self.req(Method::GET, "auth/me", Some(seerr_id)))
                .await
            {
                Ok(u) => {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct Public {
                        #[serde(default)]
                        cache_images: bool,
                    }
                    let public: Option<Public> =
                        net::send_json(self.req(Method::GET, "settings/public", None))
                            .await
                            .ok();
                    return Ok(SeerrBridgeInfo {
                        seerr_user_id: u.id,
                        user_name: u.display_name.unwrap_or_else(|| id.media_name.clone()),
                        public_url: self.public_url.clone(),
                        cache_images: public.is_some_and(|p| p.cache_images),
                    });
                }
                // The linked Seerr user was deleted: forget it and link again once.
                Err(net::Error::Unauthorized | net::Error::NotFound) if attempt == 0 => {
                    db.forget_seerr_link(&id.user_key)
                        .map_err(Self::unavailable)?;
                }
                Err(e) => return Err(Self::unavailable(e)),
            }
        }
        Err(BridgeError::Unavailable(
            "could not link a Seerr user".into(),
        ))
    }

    /// Forwards one allow-listed call as the caller's Seerr user. The body and status are
    /// passed through unchanged so the client parses Seerr's own JSON.
    pub async fn forward(
        &self,
        db: &Db,
        id: &Identity,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: Bytes,
    ) -> Result<Response, BridgeError> {
        if self.dry_run && method != Method::GET {
            eprintln!(
                "[dry-run] would forward {method} {path} for {}",
                id.user_key
            );
            return Err(BridgeError::DryRun);
        }
        let seerr_id = self.link(db, id).await?;
        let pq = match query {
            Some(q) if !q.is_empty() => format!("{path}?{q}"),
            _ => path.to_owned(),
        };
        let mut req = self.req(method, &pq, Some(seerr_id));
        if !body.is_empty() {
            req = req
                .header(header::CONTENT_TYPE, "application/json")
                .body(body.to_vec());
        }
        let res = req.send().await.map_err(Self::unavailable)?;
        let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let ctype = res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| HeaderValue::from_bytes(v.as_bytes()).ok());
        let bytes = res.bytes().await.map_err(Self::unavailable)?;
        let mut out = (status, bytes).into_response();
        if let Some(c) = ctype {
            out.headers_mut().insert(header::CONTENT_TYPE, c);
        }
        Ok(out)
    }
}

/// Never stored nor shown: the bridge acts through the API key, the password only has to be
/// unguessable.
fn random_password() -> String {
    crate::transfer::random_hex::<32>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list() {
        assert!(allowed(&Method::GET, "search"));
        assert!(allowed(&Method::GET, "tv/1399"));
        assert!(allowed(&Method::GET, "tv/1399/season/2"));
        assert!(allowed(&Method::POST, "request"));
        assert!(allowed(&Method::DELETE, "request/12"));
        assert!(!allowed(&Method::GET, "settings/main"));
        assert!(!allowed(&Method::GET, "user"));
        assert!(!allowed(&Method::POST, "user"));
        assert!(!allowed(&Method::DELETE, "request/../user/1"));
        assert!(!allowed(&Method::POST, "request/12/approve"));
        assert!(!allowed(&Method::GET, "tv/abc"));
    }

    #[test]
    fn jellyfin_ids_compare_without_dashes() {
        assert!(same_jellyfin_id(
            "5f0f3d0e-6f3c-4a3e-9b1a-0123456789ab",
            "5F0F3D0E6F3C4A3E9B1A0123456789AB"
        ));
    }

    #[test]
    fn passwords_differ() {
        let a = random_password();
        assert_eq!(a.len(), 64);
        assert_ne!(a, random_password());
    }
}
