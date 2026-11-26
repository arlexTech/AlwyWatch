// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keep logic: Maintainerr sync, vote resolution, reconciliation, "watched" detection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use keep_service::{
    CountdownResult, LeavingItem, MyVote, Protection, Rights, ServerType, VoteKind, VoteRequest,
};
use maintainerr::{DAY, Maintainerr};
use net::reqwest::Client;
use serde::Deserialize;
use tokio::sync::Notify;

use crate::auth::{Auth, Identity};
use crate::config::Config;
use crate::db::{Db, Desired, ItemRow, now};

pub struct State {
    pub config: Config,
    pub db: Db,
    pub http: Client,
    pub maintainerr: Maintainerr,
    pub auth: Auth,
    /// Seerr for users of the other media server type (None when not configured).
    pub seerr: Option<crate::seerr::Bridge>,
    /// Wakes the reconciler (after votes and syncs).
    pub reconcile: Notify,
    /// Wakes the "watched" checker (webhooks).
    pub watch: Notify,
    /// Per Jellyfin server: ("movie"|"show", provider:id) -> Jellyfin item id.
    jf_index: Mutex<HashMap<String, (Instant, ProviderIndex)>>,
    /// The titles of the library Maintainerr manages, by provider id (to keep a title that is
    /// in no deletion list).
    library: tokio::sync::Mutex<Option<(Instant, Arc<LibraryIndex>)>>,
    /// Serialises Maintainerr writes (it has its own lock and answers 409 when busy).
    write_lock: tokio::sync::Mutex<()>,
    /// Pending device-to-device transfers (in memory only).
    pub transfers: Arc<crate::transfer::Transfers>,
    /// Watch together rooms (in memory only).
    pub rooms: Arc<crate::together::Rooms>,
    /// TVs a phone can play on (in memory only).
    pub remotes: Arc<crate::remote::Remotes>,
    /// Passkey challenges in progress (in memory only).
    pub passkeys: Arc<crate::passkey::Flows>,
    /// Pocket ID sign-ins in progress (in memory only).
    pub oidc: Arc<crate::oidc::Oidc>,
    /// Attempt limits (see `limit.rs`).
    pub limits: Arc<Limits>,
    /// Maintainerr's state from the last sync: 0 not tried yet, 1 answered, 2 failed.
    pub maintainerr_state: std::sync::atomic::AtomicU8,
    /// Unix seconds and title count of the last successful reading (admin status).
    pub last_sync: std::sync::atomic::AtomicI64,
    pub last_sync_items: std::sync::atomic::AtomicUsize,
    /// The saved settings this server runs with (Settings → Admin → Server settings).
    pub stored: Arc<tokio::sync::Mutex<crate::settings::Stored>>,
    /// Wakes `main` to switch to the saved settings.
    pub reload: Arc<Notify>,
    /// While no media server is set up: the code printed in the log that the setup page asks
    /// for (nobody else can claim a new server).
    pub setup_code: Option<String>,
    /// Wrong setup codes so far, from anyone.
    pub setup_failures: std::sync::atomic::AtomicU32,
    /// Why Maintainerr is not used although it is set up (an unsupported version).
    pub maintainerr_problem: std::sync::Mutex<Option<String>>,
    /// The admin dashboard's live readings and stats cache.
    pub dashboard: crate::dashboard::Dash,
}

pub struct Limits {
    /// Rejected tokens, per client address.
    pub failed_auth: crate::limit::Limiter,
    /// Transfers started (no sign-in needed), per client address.
    pub transfer_starts: crate::limit::Limiter,
    /// Transfer codes tried, per user: nobody guesses someone else's six digits.
    pub transfer_codes: crate::limit::Limiter,
    /// Invitations sent, per user (no spamming people with rooms).
    pub together_invites: crate::limit::Limiter,
    /// Display name and avatar changes, per user.
    pub profile_changes: crate::limit::Limiter,
    /// Rooms opened, per user.
    pub together_creates: crate::limit::Limiter,
    /// Identity link attempts, per person (each validates a second token).
    pub links: crate::limit::Limiter,
    /// Devices signed in to a media server from the account, per person.
    pub server_sign_ins: crate::limit::Limiter,
    /// Failed account sign-ins, per client address.
    pub account_logins: crate::limit::Limiter,
    /// Failed account sign-ins, per username from every address together.
    pub username_logins: crate::limit::Limiter,
    /// Accounts created, per client address.
    pub account_signups: crate::limit::Limiter,
    /// Invitation emails sent, per admin.
    pub invite_emails: crate::limit::Limiter,
    /// Wrong setup codes, per client address (all of them together are capped too).
    pub setup_codes: crate::limit::Limiter,
    /// Invitation codes looked at, right or wrong, per client address.
    pub invite_checks: crate::limit::Limiter,
    /// Server settings tests (each reaches a service, an email may be sent), per admin.
    pub settings_tests: crate::limit::Limiter,
    /// Password and username claims, per user.
    pub claims: crate::limit::Limiter,
    /// Passkey and Pocket ID sign-ins started, per client address.
    pub flow_starts: crate::limit::Limiter,
    /// "Forgot password" requests, per address.
    pub password_resets: crate::limit::Limiter,
}

