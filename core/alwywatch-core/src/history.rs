// SPDX-License-Identifier: AGPL-3.0-or-later
//! Copy a viewer's watch history from one server to another (Plex → Jellyfin when moving, or
//! the other way): what they watched and where they stopped. Each person does it with their own
//! accounts, so no admin access or user mapping is needed.
//!
//! Titles are matched by external ids (TMDB, IMDb, TVDB), then by title and year; episodes by
//! their show and season/episode number. It only adds: nothing is unmarked, and a resume point
//! never moves backwards. `history_copy_plan` writes nothing, so the viewer sees what will
//! change before `history_copy_apply`.

use std::collections::HashMap;

use media_api::{Item, ItemKind, MediaServer, UserData, WatchState};

use crate::app::{AlwyWatch, Core, ids_match, same_title};
use crate::error::{CoreError, Result};

/// A resume point this close to the target's is the same place (no change).
pub(crate) const SAME_PLACE_MS: u64 = 30_000;

/// One item to update on the target server.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HistoryChange {
    /// The item on the target server.
    pub item_id: String,
    /// "Arrival (2016)", "Silo S1 · E3".
    pub label: String,
    pub played: bool,
    pub position_ms: u64,
    pub last_played_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct HistoryPlan {
    pub changes: Vec<HistoryChange>,
    /// Already watched, or at the same place or further, on the target.
    pub up_to_date: u32,
    /// Titles the target server doesn't have (labels).
    pub not_found: Vec<String>,
}

/// Where a check or a copy is: a step, and a count when it is known (`total` 0: not yet).
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct HistoryProgress {
    pub running: bool,
    pub step: String,
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct HistoryCopyResult {
    pub applied: u32,
    pub failed: u32,
}

pub(crate) fn label(i: &Item) -> String {
    match (i.kind, i.season_number, i.episode_number) {
        (ItemKind::Episode, Some(s), Some(e)) => format!(
            "{} S{s} · E{e}",
            i.series_title.as_deref().unwrap_or(&i.title)
        ),
        _ => match i.year {
            Some(y) => format!("{} ({y})", i.title),
            None => i.title.clone(),
        },
    }
}

/// The same movie or show among `candidates`: external ids first, then title and year.
fn same_title_in<'a>(src: &Item, candidates: &'a [Item]) -> Option<&'a Item> {
    let p = &src.provider_ids;
    candidates
        .iter()
        .find(|c| ids_match(&c.provider_ids, &p.tmdb, &p.imdb, &p.tvdb))
        .or_else(|| {
            candidates.iter().find(|c| {
                same_title(&c.title, &src.title)
                    && (src.year.is_none() || c.year.is_none() || c.year == src.year)
            })
        })
}

/// What to write on the target for `src`, given the target's state; None when up to date.
fn decide(src: &UserData, target: Option<&UserData>) -> Option<WatchState> {
    if target.is_some_and(|t| t.played) {
        return None;
    }
    if src.played {
        return Some(WatchState {
            played: true,
            position_ms: 0,
            last_played_at: src.last_played_at,
        });
    }
    let at = target.map_or(0, |t| t.position_ms);
    (src.position_ms > at + SAME_PLACE_MS).then_some(WatchState {
        played: false,
        position_ms: src.position_ms,
        last_played_at: src.last_played_at,
    })
}

/// Builds the plan once the target items are matched: `matched` pairs a source item with its
/// target item (None: not on the target); `target_state` is the target's history by id.
fn plan_from(
    matched: Vec<(&Item, Option<&Item>)>,
    target_state: &HashMap<String, UserData>,
) -> HistoryPlan {
    let mut plan = HistoryPlan::default();
    for (src, target) in matched {
        let Some(t) = target else {
            plan.not_found.push(label(src));
            continue;
        };
        match decide(&src.user, target_state.get(&t.id)) {
            Some(w) => plan.changes.push(HistoryChange {
                item_id: t.id.clone(),
                label: label(src),
                played: w.played,
                position_ms: w.position_ms,
                last_played_at: w.last_played_at,
            }),
            None => plan.up_to_date += 1,
        }
    }
    plan.not_found.sort();
    plan.not_found.dedup();
    plan
}

impl Core {
    pub(crate) fn progress(&self, step: impl Into<String>, done: u32, total: u32) {
        *self.history.lock().unwrap_or_else(|e| e.into_inner()) = HistoryProgress {
            running: true,
            step: step.into(),
            done,
            total,
        };
    }

    pub(crate) fn progress_end(&self) {
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .running = false;
    }

