// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings that follow the viewer across devices. Kept on the AlwyWatch server when one is
//! connected (`/v1/me/settings`, any server type), else with their Jellyfin user (Jellyfin's
//! per-app `DisplayPreferences`, free key/values: nothing to deploy). Per setting, the newer
//! change wins (each carries its time).
//!
//! Only what is about the viewer travels (languages, subtitle mode, skip choices, per-title
//! tracks, which servers the home shows); what depends on the screen or the hardware stays on
//! the device (refresh rate, passthrough, subtitle size, clock, scroll positions…).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use media_api::AnyServer;

use crate::app::{AlwyWatch, Core};
use crate::error::Result;

const CLIENT: &str = "AlwyWatch";
/// Local change times (ms) of synced settings, by key.
const META: &str = "settings_sync.meta";
const SEP: char = '\u{1f}';

/// Whether a setting follows the viewer to other devices.
pub(crate) fn synced(key: &str) -> bool {
    const KEYS: &[&str] = &[
        "player.audio_lang",
        "player.auto_next",
        "player.auto_skip_intro",
        "player.countdown",
        "player.resume_rewind",
        "player.seek_back",
        "player.seek_forward",
        "player.still_watching",
        "player.sub_lang",
        "player.sub_mode",
        "player.sub_color",
        "player.sub_bg",
        "ui.combined",
        "ui.preferred_account",
        "ui.theme_music",
        "ui.confirm_quit",
        "ui.menu_hidden",
        "ui.menu_libs",
        "current_account",
        keep_service::account::SERVERS_SETTING,
        "together.others_title",
        "together.others_play",
        "together.others_seek",
    ];
    KEYS.contains(&key) || key.starts_with("tracks.") || key.starts_with("version.")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A remote value: "time ␟ value".
fn parse(v: &str) -> Option<(i64, &str)> {
    let (t, v) = v.split_once(SEP)?;
    Some((t.parse().ok()?, v))
}

/// What to do with one setting. `local`/`remote`: (time, value). Ties go to the remote side, so
/// a device that never synced takes the shared values (its untimed settings count as time 1).
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Keep,
    Pull,
    Push,
}

fn decide(local: Option<(i64, &str)>, remote: Option<(i64, &str)>) -> Step {
    match (local, remote) {
        (Some((_, lv)), Some((_, rv))) if lv == rv => Step::Keep,
        (Some((lt, _)), Some((rt, _))) if lt > rt => Step::Push,
        (_, Some(_)) => Step::Pull,
        (Some(_), None) => Step::Push,
        (None, None) => Step::Keep,
    }
}

impl Core {
    fn settings_meta(&self) -> HashMap<String, i64> {
        self.store
            .setting(META)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }

    fn save_settings_meta(&self, m: &HashMap<String, i64>) {
        if let Ok(v) = serde_json::to_string(m) {
            let _ = self.store.set_setting(META, &v);
        }
    }

    /// The Jellyfin account that carries the settings: the preferred or current one if it is
    /// Jellyfin, else the first Jellyfin account signed in.
    fn settings_account(&self) -> Option<String> {
        let pick = |k: &str| self.store.setting(k).ok().flatten();
        let jellyfin = |id: &str| matches!(self.server(id).as_deref(), Ok(AnyServer::Jellyfin(_)));
        [pick("ui.preferred_account"), pick("current_account")]
            .into_iter()
            .flatten()
            .find(|id| jellyfin(id))
            .or_else(|| {
                self.store
                    .accounts()
                    .ok()?
                    .into_iter()
                    .map(|a: crate::Account| a.id)
                    .find(|id| jellyfin(id))
            })
    }

