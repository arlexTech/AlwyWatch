// SPDX-License-Identifier: AGPL-3.0-or-later
//! Several servers' homes shown as one ("all servers together"): the same title on two servers
//! appears once, from the preferred server, and every item remembers which account it is from.

use std::collections::{HashMap, HashSet};

use crate::{Home, Item, ItemKind, LibraryKind, MediaLibrary};

/// One account's home, in preference order (the first wins duplicates).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AccountHome {
    pub account_id: String,
    pub home: Home,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MergedHome {
    pub home: Home,
    /// Account of every item and library id in `home`.
    pub owners: HashMap<String, String>,
}

/// The same title on two servers: same TMDB/IMDB/TVDB id; shows by name; episodes by show,
/// season and number.
#[uniffi::export]
pub fn same_as(item: &Item) -> String {
    let kind = format!("{:?}", item.kind);
    match item.kind {
        ItemKind::Episode => format!(
            "ep:{}:{:?}:{:?}",
            item.series_title.as_deref().unwrap_or("").to_lowercase(),
            item.season_number,
            item.episode_number
        ),
        // Shows by name: rails build some from a new episode, without the show's ids.
        ItemKind::Series => format!("show:{}", normalized(&item.title)),
        _ => {
            let p = &item.provider_ids;
            if let Some(t) = &p.tmdb {
                format!("tmdb:{kind}:{t}")
            } else if let Some(i) = &p.imdb {
                format!("imdb:{i}")
            } else if let Some(t) = &p.tvdb {
                format!("tvdb:{kind}:{t}")
            } else {
                format!("title:{kind}:{}:{:?}", item.title.to_lowercase(), item.year)
            }
        }
    }
}

/// Continue watching keeps one entry per show (an episode stands for its show).
fn show_key(i: &Item) -> String {
    match i.kind {
        ItemKind::Episode => format!(
            "show:{}",
            normalized(i.series_title.as_deref().unwrap_or(""))
        ),
        _ => same_as(i),
    }
}