    pub(crate) fn server_name(&self, account: &str) -> String {
        let accounts: Vec<crate::Account> = self.store.accounts().unwrap_or_default();
        accounts
            .into_iter()
            .find(|a| a.id == account)
            .map_or_else(|| i18n::tr!("the server"), |a| a.server_name)
    }

    async fn history_plan(&self, from: &str, to: &str) -> Result<HistoryPlan> {
        if from == to {
            return Err(CoreError::Invalid {
                detail: i18n::tr!("pick two different servers"),
            });
        }
        let from_name = self.server_name(from);
        self.progress(
            i18n::tr!("Reading what you watched on {server}", server = from_name),
            0,
            0,
        );
        let history = self.server(from)?.watch_history().await?;
        let target_state = self.history_state(to).await?;
        let matched = self.match_on(from, to, &history).await?;
        Ok(plan_from(
            matched.iter().map(|(s, t)| (s, t.as_ref())).collect(),
            &target_state,
        ))
    }

    /// An account's history by item id (watched or started items only).
    pub(crate) async fn history_state(&self, account: &str) -> Result<HashMap<String, UserData>> {
        let name = self.server_name(account);
        self.progress(
            i18n::tr!("Reading what you watched on {server}", server = name),
            0,
            0,
        );
        Ok(self
            .server(account)?
            .watch_history()
            .await?
            .into_iter()
            .map(|i| (i.id, i.user))
            .collect())
    }

    /// Each item of `history` (from `from`) with the same title on `to`, if it has it.
    pub(crate) async fn match_on(
        &self,
        from: &str,
        to: &str,
        history: &[Item],
    ) -> Result<Vec<(Item, Option<Item>)>> {
        let (src, dst) = (self.server(from)?, self.server(to)?);
        let to_name = self.server_name(to);
        let src_shows = src.provider_index(ItemKind::Series).await?;
        self.progress(
            i18n::tr!("Listing the movies and shows on {server}", server = to_name),
            0,
            0,
        );
        let dst_movies = dst.provider_index(ItemKind::Movie).await?;
        let dst_shows = dst.provider_index(ItemKind::Series).await?;
        let show_count = history
            .iter()
            .filter_map(|i| i.series_id.as_ref())
            .collect::<std::collections::HashSet<_>>()
            .len() as u32;
        self.progress(
            i18n::tr!("Finding your shows' episodes on {server}", server = to_name),
            0,
            show_count,
        );

        // Each show's episodes on the target, by (season, episode), fetched once per show.
        let mut episodes: HashMap<String, HashMap<(u32, u32), Item>> = HashMap::new();
        let mut show_of: HashMap<String, Option<String>> = HashMap::new();
        for e in history.iter().filter(|i| i.kind == ItemKind::Episode) {
            let Some(sid) = &e.series_id else { continue };
            if show_of.contains_key(sid) {
                continue;
            }
            // The source show (for its ids), else the episode's own show title.
            let src_show = src_shows
                .iter()
                .find(|s| &s.id == sid)
                .cloned()
                .unwrap_or(Item {
                    kind: ItemKind::Series,
                    title: e.series_title.clone().unwrap_or_default(),
                    year: None,
                    ..e.clone()
                });
            let target = same_title_in(&src_show, &dst_shows).map(|s| s.id.clone());
            if let Some(tid) = &target
                && !episodes.contains_key(tid)
            {
                let mut by_number = HashMap::new();
                for season in dst.seasons(tid).await? {
                    for ep in dst.episodes(tid, &season.id).await? {
                        if let (Some(s), Some(n)) = (ep.season_number, ep.episode_number) {
                            by_number.insert((s, n), ep);
                        }
                    }
                }
                episodes.insert(tid.clone(), by_number);
            }
            show_of.insert(sid.clone(), target);
            self.progress(
                i18n::tr!("Finding your shows' episodes on {server}", server = to_name),
                show_of.len() as u32,
                show_count,
            );
        }

        Ok(history
            .iter()
            .map(|i| {
                let target = match i.kind {
                    ItemKind::Movie => same_title_in(i, &dst_movies).cloned(),
                    ItemKind::Episode => i
                        .series_id
                        .as_ref()
                        .and_then(|sid| show_of.get(sid).cloned().flatten())
                        .and_then(|tid| {
                            let key = (i.season_number?, i.episode_number?);
                            episodes.get(&tid)?.get(&key).cloned()
                        }),
                    _ => None,
                };
                (i.clone(), target)
            })
            .collect())
    }
}

#[uniffi::export]
impl AlwyWatch {
    /// What copying `from`'s watch history to `to` would change (writes nothing). Reads the
    /// whole history of both: can take a minute on large libraries.
    pub async fn history_copy_plan(
        &self,
        from_account: String,
        to_account: String,
    ) -> Result<HistoryPlan> {
        self.run(|c| async move {
            let r = c.history_plan(&from_account, &to_account).await;
            c.progress_end();
            r
        })
        .await
    }

