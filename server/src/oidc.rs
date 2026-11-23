// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signing in with Pocket ID (OpenID Connect), as alwymail does: authorization code flow with
//! PKCE (S256), a CSRF `state` and a `nonce`; the identity (`sub`, name) comes from the
//! issuer's userinfo endpoint over TLS with the access token just received from its token
//! endpoint (no JWT signature to check, so no JWT crate).
//!
//! A flow belongs to the browser that started it: `start` hands that browser a cookie and the
//! callback only goes on with it, so a callback address made by someone else signs nobody in.
//! Linking is never finished by the callback alone either: the account's own session confirms
//! it (`POST /v1/account/oidc/link`), so nobody can get another person's Pocket ID linked to
//! their account by sending them a link.
//!
//! `GET /v1/account/oidc/start?next=/q/123456` → Pocket ID → `GET /v1/account/oidc/callback`
//! → the web app with a session (`/account/signed-in#token=…&next=…`). With
//! `ticket=` (from `POST /v1/account/oidc/ticket`, signed in) the identity is linked to that
//! account instead. Pocket ID users are the owner's own: a first sign-in creates the account,
//! no invitation needed.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use net::reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::accounts::{random_hex, sha256_hex};
use crate::passkey::b64url;

const FLOW_TTL: Duration = Duration::from_secs(600);
const TICKET_TTL: Duration = Duration::from_secs(120);
const MAX_PENDING: usize = 500;

#[derive(Debug, Clone)]
pub struct PocketId {
    pub issuer: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}

impl PocketId {
    /// From `ALWYWATCH_POCKETID_ISSUER`, `_CLIENT_ID`, `_CLIENT_SECRET`; None when unset.
    pub fn from_env(public_url: &str) -> Option<Self> {
        let get = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        Some(Self {
            issuer: get("ALWYWATCH_POCKETID_ISSUER")?
                .trim_end_matches('/')
                .to_owned(),
            client_id: get("ALWYWATCH_POCKETID_CLIENT_ID")?,
            client_secret: get("ALWYWATCH_POCKETID_CLIENT_SECRET")?,
            redirect_uri: format!(
                "{}/v1/account/oidc/callback",
                public_url.trim_end_matches('/')
            ),
        })
    }

    #[cfg(test)]
    pub fn for_tests(issuer: &str, public_url: &str) -> Self {
        Self {
            issuer: issuer.trim_end_matches('/').to_owned(),
            client_id: "alwywatch".into(),
            client_secret: "secret".into(),
            redirect_uri: format!("{public_url}/v1/account/oidc/callback"),
        }
    }
}

#[derive(Debug, Clone)]
struct Endpoints {
    authorization: String,
    token: String,
    userinfo: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Sign in (or create the account); then the web app goes to `next`.
    Login { next: String },
    /// Add this identity to a signed-in account.
    Link { account: String },
}

struct Pending {
    intent: Intent,
    verifier: String,
    /// Hash of the cookie given to the browser that started the flow.
    browser: String,
    created: Instant,
}

/// What Pocket ID said about the person.
#[derive(Debug, Clone, PartialEq)]
pub struct OidcUser {
    pub sub: String,
    pub name: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum OidcError {
    Expired,
    /// The callback came from another browser than the one that started.
    OtherBrowser,
    Provider(String),
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidcError::Expired => write!(f, "this sign-in took too long, try again"),
            OidcError::OtherBrowser => {
                write!(
                    f,
                    "this sign-in was started in another browser, start again here"
                )
            }
            OidcError::Provider(m) => write!(f, "Pocket ID: {m}"),
        }
    }
}

/// Pending sign-ins and link tickets (in memory).
#[derive(Default)]
pub struct Oidc {
    endpoints: tokio::sync::Mutex<Option<Endpoints>>,
    pending: Mutex<HashMap<String, Pending>>,
    /// Ticket hash → (account, created): stands for a session in a browser tab.
    tickets: Mutex<HashMap<String, (String, Instant)>>,
    /// Link id hash → (account, Pocket ID user, created): back from Pocket ID, waiting for the
    /// account's session to confirm.
    links: Mutex<HashMap<String, (String, String, Instant)>>,
}

/// The cookie that ties a flow to the browser that started it.
pub const COOKIE: &str = "aw_oidc";

/// `Set-Cookie` for [`COOKIE`]: only sent back to the callback, not readable by scripts.
pub fn cookie(value: &str) -> String {
    format!(
        "{COOKIE}={value}; Path=/v1/account/oidc; Max-Age={}; HttpOnly; Secure; SameSite=Lax",
        FLOW_TTL.as_secs()
    )
}

/// The value of [`COOKIE`] in a `Cookie` header.
pub fn cookie_of(header: &str) -> Option<&str> {
    header.split(';').find_map(|c| {
        let (k, v) = c.trim().split_once('=')?;
        (k == COOKIE).then_some(v)
    })
}

/// Where the web app may go after signing in: a path on this site.
pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n)
            if n.starts_with('/')
                && !n.starts_with("//")
                && !n.contains('\\')
                && n.len() < 200
                && n.chars().all(|c| c.is_ascii_graphic()) =>
        {
            n.to_owned()
        }
        _ => "/".to_owned(),
    }
}

fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Oidc {
    async fn endpoints(&self, http: &Client, cfg: &PocketId) -> Result<Endpoints, OidcError> {
        let mut cached = self.endpoints.lock().await;
        if let Some(e) = cached.as_ref() {
            return Ok(e.clone());
        }
        let doc: Value = http
            .get(format!("{}/.well-known/openid-configuration", cfg.issuer))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| OidcError::Provider(e.to_string()))?
            .json()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        let get = |k: &str| {
            doc.get(k)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| OidcError::Provider(format!("no {k} in its configuration")))
        };
        let e = Endpoints {
            authorization: get("authorization_endpoint")?,
            token: get("token_endpoint")?,
            userinfo: get("userinfo_endpoint")?,
        };
        *cached = Some(e.clone());
        Ok(e)
    }

    /// A one-time ticket for linking from a browser tab (the tab has no session header).
    pub fn ticket(&self, account: &str) -> String {
        let t = random_hex(24);
        let mut m = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, (_, at)| at.elapsed() < TICKET_TTL);
        m.insert(
            sha256_hex(t.as_bytes()),
            (account.to_owned(), Instant::now()),
        );
        t
    }

    pub fn take_ticket(&self, t: &str) -> Option<String> {
        let mut m = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        let (account, at) = m.remove(&sha256_hex(t.trim().as_bytes()))?;
        (at.elapsed() < TICKET_TTL).then_some(account)
    }

    /// Back from Pocket ID to link `sub` to `account`: the id the account's session confirms
    /// with.
    pub fn hold_link(&self, account: &str, sub: &str) -> String {
        let id = random_hex(24);
        let mut m = self.links.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, (_, _, at)| at.elapsed() < TICKET_TTL);
        m.insert(
            sha256_hex(id.as_bytes()),
            (account.to_owned(), sub.to_owned(), Instant::now()),
        );
        id
    }

    /// The (account, Pocket ID user) of a held link; one time.
    pub fn take_link(&self, id: &str) -> Option<(String, String)> {
        let mut m = self.links.lock().unwrap_or_else(|e| e.into_inner());
        let (account, sub, at) = m.remove(&sha256_hex(id.trim().as_bytes()))?;
        (at.elapsed() < TICKET_TTL).then_some((account, sub))
    }

    /// The Pocket ID address to send the browser to, and the cookie value to give it.
    pub async fn start(
        &self,
        http: &Client,
        cfg: &PocketId,
        intent: Intent,
    ) -> Result<(String, String), OidcError> {
        let ep = self.endpoints(http, cfg).await?;
        let state = random_hex(24);
        let nonce = random_hex(24);
        let verifier = random_hex(32);
        let browser = random_hex(24);
        let challenge =
            b64url(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()).as_ref());
        {
            let mut m = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            m.retain(|_, p| p.created.elapsed() < FLOW_TTL);
            if m.len() >= MAX_PENDING {
                return Err(OidcError::Provider(
                    "too many sign-ins at once, try again".into(),
                ));
            }
            m.insert(
                state.clone(),
                Pending {
                    intent,
                    verifier,
                    browser: sha256_hex(browser.as_bytes()),
                    created: Instant::now(),
                },
            );
        }
        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}\
             &code_challenge={}&code_challenge_method=S256",
            ep.authorization,
            pct(&cfg.client_id),
            pct(&cfg.redirect_uri),
            pct("openid profile"),
            pct(&state),
            pct(&nonce),
            pct(&challenge),
        );
        Ok((url, browser))
    }

    /// The callback: the intent it was started with, and who signed in.
    pub async fn finish(
        &self,
        http: &Client,
        cfg: &PocketId,
        code: &str,
        state: &str,
        browser: Option<&str>,
    ) -> Result<(Intent, OidcUser), OidcError> {
        let pending = {
            let mut m = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            m.remove(state)
        }
        .filter(|p| p.created.elapsed() < FLOW_TTL)
        .ok_or(OidcError::Expired)?;
        // Both sides are hashes of 24 random bytes: nothing to learn from how they compare.
        if browser.map(|b| sha256_hex(b.as_bytes())).as_deref() != Some(pending.browser.as_str()) {
            return Err(OidcError::OtherBrowser);
        }
        let ep = self.endpoints(http, cfg).await?;
        let form = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&client_secret={}&code_verifier={}",
            pct(code),
            pct(&cfg.redirect_uri),
            pct(&cfg.client_id),
            pct(&cfg.client_secret),
            pct(&pending.verifier),
        );
        #[derive(Deserialize)]
        struct Tokens {
            access_token: String,
        }
        let tokens: Tokens = http
            .post(&ep.token)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| OidcError::Provider(e.to_string()))?
            .json()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        let info: Value = http
            .get(&ep.userinfo)
            .bearer_auth(&tokens.access_token)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| OidcError::Provider(e.to_string()))?
            .json()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        let text = |k: &str| {
            info.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let sub = text("sub").ok_or_else(|| OidcError::Provider("no user id".into()))?;
        Ok((
            pending.intent,
            OidcUser {
                sub,
                name: text("name").or_else(|| text("given_name")),
                username: text("preferred_username"),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_paths_on_this_site_come_back() {
        assert_eq!(safe_next(Some("/q/123456")), "/q/123456");
        assert_eq!(safe_next(Some("//evil.example")), "/");
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(Some("/a b")), "/");
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn the_cookie_is_found_among_others() {
        assert_eq!(cookie_of("a=1; aw_oidc=xyz; b=2"), Some("xyz"));
        assert_eq!(cookie_of("a=1"), None);
        assert!(cookie("v").contains("HttpOnly; Secure; SameSite=Lax"));
    }

    #[test]
    fn held_links_are_one_time() {
        let o = Oidc::default();
        let id = o.hold_link("acc", "sub");
        assert_eq!(o.take_link(&id), Some(("acc".into(), "sub".into())));
        assert!(o.take_link(&id).is_none());
    }

    #[test]
    fn tickets_are_one_time() {
        let o = Oidc::default();
        let t = o.ticket("acc");
        assert_eq!(o.take_ticket(&t).as_deref(), Some("acc"));
        assert!(o.take_ticket(&t).is_none());
    }
}