impl Default for Limits {
    fn default() -> Self {
        use crate::limit::Limiter;
        use std::time::Duration;
        let min = |m: u64| Duration::from_secs(60 * m);
        Self {
            failed_auth: Limiter::new(20, min(10)),
            transfer_starts: Limiter::new(10, min(10)),
            transfer_codes: Limiter::new(10, min(10)),
            together_invites: Limiter::new(60, min(10)),
            together_creates: Limiter::new(20, min(60)),
            profile_changes: Limiter::new(20, min(10)),
            links: Limiter::new(10, min(10)),
            server_sign_ins: Limiter::new(20, min(10)),
            account_logins: Limiter::new(10, min(10)),
            username_logins: Limiter::new(200, min(60)),
            account_signups: Limiter::new(5, min(60)),
            invite_emails: Limiter::new(20, min(60)),
            setup_codes: Limiter::new(10, min(10)),
            invite_checks: Limiter::new(30, min(10)),
            settings_tests: Limiter::new(60, min(60)),
            claims: Limiter::new(10, min(10)),
            flow_starts: Limiter::new(30, min(10)),
            password_resets: Limiter::new(10, min(60)),
        }
    }
}

pub type Shared = Arc<State>;

/// ("movie"|"show", "provider:id") -> Jellyfin item id.
type ProviderIndex = HashMap<(String, String), String>;

/// ("movie"|"show", "provider:id") -> the title on the server Maintainerr manages.
type LibraryIndex = HashMap<(String, String), maintainerr::Meta>;

#[derive(Debug)]
pub enum VoteError {
    NotFound,
    Forbidden(String),
    Invalid(String),
}

