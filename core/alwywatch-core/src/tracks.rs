// SPDX-License-Identifier: AGPL-3.0-or-later
//! Remembered audio/subtitle choices, stored per sign-in and per title (rules: `track_rules`).

use crate::app::AlwyWatch;
use crate::error::Result;
use crate::track_rules::{TrackPref, decode, encode, split_key};

#[uniffi::export]
impl AlwyWatch {
    /// The audio/subtitle choice of a movie or show (`title_key` from `track_title_key`): the
    /// newest of the title's choice on any server (`tracks.title.…`) and this sign-in's own.
    pub fn track_pref(&self, account_id: String, title_key: String) -> Result<Option<TrackPref>> {
        let (id, shared) = split_key(&title_key);
        let store = self.store();
        let own_key = format!("tracks.{account_id}.{id}");
        let own = store.setting(&own_key)?;
        let Some(shared) = shared else {
            return Ok(own.as_deref().and_then(decode));
        };
        let shared_key = format!("tracks.title.{shared}");
        let title = store.setting(&shared_key)?;
        // Both: the one changed last (a choice on another server is newer than this one's).
        let pick = match (&own, &title) {
            (Some(_), Some(_)) => {
                let meta = self.core.settings_meta();
                let at = |k: &str| meta.get(k).copied().unwrap_or(0);
                if at(&shared_key) >= at(&own_key) {
                    title
                } else {
                    own
                }
            }
            (None, t) => t.clone(),
            (Some(o), None) => {
                // Chosen before choices followed the title: share it from now on.
                self.set_setting(shared_key, o.clone())?;
                own.clone()
            }
        };
        Ok(pick.as_deref().and_then(decode))
    }

    /// Saves the choice for this sign-in and for the title on every server.
    pub fn set_track_pref(
        &self,
        account_id: String,
        title_key: String,
        pref: TrackPref,
    ) -> Result<()> {
        let (id, shared) = split_key(&title_key);
        let value = encode(&pref);
        self.set_setting(format!("tracks.{account_id}.{id}"), value.clone())?;
        if let Some(shared) = shared {
            self.set_setting(format!("tracks.title.{shared}"), value)?;
        }
        Ok(())
    }
}
