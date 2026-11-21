// SPDX-License-Identifier: AGPL-3.0-or-later
//! Server-agnostic models shared by the Jellyfin and Plex clients and exported to the UIs.

use serde::{Deserialize, Serialize};

macro_rules! record {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
        pub struct $name { $($body)* }
    };
}

macro_rules! simple_enum {
    ($(#[$m:meta])* pub enum $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
        pub enum $name { $($body)* }
    };
}

simple_enum! {
    pub enum ServerKind { Jellyfin, Plex }
}

record! {
    /// Identifies this app to media servers (Jellyfin `Authorization` header, Plex `X-Plex-*` headers).
    pub struct ClientInfo {
        pub product: String,
        pub version: String,
        pub device_name: String,
        /// Stable per-install identifier (generated once by the app).
        pub device_id: String,
        /// Platform name, e.g. "Android", "Android TV", "Linux", "Windows".
        pub platform: String,
    }
}

record! {
    pub struct ServerInfo {
        pub kind: ServerKind,
        /// Jellyfin server id or Plex machine identifier.
        pub id: String,
        pub name: String,
        pub version: String,
    }
}

record! {
    /// An authenticated user on one server. The token is handed to the platform for
    /// secure storage and is never persisted or logged by the core.
    pub struct Session {
        pub kind: ServerKind,
        pub base_url: String,
        pub server_id: String,
        pub server_name: String,
        pub user_id: String,
        pub user_name: String,
        pub is_admin: bool,
        pub token: String,
    }
}

simple_enum! {
    pub enum LibraryKind { Movies, Shows, Other }
}

record! {
    pub struct MediaLibrary {
        pub id: String,
        pub title: String,
        pub kind: LibraryKind,
    }
}

simple_enum! {
    pub enum ItemKind { Movie, Series, Season, Episode, Collection, Folder, Other }
}

record! {
    #[derive(Default)]
    pub struct ProviderIds {
        pub tmdb: Option<String>,
        pub tvdb: Option<String>,
        pub imdb: Option<String>,
    }
}

simple_enum! {
    /// `SeriesPrimary`: an episode's show poster (its `Primary` is the episode still).
    pub enum ImageKind { Primary, Backdrop, Thumb, Logo, SeriesPrimary }
}

record! {
    /// A server image. `path` is server-relative; the core turns it into a sized request.
    pub struct ImageRef {
        pub kind: ImageKind,
        pub path: String,
        pub blurhash: Option<String>,
    }
}

record! {
    #[derive(Default)]
    pub struct UserData {
        pub played: bool,
        pub position_ms: u64,
        pub play_count: u32,
        pub favorite: bool,
        /// Unix seconds.
        pub last_played_at: Option<i64>,
        pub unplayed_count: Option<u32>,
    }
}

record! {
    pub struct Item {
        pub id: String,
        pub kind: ItemKind,
        pub title: String,
        pub sort_title: Option<String>,
        pub overview: Option<String>,
        pub year: Option<i32>,
        pub runtime_ms: Option<u64>,
        pub community_rating: Option<f32>,
        pub official_rating: Option<String>,
        pub genres: Vec<String>,
        pub provider_ids: ProviderIds,
        pub series_id: Option<String>,
        pub series_title: Option<String>,
        pub season_id: Option<String>,
        pub season_number: Option<u32>,
        pub episode_number: Option<u32>,
        pub child_count: Option<u32>,
        /// ISO date (YYYY-MM-DD) when known.
        pub premiere_date: Option<String>,
        pub images: Vec<ImageRef>,
        pub user: UserData,
    }
}

record! {
    pub struct Page {
        pub items: Vec<Item>,
        pub start: u32,
        pub total: u32,
    }
}

simple_enum! {
    pub enum SortBy { Title, DateAdded, ReleaseDate, Rating, LastPlayed, Random }
}

record! {
    pub struct ItemQuery {
        pub library_id: Option<String>,
        pub start: u32,
        pub limit: u32,
        pub sort_by: SortBy,
        pub descending: bool,
        pub unplayed_only: bool,
        pub search: Option<String>,
        /// Only this genre (an id from `genres`).
        pub genre_id: Option<String>,
        /// The items of this collection instead of the library's.
        pub collection_id: Option<String>,
    }
}

record! {
    /// A genre of a library, to browse it ("categories").
    pub struct Genre {
        pub id: String,
        pub name: String,
    }
}

impl ItemQuery {
    pub fn library(library_id: impl Into<String>, start: u32, limit: u32) -> Self {
        Self {
            library_id: Some(library_id.into()),
            start,
            limit,
            sort_by: SortBy::Title,
            descending: false,
            unplayed_only: false,
            search: None,
            genre_id: None,
            collection_id: None,
        }
    }
}

simple_enum! {
    pub enum StreamKind { Video, Audio, Subtitle }
}

simple_enum! {
    pub enum HdrKind { Sdr, Hdr10, Hdr10Plus, Hlg, DolbyVision }
}

record! {
    pub struct MediaStream {
        /// Server stream index (Jellyfin `Index`, Plex stream id).
        pub index: i64,
        pub kind: StreamKind,
        pub codec: String,
        pub profile: Option<String>,
        pub language: Option<String>,
        pub title: Option<String>,
        pub display_title: Option<String>,
        pub channels: Option<u32>,
        pub channel_layout: Option<String>,
        pub bit_depth: Option<u32>,
        pub width: Option<u32>,
        pub height: Option<u32>,
        pub hdr: Option<HdrKind>,
        pub dv_profile: Option<u32>,
        pub is_default: bool,
        pub is_forced: bool,
        pub is_external: bool,
        /// For external subtitles: request to fetch the file.
        pub external: Option<HttpRequest>,
    }
}

record! {
    /// A ready-to-use HTTP request. Auth is in headers, never in the URL, so URLs are safe to log.
    pub struct HttpRequest {
        pub url: String,
        pub headers: Vec<Header>,
    }
}

record! {
    pub struct Header {
        pub name: String,
        pub value: String,
    }
}

record! {
    pub struct MediaSource {
        pub id: String,
        pub container: Option<String>,
        pub size: Option<u64>,
        pub bitrate: Option<u64>,
        pub duration_ms: Option<u64>,
        pub file_name: Option<String>,
        pub streams: Vec<MediaStream>,
        /// Original file, played as-is (direct play).
        pub direct: HttpRequest,
        /// Server transcode, only used when the device cannot play the original.
        pub transcode: Option<HttpRequest>,
        pub supports_direct_play: bool,
        pub default_audio_index: Option<i64>,
        pub default_subtitle_index: Option<i64>,
    }
}

record! {
    pub struct PlaybackInfo {
        pub item_id: String,
        pub play_session_id: Option<String>,
        pub sources: Vec<MediaSource>,
    }
}

record! {
    /// What the device can decode or pass through. Built by each platform from its codec lists.
    pub struct DeviceProfile {
        pub max_bitrate: Option<u64>,
        pub containers: Vec<String>,
        pub video_codecs: Vec<String>,
        /// Codecs (of `video_codecs`) the device also decodes at 10 bits (HEVC Main 10, VP9
        /// profile 2, AV1 Main 10, H.264 High 10). Others are limited to 8 bits.
        pub video_10bit_codecs: Vec<String>,
        pub audio_codecs: Vec<String>,
        pub subtitle_codecs: Vec<String>,
        pub max_width: Option<u32>,
        pub max_height: Option<u32>,
        pub hdr: Vec<HdrKind>,
    }
}

simple_enum! {
    pub enum PlaybackEvent { Start, Progress, Pause, Stop }
}

record! {
    /// What a viewer did with an item, to copy it to another server (history copy).
    pub struct WatchState {
        pub played: bool,
        /// Resume position; 0 when played or not started.
        pub position_ms: u64,
        /// Unix seconds.
        pub last_played_at: Option<i64>,
    }
}

/// Subtitles of a server conversion (they are burned into the picture or left out).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum SubtitleSelection {
    /// Whatever the server picks by itself (Plex: the account's own choice; Jellyfin: its
    /// defaults).
    #[default]
    Default,
    /// No subtitles.
    Off,
    /// This subtitle stream (`MediaStream::index`), burned in.
    Stream { index: i64 },
}

