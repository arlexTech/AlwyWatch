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
mod tracks;

pub use app::*;
pub use downloads::{DeleteRule, Download, DownloadState, EpisodeOf};
pub use error::{CoreError, Result};
pub use history::{HistoryChange, HistoryCopyResult, HistoryPlan};
pub use keep_service::together::{
    TogetherCreate, TogetherJoined, TogetherMedia, TogetherMember, TogetherState, TogetherUpdate,
};
pub use keep_service::{
    AdminUser, CountdownResult, LeavingItem, Me, MyVote, Protection, Rights, ServerEntry,
    ServerType, VoteCounts, VoteKind, VoteRequest,
};
pub use media_api::jellyfin::QuickConnectCode;
pub use media_api::model::*;
pub use media_api::plex::{PlexHomeUser, PlexPin, PlexServerResource};
pub use merge::{AccountHome, MergedHome, merge_homes, same_as};
pub use playback::{PlaybackPrompt, playback_prompt};
pub use seerr::{
    Availability, QuickConnect as SeerrQuickConnect, RequestStatus, SeerrCast, SeerrCollection,
    SeerrCollectionRef, SeerrDetails, SeerrInfo, SeerrLogin, SeerrMediaType, SeerrRequest,
    SeerrResult, SeerrSeason,
};
pub use tracks::{
    TrackChoice, TrackInfo, TrackKind, TrackPref, TrackRules, choose_tracks, lang3, pick_track,
    remembered_track, track_title_key,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_mentions_name_and_version() {
        let s = greet("TV".into());
        assert!(s.contains("TV"));
        assert!(s.contains(env!("CARGO_PKG_VERSION")));
    }
}