    /// Where the running check or copy is (poll it while waiting).
    pub fn history_copy_progress(&self) -> HistoryProgress {
        self.history_progress()
    }

    /// Writes the plan's changes on `to` (one request each); continues past failures.
    pub async fn history_copy_apply(
        &self,
        to_account: String,
        changes: Vec<HistoryChange>,
    ) -> Result<HistoryCopyResult> {
        self.run(|c| async move {
            let srv = c.server(&to_account)?;
            let mut r = HistoryCopyResult::default();
            let total = changes.len() as u32;
            let step = i18n::tr!("Copying to {server}", server = c.server_name(&to_account));

            c.progress(step.clone(), 0, total);
            for ch in changes {
                let state = WatchState {
                    played: ch.played,
                    position_ms: ch.position_ms,
                    last_played_at: ch.last_played_at,
                };
                match srv.set_watch_state(&ch.item_id, &state).await {
                    Ok(()) => r.applied += 1,
                    Err(_) => r.failed += 1,
                }
                c.progress(step.clone(), r.applied + r.failed, total);
            }
            c.progress_end();
            Ok(r)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_api::ProviderIds;

    fn item(id: &str, kind: ItemKind, title: &str, user: UserData) -> Item {
        Item {
            id: id.into(),
            kind,
            title: title.into(),
            sort_title: None,
            overview: None,
            year: Some(2016),
            runtime_ms: None,
            community_rating: None,
            official_rating: None,
            genres: vec![],
            provider_ids: ProviderIds::default(),
            series_id: None,
            series_title: None,
            season_id: None,
            season_number: None,
            episode_number: None,
            child_count: None,
            premiere_date: None,
            images: vec![],
            user,
        }
    }

    fn played() -> UserData {
        UserData {
            played: true,
            last_played_at: Some(1_700_000_000),
            ..Default::default()
        }
    }

    fn at(ms: u64) -> UserData {
        UserData {
            position_ms: ms,
            ..Default::default()
        }
    }

    #[test]
    fn only_adds_and_never_goes_backwards() {
        // Watched on the source: marked watched.
        assert!(decide(&played(), None).unwrap().played);
        // Already watched on the target: nothing, even from a started source.
        assert_eq!(decide(&at(600_000), Some(&played())), None);
        // Further on the source: moves forward.
        assert_eq!(
            decide(&at(600_000), Some(&at(60_000))).unwrap().position_ms,
            600_000
        );
        // Same place, or further on the target: nothing.
        assert_eq!(decide(&at(600_000), Some(&at(590_000))), None);
        assert_eq!(decide(&at(600_000), Some(&at(900_000))), None);
    }

    #[test]
    fn matches_by_ids_then_title_and_year() {
        let mut src = item("p1", ItemKind::Movie, "Arrival", played());
        src.provider_ids.tmdb = Some("329865".into());
        let mut by_id = item("j9", ItemKind::Movie, "L'Arrivée", UserData::default());
        by_id.provider_ids.tmdb = Some("329865".into());
        let by_title = item("j1", ItemKind::Movie, "arrival", UserData::default());
        assert_eq!(
            same_title_in(&src, &[by_title.clone(), by_id]).unwrap().id,
            "j9"
        );
        assert_eq!(
            same_title_in(&src, std::slice::from_ref(&by_title))
                .unwrap()
                .id,
            "j1"
        );
        let mut other_year = by_title;
        other_year.year = Some(1999);
        assert!(same_title_in(&src, &[other_year]).is_none());
    }

    #[test]
    fn plan_lists_changes_up_to_date_and_missing() {
        let a = item("p1", ItemKind::Movie, "Arrival", played());
        let mut ep = item("p2", ItemKind::Episode, "Pilot", at(1_200_000));
        ep.series_title = Some("Silo".into());
        (ep.season_number, ep.episode_number) = (Some(1), Some(3));
        let gone = item("p3", ItemKind::Movie, "Gone", played());
        let ta = item("j1", ItemKind::Movie, "Arrival", UserData::default());
        let te = item("j2", ItemKind::Episode, "Pilot", UserData::default());
        let state = HashMap::from([("j1".to_owned(), played())]);
        let plan = plan_from(
            vec![(&a, Some(&ta)), (&ep, Some(&te)), (&gone, None)],
            &state,
        );
        assert_eq!(plan.up_to_date, 1);
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(plan.changes[0].label, "Silo S1 · E3");
        assert_eq!(plan.changes[0].position_ms, 1_200_000);
        assert_eq!(plan.not_found, vec!["Gone (2016)".to_owned()]);
    }
}
