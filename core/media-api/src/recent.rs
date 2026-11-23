// SPDX-License-Identifier: AGPL-3.0-or-later
//! "Recently added" of a shows library as the apps show it: one card per show, newest arrival
//! first, each saying what is new ("S1 · E6 · Edenia", "S2 · E3–E5", "New show"…).
//!
//! The servers list episodes (Plex, Jellyfin with `groupItems=false`), sometimes a season
//! (Plex, when a whole season arrived) or the show itself (Jellyfin's grouped mode, a show
//! added at once). Everything here is pure, shared by the core (phone, TV, desktop) and the web.

use std::collections::HashMap;

use i18n::{tr, trn};
use serde::{Deserialize, Serialize};

use crate::model::{ImageKind, ImageRef, Item, ItemKind, ProviderIds};

/// What is new for one card of a "recently added" row: the line under its poster and what
/// opening it shows. Cards without a note (movies) show their usual text and open themselves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct RailNote {
    /// The card's item (the show).
    pub item_id: String,
    /// "S1 · E6 · Edenia", "S2 · E3–E5", "S2 · 3 new episodes", "Season 2",
    /// "2 new seasons", "5 new episodes", "New show · 8 episodes".
    pub label: String,
    /// What opening the card shows: the episode when only one arrived, else the show.
    pub open_id: String,
    /// The season to show on the show page (the first season concerned), when known.
    pub open_season_id: Option<String>,
}

/// A "recently added" row: the cards and a note for each show card.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentlyAdded {
    pub items: Vec<Item>,
    pub notes: Vec<RailNote>,
}

/// Groups a server's "recently added" list (newest first) into at most `max` cards: one per
/// show, placed at its newest arrival, with a note saying what is new. Movies and anything that
/// is not part of a show pass through as they are, without a note.
pub fn recently_added(items: Vec<Item>, max: usize) -> RecentlyAdded {
    // Cards in order of first appearance; a show's entries gathered under its card.
    let mut order: Vec<Result<String, Item>> = Vec::new();
    let mut shows: HashMap<String, Vec<Item>> = HashMap::new();
    for i in items {
        let show = match i.kind {
            ItemKind::Series => Some(i.id.clone()),
            ItemKind::Episode | ItemKind::Season => i.series_id.clone(),
            _ => None,
        };
        match show {
            Some(id) => {
                let entries = shows.entry(id.clone()).or_default();
                if entries.is_empty() {
                    order.push(Ok(id));
                }
                entries.push(i);
            }
            None => order.push(Err(i)),
        }
    }
    let mut out = RecentlyAdded {
        items: Vec::new(),
        notes: Vec::new(),
    };
    for card in order.into_iter().take(max) {
        match card {
            Err(item) => out.items.push(item),
            Ok(id) => {
                let entries = &shows[&id];
                let item = entries
                    .iter()
                    .find(|e| e.kind == ItemKind::Series)
                    .cloned()
                    .unwrap_or_else(|| show_card(&entries[0]));
                out.notes.push(note(&item.id, entries));
                out.items.push(item);
            }
        }
    }
    out
}

/// The show of an episode or season, as a card: the show's id, title and poster (an episode's
/// own image is a still), its backdrop. Anything else is returned as it is.
pub fn show_card(i: &Item) -> Item {
    let (ItemKind::Episode | ItemKind::Season, Some(series_id)) = (i.kind, i.series_id.clone())
    else {
        return i.clone();
    };
    let poster = i
        .images
        .iter()
        .find(|im| im.kind == ImageKind::SeriesPrimary)
        .or_else(|| i.images.iter().find(|im| im.kind == ImageKind::Primary))
        .cloned()
        .map(|im| ImageRef {
            kind: ImageKind::Primary,
            ..im
        });
    let images = poster
        .into_iter()
        .chain(
            i.images
                .iter()
                .filter(|im| im.kind == ImageKind::Backdrop)
                .cloned(),
        )
        .collect();
    Item {
        id: series_id,
        kind: ItemKind::Series,
        title: i.series_title.clone().unwrap_or_else(|| i.title.clone()),
        series_id: None,
        series_title: None,
        season_id: None,
        season_number: None,
        episode_number: None,
        child_count: None,
        // The episode's ids are not the show's: matching falls back to the title.
        provider_ids: ProviderIds::default(),
        overview: None,
        runtime_ms: None,
        images,
        ..i.clone()
    }
}

