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
    /// Where downloaded media files go (user-visible storage on desktop, app storage on Android).
    pub downloads_dir: String,
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
    /// Plex only: the plex.tv account token (differs from the server token for shared users).
    /// Stored by the platform too: Seerr and the keep service authenticate with it.
    pub plex_account_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct LibraryRail {
    pub library: MediaLibrary,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct Home {
    pub resume: Vec<Item>,
    pub next_up: Vec<Item>,
    pub libraries: Vec<MediaLibrary>,
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
    keep: RwLock<Option<keep_service::KeepClient>>,
    seerr: RwLock<Option<seerr::SeerrClient>>,
    downloads: downloads::Downloads,
    downloads_running: std::sync::atomic::AtomicBool,
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
        SignedIn {
            account,
            token,
            plex_account_token: None,
        }
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
                detail: e.to_string(),
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
                detail: e.to_string(),
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
            detail: e.to_string(),
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
                detail: e.to_string(),
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
        let downloads = downloads::Downloads::open(
            &data.join("downloads.sqlite"),
            &PathBuf::from(&config.downloads_dir),
            net::transfer_client(&ua)?,
        )
        .map_err(|e| CoreError::Storage {
            detail: e.to_string(),
        })?;
        Ok(Arc::new(Self {
            core: Arc::new(Core {
                keep: RwLock::new(None),
                seerr: RwLock::new(None),
                downloads,
                downloads_running: std::sync::atomic::AtomicBool::new(false),
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
            let mut s = c.signed_in(session)?;
            s.plex_account_token = Some(account_token);
            Ok(s)
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

    // ----- downloads -------------------------------------------------------------------------

    /// Queues the original file of an item for download (does not start it: see `run_downloads`).
    pub async fn download_item(&self, account_id: String, item_id: String) -> Result<Download> {
        self.run(|c| async move {
            let srv = c.server(&account_id)?;
            let src = srv.download_source(&item_id).await?;
            // A readable title ("Show · S1 · E2 · Title"): cached item, else ask the server
            // (episodes started from a show page are only cached as part of a list).
            let item = match c
                .store
                .cache_get::<Item>(&key(&account_id, &["item", &item_id]))?
            {
                Some(i) => Some(i.value),
                None => srv.item(&item_id).await.ok(),
            };
            let title = match item {
                Some(i) => match (i.kind, &i.series_title) {
                    (ItemKind::Episode, Some(series)) => {
                        match (i.season_number, i.episode_number) {
                            (Some(s), Some(e)) => format!("{series} · S{s} · E{e} · {}", i.title),
                            _ => format!("{series} · {}", i.title),
                        }
                    }
                    _ => i.title,
                },
                None => src.file_name.clone(),
            };
            let id = c
                .downloads
                .enqueue(downloads::DownloadRequest {
                    account_id: account_id.clone(),
                    item_id: item_id.clone(),
                    title,
                    url: src.request.url,
                    headers: src
                        .request
                        .headers
                        .into_iter()
                        .map(|h| (h.name, h.value))
                        .collect(),
                    file_name: src.file_name,
                    expected_size: src.size,
                })
                .map_err(dl_err)?;
            c.downloads.get(id).map_err(dl_err)
        })
        .await
    }

    pub fn downloads(&self) -> Result<Vec<Download>> {
        self.core.downloads.list().map_err(dl_err)
    }

    pub fn pause_download(&self, id: i64) -> Result<()> {
        self.core.downloads.pause(id).map_err(dl_err)
    }

    pub fn resume_download(&self, id: i64) -> Result<()> {
        self.core.downloads.resume(id).map_err(dl_err)
    }

    pub fn remove_download(&self, id: i64) -> Result<()> {
        self.core.downloads.remove(id).map_err(dl_err)
    }

    /// The local file of a completed download, for offline playback.
    pub fn downloaded_file(&self, account_id: String, item_id: String) -> Option<String> {
        self.core
            .downloads
            .completed_file(&account_id, &item_id)
            .ok()
            .flatten()
    }

    /// True when downloads are queued or waiting for the network (platforms schedule a rerun).
    pub fn has_pending_downloads(&self) -> bool {
        self.core
            .downloads
            .pending()
            .map(|p| !p.is_empty())
            .unwrap_or(false)
    }

    /// Runs queued downloads one after another until none is left, or stops early when the
    /// network is down (downloads left `Waiting`).
    /// Only one runner at a time; returns how many completed. Android calls this from a
    /// foreground WorkManager job, the desktop app from a background task.
    pub async fn run_downloads(&self) -> Result<u32> {
        self.run(|c| async move {
            use std::sync::atomic::Ordering;
            if c.downloads_running.swap(true, Ordering::SeqCst) {
                return Ok(0);
            }
            let mut done = 0;
            let result = async {
                loop {
                    let pending = c.downloads.pending().map_err(dl_err)?;
                    let Some(id) = pending.first().copied() else {
                        break;
                    };
                    match c.downloads.run(id).await.map_err(dl_err)? {
                        DownloadState::Completed => done += 1,
                        // The network is down: stop here, the platform reruns us when it is back.
                        DownloadState::Waiting => break,
                        // Paused, failed or no space: leave it and continue with the next one.
                        _ => {}
                    }
                }
                Ok(done)
            }
            .await;
            c.downloads_running.store(false, Ordering::SeqCst);
            result
        })
        .await
    }

    // ----- settings and UI state ------------------------------------------------------------

    pub fn get_setting(&self, key: String) -> Result<Option<String>> {
        Ok(self.core.store.setting(&key)?)
    }

    pub fn set_setting(&self, key: String, value: String) -> Result<()> {
        Ok(self.core.store.set_setting(&key, &value)?)
    }
}

/// The Seerr connection as stored locally (the session cookie is kept by the platform).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct SeerrConnection {
    pub base_url: String,
    pub user_name: String,
    pub cache_images: bool,
    /// Through the AlwyWatch server's Seerr bridge (no cookie; uses the keep connection).
    #[serde(default)]
    pub via_keep: bool,
}

/// Result of a Seerr sign-in: the platform stores `cookie` in secure storage.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SeerrSignedIn {
    pub connection: SeerrConnection,
    pub cookie: String,
}

const SEERR_KEY: &str = "seerr.connection";

impl Core {
    fn seerr(&self) -> Result<seerr::SeerrClient> {
        if let Some(c) = self.seerr.read().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(c);
        }
        // Bridged: built from the keep connection, so it follows its token and lifetime.
        let bridged = self
            .store
            .setting(SEERR_KEY)?
            .and_then(|s| serde_json::from_str::<SeerrConnection>(&s).ok())
            .is_some_and(|c| c.via_keep);
        if bridged && let Ok(keep) = self.keep() {
            return Ok(self.seerr_bridged(&keep));
        }
        Err(CoreError::NotSignedIn {
            account_id: "seerr".into(),
        })
    }

    fn seerr_bridged(&self, keep: &keep_service::KeepClient) -> seerr::SeerrClient {
        seerr::SeerrClient::bridged(
            self.http.clone(),
            &format!("{}{}", keep.base_url(), keep_service::SEERR_BRIDGE_PATH),
            vec![
                ("Authorization".into(), format!("Bearer {}", keep.token())),
                (keep_service::SERVER_HEADER.into(), keep.server_id().into()),
            ],
        )
    }

    async fn seerr_finish(
        &self,
        url: &str,
        session: Result<seerr::SeerrSession>,
    ) -> Result<SeerrSignedIn> {
        let session = session?;
        let info = seerr::SeerrAuth::new(self.http.clone()).info(url).await?;
        let connection = SeerrConnection {
            base_url: session.base_url.clone(),
            user_name: session.user.display_name.clone(),
            cache_images: info.cache_images,
            via_keep: false,
        };
        self.store.set_setting(
            SEERR_KEY,
            &serde_json::to_string(&connection).unwrap_or_default(),
        )?;
        *self.seerr.write().unwrap_or_else(|e| e.into_inner()) = Some(seerr::SeerrClient::new(
            self.http.clone(),
            &session.base_url,
            &session.cookie,
            info.cache_images,
        ));
        Ok(SeerrSignedIn {
            connection,
            cookie: session.cookie,
        })
    }
}

#[uniffi::export]
impl AlwyWatch {
    /// What a Seerr instance accepts for sign-in (Plex, Jellyfin, Emby, local).
    pub async fn seerr_info(&self, url: String) -> Result<SeerrInfo> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            Ok(seerr::SeerrAuth::new(c.http.clone()).info(&base).await?)
        })
        .await
    }

    /// `plex_account_token` comes from a plex.tv PIN sign-in (`plex_create_pin`/`plex_check_pin`).
    pub async fn seerr_sign_in_plex(
        &self,
        url: String,
        plex_account_token: String,
    ) -> Result<SeerrSignedIn> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let r = seerr::SeerrAuth::new(c.http.clone())
                .sign_in_plex(&base, &plex_account_token)
                .await
                .map_err(CoreError::from);
            c.seerr_finish(&base, r).await
        })
        .await
    }

    pub async fn seerr_sign_in_jellyfin(
        &self,
        url: String,
        username: String,
        password: String,
    ) -> Result<SeerrSignedIn> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let r = seerr::SeerrAuth::new(c.http.clone())
                .sign_in_jellyfin(&base, &username, &password)
                .await
                .map_err(CoreError::from);
            c.seerr_finish(&base, r).await
        })
        .await
    }

    pub async fn seerr_sign_in_local(
        &self,
        url: String,
        email: String,
        password: String,
    ) -> Result<SeerrSignedIn> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let r = seerr::SeerrAuth::new(c.http.clone())
                .sign_in_local(&base, &email, &password)
                .await
                .map_err(CoreError::from);
            c.seerr_finish(&base, r).await
        })
        .await
    }

    pub async fn seerr_quick_connect_start(&self, url: String) -> Result<SeerrQuickConnect> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            Ok(seerr::SeerrAuth::new(c.http.clone())
                .quick_connect_start(&base)
                .await?)
        })
        .await
    }

    /// `CoreError::Pending` until approved in a Jellyfin app.
    pub async fn seerr_quick_connect_poll(
        &self,
        url: String,
        secret: String,
    ) -> Result<SeerrSignedIn> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let r = seerr::SeerrAuth::new(c.http.clone())
                .quick_connect_poll(&base, &secret)
                .await
                .map_err(CoreError::from);
            c.seerr_finish(&base, r).await
        })
        .await
    }

    /// The stored Seerr connection, if any (no network).
    pub fn seerr_connection(&self) -> Option<SeerrConnection> {
        self.core
            .store
            .setting(SEERR_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    /// Seerr through the connected AlwyWatch server, for accounts Seerr itself cannot sign in
    /// (e.g. Jellyfin users while Seerr runs in Plex mode). The server links or creates the
    /// user's Seerr account; nothing needs storing on the device.
    pub async fn seerr_connect_bridge(&self) -> Result<SeerrConnection> {
        self.run(|c| async move {
            let keep = c.keep()?;
            let info = keep.seerr_bridge().await?;
            let conn = SeerrConnection {
                base_url: format!("{}{}", keep.base_url(), keep_service::SEERR_BRIDGE_PATH),
                user_name: info.user_name,
                cache_images: false,
                via_keep: true,
            };
            c.store
                .set_setting(SEERR_KEY, &serde_json::to_string(&conn).unwrap_or_default())?;
            *c.seerr.write().unwrap_or_else(|e| e.into_inner()) = None;
            c.store.cache_delete("seerr/requests")?;
            Ok(conn)
        })
        .await
    }

    /// Re-attaches Seerr with the cookie from secure storage. No network. Not needed for a
    /// bridged connection (`via_keep`).
    pub fn seerr_activate(&self, cookie: String) -> Result<()> {
        let conn = self.seerr_connection().ok_or(CoreError::NotFound)?;
        if conn.via_keep {
            return Ok(());
        }
        *self.core.seerr.write().unwrap_or_else(|e| e.into_inner()) =
            Some(seerr::SeerrClient::new(
                self.core.http.clone(),
                &conn.base_url,
                &cookie,
                conn.cache_images,
            ));
        Ok(())
    }

    pub fn seerr_active(&self) -> bool {
        self.core.seerr().is_ok()
    }

    pub fn seerr_sign_out(&self) -> Result<()> {
        *self.core.seerr.write().unwrap_or_else(|e| e.into_inner()) = None;
        self.core.store.remove_setting(SEERR_KEY)?;
        self.core.store.cache_delete("seerr/requests")?;
        Ok(())
    }

    pub async fn seerr_search(&self, query: String) -> Result<Vec<SeerrResult>> {
        self.run(|c| async move { Ok(c.seerr()?.search(&query, 1).await?) })
            .await
    }

    pub async fn seerr_seasons(&self, tmdb_id: i64) -> Result<Vec<SeerrSeason>> {
        self.run(|c| async move { Ok(c.seerr()?.seasons(tmdb_id).await?) })
            .await
    }

    /// Requests a movie or a show (`seasons` empty = every season).
    pub async fn seerr_request(
        &self,
        media_type: SeerrMediaType,
        tmdb_id: i64,
        seasons: Vec<u32>,
    ) -> Result<SeerrRequest> {
        self.run(|c| async move {
            let r = c.seerr()?.request(media_type, tmdb_id, &seasons).await?;
            c.store.cache_mark_stale("seerr/requests")?;
            Ok(r)
        })
        .await
    }

    pub async fn seerr_cancel(&self, request_id: i64) -> Result<()> {
        self.run(|c| async move {
            c.seerr()?.cancel(request_id).await?;
            c.store.cache_mark_stale("seerr/requests")?;
            Ok(())
        })
        .await
    }

    pub fn cached_seerr_requests(&self) -> Result<Option<Vec<SeerrRequest>>> {
        Ok(self
            .core
            .store
            .cache_get::<Vec<SeerrRequest>>("seerr/requests")?
            .map(|c| c.value))
    }

    pub async fn refresh_seerr_requests(&self, force: bool) -> Result<Vec<SeerrRequest>> {
        self.run(|c| async move {
            let client = c.seerr()?;
            let (v, _) = c
                .swr("seerr/requests", TTL_HOME, force, || async {
                    Ok(client.requests(50).await?)
                })
                .await?;
            Ok(v)
        })
        .await
    }

    /// Local file for an arbitrary public image URL (Seerr/TMDB posters), cached like the rest.
    pub async fn remote_image(&self, url: String) -> Result<String> {
        self.run(|c| async move {
            let key = format!("url\u{1f}{url}");
            if let Some(p) = c.images.get(&key)? {
                return Ok(p.to_string_lossy().into_owned());
            }
            let _permit = c
                .image_permits
                .acquire()
                .await
                .map_err(|e| CoreError::Internal {
                    detail: e.to_string(),
                })?;
            let bytes = net::send(c.http.get(&url))
                .await?
                .bytes()
                .await
                .map_err(net::Error::from)?;
            Ok(c.images.put(&key, &bytes)?.to_string_lossy().into_owned())
        })
        .await
    }

    pub fn cached_remote_image(&self, url: String) -> Option<String> {
        self.core
            .images
            .get(&format!("url\u{1f}{url}"))
            .ok()
            .flatten()
            .map(|p| p.to_string_lossy().into_owned())
    }
}

/// The keep service connection as stored locally (the token stays in secure storage: it is the
/// account's own Jellyfin token or plex.tv account token).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct KeepConnection {
    pub base_url: String,
    pub account_id: String,
    pub server_id: String,
    pub user_name: String,
    pub is_admin: bool,
}