    /// Merges this device's synced settings with the shared ones. Returns how many changed
    /// here (the UI reloads them when > 0).
    pub(crate) async fn sync_settings(&self) -> Result<u32> {
        // Where they are kept: the AlwyWatch server when connected (any server type), else
        // the Jellyfin user's app preferences.
        // An AlwyWatch server that can't be reached (down, not deployed yet) falls back to
        // Jellyfin; change times keep both consistent once it is back.
        // This device's servers join the synced list.
        if self.remember_servers().unwrap_or(false) {
            let mut meta = self.settings_meta();
            meta.insert(keep_service::account::SERVERS_SETTING.into(), now_ms());
            self.save_settings_meta(&meta);
        }
        let mut keep = self.person_client().ok();
        let from_keep = match &keep {
            Some(k) => match k.settings().await {
                Ok(m) => Some(m),
                Err(e) if crate::CoreError::from(e.clone()).is_transient() => None,
                Err(e) => return Err(e.into()),
            },
            None => None,
        };
        if from_keep.is_none() {
            keep = None;
        }
        let mut jellyfin: Option<(Arc<AnyServer>, serde_json::Value)> = None;
        let remote: HashMap<String, String> = if let Some(m) = from_keep {
            m
        } else {
            let Some(account) = self.settings_account() else {
                return Ok(0);
            };
            let srv = self.server(&account)?;
            let AnyServer::Jellyfin(jf) = srv.as_ref() else {
                return Ok(0);
            };
            let prefs = jf.app_prefs(CLIENT).await?;
            let map = prefs
                .get("CustomPrefs")
                .and_then(|v| v.as_object())
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default();
            jellyfin = Some((srv.clone(), prefs));
            map
        };

        let mut meta = self.settings_meta();
        let mut keys: Vec<String> = remote.keys().filter(|k| synced(k)).cloned().collect();
        for (k, _) in self.store.settings_with_prefix("")? {
            if synced(&k) && !keys.contains(&k) {
                keys.push(k);
            }
        }
        let mut pulled = 0;
        let mut push: HashMap<String, String> = HashMap::new();
        for k in keys {
            let lv = self.store.setting(&k)?;
            let local = lv
                .as_deref()
                .map(|v| (meta.get(&k).copied().unwrap_or(1), v));
            let remote_v = remote.get(&k).and_then(|v| parse(v));
            match decide(local, remote_v) {
                Step::Keep => {}
                Step::Pull => {
                    let (t, v) = remote_v.unwrap_or_default();
                    self.store.set_setting(&k, v)?;
                    // Per-title choices are read when the player opens: screens needn't
                    // reload for them (that rebuilt every screen, losing scroll and TV focus,
                    // each time a title was watched on another device).
                    if !k.starts_with("tracks.") && !k.starts_with("version.") {
                        pulled += 1;
                    }
                    meta.insert(k, t);
                }
                Step::Push => {
                    let (t, v) = local.unwrap_or_default();
                    meta.insert(k.clone(), t);
                    push.insert(k, format!("{t}{SEP}{v}"));
                }
            }
        }
        self.save_settings_meta(&meta);
        if !push.is_empty() {
            if let Some(k) = &keep {
                k.set_settings(&push).await?;
            } else if let Some((srv, mut prefs)) = jellyfin
                && let AnyServer::Jellyfin(jf) = srv.as_ref()
            {
                // Other apps' entries (Jellyfin's own, some null) are kept as they are.
                let mut out = prefs
                    .get("CustomPrefs")
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                for (k, v) in push {
                    out.insert(k, v.into());
                }
                prefs["CustomPrefs"] = serde_json::Value::Object(out);
                jf.set_app_prefs(CLIENT, &prefs).await?;
            }
        }
        Ok(pulled)
    }

    /// A synced setting changed here: note its time and share it a moment later (one push for
    /// a burst of changes).
    pub(crate) fn setting_changed(self: &Arc<Self>, key: &str) {
        let mut meta = self.settings_meta();
        meta.insert(key.to_owned(), now_ms());
        self.save_settings_meta(&meta);
        let generation = self.settings_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let c = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            if c.settings_gen.load(Ordering::SeqCst) == generation {
                let _ = c.sync_settings().await;
            }
        });
    }
}

#[uniffi::export]
impl AlwyWatch {
    /// Takes the settings changed on other devices and shares this device's (via the Jellyfin
    /// user). Call at start and when the app comes back; > 0: settings screens show changed,
    /// reload them (per-title track and version choices are taken but not counted).
    pub async fn sync_settings(&self) -> Result<u32> {
        self.run(|c| async move { c.sync_settings().await }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_wins_and_ties_take_the_shared_value() {
        assert_eq!(decide(Some((5, "a")), Some((3, "b"))), Step::Push);
        assert_eq!(decide(Some((3, "a")), Some((5, "b"))), Step::Pull);
        // Never synced here (untimed = 1) vs shared at 1: take the shared one.
        assert_eq!(decide(Some((1, "a")), Some((1, "b"))), Step::Pull);
        assert_eq!(decide(Some((9, "a")), Some((1, "a"))), Step::Keep);
        assert_eq!(decide(Some((1, "a")), None), Step::Push);
        assert_eq!(decide(None, Some((2, "b"))), Step::Pull);
    }

    #[test]
    fn only_viewer_settings_travel() {
        assert!(synced("player.sub_mode"));
        assert!(synced("tracks.plex:1:2.345"));
        assert!(synced("ui.combined"));
        assert!(!synced("player.match_rate"));
        assert!(!synced("player.sub_size"));
        assert!(!synced("ui.clock"));
        assert!(!synced("ui.scroll.home"));
        assert!(!synced("settings_sync.meta"));
    }

    #[test]
    fn remote_values_parse() {
        assert_eq!(parse("17\u{1f}fra"), Some((17, "fra")));
        assert_eq!(parse("bad"), None);
    }
}
