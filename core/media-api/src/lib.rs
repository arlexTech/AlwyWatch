// SPDX-License-Identifier: AGPL-3.0-or-later
//! Common media-server abstraction with Jellyfin and Plex implementations.
//!
//! [`MediaServer`] is the read/playback surface the apps use. Sign-in flows differ per server
//! and live on [`jellyfin::JellyfinAuth`] and [`plex::PlexAuth`]. [`AnyServer`] dispatches
//! statically to one of the two clients (no boxing, no `async-trait`).

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

pub mod jellyfin;
pub mod mkv;
pub mod model;
pub mod plex;
pub mod quality;
pub mod recent;

use std::future::Future;

pub use model::*;
pub use net::{Error, Result};
pub use recent::{RailNote, RecentlyAdded, recently_added, show_card};

/// `Send` on native (the core runs requests on a multi-threaded runtime); nothing in a browser,
/// where futures are single-threaded (fetch promises are never `Send`).
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// No track choice: the server's own.
static NO_SELECTION: TrackSelection = TrackSelection {
    source_id: None,
    audio_index: None,
    subtitle: SubtitleSelection::Default,
    quality: None,
    reencode: false,
};

/// The read and playback surface every supported server implements.
pub trait MediaServer: Send + Sync {
    fn session(&self) -> &Session;

