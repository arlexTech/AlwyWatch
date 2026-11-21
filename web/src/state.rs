// SPDX-License-Identifier: AGPL-3.0-or-later
//! App state: signed-in accounts (browser storage), the shared HTTP client, and builders for
//! the media, keep and Seerr clients. Everything else talks to the servers through these.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use keep_service::KeepClient;
use leptos::prelude::*;
use media_api::{
    AnyServer, ClientInfo, Error, HttpRequest, ImageKind, Item, MediaServer, ServerKind, Session,
};
use serde::{Serialize, de::DeserializeOwned};

const ACCOUNTS: &str = "aw.accounts";
const DEVICE: &str = "aw.device";
const KEEP: &str = "aw.keep";
/// plex.tv account tokens of Plex accounts (the watchlist lives on plex.tv), by account key.
const PLEX_TV: &str = "aw.plextv";
/// Copies of server data kept for an instant next start; they go with the accounts.
pub const HOME: &str = "aw.home";
pub const LIBS: &str = "aw.libs";
pub const LEAVING: &str = "aw.leaving";
pub const REQUESTS_MINE: &str = "aw.requests.mine";
pub const REQUESTS_ALL: &str = "aw.requests.all";
pub const REQUESTS_EVERYONE: &str = "aw.requests.everyone";

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
    /// An account a server refused (session ended): the menu offers to sign in again.
    pub expired: RwSignal<Option<String>>,
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
            expired: RwSignal::new(None),
        }
    }

    /// Notes a failed call on an account: a refusal means its session ended.
    pub fn failed(&self, account: &str, e: &Error) {
        if *e == Error::Unauthorized && self.session(account).is_some() {
            self.expired.try_set(Some(account.to_owned()));
        }
    }

    pub fn plex_tv_token(&self, account: &str) -> Option<String> {
        load::<HashMap<String, String>>(PLEX_TV)?.remove(account)
    }

    pub fn set_plex_tv_token(&self, account: &str, token: &str) {
        let mut all = load::<HashMap<String, String>>(PLEX_TV).unwrap_or_default();
        all.insert(account.to_owned(), token.to_owned());
        save(PLEX_TV, &all);
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

    /// Signs an account out of this browser: its token and every copy of its data go.
    pub fn remove_account(&self, key: &str) {
        self.accounts
            .update(|a| a.retain(|x| account_key(x) != key));
        save(ACCOUNTS, &self.accounts.get_untracked());
        if let Some(mut all) = load::<HashMap<String, String>>(PLEX_TV) {
            all.remove(key);
            save(PLEX_TV, &all);
        }
        // Rails and libraries mix accounts: rebuilt from the remaining ones.
        remove(HOME);
        remove(LIBS);
        if self.keep.get_untracked().is_some_and(|k| k.account == key) {
            self.set_keep(None);
            for k in [LEAVING, REQUESTS_MINE, REQUESTS_ALL, REQUESTS_EVERYONE] {
                remove(k);
            }
        }
        if self.expired.get_untracked().as_deref() == Some(key) {
            self.expired.set(None);
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

    /// The token the AlwyWatch server checks: Jellyfin's session token; for Plex the plex.tv
    /// account token (the session's is the media server's own, which the AlwyWatch server
    /// can't verify), kept at Plex sign-in. Plex accounts signed in before have none.
    pub fn keep_token(&self, s: &Session) -> Option<String> {
        match s.kind {
            ServerKind::Jellyfin => Some(s.token.clone()),
            ServerKind::Plex => self.plex_tv_token(&account_key(s)),
        }
    }

    /// A keep client for any account (to link it), or the linked one.
    pub fn keep_for(&self, s: &Session) -> Option<KeepClient> {
        let token = self.keep_token(s)?;
        Some(KeepClient::new(
            self.http.clone(),
            &Self::origin(),
            &s.server_id,
            &token,
        ))
    }

    pub fn keep_client(&self) -> Option<KeepClient> {
        let k = self.keep.get_untracked()?;
        self.keep_for(&self.session(&k.account)?)
    }

    pub fn seerr(&self) -> Option<seerr::SeerrClient> {
        let k = self.keep.get_untracked()?;
        let s = self.session(&k.account)?;
        let token = self.keep_token(&s)?;
        Some(seerr::SeerrClient::bridged(
            self.http.clone(),
            &format!("{}{}", Self::origin(), keep_service::SEERR_BRIDGE_PATH),
            vec![
                ("Authorization".into(), format!("Bearer {token}")),
                (keep_service::SERVER_HEADER.into(), s.server_id.clone()),
            ],
        ))
    }
}

/// Whether `url` is on the account's own server (only those get its token).
fn on_server(url: &str, s: &Session) -> bool {
    let base = s.base_url.trim_end_matches('/');
    url.strip_prefix(base)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?']))
}

/// Adds the account token to a media URL: `<img>` and `<video>` can't send headers. URLs
/// elsewhere (cast photos on a CDN…) are left as they are: the token never leaves its server.
pub fn with_token(url: &str, s: &Session) -> String {
    let (name, already) = match s.kind {
        ServerKind::Jellyfin => ("api_key", "api_key="),
        ServerKind::Plex => ("X-Plex-Token", "X-Plex-Token="),
    };
    if !on_server(url, s) || url.contains(already) || url.to_ascii_lowercase().contains("apikey=") {
        return url.to_owned();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{name}={}", net::percent_encode(&s.token))
}

/// A playable URL for a request: the token, and for Plex the client headers as parameters
/// (its transcoder refuses requests that don't say which client they come from).
pub fn media_url(req: &HttpRequest, s: &Session) -> String {
    let mut url = with_token(&req.url, s);
    if s.kind == ServerKind::Plex && on_server(&url, s) {
        for h in &req.headers {
            let lower = h.name.to_ascii_lowercase();
            if !lower.starts_with("x-plex-")
                || lower == "x-plex-token"
                || url.contains(&format!("{}=", h.name))
            {
                continue;
            }
            let sep = if url.contains('?') { '&' } else { '?' };
            url = format!("{url}{sep}{}={}", h.name, net::percent_encode(&h.value));
        }
    }
    url
}

/// A CSS `url(...)` value that a quote, bracket or backslash in the URL can't break out of.
pub fn css_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len() + 7);
    out.push_str("url(\"");
    for c in url.chars() {
        match c {
            '"' => out.push_str("%22"),
            '\'' => out.push_str("%27"),
            '\\' => out.push_str("%5C"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push_str("\")");
    out
}

/// Errors from a media server in plain words.
pub fn plain(e: &Error) -> String {
    match e {
        Error::Unauthorized => {
            "The server refused: your session may have ended. Sign in again in Settings.".into()
        }
        Error::NotFound => "This isn't on the server any more.".into(),
        Error::Network { .. } => "Can't reach the server. Check your connection.".into(),
        Error::Status { status } if *status >= 500 => {
            "The server had a problem. Try again in a moment.".into()
        }
        Error::Status { status } => format!("The server refused this (error {status})."),
        Error::Decode { .. } => "The server answered something unexpected.".into(),
        Error::Invalid { message } => message.clone(),
        Error::Pending => "Still waiting for an answer.".into(),
    }
}

/// Errors from the AlwyWatch server in plain words.
pub fn plain_keep(e: &Error) -> String {
    match e {
        Error::Unauthorized => {
            "The AlwyWatch server refused: you're not allowed to do this, or your session ended."
                .into()
        }
        Error::NotFound => "The AlwyWatch server doesn't know this.".into(),
        Error::Network { .. } => "Can't reach your AlwyWatch server right now.".into(),
        Error::Status { status } if *status >= 500 => {
            "The AlwyWatch server had a problem. Try again in a moment.".into()
        }
        other => plain(other),
    }
}

/// Runs futures at the same time (one request per server, not one after the other).
pub async fn join_all<F: Future>(futs: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    let mut futs: Vec<Pin<Box<F>>> = futs.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = futs.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (f, o) in futs.iter_mut().zip(out.iter_mut()) {
            if o.is_none() {
                match f.as_mut().poll(cx) {
                    Poll::Ready(v) => *o = Some(v),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    out.into_iter().flatten().collect()
}

/// Two futures of different kinds at the same time.
pub async fn join2<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let (mut a, mut b) = (Box::pin(a), Box::pin(b));
    let (mut ra, mut rb) = (None, None);
    std::future::poll_fn(|cx| {
        if ra.is_none()
            && let Poll::Ready(v) = a.as_mut().poll(cx)
        {
            ra = Some(v);
        }
        if rb.is_none()
            && let Poll::Ready(v) = b.as_mut().poll(cx)
        {
            rb = Some(v);
        }
        if ra.is_some() && rb.is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    match (ra, rb) {
        (Some(a), Some(b)) => (a, b),
        _ => unreachable!("both finished"),
    }
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

thread_local! {
    /// The same state for code running outside the component tree: futures started with
    /// `spawn_local` don't carry the reactive owner, so `use_context` finds nothing there.
    static APP: std::cell::OnceCell<Ctx> = const { std::cell::OnceCell::new() };
}

/// Creates the app state, reachable from components and from spawned futures alike.
pub fn install() -> Ctx {
    let c = Ctx::new();
    APP.with(|a| {
        let _ = a.set(c.clone());
    });
    c
}

pub fn ctx() -> Ctx {
    use_context::<Ctx>()
        .or_else(|| APP.with(|a| a.get().cloned()))
        .expect("app state installed at start")
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

#[cfg(test)]
mod tests {
    use super::*;
    use media_api::Header;

    fn session(kind: ServerKind, base: &str) -> Session {
        Session {
            kind,
            base_url: base.into(),
            server_id: "s".into(),
            server_name: "S".into(),
            user_id: "u".into(),
            user_name: "U".into(),
            is_admin: false,
            token: "t k".into(),
        }
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = Box::pin(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn the_token_only_goes_to_its_server() {
        let jf = session(ServerKind::Jellyfin, "https://jf.example");
        assert_eq!(
            with_token("https://jf.example/Items/1/Images/Primary", &jf),
            "https://jf.example/Items/1/Images/Primary?api_key=t%20k"
        );
        assert_eq!(
            with_token("https://jf.example/a?x=1", &jf),
            "https://jf.example/a?x=1&api_key=t%20k"
        );
        // Elsewhere, or a look-alike host: untouched.
        for url in [
            "https://metadata-static.plex.tv/p.jpg",
            "https://jf.example.evil.com/x",
            "https://jf.examplex/x",
        ] {
            assert_eq!(with_token(url, &jf), url);
        }
        let px = session(ServerKind::Plex, "https://plex.example:32400");
        let req = HttpRequest {
            url: "https://plex.example:32400/video/:/transcode/universal/start.m3u8?session=1"
                .into(),
            headers: vec![
                Header {
                    name: "X-Plex-Client-Identifier".into(),
                    value: "dev 1".into(),
                },
                Header {
                    name: "X-Plex-Token".into(),
                    value: "t k".into(),
                },
                Header {
                    name: "Accept".into(),
                    value: "application/json".into(),
                },
            ],
        };
        assert_eq!(
            media_url(&req, &px),
            "https://plex.example:32400/video/:/transcode/universal/start.m3u8?session=1&X-Plex-Token=t%20k&X-Plex-Client-Identifier=dev%201"
        );
    }

    #[test]
    fn css_urls_cannot_be_broken_out_of() {
        assert_eq!(css_url("https://a/b.jpg"), "url(\"https://a/b.jpg\")");
        assert_eq!(
            css_url("https://a/x');color:red;(\"\\\n"),
            "url(\"https://a/x%27%29;color:red;%28%22%5C\")"
        );
    }

    #[test]
    fn joins_keep_the_order() {
        let out = block_on(join_all((1..=3).map(|i| async move { i * 2 })));
        assert_eq!(out, vec![2, 4, 6]);
        assert_eq!(block_on(join2(async { 1 }, async { "a" })), (1, "a"));
    }

    #[test]
    fn errors_read_plainly() {
        assert!(plain(&Error::Unauthorized).contains("Sign in again"));
        assert!(plain(&Error::Status { status: 502 }).contains("problem"));
        assert!(
            plain_keep(&Error::Network {
                message: "x".into()
            })
            .contains("AlwyWatch")
        );
    }
}