impl State {
    pub fn new(config: Config, db: Db, http: Client, plex_tv: &str) -> Self {
        let maintainerr = Maintainerr::new(
            http.clone(),
            &config.maintainerr.url,
            config.secrets.maintainerr_api_key.clone(),
        );
        let seerr = crate::seerr::Bridge::new(
            http.clone(),
            config.seerr.as_ref(),
            config.secrets.seerr_api_key.as_deref(),
            config.dry_run,
        );
        Self {
            auth: Auth::new(http.clone(), plex_tv),
            seerr,
            maintainerr,
            config,
            db,
            http,
            reconcile: Notify::new(),
            watch: Notify::new(),
            jf_index: Mutex::new(HashMap::new()),
            library: Default::default(),
            transfers: Default::default(),
            rooms: Default::default(),
            remotes: Default::default(),
            passkeys: Default::default(),
            oidc: Default::default(),
            limits: Default::default(),
            maintainerr_state: Default::default(),
            last_sync: Default::default(),
            last_sync_items: Default::default(),
            stored: Default::default(),
            reload: Default::default(),
            setup_code: None,
            setup_failures: Default::default(),
            maintainerr_problem: Default::default(),
            dashboard: Default::default(),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    // ----- Maintainerr sync -----------------------------------------------------------------

    /// Snapshots every collection item (deletion date, ids, poster) and fills titles.
    pub async fn sync(&self) -> Result<usize, String> {
        let collections = self
            .maintainerr
            .collections()
            .await
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        for c in &collections {
            let list = self
                .maintainerr
                .collection_items(c.id)
                .await
                .map_err(|e| e.to_string())?;
            items.push((c, list));
        }
        self.db.begin_sync().map_err(|e| e.to_string())?;
        let mut n = 0;
        for (c, list) in items {
            let kind = if c.kind.contains("show")
                || c.kind.contains("season")
                || c.kind.contains("episode")
            {
                "show"
            } else {
                "movie"
            };
            for i in list {
                let poster = i.poster_url();
                self.db
                    .upsert_collection_item(&crate::db::NewItem {
                        key: &i.media_server_id,
                        collection_id: c.id,
                        collection_title: &c.title,
                        active: c.is_active,
                        deletion_date: i.deletion_date(c),
                        poster: poster.as_deref(),
                        tmdb: i.tmdb_id.map(|v| v.to_string()),
                        tvdb: i.tvdb_id.map(|v| v.to_string()),
                        kind,
                    })
                    .map_err(|e| e.to_string())?;
                n += 1;
            }
        }
        // Titles: fetched once per item (Maintainerr only returns ids in collections).
        loop {
            let missing = self.db.items_missing_meta(50).map_err(|e| e.to_string())?;
            if missing.is_empty() {
                break;
            }
            for key in missing {
                match self.maintainerr.meta(&key).await {
                    Ok(m) => self
                        .db
                        .set_meta(&key, &m.title, m.year, m.tmdb_id, m.tvdb_id),
                    // Unknown item (deleted from the server): remember we tried.
                    Err(_) => self.db.set_meta(&key, "", None, None, None),
                }
                .map_err(|e| e.to_string())?;
            }
        }
        self.reconcile.notify_one();
        Ok(n)
    }

    // ----- views ----------------------------------------------------------------------------

    pub fn rights_of(&self, id: &Identity) -> Rights {
        self.db.rights(&id.user_key).unwrap_or_default()
    }

    fn item_view(
        &self,
        row: &ItemRow,
        desired: &Desired,
        mine: Option<MyVote>,
        server_item_id: Option<String>,
    ) -> LeavingItem {
        let protection = if desired.forever {
            Protection::Forever
        } else if desired.until_watched {
            Protection::UntilWatched
        } else if (row.in_collection || row.deletion_date.is_some())
            && desired
                .extend_until
                .is_some_and(|e| row.deletion_date.is_none_or(|d| e > d))
        {
            // (A title in no list has no date to extend: its "keep N days" waits for one.)
            Protection::Extended
        } else {
            Protection::None
        };
        let effective = match protection {
            Protection::Forever | Protection::UntilWatched => None,
            Protection::Extended => desired.extend_until,
            Protection::None => row.deletion_date,
        };
        LeavingItem {
            key: row.key.clone(),
            title: row
                .title
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| format!("Item {}", row.key)),
            year: row.year,
            kind: row.kind.clone().unwrap_or_else(|| "movie".into()),
            poster_url: row.poster.clone(),
            tmdb_id: row.tmdb.clone(),
            tvdb_id: row.tvdb.clone(),
            server_item_id,
            collection_id: row.collection_id,
            collection_title: row.collection_title.clone(),
            collection_active: row.collection_active && row.in_collection,
            deletion_date: row.deletion_date,
            effective_deletion_date: effective,
            protection,
            my_vote: mine,
            counts: desired.counts.clone(),
            listed: row.in_collection,
        }
    }

    /// Everything leaving (or kept), most urgent first: the earliest effective deletion date,
    /// protected items last.
    pub async fn leaving_soon(&self, who: &Identity) -> Result<Vec<LeavingItem>, String> {
        let now = now();
        let items = self
            .db
            .visible_items(&who.user_key)
            .map_err(|e| e.to_string())?;
        let votes = self.db.votes().map_err(|e| e.to_string())?;
        let mut by_item: HashMap<&str, Vec<&crate::db::VoteRow>> = HashMap::new();
        for v in &votes {
            by_item.entry(v.item_key.as_str()).or_default().push(v);
        }
        let mapper = self.mapper(who).await;
        let mut out: Vec<LeavingItem> = items
            .iter()
            .map(|row| {
                let vs = by_item.get(row.key.as_str()).cloned().unwrap_or_default();
                let desired = Desired::from_votes(vs.iter().copied(), now);
                let mine = vs
                    .iter()
                    .find(|v| v.user_key == who.user_key && (v.fulfilled_at.is_none()))
                    .map(|v| v.vote.clone());
                self.item_view(row, &desired, mine, mapper.map(row))
            })
            .collect();
        out.sort_by_key(|i| {
            (
                i.effective_deletion_date.is_none(),
                i.effective_deletion_date.unwrap_or(i64::MAX),
                i.title.clone(),
            )
        });
        Ok(out)
    }

    /// One item as the caller sees it (also one that Leaving soon doesn't list: a title kept
    /// ahead of any deletion list, or one the caller doesn't care about).
    pub async fn item_for(&self, who: &Identity, key: &str) -> Result<LeavingItem, String> {
        let row = self
            .db
            .item(key)
            .map_err(|e| e.to_string())?
            .ok_or("not found")?;
        let votes = self.db.votes().map_err(|e| e.to_string())?;
        let vs: Vec<&crate::db::VoteRow> = votes.iter().filter(|v| v.item_key == key).collect();
        let desired = Desired::from_votes(vs.iter().copied(), now());
        let mine = vs
            .iter()
            .find(|v| v.user_key == who.user_key && v.fulfilled_at.is_none())
            .map(|v| v.vote.clone());
        let on_my_server = self.mapper(who).await.map(&row);
        Ok(self.item_view(&row, &desired, mine, on_my_server))
    }

    /// The keep entry of a movie or show that is in no deletion list, so it can be kept
    /// before it gets there. The title is found on the server Maintainerr manages: by its id
    /// when the caller uses that server, else by its TMDB/TVDB id in that server's libraries.
    pub async fn keep_entry(
        &self,
        who: &Identity,
        req: &keep_service::TitleLookup,
    ) -> Result<LeavingItem, VoteError> {
        let invalid = |m: &str| VoteError::Invalid(m.into());
        let internal = |e: String| VoteError::Invalid(e);
        if req.kind != "movie" && req.kind != "show" {
            return Err(invalid("only movies and shows can be kept"));
        }
        let (tmdb, tvdb) = (req.tmdb_id.as_deref(), req.tvdb_id.as_deref());
        let same_server = who.server_id == self.config.maintainerr.server;
        // The id only says something on Maintainerr's own server; and even there the page may
        // show another server's copy (accounts shown together), so the provider ids follow.
        let by_id = if same_server {
            self.db
                .item(&req.item_id)
                .map_err(|e| internal(e.to_string()))?
                .filter(|row| row.kind.as_deref() == Some(req.kind.as_str()))
        } else {
            None
        };
        let known = match by_id {
            Some(row) => Some(row),
            None => self
                .db
                .item_by_ids(&req.kind, tmdb, tvdb)
                .map_err(|e| internal(e.to_string()))?,
        };
        let key = match known {
            Some(row) => row.key,
            None => {
                let by_id = if same_server {
                    self.maintainerr
                        .meta(&req.item_id)
                        .await
                        .ok()
                        .filter(|m| m.kind == req.kind)
                } else {
                    None
                };
                let found = match by_id {
                    Some(m) => Some(m),
                    None => self
                        .library_title(&req.kind, tmdb, tvdb)
                        .await
                        .map_err(internal)?,
                };
                let Some(found) = found else {
                    // (To tell a title that really isn't there from ids that don't match.)
                    eprintln!(
                        "[keep] keep-entry: no {} on {} for tmdb {:?} tvdb {:?} (asked from {})",
                        req.kind, self.config.maintainerr.server, tmdb, tvdb, who.server_id
                    );
                    return Err(VoteError::NotFound);
                };
                self.db
                    .adopt_item(
                        &found.id,
                        &req.kind,
                        &found.title,
                        found.year,
                        found.tmdb_id.as_deref().or(tmdb),
                        found.tvdb_id.as_deref().or(tvdb),
                    )
                    .map_err(|e| internal(e.to_string()))?;
                found.id
            }
        };
        let mut item = self.item_for(who, &key).await.map_err(internal)?;
        // The caller's own id for it (the provider-id index may not know a new title yet).
        item.server_item_id = Some(req.item_id.clone());
        Ok(item)
    }

    // ----- votes ----------------------------------------------------------------------------

    pub fn vote(&self, who: &Identity, key: &str, req: &VoteRequest) -> Result<(), VoteError> {
        let item = self
            .db
            .item(key)
            .map_err(|e| VoteError::Invalid(e.to_string()))?
            .ok_or(VoteError::NotFound)?;
        let rights = self.rights_of(who);
        let now = now();
        let mut vote = MyVote {
            kind: req.kind,
            until: None,
            reminder_days: None,
            reason: None,
            created_at: now,
        };
        match req.kind {
            VoteKind::DontCare => {}
            VoteKind::KeepDays => {
                let days = req
                    .days
                    .filter(|d| *d >= 1)
                    .ok_or_else(|| VoteError::Invalid("days must be at least 1".into()))?;
                let max = rights.max_keep_days.unwrap_or(3650).min(3650);
                if days > max {
                    return Err(VoteError::Forbidden(format!(
                        "you can keep for at most {max} days"
                    )));
                }
                // Counted from the later of today and the current deletion date.
                let base = item.deletion_date.unwrap_or(now).max(now);
                vote.until = Some(base + i64::from(days) * DAY);
            }
            VoteKind::KeepUntilWatched => {
                vote.reminder_days = req.reminder_days.filter(|d| *d > 0);
            }
            VoteKind::KeepForever => {
                if !rights.can_keep_forever {
                    return Err(VoteError::Forbidden(
                        "keeping forever is disabled for your account".into(),
                    ));
                }
                let already = self
                    .db
                    .votes()
                    .map_err(|e| VoteError::Invalid(e.to_string()))?
                    .iter()
                    .any(|v| {
                        v.item_key == key
                            && v.user_key == who.user_key
                            && v.vote.kind == VoteKind::KeepForever
                            && v.fulfilled_at.is_none()
                    });
                if let Some(max) = rights.max_forever
                    && !already
                    && self.db.forever_count(&who.user_key).unwrap_or(0) >= max
                {
                    return Err(VoteError::Forbidden(format!(
                        "you can keep at most {max} items forever"
                    )));
                }
                vote.reason = req.reason.clone().map(|r| r.chars().take(200).collect());
            }
        }
        self.db
            .set_vote(key, &who.user_key, &vote)
            .map_err(|e| VoteError::Invalid(e.to_string()))?;
        self.db
            .log(&who.user_key, "vote", &format!("{key} {:?}", req.kind));
        self.reconcile.notify_one();
        Ok(())
    }

    pub fn unvote(&self, who: &Identity, key: &str) -> Result<(), VoteError> {
        if self
            .db
            .delete_vote(key, &who.user_key)
            .map_err(|e| VoteError::Invalid(e.to_string()))?
        {
            self.db.log(&who.user_key, "unvote", key);
            self.reconcile.notify_one();
        }
        Ok(())
    }

    // ----- reconciliation -------------------------------------------------------------------

    /// Applies the resolved votes to Maintainerr. Idempotent: compares with what was applied.
    pub async fn reconcile_once(&self) -> Result<u32, String> {
        let _guard = self.write_lock.lock().await;
        let now = now();
        let votes = self.db.votes().map_err(|e| e.to_string())?;
        let mut keys: Vec<String> = votes.iter().map(|v| v.item_key.clone()).collect();
        keys.extend(self.db.excluded_items().map_err(|e| e.to_string())?);
        keys.sort();
        keys.dedup();
        let mut changes = 0;
        for key in keys {
            let desired = Desired::from_votes(votes.iter().filter(|v| v.item_key == key), now);
            let excluded = self.db.excluded(&key).map_err(|e| e.to_string())?;
            if desired.needs_exclusion() && !excluded {
                if self.config.dry_run {
                    eprintln!("[dry-run] would add a global exclusion for {key}");
                } else {
                    self.with_retry(|| self.maintainerr.add_exclusion(&key))
                        .await?;
                }
                self.db
                    .set_excluded(&key, true)
                    .map_err(|e| e.to_string())?;
                self.db.log("reconciler", "exclude", &key);
                changes += 1;
            } else if !desired.needs_exclusion() && excluded {
                if self.config.dry_run {
                    eprintln!("[dry-run] would remove the global exclusions of {key}");
                } else {
                    self.with_retry(|| self.maintainerr.remove_global_exclusions(&key))
                        .await?;
                }
                self.db
                    .set_excluded(&key, false)
                    .map_err(|e| e.to_string())?;
                self.db.log("reconciler", "unexclude", &key);
                changes += 1;
            }
            if !desired.needs_exclusion()
                && let Some(target) = desired.extend_until
                && let Some(item) = self.db.item(&key).map_err(|e| e.to_string())?
                && item.in_collection
                && let (Some(collection), Some(current)) = (item.collection_id, item.deletion_date)
                && let Some(days) = maintainerr::postpone_days(current, now, target)
            {
                let new_date = if self.config.dry_run {
                    eprintln!("[dry-run] would postpone {key} by {days} days");
                    None
                } else {
                    self.with_retry(|| self.maintainerr.postpone(collection, &key, Some(days)))
                        .await?
                };
                let date = new_date.unwrap_or(current.max(now) + days * DAY);
                self.db
                    .set_deletion_date(&key, Some(date))
                    .map_err(|e| e.to_string())?;
                self.db
                    .log("reconciler", "postpone", &format!("{key} +{days}d"));
                changes += 1;
            }
        }
        Ok(changes)
    }

    /// Retries Maintainerr writes while it is busy (409) or briefly unreachable.
    async fn with_retry<T, F, Fut>(&self, f: F) -> Result<T, String>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = net::Result<T>>,
    {
        let mut last = String::new();
        for attempt in 0..6u32 {
            match f().await {
                Ok(v) => return Ok(v),
                Err(e @ net::Error::Status { status: 409 }) => last = e.to_string(),
                Err(e) if e.is_transient() => last = e.to_string(),
                Err(e) => return Err(e.to_string()),
            }
            tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
        }
        Err(format!("Maintainerr stayed busy: {last}"))
    }

    // ----- admin countdown ------------------------------------------------------------------

    /// Restarts the countdown of every item of a collection (today, or spread over N days) so
    /// overdue items get a real deletion date before the collection is activated.
    pub async fn countdown(
        &self,
        admin: &Identity,
        collection_id: i64,
        stagger_days: u32,
    ) -> Result<CountdownResult, String> {
        let _guard = self.write_lock.lock().await;
        let collections = self
            .maintainerr
            .collections()
            .await
            .map_err(|e| e.to_string())?;
        let c = collections
            .iter()
            .find(|c| c.id == collection_id)
            .ok_or("collection not found")?;
        let mut items = self
            .maintainerr
            .collection_items(collection_id)
            .await
            .map_err(|e| e.to_string())?;
        items.sort_by_key(|i| i.add_date);
        let n = items.len().max(1);
        let (mut reset, mut skipped) = (0u32, 0u32);
        if self.config.dry_run {
            eprintln!(
                "[dry-run] would restart the countdown of {} items in collection {collection_id} over {stagger_days} days",
                items.len()
            );
            return Ok(CountdownResult {
                reset: 0,
                skipped: items.len() as u32,
            });
        }
        for (idx, i) in items.iter().enumerate() {
            let r = async {
                self.with_retry(|| self.maintainerr.postpone(c.id, &i.media_server_id, None))
                    .await?;
                let extra = (idx as u64 * u64::from(stagger_days) / n as u64) as i64;
                if extra > 0 {
                    self.with_retry(|| {
                        self.maintainerr
                            .postpone(c.id, &i.media_server_id, Some(extra))
                    })
                    .await?;
                }
                Ok::<_, String>(())
            }
            .await;
            match r {
                Ok(()) => reset += 1,
                Err(_) => skipped += 1,
            }
        }
        self.db.log(&admin.user_key, "countdown", &format!("collection {collection_id}: {reset} reset, {skipped} skipped, stagger {stagger_days}d"));
        drop(_guard);
        // Fresh dates, then re-apply extensions on top of them.
        let _ = self.sync().await;
        Ok(CountdownResult { reset, skipped })
    }

    // ----- "watched" detection --------------------------------------------------------------

    /// Fulfils "keep until watched" votes of users who have now watched the item.
    pub async fn check_watched(&self) -> Result<u32, String> {
        let votes = self.db.votes().map_err(|e| e.to_string())?;
        let mut done = 0;
        for v in votes
            .iter()
            .filter(|v| v.vote.kind == VoteKind::KeepUntilWatched && v.fulfilled_at.is_none())
        {
            let Some(item) = self.db.item(&v.item_key).map_err(|e| e.to_string())? else {
                continue;
            };
            // Watched on any sign-in of the person counts.
            let mut keys: Vec<String> = self
                .db
                .identities_of(&v.user_key)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|i| i.user_key)
                .collect();
            if keys.is_empty() {
                keys.push(v.user_key.clone());
            }
            let mut watched = None;
            for key in &keys {
                let w = match key.split(':').collect::<Vec<_>>().as_slice() {
                    ["plex", _, uid] => self.plex_watched(uid, &item).await,
                    ["jellyfin", server, uid] => self.jellyfin_watched(server, uid, &item).await,
                    _ => None,
                };
                if w == Some(true) {
                    watched = w;
                    break;
                }
                watched = watched.or(w);
            }
            if watched == Some(true) {
                self.db
                    .fulfill(&v.item_key, &v.user_key)
                    .map_err(|e| e.to_string())?;
                self.db.log(
                    "watcher",
                    "watched",
                    &format!("{} by {}", v.item_key, v.user_key),
                );
                done += 1;
            }
        }
        if done > 0 {
            self.reconcile.notify_one();
        }
        Ok(done)
    }