const KEEP_KEY: &str = "keep.connection";
/// AlwyWatch server address entered on the add-server screen, connected after sign-in.
const KEEP_PRESET_KEY: &str = "keep.preset_url";
const LEAVING_KEY: &str = "keep/leaving-soon";

impl Core {
    fn keep(&self) -> Result<keep_service::KeepClient> {
        self.keep
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(CoreError::NotSignedIn {
                account_id: "keep".into(),
            })
    }

    fn keep_client(&self, base: &str, server_id: &str, token: &str) -> keep_service::KeepClient {
        keep_service::KeepClient::new(self.http.clone(), base, server_id, token)
    }

    /// Keeps the cached list in sync after a vote (so the next frame is already right).
    fn keep_patch(&self, item: &LeavingItem) {
        if let Ok(Some(mut c)) = self.store.cache_get::<Vec<LeavingItem>>(LEAVING_KEY) {
            if let Some(slot) = c.value.iter_mut().find(|i| i.key == item.key) {
                *slot = item.clone();
            }
            let _ = self.store.cache_put(LEAVING_KEY, &c.value);
        }
    }
}

#[uniffi::export]
impl AlwyWatch {
    /// Media servers a keep service serves (public), to offer them without typing URLs.
    pub async fn keep_servers(&self, url: String) -> Result<Vec<ServerEntry>> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            Ok(keep_service::KeepClient::servers(&c.http, &base).await?)
        })
        .await
    }

    /// Connects the keep service as one of the signed-in accounts. `token` is that account's
    /// Jellyfin token, or for Plex the plex.tv account token.
    pub async fn keep_connect(
        &self,
        url: String,
        account_id: String,
        token: String,
    ) -> Result<KeepConnection> {
        let account = self
            .accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or(CoreError::NotFound)?;
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let client = c.keep_client(&base, &account.server_id, &token);
            let me = client.me().await?;
            let conn = KeepConnection {
                base_url: base,
                account_id,
                server_id: account.server_id,
                user_name: me.name,
                is_admin: me.is_admin,
            };
            c.store
                .set_setting(KEEP_KEY, &serde_json::to_string(&conn).unwrap_or_default())?;
            *c.keep.write().unwrap_or_else(|e| e.into_inner()) = Some(client);
            Ok(conn)
        })
        .await
    }

    /// Remembers the AlwyWatch server a user picked a media server from, so `keep_onboard`
    /// can connect it right after they sign in.
    pub fn keep_set_preset(&self, url: String) -> Result<()> {
        let base = net::normalize_base_url(&url)?;
        Ok(self.core.store.set_setting(KEEP_PRESET_KEY, &base)?)
    }

    /// After a sign-in: connects the preset AlwyWatch server as that account (if any and not
    /// connected yet), then Seerr through it when Seerr is not set up. Seerr failing (bridge not
    /// offered) is not an error. `token` is the same one `keep_connect` takes.
    pub async fn keep_onboard(
        &self,
        account_id: String,
        token: String,
    ) -> Result<Option<KeepConnection>> {
        let Some(url) = self.core.store.setting(KEEP_PRESET_KEY)? else {
            return Ok(None);
        };
        if self.keep_connection().is_some() {
            self.core.store.remove_setting(KEEP_PRESET_KEY)?;
            return Ok(None);
        }
        let conn = self.keep_connect(url, account_id, token).await?;
        self.core.store.remove_setting(KEEP_PRESET_KEY)?;
        if self.seerr_connection().is_none() {
            let _ = self.seerr_connect_bridge().await;
        }
        Ok(Some(conn))
    }

    pub fn keep_connection(&self) -> Option<KeepConnection> {
        self.core
            .store
            .setting(KEEP_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    /// Re-attaches the keep service with the token from secure storage. No network.
    pub fn keep_activate(&self, token: String) -> Result<()> {
        let conn = self.keep_connection().ok_or(CoreError::NotFound)?;
        *self.core.keep.write().unwrap_or_else(|e| e.into_inner()) = Some(self.core.keep_client(
            &conn.base_url,
            &conn.server_id,
            &token,
        ));
        Ok(())
    }

    pub fn keep_active(&self) -> bool {
        self.core.keep().is_ok()
    }

    pub fn keep_disconnect(&self) -> Result<()> {
        *self.core.keep.write().unwrap_or_else(|e| e.into_inner()) = None;
        self.core.store.remove_setting(KEEP_KEY)?;
        self.core.store.cache_delete(LEAVING_KEY)?;
        if self.seerr_connection().is_some_and(|c| c.via_keep) {
            self.seerr_sign_out()?;
        }
        Ok(())
    }

    pub async fn keep_me(&self) -> Result<Me> {
        self.run(|c| async move { Ok(c.keep()?.me().await?) }).await
    }

    /// Leaving soon, most urgent first (cached, instant).
    pub fn cached_leaving_soon(&self) -> Result<Option<Vec<LeavingItem>>> {
        Ok(self
            .core
            .store
            .cache_get::<Vec<LeavingItem>>(LEAVING_KEY)?
            .map(|c| c.value))
    }

    pub async fn refresh_leaving_soon(&self, force: bool) -> Result<Vec<LeavingItem>> {
        self.run(|c| async move {
            let client = c.keep()?;
            let (v, _) = c
                .swr(LEAVING_KEY, TTL_LIST, force, || async {
                    Ok(client.leaving_soon().await?)
                })
                .await?;
            Ok(v)
        })
        .await
    }

    /// Most urgent items I have not voted on (swipe mode).
    pub async fn keep_triage(&self, limit: u32) -> Result<Vec<LeavingItem>> {
        self.run(|c| async move { Ok(c.keep()?.triage(limit).await?) })
            .await
    }

    pub async fn keep_vote(&self, key: String, vote: VoteRequest) -> Result<LeavingItem> {
        self.run(|c| async move {
            let item = c.keep()?.vote(&key, &vote).await?;
            c.keep_patch(&item);
            Ok(item)
        })
        .await
    }

    pub async fn keep_unvote(&self, key: String) -> Result<LeavingItem> {
        self.run(|c| async move {
            let item = c.keep()?.unvote(&key).await?;
            c.keep_patch(&item);
            Ok(item)
        })
        .await
    }

    /// Until-watched keeps of mine whose reminder is due today (for local notifications).
    pub fn keep_due_reminders(&self, now: i64) -> Vec<LeavingItem> {
        let Ok(Some(list)) = self.cached_leaving_soon() else {
            return vec![];
        };
        list.into_iter()
            .filter(|i| {
                i.my_vote.as_ref().is_some_and(|v| {
                    v.kind == VoteKind::KeepUntilWatched
                        && v.reminder_days.is_some_and(|d| {
                            let period = i64::from(d.max(1)) * 86_400;
                            let age = now - v.created_at;
                            age >= period && age % period < 86_400
                        })
                })
            })
            .collect()
    }

    pub async fn keep_admin_users(&self) -> Result<Vec<AdminUser>> {
        self.run(|c| async move { Ok(c.keep()?.admin_users().await?) })
            .await
    }

    pub async fn keep_admin_set_rights(
        &self,
        user_key: String,
        rights: Rights,
    ) -> Result<AdminUser> {
        self.run(|c| async move { Ok(c.keep()?.admin_set_rights(&user_key, &rights).await?) })
            .await
    }

    pub async fn keep_admin_countdown(
        &self,
        collection_id: i64,
        stagger_days: u32,
    ) -> Result<CountdownResult> {
        self.run(|c| async move {
            let r = c
                .keep()?
                .admin_countdown(
                    collection_id,
                    &keep_service::CountdownRequest { stagger_days },
                )
                .await?;
            c.store.cache_mark_stale(LEAVING_KEY)?;
            Ok(r)
        })
        .await
    }
}

fn dl_err(e: downloads::Error) -> CoreError {
    match e {
        downloads::Error::NotFound => CoreError::NotFound,
        other => CoreError::Invalid {
            detail: other.to_string(),
        },
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
