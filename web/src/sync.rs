// SPDX-License-Identifier: AGPL-3.0-or-later
//! What follows the viewer across devices, shared with the apps through the AlwyWatch server
//! (`/v1/me/settings`): the playback preferences and the list of servers (no tokens: each
//! device signs in itself). Same keys and rules as the apps' settings sync (core
//! `settings_sync.rs`): each value is "change time ␟ value", the newer change wins, a tie goes
//! to the shared value (a browser that never synced takes what the apps have).

use std::collections::HashMap;

use keep_service::account::{SERVERS_SETTING, SavedServer};
use media_api::ServerKind;

use crate::prefs::{self, PlayerPrefs};
use leptos::prelude::*;

use crate::state;

/// Change times (ms) of this browser's synced values, by key.
const META: &str = "aw.sync.meta";
/// The account's servers as last read (offered when adding a server here).
pub const SERVERS: &str = "aw.sync.servers";
const SEP: char = '\u{1f}';

fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

fn flag(b: bool) -> String {
    if b { "1" } else { "0" }.into()
}

/// This browser's values, as the apps write them.
fn local_values(p: &PlayerPrefs) -> Vec<(&'static str, String)> {
    vec![
        ("player.auto_skip_intro", flag(p.auto_skip_intro)),
        ("player.auto_next", flag(p.auto_next)),
        ("player.countdown", p.countdown_s.to_string()),
        ("player.sub_mode", p.sub_mode.clone()),
        ("player.sub_lang", p.sub_lang.clone()),
        ("player.audio_lang", p.audio_lang.clone()),
        ("player.seek_back", p.seek_back_s.to_string()),
        ("player.seek_forward", p.seek_forward_s.to_string()),
        ("player.resume_rewind", p.resume_rewind_s.to_string()),
        ("player.still_watching", p.still_watching_h.to_string()),
        ("player.sub_color", p.sub_color.clone()),
        ("player.sub_bg", flag(p.sub_bg)),
        ("ui.menu_libs", p.menu_libs.join(",")),
        ("ui.menu_hidden", p.menu_hidden.join(",")),
        ("together.others_title", flag(p.together.change_title)),
        ("together.others_play", flag(p.together.play_pause)),
        ("together.others_seek", flag(p.together.seek)),
    ]
}

/// Takes one shared value into the preferences; false when it doesn't apply here.
fn apply(p: &mut PlayerPrefs, key: &str, v: &str) -> bool {
    let on = v != "0";
    match key {
        "player.auto_skip_intro" => p.auto_skip_intro = v == "1",
        "player.auto_next" => p.auto_next = on,
        "player.countdown" => match v.parse() {
            Ok(n) => p.countdown_s = n,
            Err(_) => return false,
        },
        "player.sub_mode" => p.sub_mode = v.to_owned(),
        "player.sub_lang" => p.sub_lang = v.to_owned(),
        "player.audio_lang" => p.audio_lang = v.to_owned(),
        "player.seek_back"
        | "player.seek_forward"
        | "player.resume_rewind"
        | "player.still_watching" => {
            let Ok(n) = v.parse() else { return false };
            match key {
                "player.seek_back" => p.seek_back_s = n,
                "player.seek_forward" => p.seek_forward_s = n,
                "player.resume_rewind" => p.resume_rewind_s = n,
                _ => p.still_watching_h = n,
            }
        }
        "player.sub_color" => p.sub_color = v.to_owned(),
        "player.sub_bg" => p.sub_bg = v == "1",
        "ui.menu_libs" | "ui.menu_hidden" => {
            let ids = v
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            if key == "ui.menu_libs" {
                p.menu_libs = ids;
            } else {
                p.menu_hidden = ids;
            }
        }
        "together.others_title" => p.together.change_title = on,
        "together.others_play" => p.together.play_pause = on,
        "together.others_seek" => p.together.seek = on,
        _ => return false,
    }
    true
}

/// Preferences saved here: the keys that changed get this moment as their time, and are sent
/// a moment later.
pub fn changed(before: &PlayerPrefs, after: &PlayerPrefs) {
    let old: HashMap<_, _> = local_values(before).into_iter().collect();
    let mut meta = state::load::<HashMap<String, i64>>(META).unwrap_or_default();
    let mut any = false;
    for (k, v) in local_values(after) {
        if old.get(k) != Some(&v) {
            meta.insert(k.to_owned(), now_ms());
            any = true;
        }
    }
    if any {
        state::save(META, &meta);
        leptos::task::spawn_local(async {
            state::sleep(1500).await;
            let _ = sync().await;
        });
    }
}

