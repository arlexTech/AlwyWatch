// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch core facade.
//!
//! Everything the native UIs need is exported from here. Android reaches it through
//! UniFFI-generated Kotlin bindings; the desktop app links it as a normal Rust crate.
//!
//! Data access follows one pattern everywhere (cache-first, stale-while-revalidate):
//! `cached_x()` is a synchronous SQLite read that returns what we already have (or `None`),
//! `refresh_x(force)` fetches from the server, stores and returns the fresh value. It skips the
//! network when the cached value is younger than a short TTL, unless `force` is set. UIs render
//! the cached value immediately, then the refreshed one.

uniffi::setup_scaffolding!();

#[cfg(target_os = "android")]
mod android;
mod app;
mod error;
mod history;
mod merge;
mod playback;
mod settings_sync;
mod sync;
mod tracks;

pub use app::*;
pub use downloads::{DeleteRule, Download, DownloadState, EpisodeOf};
pub use error::{CoreError, Result};
pub use history::{HistoryChange, HistoryCopyResult, HistoryPlan, HistoryProgress};
pub use keep_service::identity::LinkedIdentity;
pub use keep_service::together::{
    TogetherCreate, TogetherInvite, TogetherInvitee, TogetherJoined, TogetherMedia, TogetherMember,
    TogetherPerson, TogetherRoom, TogetherRooms, TogetherState, TogetherUpdate,
};
pub use keep_service::{
    AdminUser, Avatar, AvatarPreset, CountdownResult, LeavingItem, Me, MyVote, Protection, Rights,
    ServerEntry, ServerType, VoteCounts, VoteKind, VoteRequest,
};
pub use media_api::jellyfin::QuickConnectCode;
pub use media_api::model::*;
pub use media_api::plex::{PlexHomeUser, PlexPin, PlexServerResource};
pub use media_api::recent::{RailNote, recently_added};
pub use merge::{AccountHome, MergedHome, merge_homes, same_as};
pub use playback::{PlaybackPrompt, playback_prompt};
pub use seerr::{
    Availability, QuickConnect as SeerrQuickConnect, RequestStatus, SeasonChoice, SeasonPlan,
    SeasonState, SeerrCast, SeerrCollection, SeerrCollectionRef, SeerrDetails, SeerrInfo,
    SeerrLogin, SeerrMediaType, SeerrRequest, SeerrResult, SeerrSeason, SeerrSeasonInfo,
};
pub use sync::{SyncPair, SyncStatus};
pub use tracks::{
    TrackChoice, TrackInfo, TrackKind, TrackPref, TrackRules, choose_tracks, conversion_tracks,
    lang3, pick_track, remembered_track, source_tracks, track_title_key,
};

/// Version of the core library, shown in the about screens.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Skeleton smoke test used by the hello-world screens.
#[uniffi::export]
pub fn greet(name: String) -> String {
    format!(
        "Hello {name}, from the AlwyWatch Rust core {}",
        core_version()
    )
}

/// Normalizes a user-typed server address (adds https://, trims slashes) or explains what is wrong.
#[uniffi::export]
pub fn normalize_server_url(input: String) -> Result<String> {
    Ok(net::normalize_base_url(&input)?)
}

/// Each season of a show with its state, and what can be requested, from the seasons
/// `seerr_season_info` returns. `have`: the season numbers the media server being browsed shows.
#[uniffi::export]
pub fn season_plan(seasons: Vec<SeerrSeasonInfo>, have: Vec<u32>) -> SeasonPlan {
    seerr::season_plan(&seasons, &have)
}

/// A season's state in plain words ("On your server", "Requested"…).
#[uniffi::export]
pub fn season_state_label(state: SeasonState) -> String {
    seerr::season_state_label(state).to_owned()
}

/// The confirmation once seasons are requested ("Requested: seasons 2, 3. They'll appear…").
#[uniffi::export]
pub fn seasons_requested_message(numbers: Vec<u32>) -> String {
    seerr::seasons_requested_message(&numbers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_mentions_name_and_version() {
        let s = greet("TV".into());
        assert!(s.contains("TV"));
        assert!(s.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn season_plan_is_exported_with_its_words() {
        let s = |n: u32, availability| SeerrSeasonInfo {
            number: n,
            name: format!("Season {n}"),
            episodes: 8,
            availability,
            requested: false,
            downloading: false,
        };
        let p = season_plan(
            vec![s(1, Availability::Available), s(2, Availability::Unknown)],
            vec![1],
        );
        assert_eq!(p.missing, vec![2]);
        assert!(p.has_some);
        assert_eq!(
            p.seasons[1].label,
            season_state_label(SeasonState::Requestable)
        );
        assert_eq!(
            seasons_requested_message(vec![2, 3]),
            "Requested: seasons 2, 3. They'll appear here when they're ready."
        );
    }
}
