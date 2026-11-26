// SPDX-License-Identifier: AGPL-3.0-or-later
//! Audio and subtitle choices per movie or show, the same as in the apps: the same rules (the
//! core's `track_rules.rs`, compiled here as is) and the same values, shared through the
//! AlwyWatch server's synced settings (`tracks.<sign-in>.<id>` and `tracks.title.<title>`).

#[path = "../../core/alwywatch-core/src/track_rules.rs"]
#[allow(dead_code)]
mod rules;

use std::collections::HashMap;

use media_api::{Item, MediaSource, ServerKind, Session, SubtitleSelection};
pub use rules::{TrackKind, TrackPref, TrackRules};

use crate::state;

/// Every `tracks.*` value known here: "change time ␟ value", as the server keeps them.
pub const STORE: &str = "aw.tracks";
const SEP: char = '\u{1f}';

fn parse(v: &str) -> Option<(i64, &str)> {
    let (t, v) = v.split_once(SEP)?;
    Some((t.parse().ok()?, v))
}

/// The sign-in as the apps name it ("jellyfin:<server>:<user>").
pub fn account_id(s: &Session) -> String {
    let kind = match s.kind {
        ServerKind::Jellyfin => "jellyfin",
        ServerKind::Plex => "plex",
    };
    format!("{kind}:{}:{}", s.server_id, s.user_id)
}

/// The keys of a title: this sign-in's own, and the title's on every server.
fn keys(s: &Session, item: &Item) -> (String, Option<String>) {
    let key = rules::track_title_key(item.clone());
    let (id, shared) = rules::split_key(&key);
    (
        format!("tracks.{}.{id}", account_id(s)),
        shared.map(|t| format!("tracks.title.{t}")),
    )
}

/// The title's remembered choice: whichever of the two was changed last.
pub fn pref(s: &Session, item: &Item) -> Option<TrackPref> {
    let store = state::load::<HashMap<String, String>>(STORE).unwrap_or_default();
    let (own, shared) = keys(s, item);
    let get = |k: &str| {
        store
            .get(k)
            .and_then(|v| parse(v))
            .map(|(t, v)| (t, v.to_owned()))
    };
    let newest = match (get(&own), shared.as_deref().and_then(get)) {
        (Some(a), Some(b)) => Some(if b.0 >= a.0 { b } else { a }),
        (a, b) => a.or(b),
    };
    newest.and_then(|(_, v)| rules::decode(&v))
}

/// Remembers a choice made here (both keys), and shares it a moment later.
pub fn save(s: &Session, item: &Item, pref: &TrackPref) {
    let mut store = state::load::<HashMap<String, String>>(STORE).unwrap_or_default();
    let (own, shared) = keys(s, item);
    let v = format!("{}{SEP}{}", js_sys::Date::now() as i64, rules::encode(pref));
    store.insert(own, v.clone());
    if let Some(k) = shared {
        store.insert(k, v);
    }
    state::save(STORE, &store);
    leptos::task::spawn_local(async {
        state::sleep(1500).await;
        let _ = crate::sync::sync().await;
    });
}

/// Takes the shared `tracks.*` values that are newer than ours; returns ours that are newer
/// than the shared ones (to send).
pub fn merge(remote: &HashMap<String, String>) -> HashMap<String, String> {
    merge_prefix(remote, "tracks.", STORE)
}

/// The same, for the shared values whose keys start with `prefix`, kept here under `store_key`
/// ("time ␟ value" each, as on the server).
pub fn merge_prefix(
    remote: &HashMap<String, String>,
    prefix: &str,
    store_key: &str,
) -> HashMap<String, String> {
    let mut store = state::load::<HashMap<String, String>>(store_key).unwrap_or_default();
    let mut push = HashMap::new();
    for (k, rv) in remote.iter().filter(|(k, _)| k.starts_with(prefix)) {
        let rt = parse(rv).map_or(0, |(t, _)| t);
        let lt = store.get(k).and_then(|v| parse(v)).map_or(-1, |(t, _)| t);
        if rt >= lt {
            store.insert(k.clone(), rv.clone());
        }
    }
    for (k, lv) in &store {
        let lt = parse(lv).map_or(0, |(t, _)| t);
        let rt = remote.get(k).and_then(|v| parse(v)).map_or(-1, |(t, _)| t);
        if lt > rt {
            push.insert(k.clone(), lv.clone());
        }
    }
    state::save(store_key, &store);
    push
}

/// What to play at start, by the apps' rules: the audio stream (None: the file's default) and
/// the subtitle stream (None: off).
pub fn start(
    pref: Option<TrackPref>,
    audio_lang: &str,
    sub_mode: &str,
    sub_lang: &str,
    source: &MediaSource,
) -> (Option<i64>, Option<i64>) {
    let sel = rules::conversion_tracks(
        pref,
        TrackRules {
            audio_language: audio_lang.to_owned(),
            subtitle_mode: sub_mode.to_owned(),
            subtitle_language: sub_lang.to_owned(),
            default_audio: None,
        },
        source,
    );
    let sub = match sel.subtitle {
        SubtitleSelection::Stream { index } => Some(index),
        _ => None,
    };
    let audio = sel
        .audio_index
        .filter(|a| Some(*a) != source.default_audio_index);
    (audio, sub)
}

/// The choice after picking stream `index` (None: subtitles off) of `kind` in the player.
pub fn picked(
    old: Option<TrackPref>,
    kind: TrackKind,
    source: &MediaSource,
    index: Option<i64>,
) -> TrackPref {
    let stream_kind = match kind {
        TrackKind::Audio => media_api::StreamKind::Audio,
        TrackKind::Subtitle => media_api::StreamKind::Subtitle,
    };
    let id = index.and_then(|i| {
        source
            .streams
            .iter()
            .position(|s| s.index == i && s.kind == stream_kind)
            .map(|p| p as u32)
    });
    rules::pick_track(old, kind, rules::source_tracks(source), id)
}