/// The servers signed in here, as the apps list them.
fn here_servers() -> Vec<SavedServer> {
    state::ctx()
        .accounts
        .get_untracked()
        .into_iter()
        .map(|s| SavedServer {
            kind: match s.kind {
                ServerKind::Jellyfin => "jellyfin",
                ServerKind::Plex => "plex",
            }
            .into(),
            url: s.base_url.trim_end_matches('/').to_owned(),
            name: s.server_name.clone(),
        })
        .collect()
}

/// What one sync did.
pub struct Outcome {
    /// Preferences taken from the other devices.
    pub pulled: u32,
    /// Values sent to them.
    pub pushed: u32,
}

/// Reads the shared values, takes the newer ones, sends this browser's newer ones. Err: why
/// it couldn't (no AlwyWatch sign-in, server unreachable…).
pub async fn sync() -> Result<Outcome, String> {
    let c = state::ctx();
    let Some(k) = c.person_client() else {
        return Err(i18n::tr!(
            "Sign in to AlwyWatch first: your settings are kept on your account."
        ));
    };
    let remote = k.settings().await.map_err(|e| state::plain_keep(&e))?;
    let parse = |v: &String| {
        let (t, v) = v.split_once(SEP)?;
        Some((t.parse::<i64>().ok()?, v.to_owned()))
    };
    let mut meta = state::load::<HashMap<String, i64>>(META).unwrap_or_default();
    let mut p = prefs::load();
    let mut push: HashMap<String, String> = HashMap::new();
    let mut pulled = 0;
    for (key, local) in local_values(&p) {
        let lt = meta.get(key).copied().unwrap_or(1);
        match remote.get(key).and_then(parse) {
            Some((_, rv)) if rv == local => {}
            Some((rt, rv)) if rt >= lt => {
                if apply(&mut p, key, &rv) {
                    meta.insert(key.to_owned(), rt);
                    pulled += 1;
                }
            }
            // Newer here, or not shared yet: only what was chosen here (a default never set
            // would overwrite the apps' choices).
            _ if meta.contains_key(key) => {
                push.insert(key.to_owned(), format!("{lt}{SEP}{local}"));
            }
            _ => {}
        }
    }
    if pulled > 0 {
        // Not `prefs::save`: these are not changes made here (they'd be sent back as new).
        prefs::save_quietly(&p);
    }
    // Servers: the account's list plus this browser's (each device adds its own).
    let mut list: Vec<SavedServer> = remote
        .get(SERVERS_SETTING)
        .and_then(parse)
        .and_then(|(_, v)| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    let before = list.len();
    for s in here_servers() {
        if !list
            .iter()
            .any(|x| x.kind == s.kind && x.url.trim_end_matches('/') == s.url)
        {
            list.push(s);
        }
    }
    if list.len() != before {
        push.insert(
            SERVERS_SETTING.into(),
            format!(
                "{}{SEP}{}",
                now_ms(),
                serde_json::to_string(&list).unwrap_or_default()
            ),
        );
    }
    // Per-title audio/subtitle choices: the newer side wins, key by key.
    push.extend(crate::tracks::merge(&remote));
    // Titles taken out of Continue watching (the apps hide them the same way).
    push.extend(crate::tracks::merge_prefix(
        &remote,
        "hidden.",
        crate::browse::SYNCED_HIDDEN,
    ));
    state::save(SERVERS, &list);
    state::save(META, &meta);
    let pushed = push.len() as u32;
    if !push.is_empty() {
        k.set_settings(&push)
            .await
            .map_err(|e| state::plain_keep(&e))?;
    }
    Ok(Outcome { pulled, pushed })
}

/// The account's servers not signed in here (Jellyfin ones: Plex lists its servers itself).
pub fn other_servers() -> Vec<SavedServer> {
    let here = here_servers();
    state::load::<Vec<SavedServer>>(SERVERS)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            s.kind == "jellyfin" && !here.iter().any(|h| h.url == s.url.trim_end_matches('/'))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_written_reads_back() {
        let mut p = PlayerPrefs {
            auto_skip_intro: true,
            auto_next: false,
            countdown_s: 7,
            sub_mode: "forced".into(),
            sub_lang: "fra".into(),
            ..Default::default()
        };
        p.together.seek = false;
        let mut q = PlayerPrefs::default();
        for (k, v) in local_values(&p) {
            assert!(apply(&mut q, k, &v), "{k}");
        }
        assert_eq!(p, q);
    }

    #[test]
    fn apps_values_read_as_the_apps_mean_them() {
        let mut p = PlayerPrefs::default();
        // The apps write "1"/"0"; anything but "0" is on for "on unless turned off" choices.
        assert!(apply(&mut p, "player.auto_next", "0"));
        assert!(!p.auto_next);
        assert!(!apply(&mut p, "player.countdown", "soon"));
        assert!(!apply(&mut p, "ui.theme_music", "1"));
    }
}