    fn libraries(&self) -> impl Future<Output = Result<Vec<MediaLibrary>>> + MaybeSend;
    fn items(&self, query: &ItemQuery) -> impl Future<Output = Result<Page>> + MaybeSend;
    fn item(&self, id: &str) -> impl Future<Output = Result<Item>> + MaybeSend;
    fn seasons(&self, series_id: &str) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    fn episodes(
        &self,
        series_id: &str,
        season_id: &str,
    ) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    /// Partially watched items, most recent first.
    fn resume(&self, limit: u32) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    /// Next unwatched episode of series in progress.
    fn next_up(&self, limit: u32) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    fn latest(
        &self,
        library_id: &str,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    fn search(
        &self,
        query: &str,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    /// Titles like this one on the same server ("more like this"), best match first.
    fn similar(
        &self,
        item_id: &str,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    /// When each of these shows last had an episode watched to the end (unix seconds); shows
    /// never finished an episode of are left out. Orders "continue watching".
    fn shows_last_watched(
        &self,
        series_ids: &[String],
    ) -> impl Future<Output = Result<std::collections::HashMap<String, i64>>> + MaybeSend;
    /// Takes an item out of "continue watching" on the server. Returns false when the server
    /// has no such thing for it (the caller then hides it locally).
    fn remove_from_continue_watching(
        &self,
        item_id: &str,
    ) -> impl Future<Output = Result<bool>> + MaybeSend;
    /// Genres present in a library.
    fn genres(&self, library_id: &str) -> impl Future<Output = Result<Vec<Genre>>> + MaybeSend;
    /// Collections (Jellyfin box sets, Plex collections) of a library.
    fn collections(&self, library_id: &str) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    /// Playback sources, original file first. Transcoding is only offered as a fallback.
    /// The server picks the conversion's audio and subtitles; asking changes nothing on it.
    fn playback_info(
        &self,
        item_id: &str,
        profile: &DeviceProfile,
    ) -> impl Future<Output = Result<PlaybackInfo>> + MaybeSend {
        self.playback_info_with(item_id, profile, &NO_SELECTION)
    }
    /// Like `playback_info`, with the conversion (`MediaSource::transcode`) carrying these
    /// audio and subtitles. On Plex a non-default selection is saved as the account's choice
    /// for that file (`PUT /library/parts/{id}`), as Plex's own apps do: the converter reads it
    /// from there.
    fn playback_info_with(
        &self,
        item_id: &str,
        profile: &DeviceProfile,
        selection: &TrackSelection,
    ) -> impl Future<Output = Result<PlaybackInfo>> + MaybeSend;
    /// Skippable intro/credits spans, by start time. Empty when the server has not detected any.
    fn segments(&self, item_id: &str) -> impl Future<Output = Result<Vec<Segment>>> + MaybeSend;
    /// Gets an embedded text subtitle ready to be burned into a conversion. Jellyfin extracts
    /// a file's text subtitles the first time one is asked for (a read of the whole file: 30 s
    /// to over a minute for a film) and a conversion that burns one in waits for that; asked
    /// first, the picture can start meanwhile. Returns once ready; an error (a read timeout)
    /// while the server still works on it, which it finishes anyway. Plex burns in from the
    /// file itself: nothing to prepare.
    fn prepare_subtitle(
        &self,
        _item_id: &str,
        _source_id: &str,
        _index: i64,
    ) -> impl Future<Output = Result<()>> + MaybeSend {
        async { Ok(()) }
    }
    fn report_progress(
        &self,
        report: &ProgressReport,
    ) -> impl Future<Output = Result<()>> + MaybeSend;
    /// Trailers of a movie or show (server extras first, then web ones).
    fn trailers(&self, item_id: &str) -> impl Future<Output = Result<Vec<Trailer>>> + MaybeSend;
    /// The show's theme song (Plex theme, Jellyfin theme songs), if the server has one.
    fn theme_song(
        &self,
        item_id: &str,
    ) -> impl Future<Output = Result<Option<HttpRequest>>> + MaybeSend;
    /// Cast first (in billing order), then directors and writers.
    fn people(&self, item_id: &str) -> impl Future<Output = Result<Vec<Person>>> + MaybeSend;
    /// Titles of the user's watchlist that this server has: Jellyfin favorites; Plex's
    /// plex.tv watchlist (needs the plex.tv account token), matched to this server's library.
    fn watchlist(
        &self,
        plex_tv_token: Option<&str>,
    ) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    fn set_watchlisted(
        &self,
        item_id: &str,
        on: bool,
        plex_tv_token: Option<&str>,
    ) -> impl Future<Output = Result<()>> + MaybeSend;
    fn set_played(
        &self,
        item_id: &str,
        played: bool,
    ) -> impl Future<Output = Result<()>> + MaybeSend;
    /// Every movie and episode the user watched or started, with user data and external ids
    /// (episodes with their show's id and season/episode numbers). For moving history.
    fn watch_history(&self) -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    /// Of `item_ids`, those whose files are gone from the disk although the server still lists
    /// them (Plex keeps deleted titles as "unavailable" until its trash is emptied; Jellyfin
    /// drops them at the next scan, so it has none).
    fn files_gone(
        &self,
        item_ids: &[String],
    ) -> impl Future<Output = Result<Vec<String>>> + MaybeSend;
    /// Sets watched, resume position (and the last played date where the server allows it).
    fn set_watch_state(
        &self,
        item_id: &str,
        state: &WatchState,
    ) -> impl Future<Output = Result<()>> + MaybeSend;
    /// The original file (never a transcode), so downloads can be resumed.
    fn download_source(
        &self,
        item_id: &str,
    ) -> impl Future<Output = Result<DownloadSource>> + MaybeSend;
    /// Every movie or series with its external ids (light items, no user data). Used to map
    /// Maintainerr items (Plex ratingKey + tmdb/tvdb) to this server; callers cache it.
    fn provider_index(&self, kind: ItemKind)
    -> impl Future<Output = Result<Vec<Item>>> + MaybeSend;
    /// A sized image request. Pure: no network.
    fn image_request(&self, item_id: &str, image: &ImageRef, max_width: u32) -> HttpRequest;
}

/// Static dispatch over the supported servers.
#[derive(Debug, Clone)]
pub enum AnyServer {
    Jellyfin(jellyfin::JellyfinClient),
    Plex(plex::PlexClient),
}

macro_rules! dispatch {
    ($self:ident, $c:ident => $e:expr) => {
        match $self {
            AnyServer::Jellyfin($c) => $e,
            AnyServer::Plex($c) => $e,
        }
    };
}

impl AnyServer {
    /// Rebuilds a client from a stored session (token restored from secure storage).
    pub fn from_session(session: Session, client: ClientInfo, http: net::reqwest::Client) -> Self {
        match session.kind {
            ServerKind::Jellyfin => {
                AnyServer::Jellyfin(jellyfin::JellyfinClient::new(session, client, http))
            }
            ServerKind::Plex => AnyServer::Plex(plex::PlexClient::new(session, client, http)),
        }
    }
}

impl MediaServer for AnyServer {
    fn session(&self) -> &Session {
        dispatch!(self, c => c.session())
    }
    async fn libraries(&self) -> Result<Vec<MediaLibrary>> {
        dispatch!(self, c => c.libraries().await)
    }
    async fn items(&self, query: &ItemQuery) -> Result<Page> {
        dispatch!(self, c => c.items(query).await)
    }
    async fn item(&self, id: &str) -> Result<Item> {
        dispatch!(self, c => c.item(id).await)
    }
    async fn seasons(&self, series_id: &str) -> Result<Vec<Item>> {
        dispatch!(self, c => c.seasons(series_id).await)
    }
    async fn episodes(&self, series_id: &str, season_id: &str) -> Result<Vec<Item>> {
        dispatch!(self, c => c.episodes(series_id, season_id).await)
    }
    async fn resume(&self, limit: u32) -> Result<Vec<Item>> {
        dispatch!(self, c => c.resume(limit).await)
    }
    async fn next_up(&self, limit: u32) -> Result<Vec<Item>> {
        dispatch!(self, c => c.next_up(limit).await)
    }
    async fn latest(&self, library_id: &str, limit: u32) -> Result<Vec<Item>> {
        dispatch!(self, c => c.latest(library_id, limit).await)
    }
    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Item>> {
        dispatch!(self, c => c.search(query, limit).await)
    }
    async fn similar(&self, item_id: &str, limit: u32) -> Result<Vec<Item>> {
        dispatch!(self, c => c.similar(item_id, limit).await)
    }
    async fn genres(&self, library_id: &str) -> Result<Vec<Genre>> {
        dispatch!(self, c => c.genres(library_id).await)
    }
    async fn remove_from_continue_watching(&self, item_id: &str) -> Result<bool> {
        dispatch!(self, c => c.remove_from_continue_watching(item_id).await)
    }
    async fn shows_last_watched(
        &self,
        series_ids: &[String],
    ) -> Result<std::collections::HashMap<String, i64>> {
        dispatch!(self, c => c.shows_last_watched(series_ids).await)
    }
    async fn collections(&self, library_id: &str) -> Result<Vec<Item>> {
        dispatch!(self, c => c.collections(library_id).await)
    }
    async fn playback_info_with(
        &self,
        item_id: &str,
        profile: &DeviceProfile,
        selection: &TrackSelection,
    ) -> Result<PlaybackInfo> {
        dispatch!(self, c => c.playback_info_with(item_id, profile, selection).await)
    }
    async fn segments(&self, item_id: &str) -> Result<Vec<Segment>> {
        dispatch!(self, c => c.segments(item_id).await)
    }
    async fn prepare_subtitle(&self, item_id: &str, source_id: &str, index: i64) -> Result<()> {
        dispatch!(self, c => c.prepare_subtitle(item_id, source_id, index).await)
    }
    async fn report_progress(&self, report: &ProgressReport) -> Result<()> {
        dispatch!(self, c => c.report_progress(report).await)
    }
    async fn people(&self, item_id: &str) -> Result<Vec<Person>> {
        dispatch!(self, c => c.people(item_id).await)
    }
    async fn trailers(&self, item_id: &str) -> Result<Vec<Trailer>> {
        dispatch!(self, c => c.trailers(item_id).await)
    }
    async fn theme_song(&self, item_id: &str) -> Result<Option<HttpRequest>> {
        dispatch!(self, c => c.theme_song(item_id).await)
    }
    async fn watchlist(&self, plex_tv_token: Option<&str>) -> Result<Vec<Item>> {
        dispatch!(self, c => c.watchlist(plex_tv_token).await)
    }
    async fn set_watchlisted(
        &self,
        item_id: &str,
        on: bool,
        plex_tv_token: Option<&str>,
    ) -> Result<()> {
        dispatch!(self, c => c.set_watchlisted(item_id, on, plex_tv_token).await)
    }
    async fn set_played(&self, item_id: &str, played: bool) -> Result<()> {
        dispatch!(self, c => c.set_played(item_id, played).await)
    }
    async fn download_source(&self, item_id: &str) -> Result<DownloadSource> {
        dispatch!(self, c => c.download_source(item_id).await)
    }
    async fn provider_index(&self, kind: ItemKind) -> Result<Vec<Item>> {
        dispatch!(self, c => c.provider_index(kind).await)
    }
    async fn watch_history(&self) -> Result<Vec<Item>> {
        dispatch!(self, c => c.watch_history().await)
    }
    async fn files_gone(&self, item_ids: &[String]) -> Result<Vec<String>> {
        dispatch!(self, c => c.files_gone(item_ids).await)
    }
    async fn set_watch_state(&self, item_id: &str, state: &WatchState) -> Result<()> {
        dispatch!(self, c => c.set_watch_state(item_id, state).await)
    }
    fn image_request(&self, item_id: &str, image: &ImageRef, max_width: u32) -> HttpRequest {
        dispatch!(self, c => c.image_request(item_id, image, max_width))
    }
}

pub(crate) fn header(name: &str, value: impl Into<String>) -> Header {
    Header {
        name: name.to_owned(),
        value: value.into(),
    }
}

/// The last part of a file path, Unix or Windows ("/a/b/Film.mkv" → "Film.mkv").
pub(crate) fn file_name(path: &str) -> Option<String> {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