    /// Plex users through Tautulli (no Plex Pass needed). Shows count as watched once their last
    /// episode is.
    async fn plex_watched(&self, plex_user_id: &str, item: &ItemRow) -> Option<bool> {
        let t = self.config.tautulli.as_ref()?;
        let key = self.config.secrets.tautulli_api_key.as_deref()?;
        let rating_key = if item.kind.as_deref() == Some("show") {
            self.maintainerr.last_episode(&item.key).await.ok()??
        } else {
            item.key.clone()
        };
        #[derive(Deserialize)]
        struct R {
            response: Resp,
        }
        #[derive(Deserialize)]
        struct Resp {
            data: Data,
        }
        #[derive(Deserialize)]
        struct Data {
            #[serde(default)]
            data: Vec<Row>,
        }
        #[derive(Deserialize)]
        struct Row {
            #[serde(default)]
            watched_status: f64,
        }
        let req = self
            .http
            .get(format!("{}/api/v2", t.url.trim_end_matches('/')))
            .query(&[
                ("apikey", key),
                ("cmd", "get_history"),
                ("user_id", plex_user_id),
                ("rating_key", &rating_key),
                ("length", "25"),
            ]);
        // The API key is in the query string: never let the URL reach an error message.
        let res = req.send().await.map_err(|e| e.without_url()).ok()?;
        let r: R = res.json().await.map_err(|e| e.without_url()).ok()?;
        Some(
            r.response
                .data
                .data
                .iter()
                .any(|row| row.watched_status >= 1.0),
        )
    }