record! {
    /// The audio and subtitles a server conversion must carry, in the server's stream
    /// indexes (`MediaStream::index`). The default (nothing chosen) leaves both to the server.
    /// Direct play ignores it: the player switches the original file's tracks itself.
    #[derive(Default)]
    pub struct TrackSelection {
        /// The version (`MediaSource::id`) the indexes belong to; None: any that has them.
        pub source_id: Option<String>,
        /// Audio stream; None: the server's default.
        pub audio_index: Option<i64>,
        pub subtitle: SubtitleSelection,
    }
}

impl TrackSelection {
    /// Nothing chosen: the server decides, and asking has no side effect.
    pub fn is_default(&self) -> bool {
        self.audio_index.is_none() && self.subtitle == SubtitleSelection::Default
    }
}

simple_enum! {
    /// How a stream reaches the player: the original file, or the server's conversion.
    #[derive(Default)]
    pub enum PlayMethod {
        #[default]
        Direct,
        Transcode,
    }
}

record! {
    pub struct ProgressReport {
        pub item_id: String,
        pub media_source_id: Option<String>,
        pub play_session_id: Option<String>,
        pub position_ms: u64,
        pub duration_ms: Option<u64>,
        pub event: PlaybackEvent,
        pub audio_index: Option<i64>,
        pub subtitle_index: Option<i64>,
        /// Original file or conversion (Jellyfin shows and handles the session accordingly).
        /// Reports queued offline before this field existed read as `Direct`.
        #[serde(default)]
        pub play_method: PlayMethod,
    }
}

record! {
    /// A trailer: on the server (play it like any item) or elsewhere (a web page, e.g. YouTube).
    pub struct Trailer {
        pub title: String,
        pub item_id: Option<String>,
        pub url: Option<String>,
    }
}

simple_enum! {
    pub enum PersonKind { Actor, Director, Writer, Producer, Other }
}

record! {
    /// Someone in the cast or crew of a title.
    pub struct Person {
        pub name: String,
        /// The character (actors) or the job.
        pub role: Option<String>,
        pub kind: PersonKind,
        /// Absolute URL of a photo that needs no credentials.
        pub photo_url: Option<String>,
    }
}

simple_enum! {
    /// Part of an episode or movie that can be skipped.
    pub enum SegmentKind { Intro, Credits, Recap, Preview, Commercial }
}

record! {
    /// A skippable span (Plex markers, Jellyfin media segments), in milliseconds.
    pub struct Segment {
        pub kind: SegmentKind,
        pub start_ms: u64,
        pub end_ms: u64,
    }
}

record! {
    /// Original-file download (never a transcode, so it can be resumed).
    pub struct DownloadSource {
        pub request: HttpRequest,
        pub size: Option<u64>,
        pub file_name: String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_queued_before_play_method_read_as_direct() {
        let old = r#"{"item_id":"i1","media_source_id":null,"play_session_id":null,"position_ms":5,
            "duration_ms":null,"event":"Stop","audio_index":null,"subtitle_index":null}"#;
        let r: ProgressReport = serde_json::from_str(old).unwrap();
        assert_eq!(r.play_method, PlayMethod::Direct);
        assert!(TrackSelection::default().is_default());
    }
}