fn normalized(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Several homes as one: rails of the same library kind merged (alternating servers, preferred
/// first), continue watching by recency, a title on several servers kept once.
#[uniffi::export]
pub fn merge_homes(homes: Vec<AccountHome>) -> MergedHome {
    let mut owners = HashMap::new();
    let Some(first) = homes.first() else {
        return MergedHome {
            home: Home {
                resume: vec![],
                next_up: vec![],
                continue_watching: vec![],
                libraries: vec![],
                latest: vec![],
                fetched_at: 0,
            },
            owners,
        };
    };
    if homes.len() == 1 {
        let (a, h) = (&first.account_id, &first.home);
        for i in h
            .continue_watching
            .iter()
            .chain(h.latest.iter().flat_map(|r| &r.items))
        {
            owners.insert(i.id.clone(), a.clone());
        }
        for l in &h.libraries {
            owners.insert(l.id.clone(), a.clone());
        }
        for n in h.latest.iter().flat_map(|r| &r.notes) {
            note_owner(n, a, &mut owners);
        }
        return MergedHome {
            home: h.clone(),
            owners,
        };
    }
    // One rail per library kind (Movies, Shows), titled after the preferred server's.
    let mut kinds: Vec<LibraryKind> = Vec::new();
    for h in &homes {
        for r in &h.home.latest {
            if !kinds.contains(&r.library.kind) {
                kinds.push(r.library.kind);
            }
        }
    }
    let mut latest = Vec::new();
    for kind in kinds {
        let rails: Vec<(&str, &crate::LibraryRail)> = homes
            .iter()
            .flat_map(|h| {
                h.home
                    .latest
                    .iter()
                    .filter(move |r| r.library.kind == kind)
                    .map(move |r| (h.account_id.as_str(), r))
            })
            .collect();
        if kind == LibraryKind::Other {
            for (a, r) in rails {
                for i in &r.items {
                    owners.insert(i.id.clone(), a.to_owned());
                }
                for n in &r.notes {
                    note_owner(n, a, &mut owners);
                }
                latest.push(r.clone());
            }
            continue;
        }
        let (lead_owner, lead) = rails[0];
        owners.insert(lead.library.id.clone(), lead_owner.to_owned());
        let lists: Vec<(&str, &[Item])> = rails
            .iter()
            .map(|(a, r)| (*a, r.items.as_slice()))
            .collect();
        let mut rail = lead.clone();
        rail.items = interleave(&lists, &mut owners);
        // Each card keeps the note of the copy chosen (notes go by item id).
        rail.notes = Vec::new();
        for (a, r) in &rails {
            for n in &r.notes {
                let chosen = owners.get(&n.item_id).is_some_and(|o| o == a)
                    && rail.items.iter().any(|i| i.id == n.item_id);
                if chosen && !rail.notes.iter().any(|m| m.item_id == n.item_id) {
                    note_owner(n, a, &mut owners);
                    rail.notes.push(n.clone());
                }
            }
        }
        latest.push(rail);
    }
    // Library buttons: the preferred server's, plus kinds only another server has.
    let mut libraries: Vec<MediaLibrary> = Vec::new();
    for h in &homes {
        for lib in &h.home.libraries {
            if lib.kind == LibraryKind::Other || !libraries.iter().any(|l| l.kind == lib.kind) {
                libraries.push(lib.clone());
                owners.insert(lib.id.clone(), h.account_id.clone());
            }
        }
    }
    let continue_watching = by_recency(&homes, &mut owners);
    MergedHome {
        home: Home {
            continue_watching,
            libraries,
            latest,
            fetched_at: homes.iter().map(|h| h.home.fetched_at).min().unwrap_or(0),
            ..first.home.clone()
        },
        owners,
    }
}

/// What a card's note opens (an episode, a season) is on the card's server.
fn note_owner(n: &crate::RailNote, account: &str, owners: &mut HashMap<String, String>) {
    for id in std::iter::once(&n.open_id).chain(&n.open_season_id) {
        owners.insert(id.clone(), account.to_owned());
    }
}

/// Alternates servers' lists (preferred first); duplicates keep the preferred server's copy.
fn interleave(lists: &[(&str, &[Item])], owners: &mut HashMap<String, String>) -> Vec<Item> {
    let mut first: HashMap<String, (&str, &Item)> = HashMap::new();
    for (a, items) in lists {
        for i in *items {
            first.entry(same_as(i)).or_insert((a, i));
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let max = lists.iter().map(|(_, l)| l.len()).max().unwrap_or(0);
    for n in 0..max {
        for (_, items) in lists {
            let Some(i) = items.get(n) else { continue };
            let k = same_as(i);
            if !seen.insert(k.clone()) {
                continue;
            }
            let (owner, chosen) = first[&k];
            owners.insert(chosen.id.clone(), owner.to_owned());
            out.push(chosen.clone());
        }
    }
    out
}

/// Continue watching across servers: most recent first, one entry per show; a title on several
/// servers comes from the preferred one. One server's "continue" can be another's "next up".
fn by_recency(homes: &[AccountHome], owners: &mut HashMap<String, String>) -> Vec<Item> {
    let mut preferred: HashMap<String, (&str, &Item)> = HashMap::new();
    for h in homes {
        for i in &h.home.continue_watching {
            preferred
                .entry(same_as(i))
                .or_insert((h.account_id.as_str(), i));
        }
    }
    let mut all: Vec<&Item> = homes
        .iter()
        .flat_map(|h| &h.home.continue_watching)
        .collect();
    // Stable: equal dates keep the preferred server first.
    all.sort_by_key(|i| std::cmp::Reverse(i.user.last_played_at.unwrap_or(0)));
    let mut shows = HashSet::new();
    all.into_iter()
        .filter(|i| shows.insert(show_key(i)))
        .map(|i| {
            let (owner, chosen) = preferred[&same_as(i)];
            owners.insert(chosen.id.clone(), owner.to_owned());
            chosen.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LibraryRail, ProviderIds, UserData};

    fn item(id: &str, kind: ItemKind, title: &str, tmdb: Option<&str>, at: Option<i64>) -> Item {
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
            provider_ids: ProviderIds {
                tmdb: tmdb.map(Into::into),
                ..Default::default()
            },
            series_id: None,
            series_title: None,
            season_id: None,
            season_number: None,
            episode_number: None,
            child_count: None,
            premiere_date: None,
            images: vec![],
            user: UserData {
                last_played_at: at,
                ..Default::default()
            },
        }
    }

    fn episode(id: &str, show: &str, e: u32, at: i64) -> Item {
        Item {
            series_title: Some(show.into()),
            series_id: Some(format!("{id}-show")),
            season_number: Some(1),
            episode_number: Some(e),
            ..item(id, ItemKind::Episode, "ep", None, Some(at))
        }
    }

    fn lib(id: &str, kind: LibraryKind) -> MediaLibrary {
        MediaLibrary {
            id: id.into(),
            title: id.into(),
            kind,
        }
    }

    fn home(cw: Vec<Item>, movies_lib: &str, movies: Vec<Item>) -> Home {
        Home {
            resume: vec![],
            next_up: vec![],
            continue_watching: cw,
            libraries: vec![lib(movies_lib, LibraryKind::Movies)],
            latest: vec![LibraryRail {
                library: lib(movies_lib, LibraryKind::Movies),
                items: movies,
                notes: vec![],
            }],
            fetched_at: 100,
        }
    }

    #[test]
    fn duplicates_come_from_the_preferred_server_and_rails_alternate() {
        let plex = AccountHome {
            account_id: "plex".into(),
            home: home(
                vec![episode("p-silo5", "Silo", 5, 50)],
                "p-movies",
                vec![
                    item("p-dune", ItemKind::Movie, "Dune", Some("1"), None),
                    item("p-alien", ItemKind::Movie, "Alien", Some("2"), None),
                ],
            ),
        };
        let jelly = AccountHome {
            account_id: "jelly".into(),
            home: home(
                vec![
                    episode("j-silo5", "Silo", 5, 40),
                    episode("j-dmc", "Devil May Cry", 2, 90),
                ],
                "j-movies",
                vec![
                    item("j-godzilla", ItemKind::Movie, "Godzilla", Some("3"), None),
                    item("j-dune", ItemKind::Movie, "Dune", Some("1"), None),
                ],
            ),
        };
        let m = merge_homes(vec![plex, jelly]);
        let ids = |v: &[Item]| v.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
        // Most recent first; Silo once, from Plex (preferred), though Jellyfin's copy is older.
        assert_eq!(ids(&m.home.continue_watching), ["j-dmc", "p-silo5"]);
        // One Movies rail: alternating, Dune once (Plex's).
        assert_eq!(m.home.latest.len(), 1);
        assert_eq!(
            ids(&m.home.latest[0].items),
            ["p-dune", "j-godzilla", "p-alien"]
        );
        assert_eq!(m.home.libraries.len(), 1);
        assert_eq!(m.owners["j-godzilla"], "jelly");
        assert_eq!(m.owners["p-dune"], "plex");
        assert_eq!(m.owners["p-movies"], "plex");
        assert_eq!(m.home.fetched_at, 100);
    }

    #[test]
    fn show_cards_keep_the_note_of_the_copy_chosen() {
        let note = |show: &str, label: &str, open: &str| crate::RailNote {
            item_id: show.into(),
            label: label.into(),
            open_id: open.into(),
            open_season_id: Some(format!("{open}-season")),
        };
        let shows = |acc: &str, items: Vec<Item>, notes: Vec<crate::RailNote>| AccountHome {
            account_id: acc.into(),
            home: Home {
                latest: vec![LibraryRail {
                    library: lib(&format!("{acc}-shows"), LibraryKind::Shows),
                    items,
                    notes,
                }],
                ..home(vec![], "x", vec![])
            },
        };
        let plex = shows(
            "plex",
            vec![item("p-dream", ItemKind::Series, "Dreamland", None, None)],
            vec![note("p-dream", "S1 · E6 · Edenia", "p-e6")],
        );
        let jelly = shows(
            "jelly",
            vec![
                item("j-dream", ItemKind::Series, "Dreamland", None, None),
                item("j-silo", ItemKind::Series, "Silo", None, None),
            ],
            vec![
                note("j-dream", "S1 · E6 · Edenia", "j-e6"),
                note("j-silo", "S2 · E3–E5", "j-silo"),
            ],
        );
        let m = merge_homes(vec![plex, jelly]);
        let rail = m
            .home
            .latest
            .iter()
            .find(|r| r.library.kind == LibraryKind::Shows)
            .unwrap();
        let notes: Vec<(&str, &str)> = rail
            .notes
            .iter()
            .map(|n| (n.item_id.as_str(), n.open_id.as_str()))
            .collect();
        assert_eq!(notes, [("p-dream", "p-e6"), ("j-silo", "j-silo")]);
        // What a note opens is on the card's server.
        assert_eq!(m.owners["p-e6"], "plex");
        assert_eq!(m.owners["p-e6-season"], "plex");
        assert_eq!(m.owners["j-silo-season"], "jelly");
        assert!(!m.owners.contains_key("j-e6"));
    }

    #[test]
    fn shows_match_by_name() {
        let a = item("a", ItemKind::Series, "Devil May Cry", None, None);
        let b = item("b", ItemKind::Series, "devil may cry!", Some("9"), None);
        assert_eq!(same_as(&a), same_as(&b));
    }
}