    /// Jellyfin users through the admin API key: the movie, or the whole series, is played.
    async fn jellyfin_watched(
        &self,
        server_id: &str,
        user_id: &str,
        item: &ItemRow,
    ) -> Option<bool> {
        let server = self.config.server(server_id)?;
        let jf_id = self
            .jellyfin_map(server_id)
            .await
            .and_then(|m| lookup(&m, item))?;
        let key = self.config.secrets.jellyfin_key(server_id)?;
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct It {
            #[serde(default)]
            user_data: Option<Ud>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Ud {
            #[serde(default)]
            played: bool,
        }
        let req = self
            .http
            .get(format!(
                "{}/Items/{}",
                server.internal(),
                net::percent_encode(&jf_id)
            ))
            .query(&[("userId", user_id)])
            .header("Authorization", format!("MediaBrowser Token=\"{key}\""))
            .header("Accept", "application/json");
        let it: It = net::send_json(req).await.ok()?;
        Some(it.user_data.is_some_and(|u| u.played))
    }

    /// A title of the library Maintainerr manages, by provider id (TVDB first for shows). The
    /// index is read from Maintainerr at most once an hour, and once more for a title it
    /// doesn't know (added since), a minute apart at most.
    async fn library_title(
        &self,
        kind: &str,
        tmdb: Option<&str>,
        tvdb: Option<&str>,
    ) -> Result<Option<maintainerr::Meta>, String> {
        let ids: Vec<String> = {
            let (t, v) = (
                tmdb.filter(|v| !v.is_empty()).map(|v| format!("tmdb:{v}")),
                tvdb.filter(|v| !v.is_empty()).map(|v| format!("tvdb:{v}")),
            );
            let order = if kind == "show" { [v, t] } else { [t, v] };
            order.into_iter().flatten().collect()
        };
        if ids.is_empty() {
            return Ok(None);
        }
        let find = |index: &LibraryIndex| {
            ids.iter()
                .find_map(|id| index.get(&(kind.to_owned(), id.clone())))
                .cloned()
        };
        let mut cache = self.library.lock().await;
        if let Some((at, index)) = cache.as_ref() {
            let age = at.elapsed();
            if age < Duration::from_secs(3600)
                && let Some(found) = find(index)
            {
                return Ok(Some(found));
            }
            if age < Duration::from_secs(60) {
                return Ok(None);
            }
        }
        let mut index = LibraryIndex::new();
        for m in self
            .maintainerr
            .library_titles()
            .await
            .map_err(|e| e.to_string())?
        {
            for (provider, id) in [("tmdb", &m.tmdb_id), ("tvdb", &m.tvdb_id)] {
                if let Some(id) = id.as_ref().filter(|v| !v.is_empty()) {
                    index.insert((m.kind.clone(), format!("{provider}:{id}")), m.clone());
                }
            }
        }
        let found = find(&index);
        *cache = Some((Instant::now(), Arc::new(index)));
        Ok(found)
    }

    // ----- id mapping -----------------------------------------------------------------------

    async fn mapper(&self, who: &Identity) -> Mapper {
        if who.server_id == self.config.maintainerr.server {
            return Mapper::Same;
        }
        match who.kind {
            ServerType::Jellyfin => match self.jellyfin_map(&who.server_id).await {
                Some(m) => Mapper::Index(m),
                None => Mapper::None,
            },
            ServerType::Plex => Mapper::None,
        }
    }

    /// Provider-id index of a Jellyfin server (admin API key), refreshed hourly.
    async fn jellyfin_map(&self, server_id: &str) -> Option<ProviderIndex> {
        if let Some((at, m)) = self
            .jf_index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(server_id)
            && at.elapsed() < Duration::from_secs(3600)
        {
            return Some(m.clone());
        }
        let server = self.config.server(server_id)?;
        let key = self.config.secrets.jellyfin_key(server_id)?;
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Q {
            #[serde(default)]
            items: Vec<I>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct I {
            id: String,
            #[serde(rename = "Type", default)]
            kind: String,
            #[serde(default)]
            provider_ids: HashMap<String, Option<String>>,
        }
        let req = self
            .http
            .get(format!("{}/Items", server.internal()))
            .query(&[
                ("recursive", "true"),
                ("includeItemTypes", "Movie,Series"),
                ("fields", "ProviderIds"),
                ("enableImages", "false"),
                ("enableUserData", "false"),
            ])
            .header("Authorization", format!("MediaBrowser Token=\"{key}\""))
            .header("Accept", "application/json");
        let q: Q = net::send_json(req).await.ok()?;
        let mut m = HashMap::new();
        for i in q.items {
            let kind = if i.kind == "Series" { "show" } else { "movie" };
            for (p, v) in i.provider_ids {
                if let Some(v) = v.filter(|v| !v.is_empty()) {
                    m.insert(
                        (kind.to_owned(), format!("{}:{v}", p.to_ascii_lowercase())),
                        i.id.clone(),
                    );
                }
            }
        }
        self.jf_index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(server_id.to_owned(), (Instant::now(), m.clone()));
        Some(m)
    }
}

enum Mapper {
    /// The caller uses the server Maintainerr manages: ids are the same.
    Same,
    Index(ProviderIndex),
    None,
}

impl Mapper {
    fn map(&self, row: &ItemRow) -> Option<String> {
        match self {
            Mapper::Same => Some(row.key.clone()),
            Mapper::Index(m) => lookup(m, row),
            Mapper::None => None,
        }
    }
}

fn lookup(m: &ProviderIndex, row: &ItemRow) -> Option<String> {
    let kind = row.kind.clone().unwrap_or_else(|| "movie".into());
    let try_id = |p: &str, v: &Option<String>| {
        v.as_ref()
            .and_then(|v| m.get(&(kind.clone(), format!("{p}:{v}"))))
            .cloned()
    };
    // TVDB first for shows (most reliable there), TMDB first for movies.
    if kind == "show" {
        try_id("tvdb", &row.tvdb).or_else(|| try_id("tmdb", &row.tmdb))
    } else {
        try_id("tmdb", &row.tmdb).or_else(|| try_id("tvdb", &row.tvdb))
    }
}

/// Background jobs: periodic sync, reconciler (on demand + every 5 min), watched checks
/// (every 15 min + on webhooks).
/// The in-memory things that outlive a change of settings: rooms, device transfers, sign-ins
/// in progress, attempt limits, the saved settings and the switch signal.
#[derive(Clone, Default)]
pub struct Lasting {
    pub transfers: Arc<crate::transfer::Transfers>,
    pub rooms: Arc<crate::together::Rooms>,
    pub remotes: Arc<crate::remote::Remotes>,
    pub passkeys: Arc<crate::passkey::Flows>,
    pub oidc: Arc<crate::oidc::Oidc>,
    pub limits: Arc<Limits>,
    pub stored: Arc<tokio::sync::Mutex<crate::settings::Stored>>,
    pub reload: Arc<Notify>,
}

impl State {
    pub fn with_lasting(mut self, l: &Lasting) -> Self {
        self.transfers = l.transfers.clone();
        self.rooms = l.rooms.clone();
        self.remotes = l.remotes.clone();
        self.passkeys = l.passkeys.clone();
        self.oidc = l.oidc.clone();
        self.limits = l.limits.clone();
        self.stored = l.stored.clone();
        self.reload = l.reload.clone();
        self
    }

    /// Stops these background jobs between two Maintainerr writes, never in the middle of one.
    pub async fn stop_jobs(&self, jobs: Vec<tokio::task::JoinHandle<()>>) {
        let _between_writes = self.write_lock.lock().await;
        for j in jobs {
            j.abort();
        }
    }
}

/// Starts the background jobs; the handles stop them when the settings change.
pub fn spawn_jobs(state: Shared) -> Vec<tokio::task::JoinHandle<()>> {
    let d = state.clone();
    // The dashboard's graphs: a reading every 30 s even while nobody looks (cheap: one
    // Tautulli call, one Jellyfin call, /proc), so they are never empty when opened.
    let mut jobs = vec![tokio::spawn(async move {
        loop {
            if d.dashboard.idle_due().await {
                d.dash_refresh().await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })];
    if !state.config.keep_on() {
        // No Maintainerr: nothing to read, apply or release. The apps hide Leaving soon.
        state
            .maintainerr_state
            .store(2, std::sync::atomic::Ordering::Relaxed);
        return jobs;
    }
    let s = state.clone();
    jobs.push(tokio::spawn(async move {
        loop {
            let ok = match s.sync().await {
                Ok(n) => {
                    s.last_sync
                        .store(now(), std::sync::atomic::Ordering::Relaxed);
                    s.last_sync_items
                        .store(n, std::sync::atomic::Ordering::Relaxed);
                    true
                }
                Err(e) => {
                    eprintln!("[keep] sync failed: {e}");
                    false
                }
            };
            s.maintainerr_state
                .store(if ok { 1 } else { 2 }, std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(Duration::from_secs(s.config.sync_minutes.max(1) * 60)).await;
        }
    }));
    let s = state.clone();
    jobs.push(tokio::spawn(async move {
        loop {
            if let Err(e) = s.reconcile_once().await {
                eprintln!("[keep] reconcile failed: {e}");
            }
            let _ = tokio::time::timeout(Duration::from_secs(300), s.reconcile.notified()).await;
        }
    }));
    jobs.push(tokio::spawn(async move {
        loop {
            let _ = tokio::time::timeout(Duration::from_secs(900), state.watch.notified()).await;
            if let Err(e) = state.check_watched().await {
                eprintln!("[keep] watched check failed: {e}");
            }
        }
    }));
    jobs
}

/// Admin stats: who keeps what forever, and how much of it someone else keeps too.
pub fn forever_stats(
    votes: &[crate::db::ForeverVote],
    names: &std::collections::HashMap<String, (String, String, keep_service::Avatar)>,
) -> keep_service::ForeverStats {
    use keep_service::{ForeverOverlap, ForeverPerson, ForeverStats, ForeverTitle};
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    let name = |k: &str| {
        names
            .get(k)
            .map(|n| n.0.clone())
            .unwrap_or_else(|| k.to_owned())
    };
    let media_name = |k: &str| names.get(k).map(|n| n.1.clone()).unwrap_or_default();
    let avatar = |k: &str| names.get(k).map(|n| n.2.clone()).unwrap_or_default();
    // Title -> keepers, and person -> titles.
    let mut by_item: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut by_user: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for v in votes {
        by_item.entry(&v.item_key).or_default().insert(&v.user_key);
        by_user.entry(&v.user_key).or_default().insert(&v.item_key);
    }
    let mut people: Vec<ForeverPerson> = by_user
        .iter()
        .map(|(&user, items)| {
            let mut with: HashMap<&str, u32> = HashMap::new();
            let mut shared = 0;
            for item in items {
                let keepers = &by_item[item];
                if keepers.len() > 1 {
                    shared += 1;
                }
                for &other in keepers.iter().filter(|&&o| o != user) {
                    *with.entry(other).or_default() += 1;
                }
            }
            let mut overlaps: Vec<ForeverOverlap> = with
                .into_iter()
                .map(|(k, both)| ForeverOverlap {
                    user_key: k.to_owned(),
                    name: name(k),
                    both,
                })
                .collect();
            overlaps.sort_by(|a, b| b.both.cmp(&a.both).then_with(|| a.name.cmp(&b.name)));
            let count = items.len() as u32;
            ForeverPerson {
                user_key: user.to_owned(),
                name: name(user),
                media_name: media_name(user),
                avatar: avatar(user),
                count,
                shared,
                alone: count - shared,
                overlaps,
            }
        })
        .collect();
    people.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    let first: HashMap<&str, &crate::db::ForeverVote> =
        votes.iter().map(|v| (v.item_key.as_str(), v)).collect();
    let mut items: Vec<ForeverTitle> = by_item
        .iter()
        .map(|(&key, keepers)| {
            let v = first[key];
            ForeverTitle {
                key: key.to_owned(),
                title: v.title.clone().unwrap_or_else(|| key.to_owned()),
                year: v.year,
                poster_url: v.poster.clone(),
                keepers: keepers.iter().map(|k| (*k).to_owned()).collect(),
            }
        })
        .collect();
    items.sort_by(|a, b| {
        b.keepers
            .len()
            .cmp(&a.keepers.len())
            .then_with(|| a.title.cmp(&b.title))
    });
    ForeverStats {
        titles: items.len() as u32,
        shared: items.iter().filter(|i| i.keepers.len() > 1).count() as u32,
        people,
        items,
    }
}