/// "Episode 8", "Épisode 8", "8": a title that only repeats the number.
fn generic_title(title: &str, number: u32) -> bool {
    let t: String = title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let n = number.to_string();
    t == n || t == format!("episode{n}") || t == format!("épisode{n}") || t == format!("ep{n}")
}

/// What is new for one show, from its entries in the server's list (at least one).
fn note(show_id: &str, entries: &[Item]) -> RailNote {
    let to_show = |label: String, season: Option<String>| RailNote {
        item_id: show_id.to_owned(),
        label,
        open_id: show_id.to_owned(),
        open_season_id: season,
    };
    let episodes: Vec<&Item> = entries
        .iter()
        .filter(|e| e.kind == ItemKind::Episode)
        .collect();
    let seasons: Vec<&Item> = entries
        .iter()
        .filter(|e| e.kind == ItemKind::Season)
        .collect();
    let show = entries.iter().find(|e| e.kind == ItemKind::Series);

    // One new episode: it, opened on its own page (Play is one press away).
    if let ([e], [], None) = (episodes.as_slice(), seasons.as_slice(), show) {
        let label = match (e.season_number, e.episode_number) {
            (Some(s), Some(n)) if generic_title(&e.title, n) || e.title.is_empty() => {
                format!("S{s} · E{n}")
            }
            (Some(s), Some(n)) => format!("S{s} · E{n} · {}", e.title),
            _ => e.title.clone(),
        };
        return RailNote {
            item_id: show_id.to_owned(),
            label,
            open_id: e.id.clone(),
            open_season_id: e.season_id.clone(),
        };
    }

    // The seasons concerned (by number), each with its id, from episodes and season entries.
    let mut season_ids: Vec<(Option<u32>, Option<String>)> = Vec::new();
    for (n, id) in episodes
        .iter()
        .map(|e| (e.season_number, e.season_id.clone()))
        .chain(
            seasons
                .iter()
                .map(|s| (s.season_number, Some(s.id.clone()))),
        )
    {
        match season_ids.iter_mut().find(|(m, _)| *m == n) {
            Some((_, known)) => {
                if known.is_none() {
                    *known = id;
                }
            }
            None => season_ids.push((n, id)),
        }
    }
    // The first season concerned: where watching what is new starts.
    season_ids.sort_by_key(|(n, _)| n.unwrap_or(u32::MAX));
    let first_season = season_ids.first().and_then(|(_, id)| id.clone());
    let numbered: Vec<u32> = season_ids.iter().filter_map(|(n, _)| *n).collect();
    // Episodes counted from the episode entries plus what season entries contain.
    let count = episodes.len()
        + seasons
            .iter()
            .map(|s| s.child_count.unwrap_or(0) as usize)
            .sum::<usize>();

    // A show added at once: the server listed the show itself, or its very first episode is
    // among the new ones.
    let starts = show.is_some()
        || (episodes.iter()).any(|e| (e.season_number, e.episode_number) == (Some(1), Some(1)));
    if starts {
        let season_count = if numbered.is_empty() {
            show.and_then(|s| s.child_count).unwrap_or(0) as usize
        } else {
            numbered.len()
        };
        let label = if season_count > 1 {
            trn!(
                "New show · {n} season",
                "New show · {n} seasons",
                season_count
            )
        } else if count > 1 {
            trn!("New show · {n} episode", "New show · {n} episodes", count)
        } else {
            tr!("New show")
        };
        return to_show(label, first_season);
    }

    // One season.
    if let [(Some(s), season_id)] = season_ids.as_slice() {
        let season_id = season_id.clone();
        if !seasons.is_empty() {
            return to_show(tr!("Season {number}", number = s), season_id);
        }
        let mut numbers: Vec<u32> = episodes.iter().filter_map(|e| e.episode_number).collect();
        numbers.sort_unstable();
        numbers.dedup();
        let label = match (numbers.first(), numbers.last()) {
            (Some(&a), Some(&b))
                if numbers.len() == episodes.len() && (b - a) as usize + 1 == numbers.len() =>
            {
                if a == b {
                    format!("S{s} · E{a}")
                } else {
                    format!("S{s} · E{a}–E{b}")
                }
            }
            _ => trn!(
                "S{season} · {n} new episode",
                "S{season} · {n} new episodes",
                episodes.len(),
                season = s
            ),
        };
        return to_show(label, season_id);
    }

    // Several seasons, or numbers the server did not give.
    let label = if episodes.is_empty() && numbered.len() > 1 {
        trn!("{n} new season", "{n} new seasons", numbered.len())
    } else if count > 0 {
        trn!("{n} new episode", "{n} new episodes", count)
    } else {
        tr!("New episodes")
    };
    to_show(label, first_season)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UserData;

    fn item(id: &str, kind: ItemKind, title: &str) -> Item {
        Item {
            id: id.into(),
            kind,
            title: title.into(),
            sort_title: None,
            overview: None,
            year: None,
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
            user: UserData::default(),
        }
    }

    /// An episode of show `show` ("<show>-s<season>" is its season's id).
    fn ep(show: &str, s: u32, e: u32, title: &str) -> Item {
        Item {
            series_id: Some(show.into()),
            series_title: Some(show.to_uppercase()),
            season_id: Some(format!("{show}-s{s}")),
            season_number: Some(s),
            episode_number: Some(e),
            images: vec![
                ImageRef {
                    kind: ImageKind::Primary,
                    path: format!("still-{show}-{s}-{e}"),
                    blurhash: None,
                },
                ImageRef {
                    kind: ImageKind::SeriesPrimary,
                    path: format!("poster-{show}"),
                    blurhash: None,
                },
            ],
            ..item(&format!("{show}-{s}x{e}"), ItemKind::Episode, title)
        }
    }

    fn season(show: &str, s: u32, episodes: Option<u32>) -> Item {
        Item {
            series_id: Some(show.into()),
            series_title: Some(show.to_uppercase()),
            season_number: Some(s),
            child_count: episodes,
            ..item(
                &format!("{show}-s{s}"),
                ItemKind::Season,
                &format!("Season {s}"),
            )
        }
    }

    fn labels(r: &RecentlyAdded) -> Vec<(String, String)> {
        r.notes
            .iter()
            .map(|n| (n.item_id.clone(), n.label.clone()))
            .collect()
    }

    #[test]
    fn one_new_episode_says_which_and_opens_it() {
        let r = recently_added(vec![ep("dreamland", 1, 6, "Edenia")], 20);
        assert_eq!(r.items.len(), 1);
        let card = &r.items[0];
        assert_eq!(
            (card.id.as_str(), card.kind, card.title.as_str()),
            ("dreamland", ItemKind::Series, "DREAMLAND")
        );
        // The show's poster, not the episode's still.
        assert_eq!(card.images[0].path, "poster-dreamland");
        assert_eq!(card.images[0].kind, ImageKind::Primary);
        assert_eq!(
            r.notes,
            [RailNote {
                item_id: "dreamland".into(),
                label: "S1 · E6 · Edenia".into(),
                open_id: "dreamland-1x6".into(),
                open_season_id: Some("dreamland-s1".into()),
            }]
        );
    }

    #[test]
    fn a_title_that_only_repeats_the_number_is_left_out() {
        let r = recently_added(vec![ep("clover", 2, 8, "Épisode 8")], 20);
        assert_eq!(r.notes[0].label, "S2 · E8");
    }

    #[test]
    fn episodes_of_one_season_as_a_range_or_a_count() {
        let r = recently_added(
            vec![
                ep("scrubs", 2, 3, "c"),
                ep("scrubs", 2, 1, "a"),
                ep("scrubs", 2, 2, "b"),
                ep("raven", 1, 10, "x"),
                ep("raven", 1, 8, "y"),
            ],
            20,
        );
        assert_eq!(
            labels(&r),
            [
                ("scrubs".into(), "S2 · E1–E3".into()),
                ("raven".into(), "S1 · 2 new episodes".into()),
            ]
        );
        // The show page, on the season concerned.
        assert_eq!(r.notes[0].open_id, "scrubs");
        assert_eq!(r.notes[0].open_season_id.as_deref(), Some("scrubs-s2"));
    }

    #[test]
    fn whole_seasons_and_episodes_across_seasons() {
        let r = recently_added(
            vec![
                season("naruto", 10, Some(26)),
                season("astrid", 6, Some(8)),
                season("astrid", 7, Some(8)),
                ep("greys", 3, 1, "a"),
                ep("greys", 2, 27, "b"),
                ep("greys", 2, 26, "c"),
                ep("greys", 3, 2, "d"),
                ep("greys", 3, 3, "e"),
            ],
            20,
        );
        assert_eq!(
            labels(&r),
            [
                ("naruto".into(), "Season 10".into()),
                ("astrid".into(), "2 new seasons".into()),
                ("greys".into(), "5 new episodes".into()),
            ]
        );
        // Opened on the first season concerned.
        assert_eq!(r.notes[2].open_season_id.as_deref(), Some("greys-s2"));
        assert_eq!(r.notes[1].open_season_id.as_deref(), Some("astrid-s6"));
    }

    #[test]
    fn a_show_added_at_once_is_a_new_show() {
        let mut pendragon = item("pendragon", ItemKind::Series, "The Pendragon Cycle");
        pendragon.child_count = Some(1);
        pendragon.overview = Some("Merlin".into());
        let mut chicago = item("med", ItemKind::Series, "Chicago Med");
        chicago.child_count = Some(2);
        let r = recently_added(
            vec![
                pendragon,
                ep("dieu", 1, 2, "b"),
                ep("dieu", 1, 1, "a"),
                ep("dieu", 1, 3, "c"),
                chicago,
                ep("flynne", 2, 1, "x"),
                ep("flynne", 1, 1, "y"),
            ],
            20,
        );
        assert_eq!(
            labels(&r),
            [
                ("pendragon".into(), "New show".into()),
                ("dieu".into(), "New show · 3 episodes".into()),
                ("med".into(), "New show · 2 seasons".into()),
                ("flynne".into(), "New show · 2 seasons".into()),
            ]
        );
        // The show itself (with its details) is the card when the server listed it.
        assert_eq!(r.items[0].overview.as_deref(), Some("Merlin"));
        assert_eq!(r.notes[1].open_id, "dieu");
        assert_eq!(r.notes[3].open_season_id.as_deref(), Some("flynne-s1"));
    }

    #[test]
    fn unknown_numbers_fall_back_to_counts() {
        // Jellyfin's grouped entries: a season without its episode count, episodes without
        // numbers.
        let mut a = ep("x", 1, 1, "a");
        let mut b = ep("x", 1, 2, "b");
        for e in [&mut a, &mut b] {
            e.season_number = None;
            e.episode_number = None;
            e.season_id = None;
        }
        let mut odd = ep("y", 4, 2, "p");
        odd.episode_number = None;
        let odd2 = ep("y", 4, 3, "q");
        let r = recently_added(
            vec![
                a,
                b,
                season("z", 2, None),
                season("w", 3, None),
                season("w", 4, None),
                odd,
                odd2,
            ],
            20,
        );
        assert_eq!(
            labels(&r),
            [
                ("x".into(), "2 new episodes".into()),
                ("z".into(), "Season 2".into()),
                ("w".into(), "2 new seasons".into()),
                ("y".into(), "S4 · 2 new episodes".into()),
            ]
        );
        let mut single = season("v", 1, None);
        single.season_number = None;
        let r = recently_added(vec![single], 20);
        assert_eq!(r.notes[0].label, "New episodes");
    }

    #[test]
    fn newest_arrival_first_movies_untouched_and_capped() {
        let movie = item("m1", ItemKind::Movie, "Dune");
        let r = recently_added(
            vec![
                ep("a", 1, 5, "x"),
                movie.clone(),
                ep("b", 1, 5, "y"),
                ep("a", 1, 4, "z"),
                ep("c", 1, 5, "w"),
            ],
            3,
        );
        let ids: Vec<&str> = r.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["a", "m1", "b"]);
        assert_eq!(r.items[1], movie);
        // "a" counts both its episodes, though the second came later in the list.
        assert_eq!(labels(&r)[0].1, "S1 · E4–E5");
        assert_eq!(r.notes.len(), 2);
    }
}
