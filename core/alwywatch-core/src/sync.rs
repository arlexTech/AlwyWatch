// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keep two servers' watch state in sync (a viewer's Plex and Jellyfin accounts), both ways.
//!
//! Each run compares both servers with the state agreed at the previous run (a snapshot per
//! pair): a title changed on one side only takes that side's state, unwatching and rewinding
//! included; changed on both, or never synced, the more advanced one wins (watched beats
//! started, the further resume point beats the nearer). What is watched in AlwyWatch is also
//! mirrored right away (`mirror`), so the periodic run only catches what other apps did.

use std::collections::HashMap;
use std::sync::Arc;

use media_api::{MediaServer, UserData, WatchState};
use serde::{Deserialize, Serialize};

use crate::app::{AlwyWatch, Core};
use crate::error::{CoreError, Result};
use crate::history::{SAME_PLACE_MS, label};

/// A title's state as far as sync is concerned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct St {
    played: bool,
    position_ms: u64,
    #[serde(default)]
    last_played_at: Option<i64>,
}

impl From<&UserData> for St {
    fn from(u: &UserData) -> Self {
        // A watched title keeps its position: a rewatch in progress (Continue watching).
        St {
            played: u.played,
            position_ms: u.position_ms,
            last_played_at: u.last_played_at,
        }
    }
}

/// Same state: both watched or both not, at about the same place (0 for most watched titles).
fn same(a: St, b: St) -> bool {
    a.played == b.played && a.position_ms.abs_diff(b.position_ms) <= SAME_PLACE_MS
}

/// The more advanced of two states: watched if either is, at the place of the one played
/// last (a rewatch started on either side carries over); not watched on either: further on.
fn ahead(a: St, b: St) -> St {
    if !a.played && !b.played {
        return if a.position_ms >= b.position_ms { a } else { b };
    }
    let latest = match (a.last_played_at, b.last_played_at) {
        (Some(x), Some(y)) if x != y => {
            if x > y {
                a
            } else {
                b
            }
        }
        (Some(_), None) => a,
        (None, Some(_)) => b,
        // No dates to tell: the watched side, else the further one.
        _ => match (a.played, b.played) {
            (true, false) => a,
            (false, true) => b,
            _ if a.position_ms >= b.position_ms => a,
            _ => b,
        },
    };
    St {
        played: true,
        ..latest
    }
}

/// What both sides should be, from their states and the one agreed last time.
fn merge(a: St, b: St, last: Option<St>) -> St {
    if same(a, b) {
        return a;
    }
    match last {
        Some(l) => match (!same(a, l), !same(b, l)) {
            (true, false) => a,
            (false, true) => b,
            _ => ahead(a, b),
        },
        None => ahead(a, b),
    }
}

/// Two accounts kept in sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SyncPair {
    pub a: String,
    pub b: String,
}

/// The last run of a pair.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SyncStatus {
    /// Unix seconds; None: never ran.
    pub last_run_at: Option<i64>,
    /// Titles updated on either server by that run.
    pub changes: u32,
    pub failed: u32,
    pub error: Option<String>,
    /// A few of the titles it updated ("Silo S1 · E3 → HomeJelly"), newest first.
    pub recent: Vec<String>,
}

const PAIRS_KEY: &str = "sync.pairs";

fn pair_key(a: &str, b: &str) -> String {
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    format!("{x}\u{1f}{y}")
}

