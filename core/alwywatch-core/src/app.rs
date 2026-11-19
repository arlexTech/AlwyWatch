// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `AlwyWatch` object: accounts, cache-first repositories, images, playback, progress.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use media_api::jellyfin::{JellyfinAuth, QuickConnectCode};
use media_api::plex::{PlexAuth, PlexPin, PlexServerResource};
use media_api::{AnyServer, MediaServer};
use serde::{Deserialize, Serialize};
use store::{ImageCache, Store};
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;

use crate::error::{CoreError, Result};
use crate::*;

const TTL_HOME: i64 = 30;
const TTL_LIST: i64 = 120;
const TTL_DETAIL: i64 = 300;
const PLAYBACK_INFO_TTL: Duration = Duration::from_secs(120);
const IMAGE_WIDTHS: [u32; 7] = [160, 240, 320, 480, 720, 1080, 1920];

// ---------------------------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, uniffi::Record)]
pub struct CoreConfig {
    /// Persistent app data (SQLite database).
    pub data_dir: String,
    /// Disposable cache (images). The OS may clear it.
    pub cache_dir: String,
    pub client: ClientInfo,
    pub image_cache_mb: u32,
}

/// A signed-in user on one server. Stored without the token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct Account {
    pub id: String,
    pub kind: ServerKind,
    pub base_url: String,
    pub server_id: String,
    pub server_name: String,
    pub user_id: String,
    pub user_name: String,
    pub is_admin: bool,
}

