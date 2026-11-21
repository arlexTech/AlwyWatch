// SPDX-License-Identifier: AGPL-3.0-or-later
//! App state: signed-in accounts (browser storage), the shared HTTP client, and builders for
//! the media, keep and Seerr clients. Everything else talks to the servers through these.

use keep_service::KeepClient;
use leptos::prelude::*;
use media_api::{AnyServer, ClientInfo, ImageKind, Item, MediaServer, ServerKind, Session};
use serde::{Serialize, de::DeserializeOwned};

const ACCOUNTS: &str = "aw.accounts";
const DEVICE: &str = "aw.device";
const KEEP: &str = "aw.keep";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

pub fn load<T: DeserializeOwned>(key: &str) -> Option<T> {
    serde_json::from_str(&storage()?.get_item(key).ok()??).ok()
}

pub fn save<T: Serialize>(key: &str, value: &T) {
    if let Some(s) = storage() {
        let _ = s.set_item(key, &serde_json::to_string(value).unwrap_or_default());
    }
}

pub fn remove(key: &str) {
    if let Some(s) = storage() {
        let _ = s.remove_item(key);
    }
}

/// A stable id for an account in URLs: kind, server and user.
pub fn account_key(s: &Session) -> String {
    let kind = match s.kind {
        ServerKind::Jellyfin => "jf",
        ServerKind::Plex => "px",
    };
    format!("{kind}.{}.{}", s.server_id, s.user_id)
}

/// Which account signs in to the AlwyWatch server (keep, requests, watch together).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeepLink {
    pub account: String,
    pub user_name: String,
    pub is_admin: bool,
}

#[derive(Clone)]
pub struct Ctx {
    pub http: net::reqwest::Client,
    pub client: ClientInfo,
    pub accounts: RwSignal<Vec<Session>>,
    pub keep: RwSignal<Option<KeepLink>>,
}

impl Ctx {
    pub fn new() -> Self {
        let device_id = load::<String>(DEVICE).unwrap_or_else(|| {
            let mut b = [0u8; 16];
            if let Some(c) = web_sys::window().and_then(|w| w.crypto().ok()) {
                let _ = c.get_random_values_with_u8_array(&mut b);
            }
            let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
            save(DEVICE, &id);
            id
        });
        let http = net::api_client("AlwyWatch-Web").unwrap_or_default();
        Self {
            http,
            client: ClientInfo {
                product: "AlwyWatch".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                device_name: "Web browser".into(),
                device_id,
                platform: "Web".into(),
            },
            accounts: RwSignal::new(load(ACCOUNTS).unwrap_or_default()),
            keep: RwSignal::new(load(KEEP)),
        }
    }

    pub fn session(&self, key: &str) -> Option<Session> {
        self.accounts
            .get_untracked()
            .into_iter()
            .find(|s| account_key(s) == key)
    }

    pub fn server(&self, key: &str) -> Option<AnyServer> {
        self.session(key)
            .map(|s| AnyServer::from_session(s, self.client.clone(), self.http.clone()))
    }

    pub fn servers(&self) -> Vec<(String, AnyServer)> {
        self.accounts
            .get_untracked()
            .into_iter()
            .map(|s| {
                (
                    account_key(&s),
                    AnyServer::from_session(s, self.client.clone(), self.http.clone()),
                )
            })
            .collect()
    }

    pub fn add_account(&self, s: Session) {
        let key = account_key(&s);
        self.accounts.update(|a| {
            a.retain(|x| account_key(x) != key);
            a.push(s);
        });
        save(ACCOUNTS, &self.accounts.get_untracked());
    }

    pub fn remove_account(&self, key: &str) {
        self.accounts
            .update(|a| a.retain(|x| account_key(x) != key));
        save(ACCOUNTS, &self.accounts.get_untracked());
        if self.keep.get_untracked().is_some_and(|k| k.account == key) {
            self.set_keep(None);
        }
    }

    pub fn set_keep(&self, k: Option<KeepLink>) {
        match &k {
            Some(k) => save(KEEP, k),
            None => remove(KEEP),
        }
        self.keep.set(k);
    }

    /// The AlwyWatch server is the site itself (same origin: no CORS, no address to type).
    pub fn origin() -> String {
        web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default()
    }

    /// A keep client for any account (to list servers or link), or the linked one.
    pub fn keep_for(&self, s: &Session) -> KeepClient {
        KeepClient::new(self.http.clone(), &Self::origin(), &s.server_id, &s.token)
    }

    pub fn keep_client(&self) -> Option<KeepClient> {
        let k = self.keep.get_untracked()?;
        self.session(&k.account).map(|s| self.keep_for(&s))
    }

    pub fn seerr(&self) -> Option<seerr::SeerrClient> {
        let k = self.keep.get_untracked()?;
        let s = self.session(&k.account)?;
        Some(seerr::SeerrClient::bridged(
            self.http.clone(),
            &format!("{}{}", Self::origin(), keep_service::SEERR_BRIDGE_PATH),
            vec![
                ("Authorization".into(), format!("Bearer {}", s.token)),
                (keep_service::SERVER_HEADER.into(), s.server_id.clone()),
            ],
        ))
    }
}

/// Adds the account token to a media URL: `<img>` and `<video>` can't send headers.
pub fn with_token(url: &str, s: &Session) -> String {
    let (name, already) = match s.kind {
        ServerKind::Jellyfin => ("api_key", "api_key="),
        ServerKind::Plex => ("X-Plex-Token", "X-Plex-Token="),
    };
    if url.contains(already) || url.to_ascii_lowercase().contains("apikey=") {
        return url.to_owned();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{name}={}", net::percent_encode(&s.token))
}

/// The first image of these kinds, sized for `width` CSS pixels (doubled for sharp screens).
pub fn image(srv: &AnyServer, item: &Item, kinds: &[ImageKind], width: u32) -> Option<String> {
    let img = kinds
        .iter()
        .find_map(|k| item.images.iter().find(|i| i.kind == *k))?;
    let req = srv.image_request(&item.id, img, width * 2);
    Some(with_token(&req.url, srv.session()))
}

pub fn fmt_time(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn ctx() -> Ctx {
    expect_context::<Ctx>()
}

/// Waits without blocking the page (setTimeout).
pub async fn sleep(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}
