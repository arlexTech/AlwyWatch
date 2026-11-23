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
const TTL_SEGMENTS: i64 = 6 * 3600;
/// Two copies of a title count as the same cut when their lengths differ by at most this.
const SAME_CUT_MS: u64 = 2_000;

/// Intro and credits are what a player offers to skip: the kinds worth asking another server for.
fn missing_segment_kinds(list: &[Segment]) -> Vec<SegmentKind> {
    [SegmentKind::Intro, SegmentKind::Credits]
        .into_iter()
        .filter(|k| !list.iter().any(|s| s.kind == *k))
        .collect()
}

/// Adds from `theirs` the intro/credits kinds `own` lacks, keeping `own` sorted by start.
fn merge_missing_segments(own: &mut Vec<Segment>, theirs: Vec<Segment>) {
    let missing = missing_segment_kinds(own);
    own.extend(theirs.into_iter().filter(|s| missing.contains(&s.kind)));
    own.sort_by_key(|s| s.start_ms);
}
const TTL_PEOPLE: i64 = 24 * 3600;
const TTL_WATCHLIST: i64 = 120;
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
    /// The device profile this core runs for (several people on one device). None: the
    /// original profile.
    #[uniffi(default = None)]
    pub profile: Option<String>,
    /// Downloads database shared by the profiles of a device (None: in `data_dir`).
    #[uniffi(default = None)]
    pub downloads_db: Option<String>,
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
    /// Shows libraries: what is new for each show card ("S1 · E6 · Edenia", "New show"…) and
    /// what opening it shows (see [`media_api::recently_added`]). Cards without one (movies)
    /// show their usual text and open themselves.
    #[serde(default)]
    pub notes: Vec<RailNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct Home {
    pub resume: Vec<Item>,
    pub next_up: Vec<Item>,
    /// Like Plex's "Continue watching": in-progress titles and the next episode of shows being
    /// watched, one entry per show, most recent activity first. `user.last_played_at` holds
    /// that activity time (estimated for next episodes, which have none of their own).
    #[serde(default)]
    pub continue_watching: Vec<Item>,
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

/// The episodes around the one playing (previous/next buttons, "next episode").
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AdjacentEpisodes {
    pub previous: Option<Item>,
    pub next: Option<Item>,
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
            q.genre_id.as_deref().unwrap_or(""),
            q.collection_id.as_deref().unwrap_or(""),
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
    /// The AlwyWatch account session (no media server), see `account.rs`.
    pub(crate) account: RwLock<Option<keep_service::KeepClient>>,
    seerr: RwLock<Option<seerr::SeerrClient>>,
    downloads: downloads::Downloads,
    downloads_running: std::sync::atomic::AtomicBool,
    /// Owner of the downloads this profile asks for (see `downloads::Download::owners`).
    profile: String,
    client: ClientInfo,
    pub(crate) http: net::reqwest::Client,
    pub(crate) store: Arc<Store>,
    images: ImageCache,
    image_permits: Semaphore,
    servers: RwLock<HashMap<String, Arc<AnyServer>>>,
    playback: Mutex<HashMap<String, (Instant, PlaybackInfo)>>,
    /// New device: transfer keys waiting for their bundle, by handle.
    transfer_keys: Mutex<HashMap<String, keep_service::transfer::Recipient>>,
    /// Signed-in device: the key the user checked, by code (sealing uses exactly that one).
    transfer_peers: Mutex<HashMap<String, Vec<u8>>>,
    /// plex.tv account tokens by account (Plex's watchlist lives on plex.tv).
    plex_tv: RwLock<HashMap<String, String>>,
    /// Queued offline position snapshot per (account, item): row id in the pending queue.
    snapshots: Mutex<HashMap<(String, String), i64>>,
    /// Where a watch history copy is (check or copy), for the progress shown to the viewer.
    pub(crate) history: Mutex<crate::history::HistoryProgress>,
    /// Bumped by each synced setting change: only the last of a burst pushes them.
    pub(crate) settings_gen: std::sync::atomic::AtomicU64,
    /// Watch together: the AlwyWatch server's clock seen from here, and when it was measured.
    together_clock: Mutex<(keep_service::together::ClockSync, Option<Instant>)>,
}

impl Core {
    pub(crate) fn server(&self, account_id: &str) -> Result<Arc<AnyServer>> {
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

    /// Positions or watched marks changed: everything showing them (home, items, episode and
    /// season lists, library pages) is shown from cache, then refreshed on the next visit.
    pub(crate) fn watch_state_changed(&self, account: &str) -> Result<()> {
        self.store.cache_mark_stale(&key(account, &["home"]))?;
        for family in ["item", "episodes", "seasons", "items"] {
            self.store
                .cache_mark_stale_prefix(&format!("{}\u{1f}", key(account, &[family])))?;
        }
        Ok(())
    }

    /// Like `swr`, but offline (or on any error) falls back to whatever is cached.
    async fn swr_offline<T, F, Fut>(&self, key: &str, ttl: i64, fetch: F) -> Result<T>
    where
        T: Serialize + serde::de::DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        match self.swr(key, ttl, false, fetch).await {
            Ok((v, _)) => Ok(v),
            Err(e) => match self.store.cache_get::<T>(key)? {
                Some(c) => Ok(c.value),
                None => Err(e),
            },
        }
    }

    async fn episodes_offline(
        &self,
        account: &str,
        series: &str,
        season: &str,
    ) -> Result<Vec<Item>> {
        self.swr_offline(
            &key(account, &["episodes", series, season]),
            TTL_DETAIL,
            || async { Ok(self.server(account)?.episodes(series, season).await?) },
        )
        .await
    }

    /// Skippable intro/credits spans of an item (cached: they work offline once fetched).
    async fn segments(&self, account: &str, item_id: &str) -> Result<Vec<Segment>> {
        self.swr_offline(
            // "segments2": answers kept from before markers were borrowed are not reused.
            &key(account, &["segments2", item_id]),
            TTL_SEGMENTS,
            || async {
                let own = self.server(account)?.segments(item_id).await?;
                Ok(self.with_borrowed_segments(account, item_id, own).await)
            },
        )
        .await
    }

    /// One server often knows an intro the other has not detected (Plex analyses on its own
    /// schedule, Jellyfin's Intro Skipper on another). When this copy lacks the intro or the
    /// credits, they are taken from the same title on another signed-in server, provided both
    /// copies have the same length (another cut would put them at the wrong time).
    async fn with_borrowed_segments(
        &self,
        account: &str,
        item_id: &str,
        mut own: Vec<Segment>,
    ) -> Vec<Segment> {
        let missing = |list: &[Segment]| missing_segment_kinds(list);
        if missing(&own).is_empty() {
            return own;
        }
        let accounts: Vec<Account> = self.store.accounts().unwrap_or_default();
        let others: Vec<String> = accounts
            .into_iter()
            .map(|a| a.id)
            .filter(|id| id != account && self.server(id).is_ok())
            .collect();
        if others.is_empty() {
            return own;
        }
        let Ok(srv) = self.server(account) else {
            return own;
        };
        let (Ok(item), Ok(media)) = (
            srv.item(item_id).await,
            self.together_media(account, item_id).await,
        ) else {
            return own;
        };
        for other in others {
            let Ok(Some(other_id)) = self.together_find_on(&other, &media).await else {
                continue;
            };
            let Ok(other_srv) = self.server(&other) else {
                continue;
            };
            let same_cut = match (item.runtime_ms, other_srv.item(&other_id).await) {
                (Some(a), Ok(o)) => o.runtime_ms.is_some_and(|b| a.abs_diff(b) <= SAME_CUT_MS),
                _ => false,
            };
            if !same_cut {
                continue;
            }
            if let Ok(theirs) = other_srv.segments(&other_id).await {
                merge_missing_segments(&mut own, theirs);
            }
            if missing(&own).is_empty() {
                break;
            }
        }
        own
    }

    fn plex_tv_token(&self, account: &str) -> Option<String> {
        self.plex_tv
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(account)
            .cloned()
    }

    async fn watchlist(&self, account: &str, force: bool) -> Result<(Vec<Item>, i64)> {
        let token = self.plex_tv_token(account);
        self.swr(
            &key(account, &["watchlist"]),
            TTL_WATCHLIST,
            force,
            || async { Ok(self.server(account)?.watchlist(token.as_deref()).await?) },
        )
        .await
    }

    /// The episodes before and after this one, across seasons (specials aside).
    async fn adjacent_episodes(&self, account: &str, item_id: &str) -> Result<AdjacentEpisodes> {
        let none = AdjacentEpisodes {
            previous: None,
            next: None,
        };
        let item: Item = self
            .swr_offline(&key(account, &["item", item_id]), TTL_DETAIL, || async {
                Ok(self.server(account)?.item(item_id).await?)
            })
            .await?;
        if item.kind != ItemKind::Episode {
            return Ok(none);
        }
        let (Some(series), Some(season)) = (item.series_id, item.season_id) else {
            return Ok(none);
        };
        let eps = self.episodes_offline(account, &series, &season).await?;
        let Some(idx) = eps.iter().position(|e| e.id == item_id) else {
            return Ok(none);
        };
        let mut previous = idx.checked_sub(1).and_then(|i| eps.get(i).cloned());
        let mut next = eps.get(idx + 1).cloned();
        if previous.is_none() || next.is_none() {
            let seasons: Vec<Item> = self
                .swr_offline(&key(account, &["seasons", &series]), TTL_DETAIL, || async {
                    Ok(self.server(account)?.seasons(&series).await?)
                })
                .await
                .unwrap_or_default();
            let seasons: Vec<&Item> = seasons
                .iter()
                .filter(|s| s.season_number != Some(0) || s.id == season)
                .collect();
            if let Some(si) = seasons.iter().position(|s| s.id == season) {
                if previous.is_none()
                    && let Some(s) = si.checked_sub(1).and_then(|i| seasons.get(i))
                {
                    previous = self
                        .episodes_offline(account, &series, &s.id)
                        .await
                        .ok()
                        .and_then(|e| e.last().cloned());
                }
                if next.is_none()
                    && let Some(s) = seasons.get(si + 1)
                {
                    next = self
                        .episodes_offline(account, &series, &s.id)
                        .await
                        .ok()
                        .and_then(|e| e.first().cloned());
                }
            }
        }
        Ok(AdjacentEpisodes { previous, next })
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
                // Shows come episode by episode: more entries for ~20 shows once grouped.
                let fetch = if lib.kind == LibraryKind::Shows {
                    LATEST_SHOWS_FETCH
                } else {
                    LATEST_CARDS
                };
                let items = srv.latest(&lib.id, fetch).await;
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
                let r = media_api::recently_added(items, LATEST_CARDS as usize);
                latest.push((
                    i,
                    LibraryRail {
                        library,
                        items: r.items,
                        notes: r.notes,
                    },
                ));
            }
        }
        latest.sort_by_key(|(i, _)| *i);
        // Next episodes are dated by when their show was last really watched (an episode
        // finished), not by brief starts; undated ones go last in "continue watching".
        let mut next_up = next_up.unwrap_or_default();
        let shows: Vec<String> = next_up.iter().filter_map(|e| e.series_id.clone()).collect();
        let watched = srv.shows_last_watched(&shows).await.unwrap_or_default();
        for e in &mut next_up {
            e.user.last_played_at = e.series_id.as_ref().and_then(|s| watched.get(s).copied());
        }
        Ok(Home {
            // Resume/next-up failures (e.g. a server without on-deck) degrade to empty rails.
            resume: resume.unwrap_or_default(),
            next_up,
            libraries,
            latest: latest.into_iter().map(|(_, r)| r).collect(),
            // Filled from resume and next_up when handed out (also for older caches).
            continue_watching: Vec::new(),
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

    /// Never cached: the conversion it describes carries this selection (and on Plex asking
    /// saves the selection), so neither is shared with plain `playback_info`.
    async fn playback_info_with(
        &self,
        account: &str,
        item_id: &str,
        profile: &DeviceProfile,
        selection: &TrackSelection,
    ) -> Result<PlaybackInfo> {
        if selection.is_default() {
            return self.playback_info(account, item_id, profile).await;
        }
        Ok(self
            .server(account)?
            .playback_info_with(item_id, profile, selection)
            .await?)
    }

    async fn report_progress(&self, account: &str, report: ProgressReport) -> Result<()> {
        let result = match self.server(account) {
            Ok(srv) => srv.report_progress(&report).await.map_err(CoreError::from),
            Err(e) => Err(e),
        };
        if matches!(report.event, PlaybackEvent::Stop) {
            // Near the end: watched, as far as download deletion rules go (offline too).
            if let Some(d) = report.duration_ms.filter(|d| *d > 0)
                && report.position_ms as f64 >= d as f64 * WATCHED_AT
            {
                self.mark_watched(account, &report.item_id);
            }
            // Resume rails changed: keep showing them, but refresh on next visit.
            self.watch_state_changed(account)?;
        }
        match result {
            Ok(()) => Ok(()),
            // Offline: keep start/stop and the latest position so the server catches up later.
            Err(e) if e.is_transient() || matches!(e, CoreError::NotSignedIn { .. }) => {
                let slot = (account.to_owned(), report.item_id.clone());
                let mut snaps = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
                // One position snapshot per title: the newest replaces the previous one.
                if let Some(old) = snaps.remove(&slot) {
                    self.store.ack_progress(old)?;
                }
                if report.event == PlaybackEvent::Progress {
                    // Sent as a stop at that position: if the app is closed before playback
                    // ends, the server still gets where the user was.
                    let snap = ProgressReport {
                        event: PlaybackEvent::Stop,
                        ..report
                    };
                    snaps.insert(slot, self.store.queue_progress(account, &snap)?);
                } else {
                    self.store.queue_progress(account, &report)?;
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Drops items the user took out of "continue watching" here, unless watched since.
    fn without_hidden(&self, account: &str, items: Vec<Item>) -> Vec<Item> {
        items
            .into_iter()
            .filter(|i| {
                let hidden_at = self
                    .store
                    .setting(&hidden_key(account, &i.id))
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok());
                match hidden_at {
                    None => true,
                    // Its show (or itself) was watched after hiding it: back in the list.
                    Some(h) => i.user.last_played_at.is_some_and(|t| t > h),
                }
            })
            .collect()
    }

    /// Watched on this device (offline included): for deletion rules and "next N episodes",
    /// remembered even after the download is gone and before the server hears about it.
    fn mark_watched(&self, account: &str, item_id: &str) {
        let now = unix_now();
        let _ = self.downloads.mark_watched(account, item_id, now);
        let _ = self
            .store
            .set_setting(&watched_key(account, item_id), &now.to_string());
    }

    fn watched_here(&self, account: &str, item_id: &str) -> bool {
        self.store
            .setting(&watched_key(account, item_id))
            .ok()
            .flatten()
            .is_some()
    }

    /// Queues one item with its episode metadata; returns the download id.
    async fn enqueue_item(
        &self,
        account_id: &str,
        item_id: &str,
        known: Option<Item>,
        rule: DeleteRule,
    ) -> Result<i64> {
        let srv = self.server(account_id)?;
        let src = srv.download_source(item_id).await?;
        // Skip intro/credits must work offline too.
        let _ = self.segments(account_id, item_id).await;
        // A readable title ("Show · S1 · E2 · Title"): given, cached, else ask the server
        // (episodes started from a show page are only cached as part of a list).
        let item = match known {
            Some(i) => Some(i),
            None => match self
                .store
                .cache_get::<Item>(&key(account_id, &["item", item_id]))?
            {
                Some(i) => Some(i.value),
                None => srv.item(item_id).await.ok(),
            },
        };
        let episode = item.as_ref().and_then(|i| {
            (i.kind == ItemKind::Episode).then(|| downloads::EpisodeOf {
                series_id: i.series_id.clone().unwrap_or_default(),
                series_title: i.series_title.clone().unwrap_or_default(),
                season_number: i.season_number,
                episode_number: i.episode_number,
            })
        });
        let title = match item {
            Some(i) => match (i.kind, &i.series_title) {
                (ItemKind::Episode, Some(series)) => match (i.season_number, i.episode_number) {
                    (Some(s), Some(e)) => format!("{series} · S{s} · E{e} · {}", i.title),
                    _ => format!("{series} · {}", i.title),
                },
                _ => i.title,
            },
            None => src.file_name.clone(),
        };
        self.downloads
            .enqueue(downloads::DownloadRequest {
                account_id: account_id.to_owned(),
                item_id: item_id.to_owned(),
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
                episode,
                delete_rule: rule,
                owner: self.profile.clone(),
            })
            .map_err(dl_err)
    }

    /// Queues items not downloaded yet; returns how many were added.
    async fn enqueue_all(
        &self,
        account_id: &str,
        items: Vec<Item>,
        rule: DeleteRule,
    ) -> Result<u32> {
        let have: std::collections::HashSet<String> = self
            .downloads
            .list()
            .map_err(dl_err)?
            .into_iter()
            .filter(|d| d.account_id == account_id)
            .map(|d| d.item_id)
            .collect();
        let mut added = 0;
        for item in items {
            if have.contains(&item.id) {
                continue;
            }
            let id = item.id.clone();
            self.enqueue_item(account_id, &id, Some(item), rule).await?;
            added += 1;
        }
        Ok(added)
    }

    /// Every episode of a show in order, specials (season 0) aside.
    async fn all_episodes(&self, account_id: &str, series_id: &str) -> Result<Vec<Item>> {
        let srv = self.server(account_id)?;
        let mut out = Vec::new();
        for season in srv.seasons(series_id).await? {
            if season.season_number == Some(0) {
                continue;
            }
            out.extend(srv.episodes(series_id, &season.id).await?);
        }
        Ok(out)
    }

    /// Tops up one "next N episodes" show: the N unwatched episodes after the last watched one.
    async fn auto_download(&self, a: &AutoDownload) -> Result<u32> {
        let eps = self.all_episodes(&a.account_id, &a.series_id).await?;
        let downloads = self.downloads.list().map_err(dl_err)?;
        let watched_here = |id: &str| {
            self.watched_here(&a.account_id, id)
                || downloads.iter().any(|d| {
                    d.account_id == a.account_id && d.item_id == id && d.watched_at.is_some()
                })
        };
        // Watched on another device: the downloaded copy is watched too (for its rule).
        let now = unix_now();
        for ep in eps.iter().filter(|e| e.user.played) {
            self.downloads
                .mark_watched(&a.account_id, &ep.id, now)
                .map_err(dl_err)?;
        }
        let watched = |e: &Item| e.user.played || watched_here(&e.id);
        let start = eps.iter().rposition(watched).map_or(0, |i| i + 1);
        let next: Vec<Item> = eps[start..]
            .iter()
            .filter(|e| !watched(e))
            .take(a.count as usize)
            .cloned()
            .collect();
        self.enqueue_all(&a.account_id, next, a.rule).await
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
    pub(crate) core: Arc<Core>,
    rt: Runtime,
}

impl AlwyWatch {
    /// Runs a future on the core runtime so callers can await it from any executor
    /// (Kotlin coroutines, Slint's event loop).
    pub(crate) async fn run<T, F>(&self, f: impl FnOnce(Arc<Core>) -> F) -> Result<T>
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

    pub(crate) fn core(&self) -> Arc<Core> {
        self.core.clone()
    }

    pub(crate) fn history_progress(&self) -> crate::history::HistoryProgress {
        self.core
            .history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
            &config
                .downloads_db
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| data.join("downloads.sqlite")),
            &PathBuf::from(&config.downloads_dir),
            net::transfer_client(&ua)?,
        )
        .map_err(|e| CoreError::Storage {
            detail: e.to_string(),
        })?;
        Ok(Arc::new(Self {
            core: Arc::new(Core {
                keep: RwLock::new(None),
                account: RwLock::new(None),
                seerr: RwLock::new(None),
                downloads,
                downloads_running: std::sync::atomic::AtomicBool::new(false),
                profile: config
                    .profile
                    .unwrap_or_else(|| downloads::DEFAULT_OWNER.to_owned()),
                client: config.client,
                http,
                store,
                images,
                image_permits: Semaphore::new(6),
                servers: RwLock::new(HashMap::new()),
                playback: Mutex::new(HashMap::new()),
                snapshots: Mutex::new(HashMap::new()),
                transfer_keys: Mutex::new(HashMap::new()),
                transfer_peers: Mutex::new(HashMap::new()),
                plex_tv: RwLock::new(HashMap::new()),
                history: Mutex::new(Default::default()),
                settings_gen: std::sync::atomic::AtomicU64::new(0),
                together_clock: Mutex::new(Default::default()),
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
            // The first thing a newcomer types: say what is wrong with the address.
            JellyfinAuth::new(c.http.clone(), c.client.clone())
                .public_info(&base)
                .await
                .map_err(|e| match e {
                    net::Error::Network { .. } => CoreError::Invalid {
                        detail: format!(
                            "Can't reach {base}. Check the address and your connection."
                        ),
                    },
                    net::Error::NotFound | net::Error::Decode { .. } => CoreError::Invalid {
                        detail: "There is no Jellyfin server at this address.".into(),
                    },
                    e => e.into(),
                })
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
            let session = auth
                .sign_in(&base, &username, &password)
                .await
                .map_err(|e| match e {
                    net::Error::Unauthorized => CoreError::Invalid {
                        detail: "Wrong username or password.".into(),
                    },
                    e => e.into(),
                })?;
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

    /// Members of the signed-in account's Plex Home ("who's watching?"); one or none when the
    /// account is not a Plex Home.
    pub async fn plex_home_users(&self, account_token: String) -> Result<Vec<PlexHomeUser>> {
        self.run(|c| async move {
            Ok(PlexAuth::new(c.http.clone(), c.client.clone())
                .home_users(&account_token)
                .await?)
        })
        .await
    }

    /// Switches to a Plex Home member (`pin` when the user is protected). Returns the token to
    /// use instead of the account token (`plex_servers`, `plex_connect`). A wrong PIN is
    /// `Unauthorized`.
    pub async fn plex_switch_user(
        &self,
        account_token: String,
        uuid: String,
        pin: Option<String>,
    ) -> Result<String> {
        self.run(|c| async move {
            Ok(PlexAuth::new(c.http.clone(), c.client.clone())
                .switch_home_user(&account_token, &uuid, pin.as_deref())
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
                continue_watching: self.core.without_hidden(
                    &account_id,
                    continue_watching(&c.value.resume, &c.value.next_up),
                ),
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
                continue_watching: c
                    .without_hidden(&account_id, continue_watching(&home.resume, &home.next_up)),
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

    pub fn cached_genres(&self, account_id: String, library_id: String) -> Option<Vec<Genre>> {
        self.core
            .store
            .cache_get::<Vec<Genre>>(&key(&account_id, &["genres", &library_id]))
            .ok()
            .flatten()
            .map(|c| c.value)
    }

    /// A library's genres ("categories"), cached.
    pub async fn refresh_genres(
        &self,
        account_id: String,
        library_id: String,
        force: bool,
    ) -> Result<Vec<Genre>> {
        self.run(|c| async move {
            let srv = c.server(&account_id)?;
            let k = key(&account_id, &["genres", &library_id]);
            Ok(c.swr(&k, TTL_LIST, force, || async {
                Ok(srv.genres(&library_id).await?)
            })
            .await?
            .0)
        })
        .await
    }

    pub fn cached_collections(&self, account_id: String, library_id: String) -> Option<Vec<Item>> {
        self.core
            .store
            .cache_get::<Vec<Item>>(&key(&account_id, &["collections", &library_id]))
            .ok()
            .flatten()
            .map(|c| c.value)
    }

    /// A library's collections, cached; open one with `ItemQuery::collection_id`.
    pub async fn refresh_collections(
        &self,
        account_id: String,
        library_id: String,
        force: bool,
    ) -> Result<Vec<Item>> {
        self.run(|c| async move {
            let srv = c.server(&account_id)?;
            let k = key(&account_id, &["collections", &library_id]);
            Ok(c.swr(&k, TTL_LIST, force, || async {
                Ok(srv.collections(&library_id).await?)
            })
            .await?
            .0)
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

    /// Movies and shows matching `query`; matching episodes or seasons count as their show.
    pub async fn search(&self, account_id: String, query: String, limit: u32) -> Result<Vec<Item>> {
        self.run(|c| async move {
            Ok(shows_not_episodes(
                c.server(&account_id)?.search(&query, limit).await?,
            ))
        })
        .await
    }

    /// Takes an item out of "continue watching": on the server when it can (Plex; Jellyfin
    /// for titles in progress), else hidden on this device until it is watched again.
    pub async fn remove_from_continue_watching(
        &self,
        account_id: String,
        item_id: String,
    ) -> Result<()> {
        self.run(|c| async move {
            let done = c
                .server(&account_id)?
                .remove_from_continue_watching(&item_id)
                .await?;
            if !done {
                c.store
                    .set_setting(&hidden_key(&account_id, &item_id), &unix_now().to_string())?;
            }
            c.store.cache_mark_stale(&key(&account_id, &["home"]))?;
            Ok(())
        })
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
            if played {
                c.mark_watched(&account_id, &item_id);
            }
            c.watch_state_changed(&account_id)?;
            tokio::spawn(c.clone().mirror(account_id, item_id));
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

    /// Playback sources whose conversion (`MediaSource::transcode`) carries the chosen audio
    /// and subtitles (see `conversion_tracks`). For playing the conversion only: direct play
    /// switches tracks in the player. On Plex a chosen track is saved as the account's choice
    /// for the file, as Plex's own apps do. The default selection is `playback_info`.
    pub async fn playback_info_with(
        &self,
        account_id: String,
        item_id: String,
        profile: DeviceProfile,
        selection: TrackSelection,
    ) -> Result<PlaybackInfo> {
        self.run(|c| async move {
            c.playback_info_with(&account_id, &item_id, &profile, &selection)
                .await
        })
        .await
    }

    /// Skippable intro and credits of an item; empty when the server detected none.
    pub async fn segments(&self, account_id: String, item_id: String) -> Result<Vec<Segment>> {
        self.run(|c| async move { c.segments(&account_id, &item_id).await })
            .await
    }

    /// The plex.tv account token of a Plex account (from its PIN sign-in), for the watchlist.
    pub fn set_plex_account_token(&self, account_id: String, token: String) {
        self.core
            .plex_tv
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account_id, token);
    }

    /// Cast and crew of a title (cached a day; works offline from cache).
    pub async fn people(&self, account_id: String, item_id: String) -> Result<Vec<Person>> {
        self.run(|c| async move {
            c.swr_offline(
                &key(&account_id, &["people", &item_id]),
                TTL_PEOPLE,
                || async { Ok(c.server(&account_id)?.people(&item_id).await?) },
            )
            .await
        })
        .await
    }

    /// Trailers of a movie or show (cached a day).
    pub async fn trailers(&self, account_id: String, item_id: String) -> Result<Vec<Trailer>> {
        self.run(|c| async move {
            c.swr_offline(
                &key(&account_id, &["trailers", &item_id]),
                TTL_PEOPLE,
                || async { Ok(c.server(&account_id)?.trailers(&item_id).await?) },
            )
            .await
        })
        .await
    }

    /// The show's theme song to play quietly on its page (not cached: it carries credentials).
    pub async fn theme_song(
        &self,
        account_id: String,
        item_id: String,
    ) -> Result<Option<HttpRequest>> {
        self.run(|c| async move { Ok(c.server(&account_id)?.theme_song(&item_id).await?) })
            .await
    }

    pub fn cached_people(&self, account_id: String, item_id: String) -> Option<Vec<Person>> {
        self.core
            .store
            .cache_get::<Vec<Person>>(&key(&account_id, &["people", &item_id]))
            .ok()
            .flatten()
            .map(|c| c.value)
    }

    /// The account's watchlist on its server (Jellyfin favorites, Plex's plex.tv watchlist).
    pub fn cached_watchlist(&self, account_id: String) -> Option<ItemsResult> {
        self.cached_list(&key(&account_id, &["watchlist"]))
            .ok()
            .flatten()
    }

    pub async fn refresh_watchlist(&self, account_id: String, force: bool) -> Result<ItemsResult> {
        self.run(|c| async move {
            let (items, at) = c.watchlist(&account_id, force).await?;
            Ok(ItemsResult {
                items,
                fetched_at: at,
            })
        })
        .await
    }

    /// Whether a title is on the watchlist, from the cache (no network).
    pub fn is_watchlisted(&self, account_id: String, item_id: String) -> bool {
        let listed = self
            .cached_watchlist(account_id.clone())
            .is_some_and(|w| w.items.iter().any(|i| i.id == item_id));
        listed
            || self
                .cached_item(account_id, item_id)
                .ok()
                .flatten()
                .is_some_and(|i| i.item.user.favorite)
    }

    /// Adds to or removes from the watchlist; the cached list follows at once.
    pub async fn set_watchlisted(
        &self,
        account_id: String,
        item_id: String,
        on: bool,
    ) -> Result<()> {
        self.run(|c| async move {
            let token = c.plex_tv_token(&account_id);
            c.server(&account_id)?
                .set_watchlisted(&item_id, on, token.as_deref())
                .await?;
            let k = key(&account_id, &["watchlist"]);
            let mut list: Vec<Item> = c
                .store
                .cache_get::<Vec<Item>>(&k)?
                .map(|x| x.value)
                .unwrap_or_default();
            list.retain(|i| i.id != item_id);
            let item_key = key(&account_id, &["item", &item_id]);
            if let Some(mut cached) = c.store.cache_get::<Item>(&item_key)?.map(|x| x.value) {
                cached.user.favorite = on;
                c.store.cache_put(&item_key, &cached)?;
                if on {
                    list.insert(0, cached);
                }
            }
            c.store.cache_put(&k, &list)?;
            c.store.cache_mark_stale(&k)?;
            Ok(())
        })
        .await
    }

    /// The previous and next episodes of an episode, across seasons. Works offline from cache.
    pub async fn adjacent_episodes(
        &self,
        account_id: String,
        item_id: String,
    ) -> Result<AdjacentEpisodes> {
        self.run(|c| async move { c.adjacent_episodes(&account_id, &item_id).await })
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
        self.run(|c| async move {
            let stop = report.event == PlaybackEvent::Stop;
            let item = report.item_id.clone();
            c.report_progress(&account_id, report).await?;
            // Synced servers: the same state there, without waiting for the next run.
            if stop {
                tokio::spawn(c.clone().mirror(account_id, item));
            }
            Ok(())
        })
        .await
    }

    /// Sends queued progress reports (call when the network comes back). Returns how many.
    /// Checks that an account's server answers (a light authenticated call), giving up after
    /// [`PING_TIMEOUT`]. On success, also sends progress queued while offline.
    pub async fn ping(&self, account_id: String) -> Result<()> {
        self.run(|c| async move {
            let srv = c.server(&account_id)?;
            match tokio::time::timeout(PING_TIMEOUT, srv.libraries()).await {
                Ok(r) => {
                    r?;
                }
                Err(_) => {
                    return Err(CoreError::Network {
                        detail: format!("no answer after {} seconds", PING_TIMEOUT.as_secs()),
                    });
                }
            }
            let _ = c.flush_pending().await;
            Ok(())
        })
        .await
    }

    pub async fn flush_pending_progress(&self) -> Result<u32> {
        self.run(|c| async move { c.flush_pending().await }).await
    }

    // ----- downloads -------------------------------------------------------------------------

    /// Queues the original file of an item for download (does not start it: see `run_downloads`).
    /// `rule`: when it deletes itself (the default rule when `None`).
    pub async fn download_item(
        &self,
        account_id: String,
        item_id: String,
        rule: Option<DeleteRule>,
    ) -> Result<Download> {
        let rule = rule.unwrap_or_else(|| self.default_delete_rule());
        self.run(|c| async move {
            let id = c.enqueue_item(&account_id, &item_id, None, rule).await?;
            c.downloads.get(id).map_err(dl_err)
        })
        .await
    }

    /// Queues every episode of a season; returns how many were added.
    pub async fn download_season(
        &self,
        account_id: String,
        series_id: String,
        season_id: String,
        rule: Option<DeleteRule>,
    ) -> Result<u32> {
        let rule = rule.unwrap_or_else(|| self.default_delete_rule());
        self.run(|c| async move {
            let eps = c
                .server(&account_id)?
                .episodes(&series_id, &season_id)
                .await?;
            c.enqueue_all(&account_id, eps, rule).await
        })
        .await
    }

    /// Queues every episode of a show (specials aside); returns how many were added.
    pub async fn download_series(
        &self,
        account_id: String,
        series_id: String,
        rule: Option<DeleteRule>,
    ) -> Result<u32> {
        let rule = rule.unwrap_or_else(|| self.default_delete_rule());
        self.run(|c| async move {
            let eps = c.all_episodes(&account_id, &series_id).await?;
            c.enqueue_all(&account_id, eps, rule).await
        })
        .await
    }

    /// "Keep the next `count` episodes downloaded": downloads the next unwatched episodes now
    /// and, at each `sync_downloads`, replaces the watched ones (deleted by `rule`). `count = 0`
    /// stops it (downloads already there stay).
    pub async fn set_auto_download(
        &self,
        account_id: String,
        series_id: String,
        series_title: String,
        count: u32,
        rule: DeleteRule,
    ) -> Result<u32> {
        let mut all = self.auto_downloads();
        all.retain(|a| !(a.account_id == account_id && a.series_id == series_id));
        let entry = AutoDownload {
            account_id,
            series_id,
            series_title,
            count,
            rule,
        };
        if count > 0 {
            all.push(entry.clone());
        }
        self.core.store.set_setting(
            AUTO_DOWNLOADS_KEY,
            &serde_json::to_string(&all).unwrap_or_default(),
        )?;
        if count == 0 {
            return Ok(0);
        }
        self.run(|c| async move { c.auto_download(&entry).await })
            .await
    }

    /// Shows set to "keep the next N episodes".
    pub fn auto_downloads(&self) -> Vec<AutoDownload> {
        self.core
            .store
            .setting(AUTO_DOWNLOADS_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn auto_download_of(&self, account_id: String, series_id: String) -> Option<AutoDownload> {
        self.auto_downloads()
            .into_iter()
            .find(|a| a.account_id == account_id && a.series_id == series_id)
    }

    /// The rule given to new downloads when none is chosen.
    pub fn default_delete_rule(&self) -> DeleteRule {
        self.core
            .store
            .setting(DELETE_RULE_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn set_default_delete_rule(&self, rule: DeleteRule) -> Result<()> {
        Ok(self.core.store.set_setting(
            DELETE_RULE_KEY,
            &serde_json::to_string(&rule).unwrap_or_default(),
        )?)
    }

    pub fn set_download_rule(&self, id: i64, rule: DeleteRule) -> Result<()> {
        self.core
            .downloads
            .set_delete_rule(id, rule)
            .map_err(dl_err)
    }

    /// Housekeeping, offline-safe: deletes downloads whose rule is due, then (when the servers
    /// answer) tops up "next N episodes" shows. Returns how many downloads were queued; the
    /// platform then runs `run_downloads` if any are pending.
    pub async fn sync_downloads(&self) -> Result<u32> {
        let autos = self.auto_downloads();
        self.run(|c| async move {
            c.downloads
                .apply_rules(unix_now(), &c.profile)
                .map_err(dl_err)?;
            let mut queued = 0;
            for a in &autos {
                // One show failing (offline, removed) does not stop the others.
                queued += c.auto_download(a).await.unwrap_or(0);
            }
            c.downloads
                .apply_rules(unix_now(), &c.profile)
                .map_err(dl_err)?;
            Ok(queued)
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

    /// This profile no longer wants the download; the file goes when no other profile does.
    pub fn remove_download(&self, id: i64) -> Result<()> {
        self.core
            .downloads
            .release(id, &self.core.profile)
            .map(|_| ())
            .map_err(dl_err)
    }

    /// The profile this core runs for (to tell its downloads from others').
    pub fn profile(&self) -> String {
        self.core.profile.clone()
    }

    /// A profile was removed from the device: releases its downloads.
    pub fn release_profile_downloads(&self, profile: String) -> Result<()> {
        self.core.downloads.release_all(&profile).map_err(dl_err)
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

    /// What the others may do in rooms I create (Settings → Watch together; follows me to my
    /// other devices). Everything by default.
    pub fn together_rights(&self) -> keep_service::together::TogetherRights {
        let on = |k: &str| self.core.store.setting(k).ok().flatten().as_deref() != Some("0");
        keep_service::together::TogetherRights {
            change_title: on(TOGETHER_TITLE_KEY),
            play_pause: on(TOGETHER_PLAY_KEY),
            seek: on(TOGETHER_SEEK_KEY),
        }
    }

    /// Applies to rooms created from now on.
    pub fn set_together_rights(
        &self,
        rights: keep_service::together::TogetherRights,
    ) -> Result<()> {
        let v = |b: bool| if b { "1" } else { "0" }.to_owned();
        self.set_setting(TOGETHER_TITLE_KEY.into(), v(rights.change_title))?;
        self.set_setting(TOGETHER_PLAY_KEY.into(), v(rights.play_pause))?;
        self.set_setting(TOGETHER_SEEK_KEY.into(), v(rights.seek))
    }

    pub fn get_setting(&self, key: String) -> Result<Option<String>> {
        Ok(self.core.store.setting(&key)?)
    }

    pub fn set_setting(&self, key: String, value: String) -> Result<()> {
        if self.core.store.setting(&key)?.as_deref() == Some(value.as_str()) {
            return Ok(());
        }
        self.core.store.set_setting(&key, &value)?;
        // Settings that follow the viewer: shared with their other devices.
        if crate::settings_sync::synced(&key) {
            let _rt = self.rt.enter();
            self.core.setting_changed(&key);
        }
        Ok(())
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
    /// Bridged as this account rather than the keep connection's (Seerr knows one media
    /// server type: a Plex user is already a Seerr user when Seerr runs in Plex mode).
    #[serde(default)]
    pub bridge_account: Option<String>,
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
            bridge_account: None,
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
                bridge_account: None,
            };
            c.store
                .set_setting(SEERR_KEY, &serde_json::to_string(&conn).unwrap_or_default())?;
            *c.seerr.write().unwrap_or_else(|e| e.into_inner()) = None;
            c.store.cache_delete("seerr/requests")?;
            c.store.cache_delete("seerr/requests-all")?;
            Ok(conn)
        })
        .await
    }

    /// Seerr through the AlwyWatch server, signed in as `account_id` (its Jellyfin token or
    /// Plex account token). Fails when the server does not offer Seerr for that account.
    pub async fn seerr_connect_bridge_as(
        &self,
        account_id: String,
        token: String,
    ) -> Result<SeerrConnection> {
        let account = self
            .accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or(CoreError::NotFound)?;
        self.run(|c| async move {
            let base = c.keep()?.base_url().to_owned();
            let client = c.keep_client(&base, &account.server_id, &token);
            let info = client.seerr_bridge().await?;
            let conn = SeerrConnection {
                base_url: format!("{base}{}", keep_service::SEERR_BRIDGE_PATH),
                user_name: info.user_name,
                cache_images: false,
                via_keep: true,
                bridge_account: Some(account_id),
            };
            c.store
                .set_setting(SEERR_KEY, &serde_json::to_string(&conn).unwrap_or_default())?;
            *c.seerr.write().unwrap_or_else(|e| e.into_inner()) = Some(c.seerr_bridged(&client));
            c.store.cache_delete("seerr/requests")?;
            c.store.cache_delete("seerr/requests-all")?;
            Ok(conn)
        })
        .await
    }

    /// Re-attaches a Seerr bridge made with `seerr_connect_bridge_as`, with that account's token
    /// from secure storage. No network.
    pub fn seerr_activate_bridge(&self, token: String) -> Result<()> {
        let conn = self.seerr_connection().ok_or(CoreError::NotFound)?;
        let account_id = conn.bridge_account.ok_or(CoreError::NotFound)?;
        let account = self
            .accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or(CoreError::NotFound)?;
        let base = conn
            .base_url
            .strip_suffix(keep_service::SEERR_BRIDGE_PATH)
            .unwrap_or(&conn.base_url)
            .to_owned();
        let client = self.core.keep_client(&base, &account.server_id, &token);
        *self.core.seerr.write().unwrap_or_else(|e| e.into_inner()) =
            Some(self.core.seerr_bridged(&client));
        Ok(())
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
        self.core.store.cache_delete("seerr/requests-all")?;
        Ok(())
    }

    pub async fn seerr_search(&self, query: String) -> Result<Vec<SeerrResult>> {
        self.run(|c| async move { Ok(c.seerr()?.search(&query, 1).await?) })
            .await
    }

    /// A title's page (details, state, cast, saga), for titles not on the servers.
    pub async fn seerr_title(
        &self,
        media_type: SeerrMediaType,
        tmdb_id: i64,
    ) -> Result<SeerrDetails> {
        self.run(|c| async move { Ok(c.seerr()?.title(media_type, tmdb_id).await?) })
            .await
    }

    /// A saga (TMDB collection) with its films.
    pub async fn seerr_collection(&self, id: i64) -> Result<SeerrCollection> {
        self.run(|c| async move { Ok(c.seerr()?.collection(id).await?) })
            .await
    }

    pub async fn seerr_seasons(&self, tmdb_id: i64) -> Result<Vec<SeerrSeason>> {
        self.run(|c| async move { Ok(c.seerr()?.seasons(tmdb_id).await?) })
            .await
    }

    /// Every season of a show (specials included) with its state on the server, in requests
    /// and downloads. `season_plan(info, have)` turns it into what can be requested.
    pub async fn seerr_season_info(&self, tmdb_id: i64) -> Result<Vec<SeerrSeasonInfo>> {
        self.run(|c| async move { Ok(c.seerr()?.season_info(tmdb_id).await?) })
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
            c.store.cache_mark_stale("seerr/requests-all")?;
            Ok(r)
        })
        .await
    }

    pub async fn seerr_cancel(&self, request_id: i64) -> Result<()> {
        self.run(|c| async move {
            c.seerr()?.cancel(request_id).await?;
            c.store.cache_mark_stale("seerr/requests")?;
            c.store.cache_mark_stale("seerr/requests-all")?;
            Ok(())
        })
        .await
    }

    /// `all`: everyone's requests (Seerr users allowed to see them), else the user's own.
    pub fn cached_seerr_requests(&self, all: bool) -> Result<Option<Vec<SeerrRequest>>> {
        Ok(self
            .core
            .store
            .cache_get::<Vec<SeerrRequest>>(requests_key(all))?
            .map(|c| c.value))
    }

    pub async fn refresh_seerr_requests(
        &self,
        force: bool,
        all: bool,
    ) -> Result<Vec<SeerrRequest>> {
        self.run(|c| async move {
            let client = c.seerr()?;
            let (v, _) = c
                .swr(requests_key(all), TTL_HOME, force, || async {
                    Ok(client.requests(50, all).await?)
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

/// New device: what to show while waiting for another device to approve.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TransferCode {
    /// Six digits to type on the signed-in device.
    pub code: String,
    /// Four digits that must match on both screens.
    pub check: String,
    /// To wait with (`transfer_receive`).
    pub handle: String,
    pub expires_in: u64,
}

/// New device: what an approval brought. `bundle`: another device's accounts and settings
/// (to restore as before); `account_token`: a session of the approver's AlwyWatch account
/// (store it and call `account_use_token`). Either may be missing.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TransferReceived {
    pub bundle: Option<String>,
    pub account_token: Option<String>,
}

/// Signed-in device: who asks for the accounts.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TransferRequest {
    pub device: String,
    pub check: String,
}

/// Accounts, connections and chosen settings, to set up another device (tokens are added by
/// the platform, which keeps them in its secure storage).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigBundle {
    accounts: Vec<Account>,
    keep: Option<KeepConnection>,
    seerr: Option<SeerrConnection>,
    settings: Vec<(String, String)>,
}

/// A show set to "keep the next `count` episodes downloaded".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct AutoDownload {
    pub account_id: String,
    pub series_id: String,
    pub series_title: String,
    pub count: u32,
    pub rule: DeleteRule,
}

const AUTO_DOWNLOADS_KEY: &str = "downloads.auto";
const DELETE_RULE_KEY: &str = "downloads.delete_rule";
/// Played this far, an episode counts as watched (credits).
const WATCHED_AT: f64 = 0.9;

const PING_TIMEOUT: Duration = Duration::from_secs(10);
const KEEP_KEY: &str = "keep.connection";
/// AlwyWatch server address entered on the add-server screen, connected after sign-in.
const KEEP_PRESET_KEY: &str = "keep.preset_url";
const LEAVING_KEY: &str = "keep/leaving-soon";
/// Watch together: what others may do in my rooms ("0": no; else yes). Synced settings.
pub(crate) const TOGETHER_TITLE_KEY: &str = "together.others_title";
pub(crate) const TOGETHER_PLAY_KEY: &str = "together.others_play";
pub(crate) const TOGETHER_SEEK_KEY: &str = "together.others_seek";
const SERVICES_KEY: &str = "keep/services";

impl Core {
    pub(crate) fn keep(&self) -> Result<keep_service::KeepClient> {
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
            } else {
                // A title in no deletion list, looked up or voted on from its page: kept here
                // for that page (the lists leave it out unless it is kept).
                c.value.push(item.clone());
            }
            let _ = self.store.cache_put(LEAVING_KEY, &c.value);
        }
    }
}

use keep_service::leaving_or_kept;

#[uniffi::export]
impl AlwyWatch {
    // ----- moving to a new device -------------------------------------------------------------

    /// New device: registers a one-time key with the AlwyWatch server at `url` and returns the
    /// code to type on a signed-in device. No sign-in needed.
    pub async fn transfer_start(&self, url: String, device: String) -> Result<TransferCode> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let key = keep_service::transfer::Recipient::new().map_err(internal)?;
            let started = keep_service::KeepClient::transfer_start(
                &c.http,
                &base,
                &keep_service::transfer::TransferStart {
                    public_key: keep_service::transfer::to_hex(key.public_key()),
                    device,
                },
            )
            .await?;
            let check = keep_service::transfer::check_number(key.public_key());
            c.transfer_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(started.handle.clone(), key);
            Ok(TransferCode {
                code: started.code,
                check,
                handle: started.handle,
                expires_in: started.expires_in,
            })
        })
        .await
    }

    /// New device: what the approval brought, once approved (None while waiting). Opening it
    /// consumes the key.
    pub async fn transfer_receive(
        &self,
        url: String,
        handle: String,
    ) -> Result<Option<TransferReceived>> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let Some(sealed) =
                keep_service::KeepClient::transfer_poll(&c.http, &base, &handle).await?
            else {
                return Ok(None);
            };
            let key = c
                .transfer_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&handle)
                .ok_or(CoreError::NotFound)?;
            let bundle = if sealed.sealed.is_empty() {
                None
            } else {
                let bytes = keep_service::transfer::from_hex(&sealed.sealed).ok_or_else(|| {
                    CoreError::Invalid {
                        detail: "corrupted transfer".into(),
                    }
                })?;
                let plain = key.open(&bytes).map_err(|e| CoreError::Invalid {
                    detail: e.to_string(),
                })?;
                Some(String::from_utf8(plain).map_err(|_| CoreError::Invalid {
                    detail: "corrupted transfer".into(),
                })?)
            };
            Ok(Some(TransferReceived {
                bundle,
                account_token: sealed.account_token,
            }))
        })
        .await
    }

    /// Signed-in device: who asks with this code, and the check number to compare.
    pub async fn transfer_peer(&self, code: String) -> Result<TransferRequest> {
        self.run(|c| async move {
            let peer = c.person_client()?.transfer_peer(code.trim()).await?;
            let key = keep_service::transfer::from_hex(&peer.public_key).ok_or_else(|| {
                CoreError::Invalid {
                    detail: "invalid key".into(),
                }
            })?;
            let check = keep_service::transfer::check_number(&key);
            c.transfer_peers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(code.trim().to_owned(), key);
            Ok(TransferRequest {
                device: peer.device,
                check,
            })
        })
        .await
    }

    /// Signed-in device: seals `bundle` to the key checked with `transfer_peer` and sends it.
    pub async fn transfer_send(&self, code: String, bundle: String) -> Result<()> {
        self.run(|c| async move {
            let key = c
                .transfer_peers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(code.trim())
                .ok_or(CoreError::NotFound)?;
            let sealed = keep_service::transfer::seal(&key, bundle.as_bytes()).map_err(internal)?;
            c.keep()?
                .transfer_send(
                    code.trim(),
                    &keep_service::transfer::TransferSealed {
                        sealed: keep_service::transfer::to_hex(&sealed),
                        account_token: None,
                    },
                )
                .await?;
            Ok(())
        })
        .await
    }

    /// Accounts, AlwyWatch server and Seerr connections, and the given settings, as JSON.
    pub fn export_config(&self, setting_keys: Vec<String>) -> Result<String> {
        let settings = setting_keys
            .into_iter()
            .filter_map(|k| self.core.store.setting(&k).ok().flatten().map(|v| (k, v)))
            .collect();
        let bundle = ConfigBundle {
            accounts: self.accounts()?,
            keep: self.keep_connection(),
            seerr: self.seerr_connection(),
            settings,
        };
        serde_json::to_string(&bundle).map_err(internal)
    }

    /// Restores what `export_config` produced; returns the account ids (the platform then
    /// stores their tokens and activates them as at startup).
    pub fn import_config(&self, json: String) -> Result<Vec<String>> {
        let b: ConfigBundle = serde_json::from_str(&json).map_err(|e| CoreError::Invalid {
            detail: format!("unreadable transfer: {e}"),
        })?;
        for a in &b.accounts {
            self.core.store.put_account(&a.id, a)?;
        }
        if let Some(k) = &b.keep {
            self.core
                .store
                .set_setting(KEEP_KEY, &serde_json::to_string(k).map_err(internal)?)?;
        }
        if let Some(s) = &b.seerr {
            self.core
                .store
                .set_setting(SEERR_KEY, &serde_json::to_string(s).map_err(internal)?)?;
        }
        for (k, v) in &b.settings {
            self.core.store.set_setting(k, v)?;
        }
        Ok(b.accounts.into_iter().map(|a| a.id).collect())
    }

    /// Media servers a keep service serves (public), to offer them without typing URLs.
    pub async fn keep_servers(&self, url: String) -> Result<Vec<ServerEntry>> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            Ok(keep_service::KeepClient::servers(&c.http, &base).await?)
        })
        .await
    }

    /// Connects the keep service as one of the signed-in accounts (or switches which account
    /// syncs with it). `token` is that account's Jellyfin token, or for Plex the plex.tv
    /// account token.
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
        let conn = self
            .run(|c| async move {
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
                // Another account or server: its list and votes differ.
                c.store.cache_delete(LEAVING_KEY)?;
                Ok(conn)
            })
            .await?;
        // Seerr through the bridge acts as the synced account: follow the switch.
        if self.seerr_connection().is_some_and(|s| s.via_keep) {
            self.seerr_sign_out()?;
            let _ = self.seerr_connect_bridge().await;
        }
        Ok(conn)
    }

    /// Forgets the AlwyWatch server picked on the add-server screen (the user went back and
    /// chose to sign in directly).
    pub fn keep_clear_preset(&self) -> Result<()> {
        Ok(self.core.store.remove_setting(KEEP_PRESET_KEY)?)
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
        self.run(|c| async move {
            let me = c.keep()?.me().await?;
            let _ = c.store.cache_put(SERVICES_KEY, &me.services);
            Ok(me)
        })
        .await
    }

    /// What works behind the AlwyWatch server (Leaving soon: Maintainerr; Requests through it:
    /// Seerr), as last seen; everything until known. Apps hide the pages that don't work.
    pub fn keep_services(&self) -> keep_service::Services {
        self.core
            .store
            .cache_get::<keep_service::Services>(SERVICES_KEY)
            .ok()
            .flatten()
            .map(|c| c.value)
            .unwrap_or_default()
    }

    /// Asks the AlwyWatch server again (see [`Self::keep_services`]).
    pub async fn refresh_keep_services(&self) -> Result<keep_service::Services> {
        Ok(self.keep_me().await?.services)
    }

    /// The sign-ins (Plex user, Jellyfin user…) that are my one AlwyWatch account; a single
    /// entry means nothing is linked. See `docs/identity-apps.md`.
    pub async fn keep_links(&self) -> Result<Vec<LinkedIdentity>> {
        self.run(|c| async move { Ok(c.keep()?.links().await?) })
            .await
    }

    /// Makes another account signed in on this device (`account_id`, with the same token the
    /// app would give `keep_connect` for it: the plex.tv token for Plex) part of my AlwyWatch
    /// account: both sign-ins then share name, avatar, votes and rooms. Cannot be undone as
    /// such (unlinking starts that sign-in over, empty). Read `keep_me` again afterwards.
    pub async fn keep_link(
        &self,
        account_id: String,
        token: String,
    ) -> Result<Vec<LinkedIdentity>> {
        let account = self
            .accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or(CoreError::NotFound)?;
        self.run(|c| async move { Ok(c.keep()?.link(&account.server_id, &token).await?) })
            .await
    }

    /// Takes a sign-in out of my AlwyWatch account (not the main one): it becomes an
    /// account of its own again, empty.
    pub async fn keep_unlink(&self, user_key: String) -> Result<Vec<LinkedIdentity>> {
        self.run(|c| async move { Ok(c.keep()?.unlink(&user_key).await?) })
            .await
    }

    /// Leaving soon, most urgent first (cached, instant).
    pub fn cached_leaving_soon(&self) -> Result<Option<Vec<LeavingItem>>> {
        Ok(self.leaving_cache()?.map(leaving_or_kept))
    }

    /// Everything the server answered: also the titles with a choice of mine that are in no
    /// deletion list (the lists leave them out, a title's page shows the choice).
    fn leaving_cache(&self) -> Result<Option<Vec<LeavingItem>>> {
        Ok(self
            .core
            .store
            .cache_get::<Vec<LeavingItem>>(LEAVING_KEY)?
            .map(|c| c.value))
    }

    /// The keep entry of a movie or show that is in no deletion list (asked from the server,
    /// which finds it in the library deletions are managed for), to keep it ahead with
    /// [`Self::keep_vote`]. From then on [`Self::keep_status_of`] knows it. Not found there:
    /// `NotFound`.
    pub async fn keep_entry(&self, item: Item) -> Result<LeavingItem> {
        self.run(|c| async move {
            let title = keep_service::TitleLookup {
                kind: match item.kind {
                    ItemKind::Movie => "movie",
                    ItemKind::Series => "show",
                    _ => "other",
                }
                .into(),
                item_id: item.id,
                title: item.title,
                tmdb_id: item.provider_ids.tmdb,
                tvdb_id: item.provider_ids.tvdb,
            };
            let entry = c.keep()?.keep_entry(&title).await?;
            c.keep_patch(&entry);
            Ok(entry)
        })
        .await
    }

    /// The keep status of a movie or show page (from the cached Leaving soon list, instant).
    /// None: not in the list, so not scheduled for deletion (or the list isn't loaded yet).
    pub fn keep_status_of(&self, item: Item) -> Option<LeavingItem> {
        let kind = match item.kind {
            ItemKind::Movie => "movie",
            ItemKind::Series => "show",
            _ => return None,
        };
        let list = self.leaving_cache().ok().flatten()?;
        keep_service::leaving_entry(
            &list,
            &item.id,
            kind,
            item.provider_ids.tmdb.as_deref(),
            item.provider_ids.tvdb.as_deref(),
        )
        .cloned()
    }

    pub async fn refresh_leaving_soon(&self, force: bool) -> Result<Vec<LeavingItem>> {
        self.run(|c| async move {
            let client = c.keep()?;
            let (v, _) = c
                .swr(LEAVING_KEY, TTL_LIST, force, || async {
                    Ok(client.leaving_soon().await?)
                })
                .await?;
            Ok(leaving_or_kept(v))
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

    // ----- my AlwyWatch profile: display name, avatar, welcome step ---------------------------

    /// Chooses the display name others see (trimmed, inner spaces collapsed, 1 to 32
    /// characters; check it first with [`display_name_problem`]). Returns my updated `Me`.
    pub async fn keep_set_display_name(&self, name: String) -> Result<Me> {
        self.run(|c| async move { Ok(c.keep()?.set_display_name(&name).await?) })
            .await
    }

    /// Back to my media-server name (`Me::media_name`).
    pub async fn keep_reset_display_name(&self) -> Result<Me> {
        self.run(|c| async move { Ok(c.keep()?.reset_display_name().await?) })
            .await
    }

    /// Chooses a built-in avatar (an id from [`avatar_presets`]).
    pub async fn keep_set_avatar_preset(&self, preset: String) -> Result<Me> {
        self.run(|c| async move { Ok(c.keep()?.set_avatar_preset(&preset).await?) })
            .await
    }

    /// Uploads my custom avatar: the cropped square, at most 512×512 (never upscaled) and
    /// 512 KiB, `content_type` "image/jpeg" (preferred, quality ~0.88), "image/png" or
    /// "image/webp".
    pub async fn keep_upload_avatar(&self, bytes: Vec<u8>, content_type: String) -> Result<Me> {
        self.run(|c| async move { Ok(c.keep()?.upload_avatar(bytes, &content_type).await?) })
            .await
    }

    /// Back to the default avatar (initial on a colour).
    pub async fn keep_remove_avatar(&self) -> Result<Me> {
        self.run(|c| async move { Ok(c.keep()?.remove_avatar().await?) })
            .await
    }

    /// The request that fetches a user's custom avatar (`Avatar::custom_version` as
    /// `version`): URL plus the sign-in headers (the picture is not public). Cache the picture
    /// by (user_key, version): a new version means a new picture.
    pub fn keep_avatar_request(&self, user_key: String, version: u32) -> Result<HttpRequest> {
        let k = self.core.keep()?;
        Ok(HttpRequest {
            url: k.avatar_url(&user_key, version),
            headers: k
                .auth_headers()
                .into_iter()
                .map(|(name, value)| Header { name, value })
                .collect(),
        })
    }

    /// Ends the one-time welcome step for this account on every device (both Continue and
    /// Skip call it). Idempotent.
    pub async fn keep_mark_profile_set(&self) -> Result<Me> {
        self.run(|c| async move { Ok(c.keep()?.mark_profile_set().await?) })
            .await
    }

    pub async fn keep_admin_users(&self) -> Result<Vec<AdminUser>> {
        self.run(|c| async move { Ok(c.keep()?.admin_users().await?) })
            .await
    }

    /// Admins: who keeps what forever and how much of it others keep too.
    pub async fn keep_admin_forever(&self) -> Result<keep_service::ForeverStats> {
        self.run(|c| async move { Ok(c.keep()?.admin_forever().await?) })
            .await
    }

    /// Seerr's users, to link AlwyWatch users to (admins; empty when Seerr is not offered).
    pub async fn keep_admin_seerr_users(&self) -> Result<Vec<keep_service::SeerrUser>> {
        self.run(|c| async move { Ok(c.keep()?.admin_seerr_users().await?) })
            .await
    }

    /// Makes a user's requests go to an existing Seerr user (None: automatic again).
    pub async fn keep_admin_link_seerr(
        &self,
        user_key: String,
        seerr_user_id: Option<i64>,
    ) -> Result<AdminUser> {
        self.run(|c| async move { Ok(c.keep()?.admin_link_seerr(&user_key, seerr_user_id).await?) })
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

/// Cards in a home "recently added" row.
const LATEST_CARDS: u32 = 20;
/// Entries asked of a shows library: they come episode by episode (a season of 20 episodes is
/// 20 entries), grouped per show into at most [`LATEST_CARDS`] cards.
const LATEST_SHOWS_FETCH: u32 = 100;

/// Search results: matching episodes or seasons count as their show, once each.
fn shows_not_episodes(items: Vec<Item>) -> Vec<Item> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .map(|i| media_api::show_card(&i))
        .filter(|i| seen.insert(i.id.clone()))
        .collect()
}

/// A watched title that was reopened for a moment (servers keep that position and list it
/// as "in progress"): not something to continue. Its show's next episode takes the place.
/// A real rewatch counts once it is past two minutes or 5 % of the title.
pub(crate) fn finished_and_barely_reopened(i: &Item) -> bool {
    let limit = (i.runtime_ms.unwrap_or(0) / 20).max(120_000);
    i.user.played && i.user.position_ms < limit
}

/// Merges in-progress items and next episodes into one list by recency (see
/// [`Home::continue_watching`]): in-progress items by their own date, next episodes by when
/// their show was last watched (set by `fetch_home`); undated next episodes last, in the
/// server's order (the sort is stable).
pub fn continue_watching(resume: &[Item], next_up: &[Item]) -> Vec<Item> {
    let mut all: Vec<Item> = resume
        .iter()
        .filter(|i| !finished_and_barely_reopened(i))
        .chain(next_up)
        .cloned()
        .collect();
    all.sort_by_key(|i| std::cmp::Reverse(i.user.last_played_at.unwrap_or(0)));
    // One entry per show: its most recent one (an episode in progress beats the next one).
    let mut seen = std::collections::HashSet::new();
    all.retain(|i| match (&i.kind, &i.series_id) {
        (ItemKind::Episode, Some(s)) => seen.insert(s.clone()),
        _ => seen.insert(i.id.clone()),
    });
    all
}

fn requests_key(all: bool) -> &'static str {
    if all {
        "seerr/requests-all"
    } else {
        "seerr/requests"
    }
}

fn internal(e: impl std::fmt::Display) -> CoreError {
    CoreError::Internal {
        detail: e.to_string(),
    }
}

fn hidden_key(account: &str, item_id: &str) -> String {
    format!("cw-hidden\u{1f}{account}\u{1f}{item_id}")
}

fn watched_key(account: &str, item_id: &str) -> String {
    format!("watched\u{1f}{account}\u{1f}{item_id}")
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

#[cfg(test)]
mod continue_watching_tests {
    use super::*;

    pub(super) fn ep(id: &str, series: &str, at: Option<i64>, pos: u64) -> Item {
        Item {
            id: id.into(),
            kind: ItemKind::Episode,
            title: id.into(),
            sort_title: None,
            overview: None,
            year: None,
            runtime_ms: None,
            community_rating: None,
            official_rating: None,
            genres: vec![],
            provider_ids: ProviderIds::default(),
            series_id: Some(series.into()),
            series_title: Some(series.into()),
            season_id: None,
            season_number: Some(1),
            episode_number: Some(1),
            child_count: None,
            premiere_date: None,
            images: vec![],
            user: UserData {
                played: false,
                position_ms: pos,
                play_count: 0,
                favorite: false,
                last_played_at: at,
                unplayed_count: None,
            },
        }
    }

    #[test]
    fn mixes_in_progress_and_next_episodes_by_recency() {
        let now = unix_now();
        // In progress: a (2 h ago), b (5 days ago). Next up, dated by their show's last
        // finished episode: c (1 h ago), d (never: undated), b2 (show B, 1 day ago), e (10 days).
        let resume = [
            ep("a", "A", Some(now - 7200), 10),
            ep("b", "B", Some(now - 5 * 86400), 10),
        ];
        let next = [
            ep("d", "D", None, 0),
            ep("c", "C", Some(now - 3600), 0),
            ep("b2", "B", Some(now - 86400), 0),
            ep("e", "E", Some(now - 10 * 86400), 0),
        ];
        let ids: Vec<String> = continue_watching(&resume, &next)
            .into_iter()
            .map(|i| i.id)
            .collect();
        // Newest first, one per show (b2 is more recent than b), undated d last.
        assert_eq!(ids, ["c", "a", "b2", "e", "d"]);
    }

    #[test]
    fn a_watched_episode_reopened_for_a_moment_gives_way_to_the_next_one() {
        let now = unix_now();
        // Episode 5 is watched but the server kept a position 1:42 into it; episode 6 is next.
        let mut reopened = ep("e5", "S", Some(now - 60), 102_000);
        reopened.user.played = true;
        let next = [ep("e6", "S", Some(now - 3600), 0)];
        let ids = |r: &[Item]| -> Vec<String> {
            continue_watching(r, &next)
                .into_iter()
                .map(|i| i.id)
                .collect()
        };
        assert_eq!(ids(&[reopened.clone()]), ["e6"]);
        // A real rewatch (past two minutes) stays.
        reopened.user.position_ms = 600_000;
        assert_eq!(ids(&[reopened]), ["e5"]);
    }
}

#[cfg(test)]
mod latest_shows_tests {
    use super::*;

    #[test]
    fn new_episodes_and_seasons_become_their_show_once() {
        let img = |kind, path: &str| ImageRef {
            kind,
            path: path.into(),
            blurhash: None,
        };
        let mut e1 = continue_watching_tests::ep("e1", "show", None, 0);
        e1.images = vec![
            img(ImageKind::Primary, "still"),
            img(ImageKind::SeriesPrimary, "poster"),
        ];
        let e2 = continue_watching_tests::ep("e2", "show", None, 0);
        let mut season = continue_watching_tests::ep("s3", "other", None, 0);
        season.kind = ItemKind::Season;
        season.images = vec![img(ImageKind::Primary, "season-art")];
        let out = shows_not_episodes(vec![e1, e2, season]);
        assert_eq!(out.len(), 2);
        assert_eq!(
            (out[0].id.as_str(), out[0].kind),
            ("show", ItemKind::Series)
        );
        assert_eq!(out[0].images[0].path, "poster");
        assert_eq!(out[0].images[0].kind, ImageKind::Primary);
        assert_eq!(
            (out[1].id.as_str(), out[1].images[0].path.as_str()),
            ("other", "season-art")
        );
    }
}

// ----- watch together ---------------------------------------------------------------------------

/// The title of a room, found on one of the user's servers.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TogetherMatch {
    pub account_id: String,
    pub item_id: String,
}

pub(crate) fn same_title(a: &str, b: &str) -> bool {
    let n = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    !a.is_empty() && n(a) == n(b)
}

pub(crate) fn ids_match(
    p: &ProviderIds,
    tmdb: &Option<String>,
    imdb: &Option<String>,
    tvdb: &Option<String>,
) -> bool {
    let eq = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
    eq(&p.tmdb, tmdb) || eq(&p.imdb, imdb) || eq(&p.tvdb, tvdb)
}

/// Where playback should be now, from a room state received at `received_at_ms` (local
/// clock): the server's position moved on by the time since, while playing.
#[uniffi::export]
pub fn together_position(state: TogetherState, received_at_ms: i64, now_ms: i64) -> u64 {
    state.position_at(state.now_ms + (now_ms - received_at_ms).max(0))
}

/// Where playback should be at `server_now_ms` (from [`AlwyWatch::together_now_ms`]).
#[uniffi::export]
pub fn together_position_at(state: TogetherState, server_now_ms: i64) -> u64 {
    state.position_at(server_now_ms)
}

/// How long until a start everyone heard plays (None: not waiting for one). Keep the player
/// paused at the position until then, and press play right then.
#[uniffi::export]
pub fn together_starts_in(state: TogetherState, server_now_ms: i64) -> Option<i64> {
    state.starts_in(server_now_ms)
}

/// Whether this seat still has to load the round's title: load it paused, and once
/// [`together_ready_buffer_ms`] of it is buffered call [`AlwyWatch::together_ready`].
#[uniffi::export]
pub fn together_is_loading(state: TogetherState, member_id: String) -> bool {
    state.is_loading(&member_id)
}

/// How much a player buffers before it reports ready.
#[uniffi::export]
pub fn together_ready_buffer_ms() -> u64 {
    keep_service::together::READY_BUFFER_MS
}

/// How the player should follow the room now (speed or seek), see
/// [`keep_service::together::together_correction`].
#[uniffi::export]
pub fn together_correction(
    expected_ms: u64,
    actual_ms: u64,
    playing: bool,
    current_rate: f64,
    can_change_rate: bool,
) -> keep_service::together::TogetherCorrection {
    keep_service::together::together_correction(
        expected_ms,
        actual_ms,
        playing,
        current_rate,
        can_change_rate,
    )
}

/// The built-in avatars, in display order (draw: a disc of `background`, `shape` in
/// `foreground`; geometry in docs/keep-protocol.md).
#[uniffi::export]
pub fn avatar_presets() -> Vec<AvatarPreset> {
    keep_service::profile::avatar_presets()
}

/// Background (`#RRGGBB`) of someone's default avatar, from their display name.
#[uniffi::export]
pub fn avatar_default_color(name: String) -> String {
    keep_service::profile::default_avatar_color(&name)
}

/// Letter of someone's default avatar (white on [`avatar_default_color`]).
#[uniffi::export]
pub fn avatar_initial(name: String) -> String {
    keep_service::profile::avatar_initial(&name)
}

/// What is wrong with a display name, in plain words (None: fine to send).
#[uniffi::export]
pub fn display_name_problem(name: String) -> Option<String> {
    keep_service::profile::clean_display_name(&name).err()
}

/// Whether `member_id` has the host's rights in this room (any seat of the host's user):
/// show "Invite" and "Start watching", else "Waiting for <host> to start".
#[uniffi::export]
pub fn together_is_host(state: TogetherState, member_id: String) -> bool {
    state.is_host(&member_id)
}

/// Local monotonic clock (ms) for the clock sync.
fn mono_ms() -> i64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as i64
}

/// Resync the server clock this often (device clocks drift by a few ms a minute).
const CLOCK_RESYNC: Duration = Duration::from_secs(120);

impl Core {
    /// The server clock now: measured, else this device's own clock.
    pub(crate) fn together_now(&self) -> i64 {
        let measured = self
            .together_clock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .server_now(mono_ms());
        measured.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        })
    }

    /// A few quick round trips to the server clock; the best round trip (ms).
    pub(crate) async fn together_sync_clock(&self) -> Result<i64> {
        let keep = self.keep()?;
        let mut sync = keep_service::together::ClockSync::default();
        for _ in 0..5 {
            let sent = mono_ms();
            let Ok(c) = keep.together_clock().await else {
                continue;
            };
            sync.add(sent, c.now_ms, mono_ms());
        }
        let Some(rtt) = sync.best_rtt() else {
            return Err(CoreError::Network {
                detail: "the AlwyWatch server's clock did not answer".into(),
            });
        };
        *self
            .together_clock
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = (sync, Some(Instant::now()));
        Ok(rtt)
    }

    /// Measures the clock when it never was or a while ago (errors: the device clock is used).
    async fn together_clock_fresh(&self) {
        let stale = self
            .together_clock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .is_none_or(|t| t.elapsed() > CLOCK_RESYNC);
        if stale {
            let _ = self.together_sync_clock().await;
        }
    }

    /// A title as every server can find it (an episode carries its show's ids).
    async fn together_media(&self, account_id: &str, item_id: &str) -> Result<TogetherMedia> {
        let srv = self.server(account_id)?;
        let item = srv.item(item_id).await?;
        let episode = item.kind == ItemKind::Episode;
        let series = match (&item.series_id, episode) {
            (Some(id), true) => srv.item(id).await.ok(),
            _ => None,
        };
        let p = &item.provider_ids;
        Ok(TogetherMedia {
            kind: if episode { "episode" } else { "movie" }.into(),
            title: item.title.clone(),
            year: item.year,
            tmdb: p.tmdb.clone(),
            imdb: p.imdb.clone(),
            tvdb: p.tvdb.clone(),
            series_title: item.series_title.clone(),
            series_tmdb: series.as_ref().and_then(|s| s.provider_ids.tmdb.clone()),
            series_tvdb: series.as_ref().and_then(|s| s.provider_ids.tvdb.clone()),
            season: item.season_number,
            episode: item.episode_number,
        })
    }

    /// The room's title on `account_id`: by provider ids, else by title (and year).
    async fn together_find_on(
        &self,
        account_id: &str,
        m: &TogetherMedia,
    ) -> Result<Option<String>> {
        let srv = self.server(account_id)?;
        if m.kind == "episode" {
            let show_name = m.series_title.clone().unwrap_or_default();
            let shows = srv.search(&show_name, 20).await?;
            let show = shows
                .iter()
                .filter(|i| i.kind == ItemKind::Series)
                .find(|i| ids_match(&i.provider_ids, &m.series_tmdb, &None, &m.series_tvdb))
                .or_else(|| {
                    shows
                        .iter()
                        .filter(|i| i.kind == ItemKind::Series)
                        .find(|i| same_title(&i.title, &show_name))
                });
            // Search can answer with episodes standing for their show.
            let show_id = show.map(|s| s.id.clone()).or_else(|| {
                shows
                    .iter()
                    .find(|i| {
                        i.kind == ItemKind::Episode
                            && same_title(i.series_title.as_deref().unwrap_or(""), &show_name)
                    })
                    .and_then(|i| i.series_id.clone())
            });
            let Some(show_id) = show_id else {
                return Ok(None);
            };
            let seasons = srv.seasons(&show_id).await?;
            let Some(season) = seasons.iter().find(|s| s.season_number == m.season) else {
                return Ok(None);
            };
            let episodes = srv.episodes(&show_id, &season.id).await?;
            return Ok(episodes
                .iter()
                .find(|e| e.episode_number == m.episode)
                .map(|e| e.id.clone()));
        }
        let found = srv.search(&m.title, 20).await?;
        let movies = found.iter().filter(|i| i.kind == ItemKind::Movie);
        let by_id = movies
            .clone()
            .find(|i| ids_match(&i.provider_ids, &m.tmdb, &m.imdb, &m.tvdb));
        let by_name = || {
            movies.clone().find(|i| {
                same_title(&i.title, &m.title)
                    && (m.year.is_none() || i.year.is_none() || i.year == m.year)
            })
        };
        Ok(by_id.or_else(by_name).map(|i| i.id.clone()))
    }
}

#[uniffi::export]
impl AlwyWatch {
    /// "Watch together" on a title: opens a private lobby with this item on the AlwyWatch
    /// server, the caller its host. Nothing plays until [`AlwyWatch::together_start`]: open the
    /// Room screen, not the player.
    pub async fn together_create(
        &self,
        account_id: String,
        item_id: String,
    ) -> Result<TogetherJoined> {
        let rights = self.together_rights();
        self.run(|c| async move {
            let media = c.together_media(&account_id, &item_id).await?;
            Ok(c.keep()?
                .together_create(&TogetherCreate { media, rights })
                .await?)
        })
        .await
    }

    /// "New room" on the Together page: a lobby with no title yet (`state.media.title` is
    /// empty). People can be invited and join, my other devices too, while I browse; a title's
    /// "Watch in your room" ([`AlwyWatch::together_update`]) then chooses what to watch.
    pub async fn together_open(&self) -> Result<TogetherJoined> {
        let rights = self.together_rights();
        self.run(|c| async move {
            Ok(c.keep()?
                .together_create(&TogetherCreate {
                    media: TogetherMedia::none(),
                    rights,
                })
                .await?)
        })
        .await
    }

    /// My rooms and the rooms I am invited to (`invited == true`: offer Join / Not now), with
    /// the list's `version` for [`AlwyWatch::together_rooms_wait`]. Nobody sees other rooms.
    pub async fn together_rooms(&self) -> Result<TogetherRooms> {
        self.run(|c| async move { Ok(c.keep()?.together_rooms().await?) })
            .await
    }

    /// The same list once its `version` differs from `after`, or after about 25 s. Call it in a
    /// loop (passing the last `version`, 0 the first time) while the app is in the foreground:
    /// this is how invitations arrive.
    pub async fn together_rooms_wait(&self, after: u64) -> Result<TogetherRooms> {
        self.run(|c| async move { Ok(c.keep()?.together_rooms_wait(after).await?) })
            .await
    }

    /// Everyone the host can invite (all users of this AlwyWatch server but me): people I last
    /// watched with first, then by name.
    pub async fn together_people(&self) -> Result<Vec<TogetherPerson>> {
        self.run(|c| async move { Ok(c.keep()?.together_people().await?) })
            .await
    }

    /// The host invites people (`TogetherPerson::user_key`, 1 to 20 at once); `member_id` is
    /// the host's seat. Returns the room (its `invited` list).
    pub async fn together_invite(
        &self,
        code: String,
        member_id: String,
        user_keys: Vec<String>,
    ) -> Result<TogetherState> {
        self.run(|c| async move {
            Ok(c.keep()?
                .together_invite(
                    &code,
                    &TogetherInvite {
                        member_id,
                        user_keys,
                    },
                )
                .await?)
        })
        .await
    }

    /// The host withdraws an invitation.
    pub async fn together_uninvite(
        &self,
        code: String,
        member_id: String,
        user_key: String,
    ) -> Result<TogetherState> {
        self.run(|c| async move {
            Ok(c.keep()?
                .together_uninvite(&code, &member_id, &user_key)
                .await?)
        })
        .await
    }

    /// Joins a room I am invited to (or already in on another device): my seat and the room.
    /// Open the Room screen when `state.started` is false, the player in sync otherwise.
    pub async fn together_join(&self, code: String) -> Result<TogetherJoined> {
        self.run(|c| async move {
            c.together_clock_fresh().await;
            Ok(c.keep()?.together_join(&code).await?)
        })
        .await
    }

    /// "Not now" on an invitation: it goes away (the host sees it).
    pub async fn together_decline(&self, code: String) -> Result<()> {
        self.run(|c| async move { Ok(c.keep()?.together_decline(&code).await?) })
            .await
    }

    /// The host starts the lobby: playing from `position_ms` for everyone (members' `wait`
    /// returns `started == true`: open the player).
    pub async fn together_start(
        &self,
        code: String,
        member_id: String,
        position_ms: u64,
    ) -> Result<TogetherState> {
        self.run(|c| async move {
            Ok(c.keep()?
                .together_start(&code, &member_id, position_ms)
                .await?)
        })
        .await
    }

    /// Sends play/pause/seek; with `item` (account, item), a new title for everyone. In a
    /// lobby only the host may call it (to change the title; nothing plays before the start).
    pub async fn together_update(
        &self,
        code: String,
        member_id: String,
        playing: bool,
        position_ms: u64,
        account_id: Option<String>,
        item_id: Option<String>,
    ) -> Result<TogetherState> {
        self.run(|c| async move {
            let media = match (account_id, item_id) {
                (Some(a), Some(i)) => Some(c.together_media(&a, &i).await?),
                _ => None,
            };
            Ok(c.keep()?
                .together_update(
                    &code,
                    &TogetherUpdate {
                        member_id,
                        playing,
                        position_ms,
                        media,
                        started: false,
                        lobby: false,
                    },
                )
                .await?)
        })
        .await
    }

    /// The host leaves the player: back to the waiting room for everyone (nobody leaves; the
    /// others' players close). The host may then pick another title and start again.
    pub async fn together_lobby(
        &self,
        code: String,
        member_id: String,
        position_ms: u64,
    ) -> Result<TogetherState> {
        self.run(|c| async move {
            Ok(c.keep()?
                .together_lobby(&code, &member_id, position_ms)
                .await?)
        })
        .await
    }

    /// This seat's player has loaded the title of `round` (see [`together_is_loading`]). When
    /// every seat has, the room plays for everyone at once.
    pub async fn together_ready(
        &self,
        code: String,
        member_id: String,
        round: u64,
    ) -> Result<TogetherState> {
        self.run(|c| async move {
            Ok(c.keep()?
                .together_ready(
                    &code,
                    &keep_service::together::TogetherReady { member_id, round },
                )
                .await?)
        })
        .await
    }

    /// The AlwyWatch server's clock now (Unix ms), as measured from this device within a few
    /// milliseconds (the room calls keep it measured): where every seat agrees playback is.
    pub fn together_now_ms(&self) -> i64 {
        self.core.together_now()
    }

    /// The room after `after` changed, or after about 25 s. Call it in a loop while in the room
    /// (lobby included): it is the seat's heartbeat (a seat silent for a minute is dropped).
    pub async fn together_wait(
        &self,
        code: String,
        member_id: String,
        after: u64,
    ) -> Result<TogetherState> {
        self.run(|c| async move {
            c.together_clock_fresh().await;
            Ok(c.keep()?.together_wait(&code, &member_id, after).await?)
        })
        .await
    }

    /// Leaves the room for good (coming back needs a new invitation, unless another of my
    /// devices is still in it). The host leaving hands the room to the longest member.
    pub async fn together_leave(&self, code: String, member_id: String) -> Result<()> {
        self.run(|c| async move { Ok(c.keep()?.together_leave(&code, &member_id).await?) })
            .await
    }

    /// The room's title on one of `account_ids` (in that order), or none when no server has it.
    pub async fn together_find(
        &self,
        account_ids: Vec<String>,
        media: TogetherMedia,
    ) -> Result<Option<TogetherMatch>> {
        // A room without a title yet: nothing to look for.
        if media.is_none() {
            return Ok(None);
        }
        self.run(|c| async move {
            for a in account_ids {
                if let Ok(Some(item_id)) = c.together_find_on(&a, &media).await {
                    return Ok(Some(TogetherMatch {
                        account_id: a,
                        item_id,
                    }));
                }
            }
            Ok(None)
        })
        .await
    }
}

#[cfg(test)]
mod together_tests {
    use super::*;

    #[test]
    fn missing_intro_or_credits_come_from_the_other_copy_only() {
        let seg = |kind, a: u64, b: u64| Segment {
            kind,
            start_ms: a * 1000,
            end_ms: b * 1000,
        };
        // This copy knows its credits (from a chapter), the other one also the intro.
        let mut own = vec![seg(SegmentKind::Credits, 1300, 1380)];
        merge_missing_segments(
            &mut own,
            vec![
                seg(SegmentKind::Intro, 1, 86),
                seg(SegmentKind::Credits, 1302, 1380),
            ],
        );
        assert_eq!(
            own,
            vec![
                seg(SegmentKind::Intro, 1, 86),
                seg(SegmentKind::Credits, 1300, 1380)
            ]
        );
        assert!(missing_segment_kinds(&own).is_empty());
        assert_eq!(
            missing_segment_kinds(&[]),
            vec![SegmentKind::Intro, SegmentKind::Credits]
        );
    }

    #[test]
    fn titles_and_ids_match_loosely() {
        assert!(same_title("Monsters, Inc.", "monsters inc"));
        assert!(!same_title("", ""));
        let p = ProviderIds {
            tmdb: Some("585".into()),
            ..Default::default()
        };
        assert!(ids_match(&p, &Some("585".into()), &None, &None));
        assert!(!ids_match(&p, &None, &None, &None));
    }

    #[test]
    fn position_follows_the_local_clock_since_received() {
        let s = TogetherState {
            code: "1".into(),
            seq: 1,
            media: TogetherMedia {
                kind: "movie".into(),
                title: "X".into(),
                year: None,
                tmdb: None,
                imdb: None,
                tvdb: None,
                series_title: None,
                series_tmdb: None,
                series_tvdb: None,
                season: None,
                episode: None,
            },
            started: true,
            playing: true,
            position_ms: 60_000,
            host: String::new(),
            invited: vec![],
            rights: Default::default(),
            at_ms: 1_000_000,
            now_ms: 1_000_500,
            round: 1,
            loading: vec![],
            ready_by_ms: 0,
            by: String::new(),
            members: vec![],
        };
        // Received 500 ms after the stamp; 2 s later locally.
        assert_eq!(together_position(s, 50, 2_050), 62_500);
    }
}