/// Result of a sign-in: the platform stores `token` in secure storage (Android Keystore,
/// OS keyring) and passes it back through `activate_account` on the next start.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SignedIn {
    pub account: Account,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct LibraryRail {
    pub library: Library,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct Home {
    pub resume: Vec<Item>,
    pub next_up: Vec<Item>,
    pub libraries: Vec<Library>,
    pub latest: Vec<LibraryRail>,
    /// Unix seconds when this was fetched from the server.
    pub fetched_at: i64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PageResult {
    pub page: Page,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ItemResult {
    pub item: Item,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ItemsResult {
    pub items: Vec<Item>,
    pub fetched_at: i64,
}

fn account_id(s: &Session) -> String {
    let kind = match s.kind {
        ServerKind::Jellyfin => "jellyfin",
        ServerKind::Plex => "plex",
    };
    format!("{kind}:{}:{}", s.server_id, s.user_id)
}

fn account_from(s: &Session) -> Account {
    Account {
        id: account_id(s),
        kind: s.kind,
        base_url: s.base_url.clone(),
        server_id: s.server_id.clone(),
        server_name: s.server_name.clone(),
        user_id: s.user_id.clone(),
        user_name: s.user_name.clone(),
        is_admin: s.is_admin,
    }
}

fn session_from(a: &Account, token: String) -> Session {
    Session {
        kind: a.kind,
        base_url: a.base_url.clone(),
        server_id: a.server_id.clone(),
        server_name: a.server_name.clone(),
        user_id: a.user_id.clone(),
        user_name: a.user_name.clone(),
        is_admin: a.is_admin,
        token,
    }
}

fn key(account: &str, parts: &[&str]) -> String {
    let mut k = format!("acct/{account}/");
    k.push_str(&parts.join("\u{1f}"));
    k
}

fn items_key(account: &str, q: &ItemQuery) -> String {
    key(
        account,
        &[
            "items",
            q.library_id.as_deref().unwrap_or("*"),
            &format!("{:?}", q.sort_by),
            if q.descending { "d" } else { "a" },
            if q.unplayed_only { "u" } else { "-" },
            q.search.as_deref().unwrap_or(""),
            &q.start.to_string(),
            &q.limit.to_string(),
        ],
    )
}

/// Rounds a requested width up to a small set of buckets so the image cache gets hits across
/// screens and densities.
pub fn bucket_width(w: u32) -> u32 {
    IMAGE_WIDTHS
        .iter()
        .copied()
        .find(|b| *b >= w)
        .unwrap_or(*IMAGE_WIDTHS.last().unwrap())
}

// ---------------------------------------------------------------------------------------------
// Core state (runs on the core runtime)
// ---------------------------------------------------------------------------------------------

pub struct Core {
    client: ClientInfo,
    http: net::reqwest::Client,
    store: Arc<Store>,
    images: ImageCache,
    image_permits: Semaphore,
    servers: RwLock<HashMap<String, Arc<AnyServer>>>,
    playback: Mutex<HashMap<String, (Instant, PlaybackInfo)>>,
}

impl Core {
    fn server(&self, account_id: &str) -> Result<Arc<AnyServer>> {
        self.servers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(account_id)
            .cloned()
            .ok_or_else(|| CoreError::NotSignedIn {
                account_id: account_id.to_owned(),
            })
    }

    fn register(&self, session: Session) -> SignedIn {
        let account = account_from(&session);
        let token = session.token.clone();
        let server = AnyServer::from_session(session, self.client.clone(), self.http.clone());
        self.servers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account.id.clone(), Arc::new(server));
        SignedIn { account, token }
    }

    fn signed_in(&self, session: Session) -> Result<SignedIn> {
        let s = self.register(session);
        self.store.put_account(&s.account.id, &s.account)?;
        Ok(s)
    }

    /// Returns the cached value when it is fresh enough, otherwise fetches and stores.
    async fn swr<T, F, Fut>(&self, key: &str, ttl: i64, force: bool, fetch: F) -> Result<(T, i64)>
    where
        T: Serialize + serde::de::DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if !force
            && let Some(c) = self.store.cache_get::<T>(key)?
            && c.age_secs() < ttl
        {
            return Ok((c.value, c.fetched_at));
        }
        let value = fetch().await?;
        self.store.cache_put(key, &value)?;
        Ok((value, store::now()))
    }

    async fn fetch_home(&self, account: &str) -> Result<Home> {
        let srv = self.server(account)?;
        let (libraries, resume, next_up) =
            tokio::join!(srv.libraries(), srv.resume(20), srv.next_up(20));
        let libraries = libraries?;
        let mut set = tokio::task::JoinSet::new();
        for (i, lib) in libraries
            .iter()
            .filter(|l| l.kind != LibraryKind::Other)
            .cloned()
            .enumerate()
        {
            let srv = srv.clone();
            set.spawn(async move {
                let items = srv.latest(&lib.id, 20).await;
                (i, lib, items)
            });
        }
        let mut latest = Vec::new();
        while let Some(r) = set.join_next().await {
            let (i, library, items) = r.map_err(|e| CoreError::Internal {
                message: e.to_string(),
            })?;
            // A failing library rail should not hide the rest of the home screen.
            if let Ok(items) = items {
                latest.push((i, LibraryRail { library, items }));
            }
        }
        latest.sort_by_key(|(i, _)| *i);
        Ok(Home {
            // Resume/next-up failures (e.g. a server without on-deck) degrade to empty rails.
            resume: resume.unwrap_or_default(),
            next_up: next_up.unwrap_or_default(),
            libraries,
            latest: latest.into_iter().map(|(_, r)| r).collect(),
            fetched_at: store::now(),
        })
    }

    async fn image(
        &self,
        account: &str,
        item_id: &str,
        image: &ImageRef,
        width: u32,
    ) -> Result<String> {
        let srv = self.server(account)?;
        let req = srv.image_request(item_id, image, bucket_width(width));
        let cache_key = format!("{account}\u{1f}{}", req.url);
        if let Some(p) = self.images.get(&cache_key)? {
            return Ok(p.to_string_lossy().into_owned());
        }
        let _permit = self
            .image_permits
            .acquire()
            .await
            .map_err(|e| CoreError::Internal {
                message: e.to_string(),
            })?;
        // Another request may have fetched it while we waited.
        if let Some(p) = self.images.get(&cache_key)? {
            return Ok(p.to_string_lossy().into_owned());
        }
        let mut r = self.http.get(&req.url);
        for h in &req.headers {
            r = r.header(&h.name, &h.value);
        }
        let bytes = net::send(r)
            .await?
            .bytes()
            .await
            .map_err(net::Error::from)?;
        let path = self.images.put(&cache_key, &bytes)?;
        Ok(path.to_string_lossy().into_owned())
    }

    fn cached_image(
        &self,
        account: &str,
        item_id: &str,
        image: &ImageRef,
        width: u32,
    ) -> Option<String> {
        let srv = self.server(account).ok()?;
        let req = srv.image_request(item_id, image, bucket_width(width));
        let cache_key = format!("{account}\u{1f}{}", req.url);
        self.images
            .get(&cache_key)
            .ok()
            .flatten()
            .map(|p| p.to_string_lossy().into_owned())
    }

    async fn playback_info(
        &self,
        account: &str,
        item_id: &str,
        profile: &DeviceProfile,
    ) -> Result<PlaybackInfo> {
        let k = format!("{account}\u{1f}{item_id}");
        if let Some((at, info)) = self
            .playback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&k)
            && at.elapsed() < PLAYBACK_INFO_TTL
        {
            return Ok(info.clone());
        }
        let info = self
            .server(account)?
            .playback_info(item_id, profile)
            .await?;
        let mut map = self.playback.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, (at, _)| at.elapsed() < PLAYBACK_INFO_TTL);
        map.insert(k, (Instant::now(), info.clone()));
        Ok(info)
    }

    async fn report_progress(&self, account: &str, report: ProgressReport) -> Result<()> {
        let result = match self.server(account) {
            Ok(srv) => srv.report_progress(&report).await.map_err(CoreError::from),
            Err(e) => Err(e),
        };
        if matches!(report.event, PlaybackEvent::Stop) {
            // Resume rails changed: keep showing them, but refresh on next visit.
            self.store.cache_mark_stale(&key(account, &["home"]))?;
            self.store
                .cache_mark_stale(&key(account, &["item", &report.item_id]))?;
        }
        match result {
            Ok(()) => Ok(()),
            // Offline: keep start/stop/latest positions so the server catches up later.
            Err(e) if e.is_transient() || matches!(e, CoreError::NotSignedIn { .. }) => {
                if report.event != PlaybackEvent::Progress {
                    self.store.queue_progress(account, &report)?;
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn flush_pending(&self) -> Result<u32> {
        let mut sent = 0;
        for (row, account, report) in self.store.pending_progress::<ProgressReport>()? {
            let Ok(srv) = self.server(&account) else {
                continue;
            };
            match srv.report_progress(&report).await {
                Ok(()) => {
                    self.store.ack_progress(row)?;
                    sent += 1;
                }
                Err(e) if e.is_transient() => break,
                // Permanent failure (item deleted, token revoked): drop it.
                Err(_) => self.store.ack_progress(row)?,
            }
        }
        Ok(sent)
    }
}

// ---------------------------------------------------------------------------------------------
// Exported object
// ---------------------------------------------------------------------------------------------

#[derive(uniffi::Object)]
pub struct AlwyWatch {
    core: Arc<Core>,
    rt: Runtime,
}

impl AlwyWatch {
    /// Runs a future on the core runtime so callers can await it from any executor
    /// (Kotlin coroutines, Slint's event loop).
    async fn run<T, F>(&self, f: impl FnOnce(Arc<Core>) -> F) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T>> + Send + 'static,
    {
        let fut = f(self.core.clone());
        self.rt.spawn(fut).await.map_err(|e| CoreError::Internal {
            message: e.to_string(),
        })?
    }

    /// The core runtime handle, for the desktop app to spawn its own background work.
    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.rt.handle().clone()
    }

    pub fn store(&self) -> Arc<Store> {
        self.core.store.clone()
    }
}

#[uniffi::export]
impl AlwyWatch {
    /// Opens local state. No network, no heavy work: safe to call during app start.
    #[uniffi::constructor]
    pub fn new(config: CoreConfig) -> Result<Arc<Self>> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("alwywatch-core")
            .enable_all()
            .build()
            .map_err(|e| CoreError::Internal {
                message: e.to_string(),
            })?;
        let data = PathBuf::from(&config.data_dir);
        let store = Arc::new(Store::open(&data.join("alwywatch.sqlite"))?);
        let images = ImageCache::new(
            store.clone(),
            &PathBuf::from(&config.cache_dir).join("images"),
            u64::from(config.image_cache_mb.max(16)) * 1024 * 1024,
        )?;
        let ua = format!(
            "{}/{} ({})",
            config.client.product, config.client.version, config.client.platform
        );
        let http = net::api_client(&ua)?;
        Ok(Arc::new(Self {
            core: Arc::new(Core {
                client: config.client,
                http,
                store,
                images,
                image_permits: Semaphore::new(6),
                servers: RwLock::new(HashMap::new()),
                playback: Mutex::new(HashMap::new()),
            }),
            rt,
        }))
    }

    // ----- accounts --------------------------------------------------------------------------

    /// Accounts known on this device (from local storage, no tokens).
    pub fn accounts(&self) -> Result<Vec<Account>> {
        Ok(self.core.store.accounts()?)
    }

    /// Re-attaches a stored account with its token from secure storage. No network.
    pub fn activate_account(&self, account_id: String, token: String) -> Result<()> {
        let account = self
            .accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or(CoreError::NotFound)?;
        self.core.register(session_from(&account, token));
        Ok(())
    }

    pub fn is_active(&self, account_id: String) -> bool {
        self.core.server(&account_id).is_ok()
    }

    /// Forgets an account and its cached data. The platform deletes the stored token.
    pub fn remove_account(&self, account_id: String) -> Result<()> {
        self.core
            .servers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&account_id);
        self.core.store.remove_account(&account_id)?;
        self.core
            .store
            .cache_delete_prefix(&format!("acct/{account_id}/"))?;
        Ok(())
    }

    pub async fn jellyfin_server_info(&self, url: String) -> Result<ServerInfo> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            Ok(JellyfinAuth::new(c.http.clone(), c.client.clone())
                .public_info(&base)
                .await?)
        })
        .await
    }

    pub async fn jellyfin_sign_in(
        &self,
        url: String,
        username: String,
        password: String,
    ) -> Result<SignedIn> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let auth = JellyfinAuth::new(c.http.clone(), c.client.clone());
            let session = auth.sign_in(&base, &username, &password).await?;
            c.signed_in(session)
        })
        .await
    }

    pub async fn jellyfin_quick_connect_start(&self, url: String) -> Result<QuickConnectCode> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            Ok(JellyfinAuth::new(c.http.clone(), c.client.clone())
                .quick_connect_start(&base)
                .await?)
        })
        .await
    }

    /// Poll every few seconds; returns `CoreError::Pending` until approved.
    pub async fn jellyfin_quick_connect_poll(
        &self,
        url: String,
        secret: String,
    ) -> Result<SignedIn> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let auth = JellyfinAuth::new(c.http.clone(), c.client.clone());
            let session = auth.quick_connect_poll(&base, &secret).await?;
            c.signed_in(session)
        })
        .await
    }

    /// `strong = false` on TV (short code for plex.tv/link), `true` on phone/desktop (open `auth_url`).
    pub async fn plex_create_pin(&self, strong: bool) -> Result<PlexPin> {
        self.run(|c| async move {
            Ok(PlexAuth::new(c.http.clone(), c.client.clone())
                .create_pin(strong)
                .await?)
        })
        .await
    }

    /// Returns the plex.tv account token once linked, `CoreError::Pending` before.
    pub async fn plex_check_pin(&self, pin_id: u64) -> Result<String> {
        self.run(|c| async move {
            Ok(PlexAuth::new(c.http.clone(), c.client.clone())
                .check_pin(pin_id)
                .await?)
        })
        .await
    }

    pub async fn plex_servers(&self, account_token: String) -> Result<Vec<PlexServerResource>> {
        self.run(|c| async move {
            Ok(PlexAuth::new(c.http.clone(), c.client.clone())
                .servers(&account_token)
                .await?)
        })
        .await
    }

    /// Connects to one Plex server. `preferred_url` (e.g. a custom address) is tried first.
    pub async fn plex_connect(
        &self,
        account_token: String,
        server: PlexServerResource,
        preferred_url: Option<String>,
    ) -> Result<SignedIn> {
        self.run(|c| async move {
            let auth = PlexAuth::new(c.http.clone(), c.client.clone());
            let user = auth.user(&account_token).await?;
            let preferred = preferred_url
                .map(|u| net::normalize_base_url(&u))
                .transpose()?;
            let session = auth.connect(&server, &user, preferred.as_deref()).await?;
            c.signed_in(session)
        })
        .await
    }

    // ----- home ------------------------------------------------------------------------------

    pub fn cached_home(&self, account_id: String) -> Result<Option<Home>> {
        Ok(self
            .core
            .store
            .cache_get::<Home>(&key(&account_id, &["home"]))?
            .map(|c| Home {
                fetched_at: c.fetched_at,
                ..c.value
            }))
    }

    pub async fn refresh_home(&self, account_id: String, force: bool) -> Result<Home> {
        self.run(|c| async move {
            let k = key(&account_id, &["home"]);
            let (home, at) = c
                .swr(&k, TTL_HOME, force, || c.fetch_home(&account_id))
                .await?;
            Ok(Home {
                fetched_at: at,
                ..home
            })
        })
        .await
    }

    // ----- libraries and items --------------------------------------------------------------

    pub fn cached_items(&self, account_id: String, query: ItemQuery) -> Result<Option<PageResult>> {
        Ok(self
            .core
            .store
            .cache_get::<Page>(&items_key(&account_id, &query))?
            .map(|c| PageResult {
                page: c.value,
                fetched_at: c.fetched_at,
            }))
    }

    pub async fn refresh_items(
        &self,
        account_id: String,
        query: ItemQuery,
        force: bool,
    ) -> Result<PageResult> {
        self.run(|c| async move {
            let k = items_key(&account_id, &query);
            let srv = c.server(&account_id)?;
            let (page, at) = c
                .swr(&k, TTL_LIST, force, || async {
                    Ok(srv.items(&query).await?)
                })
                .await?;
            Ok(PageResult {
                page,
                fetched_at: at,
            })
        })
        .await
    }

    pub fn cached_item(&self, account_id: String, item_id: String) -> Result<Option<ItemResult>> {
        Ok(self
            .core
            .store
            .cache_get::<Item>(&key(&account_id, &["item", &item_id]))?
            .map(|c| ItemResult {
                item: c.value,
                fetched_at: c.fetched_at,
            }))
    }

    pub async fn refresh_item(
        &self,
        account_id: String,
        item_id: String,
        force: bool,
    ) -> Result<ItemResult> {
        self.run(|c| async move {
            let k = key(&account_id, &["item", &item_id]);
            let srv = c.server(&account_id)?;
            let (item, at) = c
                .swr(&k, TTL_DETAIL, force, || async {
                    Ok(srv.item(&item_id).await?)
                })
                .await?;
            Ok(ItemResult {
                item,
                fetched_at: at,
            })
        })
        .await
    }

    pub fn cached_seasons(
        &self,
        account_id: String,
        series_id: String,
    ) -> Result<Option<ItemsResult>> {
        self.cached_list(&key(&account_id, &["seasons", &series_id]))
    }

    pub async fn refresh_seasons(
        &self,
        account_id: String,
        series_id: String,
        force: bool,
    ) -> Result<ItemsResult> {
        self.run(|c| async move {
            let k = key(&account_id, &["seasons", &series_id]);
            let srv = c.server(&account_id)?;
            let (items, at) = c
                .swr(&k, TTL_DETAIL, force, || async {
                    Ok(srv.seasons(&series_id).await?)
                })
                .await?;
            Ok(ItemsResult {
                items,
                fetched_at: at,
            })
        })
        .await
    }

    pub fn cached_episodes(
        &self,
        account_id: String,
        series_id: String,
        season_id: String,
    ) -> Result<Option<ItemsResult>> {
        self.cached_list(&key(&account_id, &["episodes", &series_id, &season_id]))
    }

    pub async fn refresh_episodes(
        &self,
        account_id: String,
        series_id: String,
        season_id: String,
        force: bool,
    ) -> Result<ItemsResult> {
        self.run(|c| async move {
            let k = key(&account_id, &["episodes", &series_id, &season_id]);
            let srv = c.server(&account_id)?;
            let (items, at) = c
                .swr(&k, TTL_DETAIL, force, || async {
                    Ok(srv.episodes(&series_id, &season_id).await?)
                })
                .await?;
            Ok(ItemsResult {
                items,
                fetched_at: at,
            })
        })
        .await
    }

    pub async fn search(&self, account_id: String, query: String, limit: u32) -> Result<Vec<Item>> {
        self.run(|c| async move { Ok(c.server(&account_id)?.search(&query, limit).await?) })
            .await
    }

    pub async fn set_played(
        &self,
        account_id: String,
        item_id: String,
        played: bool,
    ) -> Result<()> {
        self.run(|c| async move {
            c.server(&account_id)?.set_played(&item_id, played).await?;
            c.store.cache_mark_stale(&key(&account_id, &["home"]))?;
            c.store
                .cache_mark_stale(&key(&account_id, &["item", &item_id]))?;
            Ok(())
        })
        .await
    }

    // ----- images ----------------------------------------------------------------------------

    /// Local file of an image if already cached (sync, for the first frame).
    pub fn cached_image(
        &self,
        account_id: String,
        item_id: String,
        image: ImageRef,
        width: u32,
    ) -> Option<String> {
        self.core.cached_image(&account_id, &item_id, &image, width)
    }

    /// Local file of an image, downloading it at most once (max 6 concurrent downloads).
    pub async fn image(
        &self,
        account_id: String,
        item_id: String,
        image: ImageRef,
        width: u32,
    ) -> Result<String> {
        self.run(|c| async move { c.image(&account_id, &item_id, &image, width).await })
            .await
    }

    pub fn image_cache_bytes(&self) -> u64 {
        self.core.images.total_bytes()
    }

    pub fn clear_image_cache(&self) -> Result<()> {
        Ok(self.core.images.clear()?)
    }

    // ----- playback --------------------------------------------------------------------------

    /// Playback sources, original file first. Kept for two minutes so a prefetch makes
    /// pressing play instant.
    pub async fn playback_info(
        &self,
        account_id: String,
        item_id: String,
        profile: DeviceProfile,
    ) -> Result<PlaybackInfo> {
        self.run(|c| async move { c.playback_info(&account_id, &item_id, &profile).await })
            .await
    }

    /// Fire-and-forget warm-up of `playback_info` (focused item, next episode).
    pub fn prefetch_playback_info(
        &self,
        account_id: String,
        item_id: String,
        profile: DeviceProfile,
    ) {
        let c = self.core.clone();
        self.rt.spawn(async move {
            let _ = c.playback_info(&account_id, &item_id, &profile).await;
        });
    }

    /// Reports playback state. Offline, start/pause/stop reports are queued and sent later.
    pub async fn report_progress(&self, account_id: String, report: ProgressReport) -> Result<()> {
        self.run(|c| async move { c.report_progress(&account_id, report).await })
            .await
    }

    /// Sends queued progress reports (call when the network comes back). Returns how many.
    pub async fn flush_pending_progress(&self) -> Result<u32> {
        self.run(|c| async move { c.flush_pending().await }).await
    }

    // ----- settings and UI state ------------------------------------------------------------

    pub fn get_setting(&self, key: String) -> Result<Option<String>> {
        Ok(self.core.store.setting(&key)?)
    }

    pub fn set_setting(&self, key: String, value: String) -> Result<()> {
        Ok(self.core.store.set_setting(&key, &value)?)
    }
}

impl AlwyWatch {
    fn cached_list(&self, k: &str) -> Result<Option<ItemsResult>> {
        Ok(self
            .core
            .store
            .cache_get::<Vec<Item>>(k)?
            .map(|c| ItemsResult {
                items: c.value,
                fetched_at: c.fetched_at,
            }))
    }
}