/// Snapshot: the agreed state per linked title ("a id \u{1f} b id", in the pair's order).
type Snapshot = HashMap<String, St>;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Core {
    fn sync_pairs(&self) -> Vec<SyncPair> {
        self.store
            .setting(PAIRS_KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }

    fn snapshot(&self, p: &SyncPair) -> Snapshot {
        self.store
            .cache_get::<Snapshot>(&format!("sync.snapshot.{}", pair_key(&p.a, &p.b)))
            .ok()
            .flatten()
            .map(|c| c.value)
            .unwrap_or_default()
    }

    fn save_snapshot(&self, p: &SyncPair, s: &Snapshot) -> Result<()> {
        Ok(self
            .store
            .cache_put(&format!("sync.snapshot.{}", pair_key(&p.a, &p.b)), s)?)
    }

    fn sync_status(&self, a: &str, b: &str) -> SyncStatus {
        self.store
            .setting(&format!("sync.status.{}", pair_key(a, b)))
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }

    fn save_status(&self, p: &SyncPair, s: &SyncStatus) {
        if let Ok(v) = serde_json::to_string(s) {
            let _ = self
                .store
                .set_setting(&format!("sync.status.{}", pair_key(&p.a, &p.b)), &v);
        }
    }

    async fn write(&self, account: &str, item: &str, s: St) -> Result<()> {
        self.server(account)?
            .set_watch_state(
                item,
                &WatchState {
                    played: s.played,
                    position_ms: s.position_ms,
                    last_played_at: s.last_played_at,
                },
            )
            .await?;
        Ok(())
    }

    /// One run for a pair: read both histories, link titles both ways, merge, write.
    async fn sync_pair(&self, p: &SyncPair) -> Result<SyncStatus> {
        let (a, b) = (p.a.as_str(), p.b.as_str());
        let (name_a, name_b) = (self.server_name(a), self.server_name(b));
        self.progress(
            i18n::tr!("Reading what you watched on {server}", server = name_a),
            0,
            0,
        );
        let hist_a = self.server(a)?.watch_history().await?;
        self.progress(
            i18n::tr!("Reading what you watched on {server}", server = name_b),
            0,
            0,
        );
        let hist_b = self.server(b)?.watch_history().await?;
        let state_a: HashMap<&str, St> = hist_a
            .iter()
            .map(|i| (i.id.as_str(), St::from(&i.user)))
            .collect();
        let state_b: HashMap<&str, St> = hist_b
            .iter()
            .map(|i| (i.id.as_str(), St::from(&i.user)))
            .collect();

        // Links: titles in either history, matched on the other server.
        let mut links: HashMap<(String, String), String> = HashMap::new();
        for (src, dst) in self.match_on(a, b, &hist_a).await? {
            if let Some(d) = dst {
                links.insert((src.id.clone(), d.id), label(&src));
            }
        }
        for (src, dst) in self.match_on(b, a, &hist_b).await? {
            if let Some(d) = dst {
                links
                    .entry((d.id, src.id.clone()))
                    .or_insert_with(|| label(&src));
            }
        }

        let last = self.snapshot(p);
        let mut next = Snapshot::new();
        let mut status = SyncStatus {
            last_run_at: Some(now()),
            ..Default::default()
        };
        let total = links.len() as u32;
        let step = i18n::tr!(
            "Syncing {server} and {other}",
            server = name_a,
            other = name_b
        );
        for (n, ((ia, ib), title)) in links.into_iter().enumerate() {
            if n % 50 == 0 {
                self.progress(step.clone(), n as u32, total);
            }
            let sa = state_a.get(ia.as_str()).copied().unwrap_or_default();
            let sb = state_b.get(ib.as_str()).copied().unwrap_or_default();
            let key = format!("{ia}\u{1f}{ib}");
            let want = merge(sa, sb, last.get(&key).copied());
            let mut ok = true;
            for (acct, item, have, name) in [(a, &ia, sa, &name_a), (b, &ib, sb, &name_b)] {
                if same(want, have) {
                    continue;
                }
                match self.write(acct, item, want).await {
                    Ok(()) => {
                        status.changes += 1;
                        if status.recent.len() < 20 {
                            status.recent.push(format!("{title} → {name}"));
                        }
                    }
                    Err(_) => {
                        status.failed += 1;
                        ok = false;
                    }
                }
            }
            // A failed write leaves the title out: next run settles it as never synced.
            if ok {
                next.insert(key, want);
            }
        }
        self.save_snapshot(p, &next)?;
        if status.changes > 0 {
            let _ = self.watch_state_changed(a);
            let _ = self.watch_state_changed(b);
        }
        Ok(status)
    }

    /// Runs every enabled pair, recording each one's status.
    pub(crate) async fn sync_all(&self) -> Vec<SyncStatus> {
        let mut out = Vec::new();
        for p in self.sync_pairs() {
            let status = match self.sync_pair(&p).await {
                Ok(s) => s,
                Err(e) => SyncStatus {
                    last_run_at: Some(now()),
                    error: Some(e.to_string()),
                    ..self.sync_status(&p.a, &p.b)
                },
            };
            self.save_status(&p, &status);
            out.push(status);
        }
        self.progress_end();
        out
    }

    /// After watching in AlwyWatch: the same state on the paired server, right away (for titles
    /// already linked by a run). Errors are left to the next run.
    pub(crate) async fn mirror(self: Arc<Self>, account: String, item: String) {
        for p in self.sync_pairs() {
            let (this_is_a, other) = if p.a == account {
                (true, p.b.clone())
            } else if p.b == account {
                (false, p.a.clone())
            } else {
                continue;
            };
            let mut snap = self.snapshot(&p);
            let Some((key, other_item)) = snap.keys().find_map(|k| {
                let (ia, ib) = k.split_once('\u{1f}')?;
                let (mine, theirs) = if this_is_a { (ia, ib) } else { (ib, ia) };
                (mine == item).then(|| (k.clone(), theirs.to_owned()))
            }) else {
                continue;
            };
            let Ok(srv) = self.server(&account) else {
                continue;
            };
            let Ok(now) = srv.item(&item).await else {
                continue;
            };
            let st = St::from(&now.user);
            if snap.get(&key).is_some_and(|l| same(*l, st)) {
                continue;
            }
            if self.write(&other, &other_item, st).await.is_ok() {
                snap.insert(key, st);
                let _ = self.save_snapshot(&p, &snap);
                let _ = self.watch_state_changed(&other);
            }
        }
    }
}

#[uniffi::export]
impl AlwyWatch {
    /// The pairs of accounts kept in sync.
    pub fn sync_pairs(&self) -> Vec<SyncPair> {
        self.core().sync_pairs()
    }

    /// Turns sync of two accounts on or off. Turning it off forgets the pair's snapshot (the
    /// next time it is turned on starts as a first sync: the more advanced state wins).
    pub fn set_sync_pair(&self, a: String, b: String, on: bool) -> Result<()> {
        if a == b {
            return Err(CoreError::Invalid {
                detail: i18n::tr!("pick two different servers"),
            });
        }
        let c = self.core();
        let mut pairs = c.sync_pairs();
        let k = pair_key(&a, &b);
        pairs.retain(|p| pair_key(&p.a, &p.b) != k);
        if on {
            pairs.push(SyncPair { a, b });
        } else {
            let _ = c
                .store
                .cache_put(&format!("sync.snapshot.{k}"), &Snapshot::new());
        }
        let v = serde_json::to_string(&pairs).map_err(|e| CoreError::Internal {
            detail: e.to_string(),
        })?;
        Ok(c.store.set_setting(PAIRS_KEY, &v)?)
    }

    pub fn sync_status(&self, a: String, b: String) -> SyncStatus {
        self.core().sync_status(&a, &b)
    }

    /// Syncs every enabled pair now (reads both histories: a minute on big libraries). Progress
    /// through `history_copy_progress`.
    pub async fn sync_now(&self) -> Result<Vec<SyncStatus>> {
        self.run(|c| async move { Ok(c.sync_all().await) }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> St {
        St {
            position_ms: ms,
            ..Default::default()
        }
    }

    fn watched() -> St {
        St {
            played: true,
            ..Default::default()
        }
    }

    #[test]
    fn first_sync_takes_the_more_advanced() {
        assert_eq!(merge(at(0), watched(), None), watched());
        assert_eq!(merge(at(600_000), at(60_000), None), at(600_000));
        assert_eq!(merge(at(60_000), at(600_000), None), at(600_000));
    }

    #[test]
    fn a_change_on_one_side_wins_even_backwards() {
        // Both watched last time; unmarked on A since: B follows.
        assert_eq!(merge(at(0), watched(), Some(watched())), at(0));
        // Rewound on B: A follows.
        assert_eq!(
            merge(at(900_000), at(100_000), Some(at(900_000))),
            at(100_000)
        );
        // Moved forward on A.
        assert_eq!(
            merge(at(1_200_000), at(900_000), Some(at(900_000))),
            at(1_200_000)
        );
    }

    #[test]
    fn both_changed_takes_the_more_advanced_and_small_moves_are_the_same_place() {
        assert_eq!(merge(at(300_000), watched(), Some(at(100_000))), watched());
        assert!(same(at(100_000), at(120_000)));
        // Agreed (within 30 s): nothing to write.
        assert_eq!(merge(at(100_000), at(120_000), Some(at(0))), at(100_000));
    }

    #[test]
    fn a_rewatch_in_progress_carries_over_either_way() {
        let on = |ms: u64, played: bool, t: i64| St {
            played,
            position_ms: ms,
            last_played_at: Some(t),
        };
        // Watched on both, started again on A since: B resumes there too, still watched.
        assert_eq!(
            merge(
                on(1_500_000, true, 200),
                on(0, true, 100),
                Some(on(0, true, 100))
            ),
            on(1_500_000, true, 200)
        );
        // First sync: watched long ago on B, started on A lately: watched, at A's place.
        assert_eq!(
            merge(on(600_000, false, 200), on(0, true, 100), None),
            on(600_000, true, 200)
        );
        // Finished on B after A's old start: watched, nothing to resume.
        assert_eq!(
            merge(on(600_000, false, 100), on(0, true, 200), None),
            on(0, true, 200)
        );
    }

    #[test]
    fn pair_keys_ignore_the_order() {
        assert_eq!(
            pair_key("plex:1", "jellyfin:2"),
            pair_key("jellyfin:2", "plex:1")
        );
    }
}
