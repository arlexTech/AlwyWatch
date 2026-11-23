// SPDX-License-Identifier: AGPL-3.0-or-later
//! Jellyfin client (built against the 12.4 OpenAPI; the shapes used here are unchanged since 10.x).
//!
//! Auth always goes in the `Authorization: MediaBrowser … Token="…"` header: servers with legacy
//! authorization disabled reject `api_key` query parameters and `X-Emby-Token`.

use std::collections::HashMap;

use net::reqwest::{Client, Method, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::*;
use crate::{Error, MediaServer, Result, header};

const TICKS_PER_MS: i64 = 10_000;
const ITEM_FIELDS: &str =
    "Overview,Genres,ProviderIds,ChildCount,PremiereDate,OfficialRating,PrimaryImageAspectRatio";
const IMAGE_TYPES: &str = "Primary,Backdrop,Thumb,Logo";

fn auth_header(client: &ClientInfo, token: Option<&str>) -> String {
    let enc = net::percent_encode;
    let mut h = format!(
        "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
        enc(&client.product),
        enc(&client.device_name),
        enc(&client.device_id),
        enc(&client.version)
    );
    if let Some(t) = token {
        h.push_str(&format!(", Token=\"{t}\""));
    }
    h
}

// ---------------------------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct QuickConnectCode {
    /// Kept by the app to poll; never shown.
    pub secret: String,
    /// Shown to the user, entered in another signed-in Jellyfin client.
    pub code: String,
}

/// Sign-in flows (no session yet).
#[derive(Debug, Clone)]
pub struct JellyfinAuth {
    http: Client,
    client: ClientInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicInfo {
    id: String,
    server_name: Option<String>,
    version: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthResult {
    user: UserDto,
    access_token: String,
    server_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserDto {
    id: String,
    name: String,
    #[serde(default)]
    policy: Option<PolicyDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PolicyDto {
    #[serde(default)]
    is_administrator: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct QuickConnectResult {
    #[serde(default)]
    authenticated: bool,
    secret: String,
    code: String,
}

impl JellyfinAuth {
    pub fn new(http: Client, client: ClientInfo) -> Self {
        Self { http, client }
    }

    fn req(&self, method: Method, base: &str, path: &str) -> RequestBuilder {
        self.http
            .request(method, net::join(base, path))
            .header("Authorization", auth_header(&self.client, None))
            .header("Accept", "application/json")
    }

    /// Unauthenticated server identity; also validates that the address is a Jellyfin server.
    pub async fn public_info(&self, base_url: &str) -> Result<ServerInfo> {
        let info: PublicInfo =
            net::send_json(self.req(Method::GET, base_url, "/System/Info/Public")).await?;
        Ok(ServerInfo {
            kind: ServerKind::Jellyfin,
            id: info.id,
            name: info.server_name.unwrap_or_else(|| "Jellyfin".into()),
            version: info.version.unwrap_or_default(),
        })
    }

    pub async fn quick_connect_enabled(&self, base_url: &str) -> Result<bool> {
        net::send_json(self.req(Method::GET, base_url, "/QuickConnect/Enabled")).await
    }

    pub async fn sign_in(&self, base_url: &str, username: &str, password: &str) -> Result<Session> {
        let req = self
            .req(Method::POST, base_url, "/Users/AuthenticateByName")
            .json(&json!({ "Username": username, "Pw": password }));
        let res: AuthResult = net::send_json(req).await?;
        self.session(base_url, res).await
    }

    pub async fn quick_connect_start(&self, base_url: &str) -> Result<QuickConnectCode> {
        let r: QuickConnectResult =
            net::send_json(self.req(Method::POST, base_url, "/QuickConnect/Initiate")).await?;
        Ok(QuickConnectCode {
            secret: r.secret,
            code: r.code,
        })
    }

    /// Returns [`Error::Pending`] until the code has been approved in another client.
    pub async fn quick_connect_poll(&self, base_url: &str, secret: &str) -> Result<Session> {
        let state: QuickConnectResult = net::send_json(
            self.req(Method::GET, base_url, "/QuickConnect/Connect")
                .query(&[("secret", secret)]),
        )
        .await?;
        if !state.authenticated {
            return Err(Error::Pending);
        }
        let res: AuthResult = net::send_json(
            self.req(
                Method::POST,
                base_url,
                "/Users/AuthenticateWithQuickConnect",
            )
            .json(&json!({ "Secret": secret })),
        )
        .await?;
        self.session(base_url, res).await
    }

    async fn session(&self, base_url: &str, res: AuthResult) -> Result<Session> {
        let name = match self.public_info(base_url).await {
            Ok(i) => i.name,
            Err(_) => "Jellyfin".into(),
        };
        Ok(Session {
            kind: ServerKind::Jellyfin,
            base_url: base_url.to_owned(),
            server_id: res.server_id,
            server_name: name,
            user_id: res.user.id,
            user_name: res.user.name,
            is_admin: res.user.policy.is_some_and(|p| p.is_administrator),
            token: res.access_token,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct QueryResult {
    items: Vec<BaseItemDto>,
    total_record_count: Option<u32>,
    start_index: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct RemoteTrailersDto {
    remote_trailers: Vec<RemoteTrailerDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct RemoteTrailerDto {
    url: String,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct PeopleDto {
    people: Vec<PersonDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct PersonDto {
    name: String,
    id: String,
    role: Option<String>,
    #[serde(rename = "Type")]
    kind: String,
    primary_image_tag: Option<String>,
}

fn convert_people(base_url: &str, list: Vec<PersonDto>) -> Vec<Person> {
    let mut out: Vec<Person> = list
        .into_iter()
        .filter(|p| !p.name.is_empty())
        .map(|p| {
            let kind = match p.kind.as_str() {
                "Actor" | "GuestStar" => PersonKind::Actor,
                "Director" => PersonKind::Director,
                "Writer" => PersonKind::Writer,
                "Producer" => PersonKind::Producer,
                _ => PersonKind::Other,
            };
            // Jellyfin serves images without credentials.
            let photo_url = p.primary_image_tag.map(|t| {
                format!(
                    "{}/Items/{}/Images/Primary?tag={}&maxWidth=240",
                    base_url.trim_end_matches('/'),
                    net::percent_encode(&p.id),
                    net::percent_encode(&t)
                )
            });
            Person {
                name: p.name,
                role: p.role.filter(|r| !r.is_empty()),
                kind,
                photo_url,
            }
        })
        .collect();
    // Stable: cast keeps its billing order.
    out.sort_by_key(|p| p.kind != PersonKind::Actor);
    out
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct BaseItemDto {
    id: String,
    name: Option<String>,
    sort_name: Option<String>,
    overview: Option<String>,
    production_year: Option<i32>,
    run_time_ticks: Option<i64>,
    community_rating: Option<f32>,
    official_rating: Option<String>,
    genres: Vec<String>,
    provider_ids: HashMap<String, Option<String>>,
    series_id: Option<String>,
    series_name: Option<String>,
    season_id: Option<String>,
    parent_index_number: Option<u32>,
    index_number: Option<u32>,
    image_tags: HashMap<String, String>,
    backdrop_image_tags: Vec<String>,
    image_blur_hashes: HashMap<String, HashMap<String, String>>,
    user_data: Option<UserDataDto>,
    child_count: Option<u32>,
    #[serde(rename = "Type")]
    kind: Option<String>,
    collection_type: Option<String>,
    premiere_date: Option<String>,
    series_primary_image_tag: Option<String>,
    parent_backdrop_item_id: Option<String>,
    parent_backdrop_image_tags: Vec<String>,
    parent_logo_item_id: Option<String>,
    parent_logo_image_tag: Option<String>,
    media_sources: Vec<MediaSourceDto>,
    path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct UserDataDto {
    played: bool,
    playback_position_ticks: i64,
    play_count: u32,
    is_favorite: bool,
    last_played_date: Option<String>,
    unplayed_item_count: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct PlaybackInfoResponse {
    media_sources: Vec<MediaSourceDto>,
    play_session_id: Option<String>,
    error_code: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct MediaSourceDto {
    id: String,
    path: Option<String>,
    container: Option<String>,
    size: Option<u64>,
    bitrate: Option<u64>,
    run_time_ticks: Option<i64>,
    supports_direct_play: bool,
    transcoding_url: Option<String>,
    media_streams: Vec<MediaStreamDto>,
    default_audio_stream_index: Option<i64>,
    default_subtitle_stream_index: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct MediaStreamDto {
    index: i64,
    #[serde(rename = "Type")]
    kind: Option<String>,
    codec: Option<String>,
    profile: Option<String>,
    language: Option<String>,
    title: Option<String>,
    display_title: Option<String>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    bit_depth: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    video_range_type: Option<String>,
    dv_profile: Option<u32>,
    is_default: bool,
    is_forced: bool,
    is_external: bool,
}

// ---------------------------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct JellyfinClient {
    session: Session,
    http: Client,
    auth: String,
}

impl JellyfinClient {
    pub fn new(session: Session, client: ClientInfo, http: Client) -> Self {
        let auth = auth_header(&client, Some(&session.token));
        Self {
            session,
            http,
            auth,
        }
    }

    fn auth_headers(&self) -> Vec<Header> {
        vec![header("Authorization", self.auth.clone())]
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, net::join(&self.session.base_url, path))
            .header("Authorization", &self.auth)
            .header("Accept", "application/json")
    }

    /// This user's preferences stored by an app on the server (`DisplayPreferences`, whose
    /// `CustomPrefs` hold free key/values): how AlwyWatch carries settings across devices.
    pub async fn app_prefs(&self, client: &str) -> Result<serde_json::Value> {
        net::send_json(
            self.req(Method::GET, "/DisplayPreferences/alwywatch")
                .query(&[("userId", self.uid()), ("client", client)]),
        )
        .await
    }

    /// Saves preferences read with `app_prefs` (the whole object, `CustomPrefs` changed).
    pub async fn set_app_prefs(&self, client: &str, prefs: &serde_json::Value) -> Result<()> {
        net::send(
            self.req(Method::POST, "/DisplayPreferences/alwywatch")
                .query(&[("userId", self.uid()), ("client", client)])
                .json(prefs),
        )
        .await?;
        Ok(())
    }

    fn uid(&self) -> &str {
        &self.session.user_id
    }

    async fn query(&self, path: &str, params: &[(&str, String)]) -> Result<QueryResult> {
        net::send_json(self.req(Method::GET, path).query(params)).await
    }

    fn list_params(&self) -> Vec<(&'static str, String)> {
        vec![
            ("userId", self.uid().to_owned()),
            ("fields", ITEM_FIELDS.into()),
            ("enableImageTypes", IMAGE_TYPES.into()),
            ("imageTypeLimit", "1".into()),
        ]
    }

    fn convert(&self, dto: BaseItemDto) -> Item {
        convert_item(dto)
    }

    fn convert_all(&self, dtos: Vec<BaseItemDto>) -> Vec<Item> {
        dtos.into_iter().map(|d| self.convert(d)).collect()
    }

    fn convert_source(
        &self,
        item_id: &str,
        s: MediaSourceDto,
        session: &Option<String>,
    ) -> MediaSource {
        let mut direct = format!(
            "{}/Videos/{}/stream?static=true&mediaSourceId={}",
            self.session.base_url,
            item_id,
            net::percent_encode(&s.id)
        );
        if let Some(ps) = session {
            direct.push_str(&format!("&playSessionId={}", net::percent_encode(ps)));
        }
        let streams = s
            .media_streams
            .into_iter()
            .filter_map(|m| {
                let kind = match m.kind.as_deref() {
                    Some("Video") => StreamKind::Video,
                    Some("Audio") => StreamKind::Audio,
                    Some("Subtitle") => StreamKind::Subtitle,
                    _ => return None,
                };
                let codec = m.codec.unwrap_or_default().to_ascii_lowercase();
                let external =
                    (kind == StreamKind::Subtitle && m.is_external).then(|| HttpRequest {
                        url: format!(
                            "{}/Videos/{}/{}/Subtitles/{}/0/Stream.{}",
                            self.session.base_url,
                            item_id,
                            net::percent_encode(&s.id),
                            m.index,
                            if codec.is_empty() {
                                "srt"
                            } else {
                                codec.as_str()
                            }
                        ),
                        headers: self.auth_headers(),
                    });
                Some(MediaStream {
                    index: m.index,
                    kind,
                    hdr: (kind == StreamKind::Video)
                        .then(|| hdr_from_range(m.video_range_type.as_deref())),
                    codec,
                    profile: m.profile,
                    language: m.language,
                    title: m.title,
                    display_title: m.display_title,
                    channels: m.channels,
                    channel_layout: m.channel_layout,
                    bit_depth: m.bit_depth,
                    width: m.width,
                    height: m.height,
                    dv_profile: m.dv_profile,
                    is_default: m.is_default,
                    is_forced: m.is_forced,
                    is_external: m.is_external,
                    external,
                })
            })
            .collect();
        MediaSource {
            file_name: s.path.as_deref().and_then(file_name),
            id: s.id,
            container: s.container,
            size: s.size,
            bitrate: s.bitrate,
            duration_ms: s.run_time_ticks.map(ticks_to_ms),
            streams,
            direct: HttpRequest {
                url: direct,
                headers: self.auth_headers(),
            },
            transcode: s.transcoding_url.map(|u| HttpRequest {
                url: net::join(&self.session.base_url, &u),
                headers: self.auth_headers(),
            }),
            supports_direct_play: s.supports_direct_play,
            default_audio_index: s.default_audio_stream_index,
            default_subtitle_index: s.default_subtitle_stream_index,
        }
    }
}

fn ticks_to_ms(t: i64) -> u64 {
    (t.max(0) / TICKS_PER_MS) as u64
}

fn file_name(path: &str) -> Option<String> {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn hdr_from_range(range: Option<&str>) -> HdrKind {
    match range.unwrap_or("SDR") {
        r if r.starts_with("DOVI") => HdrKind::DolbyVision,
        "HDR10" => HdrKind::Hdr10,
        "HDR10Plus" => HdrKind::Hdr10Plus,
        "HLG" => HdrKind::Hlg,
        _ => HdrKind::Sdr,
    }
}

fn kind_of(t: Option<&str>) -> ItemKind {
    match t.unwrap_or_default() {
        "Movie" => ItemKind::Movie,
        "Series" => ItemKind::Series,
        "Season" => ItemKind::Season,
        "Episode" => ItemKind::Episode,
        "BoxSet" => ItemKind::Collection,
        "Folder" | "CollectionFolder" | "UserView" => ItemKind::Folder,
        _ => ItemKind::Other,
    }
}

/// Unix seconds from an ISO-8601 timestamp (`2026-11-19T13:09:56.1234567Z`), without a date crate.
pub(crate) fn iso_to_unix(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.trim_end_matches('Z').split([':', '.', '+']);
    let (hh, mm, ss) = (
        t.next()?.parse::<i64>().ok()?,
        t.next()?.parse::<i64>().ok()?,
        t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
    );
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// ISO-8601 UTC from Unix seconds (inverse of `iso_to_unix`).
pub(crate) fn unix_to_iso(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.0000000Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn image(
    images: &mut Vec<ImageRef>,
    kind: ImageKind,
    type_name: &str,
    owner: &str,
    tag: &str,
    hashes: &HashMap<String, HashMap<String, String>>,
) {
    images.push(ImageRef {
        kind,
        path: format!(
            "/Items/{owner}/Images/{type_name}?tag={}",
            net::percent_encode(tag)
        ),
        blurhash: hashes.get(type_name).and_then(|m| m.get(tag)).cloned(),
    });
}

fn convert_item(d: BaseItemDto) -> Item {
    let kind = kind_of(d.kind.as_deref());
    let mut images = Vec::new();
    let h = &d.image_blur_hashes;
    if let Some(tag) = d.image_tags.get("Primary") {
        image(&mut images, ImageKind::Primary, "Primary", &d.id, tag, h);
    } else if let (Some(sid), Some(tag)) = (&d.series_id, &d.series_primary_image_tag) {
        image(&mut images, ImageKind::Primary, "Primary", sid, tag, h);
    }
    if let (Some(sid), Some(tag)) = (&d.series_id, &d.series_primary_image_tag) {
        image(
            &mut images,
            ImageKind::SeriesPrimary,
            "Primary",
            sid,
            tag,
            h,
        );
    }
    if let Some(tag) = d.backdrop_image_tags.first() {
        image(&mut images, ImageKind::Backdrop, "Backdrop", &d.id, tag, h);
    } else if let (Some(pid), Some(tag)) = (
        &d.parent_backdrop_item_id,
        d.parent_backdrop_image_tags.first(),
    ) {
        image(&mut images, ImageKind::Backdrop, "Backdrop", pid, tag, h);
    }
    if let Some(tag) = d.image_tags.get("Thumb") {
        image(&mut images, ImageKind::Thumb, "Thumb", &d.id, tag, h);
    }
    if let Some(tag) = d.image_tags.get("Logo") {
        image(&mut images, ImageKind::Logo, "Logo", &d.id, tag, h);
    } else if let (Some(pid), Some(tag)) = (&d.parent_logo_item_id, &d.parent_logo_image_tag) {
        image(&mut images, ImageKind::Logo, "Logo", pid, tag, h);
    }
    let pid = |k: &str| {
        d.provider_ids
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(k))
            .and_then(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let provider_ids = ProviderIds {
        tmdb: pid("Tmdb"),
        tvdb: pid("Tvdb"),
        imdb: pid("Imdb"),
    };
    let u = d.user_data.unwrap_or_default();
    Item {
        title: d.name.unwrap_or_default(),
        sort_title: d.sort_name,
        overview: d.overview,
        year: d.production_year,
        runtime_ms: d.run_time_ticks.map(ticks_to_ms),
        community_rating: d.community_rating,
        official_rating: d.official_rating,
        genres: d.genres,
        provider_ids,
        series_id: d.series_id,
        series_title: d.series_name,
        season_id: d.season_id,
        // A season's own number is its IndexNumber (ParentIndexNumber is for episodes).
        season_number: if kind == ItemKind::Season {
            d.index_number
        } else {
            d.parent_index_number
        },
        episode_number: if kind == ItemKind::Season {
            None
        } else {
            d.index_number
        },
        child_count: d.child_count,
        premiere_date: d.premiere_date.map(|p| p.chars().take(10).collect()),
        images,
        user: UserData {
            played: u.played,
            position_ms: ticks_to_ms(u.playback_position_ticks),
            play_count: u.play_count,
            favorite: u.is_favorite,
            last_played_at: u.last_played_date.as_deref().and_then(iso_to_unix),
            unplayed_count: u.unplayed_item_count,
        },
        id: d.id,
        kind,
    }
}

fn sort_param(s: SortBy) -> &'static str {
    match s {
        SortBy::Title => "SortName",
        // A show counts by its newest episode (movies have no such date: theirs decides).
        SortBy::DateAdded => "DateLastContentAdded,DateCreated,SortName",
        SortBy::ReleaseDate => "PremiereDate,ProductionYear,SortName",
        SortBy::Rating => "CommunityRating,SortName",
        SortBy::LastPlayed => "DatePlayed,SortName",
        SortBy::Random => "Random",
    }
}

/// Jellyfin `DeviceProfile` built from ours: direct play everything the device decodes,
/// HLS/TS transcode as the fallback, text subtitles delivered as external files. Bit depth and
/// resolution limits become codec conditions, so Jellyfin offers a transcode for files the
/// device cannot decode (e.g. 10-bit HEVC on an 8-bit-only decoder).
fn jellyfin_profile(p: &DeviceProfile) -> serde_json::Value {
    let join = |v: &Vec<String>| v.join(",");
    let mut codec_profiles: Vec<serde_json::Value> = p
        .video_codecs
        .iter()
        .filter(|c| !p.video_10bit_codecs.contains(c))
        .map(|c| {
            json!({
                "Type": "Video",
                "Codec": c,
                "Conditions": [{ "Condition": "LessThanEqual", "Property": "VideoBitDepth", "Value": "8", "IsRequired": false }],
            })
        })
        .collect();
    let mut size = vec![];
    if let Some(w) = p.max_width {
        size.push(json!({ "Condition": "LessThanEqual", "Property": "Width", "Value": w.to_string(), "IsRequired": false }));
    }
    if let Some(h) = p.max_height {
        size.push(json!({ "Condition": "LessThanEqual", "Property": "Height", "Value": h.to_string(), "IsRequired": false }));
    }
    if !size.is_empty() {
        codec_profiles.push(json!({ "Type": "Video", "Conditions": size }));
    }
    // Transcode to HEVC only when the device decodes HEVC at 10 bits too (HDR sources stay HDR);
    // otherwise H.264 first.
    let transcode_codecs = if p.video_10bit_codecs.iter().any(|c| c == "hevc") {
        "hevc,h264"
    } else {
        "h264,hevc"
    };
    let subs: Vec<_> = p
        .subtitle_codecs
        .iter()
        .flat_map(|c| {
            let embed = json!({ "Format": c, "Method": "Embed" });
            if matches!(
                c.as_str(),
                "srt" | "subrip" | "vtt" | "webvtt" | "ass" | "ssa"
            ) {
                vec![embed, json!({ "Format": c, "Method": "External" })]
            } else {
                vec![embed]
            }
        })
        .collect();
    json!({
        "Name": "AlwyWatch",
        "MaxStreamingBitrate": p.max_bitrate,
        "DirectPlayProfiles": [{
            "Type": "Video",
            "Container": join(&p.containers),
            "VideoCodec": join(&p.video_codecs),
            "AudioCodec": join(&p.audio_codecs),
        }],
        "TranscodingProfiles": [{
            "Type": "Video",
            "Container": "ts",
            "Protocol": "hls",
            "Context": "Streaming",
            "VideoCodec": transcode_codecs,
            "AudioCodec": join(&p.audio_codecs),
            "MinSegments": 1,
            "BreakOnNonKeyFrames": true,
        }],
        "SubtitleProfiles": subs,
        "CodecProfiles": codec_profiles,
        "ContainerProfiles": [],
    })
}

/// Puts the chosen tracks in a `PlaybackInfo` request body, so the `TranscodingUrl` it
/// returns carries them. A chosen subtitle is burned in when converting (players don't fetch
/// Jellyfin's separately delivered subtitles for a conversion); -1 is Jellyfin's "none".
fn apply_selection(body: &mut serde_json::Value, selection: &TrackSelection) {
    if let Some(id) = &selection.source_id {
        body["MediaSourceId"] = json!(id);
    }
    if let Some(a) = selection.audio_index {
        body["AudioStreamIndex"] = json!(a);
    }
    match selection.subtitle {
        SubtitleSelection::Default => {}
        SubtitleSelection::Off => body["SubtitleStreamIndex"] = json!(-1),
        SubtitleSelection::Stream { index } => {
            body["SubtitleStreamIndex"] = json!(index);
            body["AlwaysBurnInSubtitleWhenTranscoding"] = json!(true);
        }
    }
}

impl MediaServer for JellyfinClient {
    fn session(&self) -> &Session {
        &self.session
    }

    async fn libraries(&self) -> Result<Vec<MediaLibrary>> {
        let r = self
            .query("/UserViews", &[("userId", self.uid().to_owned())])
            .await?;
        Ok(r.items
            .into_iter()
            .map(|d| MediaLibrary {
                kind: match d.collection_type.as_deref() {
                    Some("movies") => LibraryKind::Movies,
                    Some("tvshows") => LibraryKind::Shows,
                    _ => LibraryKind::Other,
                },
                title: d.name.unwrap_or_default(),
                id: d.id,
            })
            .collect())
    }

    async fn items(&self, q: &ItemQuery) -> Result<Page> {
        let mut p = self.list_params();
        // A collection's own items are its direct children (any library).
        if let Some(c) = &q.collection_id {
            p.push(("parentId", c.clone()));
        } else {
            p.push(("recursive", "true".into()));
            p.push(("includeItemTypes", "Movie,Series".into()));
        }
        if let Some(g) = &q.genre_id {
            p.push(("genreIds", g.clone()));
        }
        p.push(("startIndex", q.start.to_string()));
        p.push(("limit", q.limit.to_string()));
        p.push(("sortBy", sort_param(q.sort_by).into()));
        p.push((
            "sortOrder",
            if q.descending {
                "Descending"
            } else {
                "Ascending"
            }
            .into(),
        ));
        p.push(("enableTotalRecordCount", "true".into()));
        if let (Some(lib), None) = (&q.library_id, &q.collection_id) {
            p.push(("parentId", lib.clone()));
        }
        if q.unplayed_only {
            p.push(("isPlayed", "false".into()));
        }
        if let Some(s) = q.search.as_ref().filter(|s| !s.is_empty()) {
            p.push(("searchTerm", s.clone()));
        }
        let r = self.query("/Items", &p).await?;
        let items = self.convert_all(r.items);
        Ok(Page {
            start: r.start_index.unwrap_or(q.start),
            total: r.total_record_count.unwrap_or(items.len() as u32),
            items,
        })
    }

    async fn shows_last_watched(
        &self,
        series_ids: &[String],
    ) -> Result<std::collections::HashMap<String, i64>> {
        // One request: the most recently finished episodes; the first one per show is its date.
        // (LastPlayedDate also moves when an episode is merely started: only played ones count.)
        let mut p = self.list_params();
        p.push(("recursive", "true".into()));
        p.push(("includeItemTypes", "Episode".into()));
        p.push(("filters", "IsPlayed".into()));
        p.push(("sortBy", "DatePlayed".into()));
        p.push(("sortOrder", "Descending".into()));
        p.push(("limit", "300".into()));
        p.push(("enableTotalRecordCount", "false".into()));
        let mut out = std::collections::HashMap::new();
        for d in self.query("/Items", &p).await?.items {
            let at = d
                .user_data
                .as_ref()
                .and_then(|u| u.last_played_date.as_deref())
                .and_then(iso_to_unix);
            if let (Some(s), Some(at)) = (d.series_id.clone(), at)
                && series_ids.contains(&s)
            {
                out.entry(s).or_insert(at);
            }
        }
        Ok(out)
    }

    async fn remove_from_continue_watching(&self, item_id: &str) -> Result<bool> {
        // Marking unplayed forgets the position (and the last played date): out of "resume".
        // Next episodes have nothing to forget: Jellyfin cannot hide them, the caller does.
        self.set_played(item_id, false).await?;
        Ok(false)
    }

    async fn genres(&self, library_id: &str) -> Result<Vec<Genre>> {
        let p = vec![
            ("userId", self.uid().to_owned()),
            ("parentId", library_id.to_owned()),
            ("sortBy", "SortName".into()),
        ];
        Ok(self
            .query("/Genres", &p)
            .await?
            .items
            .into_iter()
            .map(|d| Genre {
                id: d.id,
                name: d.name.unwrap_or_default(),
            })
            .collect())
    }

    async fn collections(&self, library_id: &str) -> Result<Vec<Item>> {
        let mut p = self.list_params();
        p.push(("parentId", library_id.to_owned()));
        p.push(("includeItemTypes", "BoxSet".into()));
        p.push(("recursive", "true".into()));
        p.push(("sortBy", "SortName".into()));
        Ok(self.convert_all(self.query("/Items", &p).await?.items))
    }

    async fn item(&self, id: &str) -> Result<Item> {
        let d: BaseItemDto = net::send_json(
            self.req(Method::GET, &format!("/Items/{}", net::percent_encode(id)))
                .query(&[("userId", self.uid()), ("fields", ITEM_FIELDS)]),
        )
        .await?;
        Ok(self.convert(d))
    }

    async fn seasons(&self, series_id: &str) -> Result<Vec<Item>> {
        let path = format!("/Shows/{}/Seasons", net::percent_encode(series_id));
        Ok(self.convert_all(self.query(&path, &self.list_params()).await?.items))
    }

    async fn episodes(&self, series_id: &str, season_id: &str) -> Result<Vec<Item>> {
        let path = format!("/Shows/{}/Episodes", net::percent_encode(series_id));
        let mut p = self.list_params();
        p.push(("seasonId", season_id.to_owned()));
        Ok(self.convert_all(self.query(&path, &p).await?.items))
    }

    async fn resume(&self, limit: u32) -> Result<Vec<Item>> {
        let mut p = self.list_params();
        p.push(("limit", limit.to_string()));
        p.push(("mediaTypes", "Video".into()));
        p.push(("enableTotalRecordCount", "false".into()));
        Ok(self.convert_all(self.query("/UserItems/Resume", &p).await?.items))
    }

    async fn next_up(&self, limit: u32) -> Result<Vec<Item>> {
        let mut p = self.list_params();
        p.push(("limit", limit.to_string()));
        p.push(("enableTotalRecordCount", "false".into()));
        Ok(self.convert_all(self.query("/Shows/NextUp", &p).await?.items))
    }

    async fn latest(&self, library_id: &str, limit: u32) -> Result<Vec<Item>> {
        let mut p = self.list_params();
        p.push(("parentId", library_id.to_owned()));
        p.push(("limit", limit.to_string()));
        // Episode by episode, like Plex (grouped, several new episodes come as their show or
        // season with nothing saying which): the apps group them per show with a label.
        p.push(("groupItems", "false".into()));
        let items: Vec<BaseItemDto> =
            net::send_json(self.req(Method::GET, "/Items/Latest").query(&p)).await?;
        Ok(self.convert_all(items))
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Item>> {
        let mut p = self.list_params();
        p.push(("searchTerm", query.to_owned()));
        p.push(("recursive", "true".into()));
        p.push(("includeItemTypes", "Movie,Series,Episode".into()));
        p.push(("limit", limit.to_string()));
        Ok(self.convert_all(self.query("/Items", &p).await?.items))
    }

    async fn playback_info_with(
        &self,
        item_id: &str,
        profile: &DeviceProfile,
        selection: &TrackSelection,
    ) -> Result<PlaybackInfo> {
        let mut body = json!({
            "UserId": self.uid(),
            "MaxStreamingBitrate": profile.max_bitrate,
            "DeviceProfile": jellyfin_profile(profile),
            "EnableDirectPlay": true,
            "EnableDirectStream": true,
            "EnableTranscoding": true,
            "AutoOpenLiveStream": true,
        });
        apply_selection(&mut body, selection);
        let r: PlaybackInfoResponse = net::send_json(
            self.req(
                Method::POST,
                &format!("/Items/{}/PlaybackInfo", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid())])
            .json(&body),
        )
        .await?;
        if let Some(code) = r.error_code {
            return Err(Error::invalid(i18n::tr!(
                "playback refused by server: {code}",
                code = code
            )));
        }
        let session = r.play_session_id;
        let mut sources: Vec<_> = r
            .media_sources
            .into_iter()
            .map(|s| self.convert_source(item_id, s, &session))
            .collect();
        // Direct-playable sources first.
        sources.sort_by_key(|s| !s.supports_direct_play);
        Ok(PlaybackInfo {
            item_id: item_id.to_owned(),
            play_session_id: session,
            sources,
        })
    }

    async fn segments(&self, item_id: &str) -> Result<Vec<Segment>> {
        // Jellyfin 10.10+: filled by a segment provider (e.g. the Intro Skipper plugin).
        let found = net::send_json::<SegmentsDto>(self.req(
            Method::GET,
            &format!("/MediaSegments/{}", net::percent_encode(item_id)),
        ))
        .await
        .map(|r| convert_segments(r.items));
        match found {
            Ok(list) if !list.is_empty() => Ok(list),
            // None detected (no provider installed): the file's own chapters, when they are
            // named like an intro or credits ("Opening", "Générique de fin"…).
            other => {
                let d: ChaptersDto = net::send_json(
                    self.req(
                        Method::GET,
                        &format!("/Items/{}", net::percent_encode(item_id)),
                    )
                    .query(&[("userId", self.uid()), ("fields", "Chapters")]),
                )
                .await
                .unwrap_or_default();
                let from_chapters = segments_from_chapters(&d.chapters, d.run_time_ticks);
                if from_chapters.is_empty() {
                    other
                } else {
                    Ok(from_chapters)
                }
            }
        }
    }

    async fn report_progress(&self, r: &ProgressReport) -> Result<()> {
        let path = match r.event {
            PlaybackEvent::Start => "/Sessions/Playing",
            PlaybackEvent::Progress | PlaybackEvent::Pause => "/Sessions/Playing/Progress",
            PlaybackEvent::Stop => "/Sessions/Playing/Stopped",
        };
        let body = json!({
            "ItemId": r.item_id,
            "MediaSourceId": r.media_source_id,
            "PlaySessionId": r.play_session_id,
            "PositionTicks": r.position_ms as i64 * TICKS_PER_MS,
            "IsPaused": r.event == PlaybackEvent::Pause,
            "PlayMethod": match r.play_method {
                PlayMethod::Direct => "DirectPlay",
                PlayMethod::Transcode => "Transcode",
            },
            "CanSeek": true,
            "AudioStreamIndex": r.audio_index,
            "SubtitleStreamIndex": r.subtitle_index,
        });
        net::send(self.req(Method::POST, path).json(&body)).await?;
        Ok(())
    }

    async fn people(&self, item_id: &str) -> Result<Vec<Person>> {
        let d: PeopleDto = net::send_json(
            self.req(
                Method::GET,
                &format!("/Items/{}", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid()), ("fields", "People")]),
        )
        .await?;
        Ok(convert_people(&self.session.base_url, d.people))
    }

    async fn trailers(&self, item_id: &str) -> Result<Vec<Trailer>> {
        let local: Vec<BaseItemDto> = net::send_json(
            self.req(
                Method::GET,
                &format!("/Items/{}/LocalTrailers", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid())]),
        )
        .await
        .unwrap_or_default();
        let mut out: Vec<Trailer> = local
            .into_iter()
            .map(|t| Trailer {
                title: t.name.unwrap_or_else(|| i18n::tr!("Trailer")),
                item_id: Some(t.id),
                url: None,
            })
            .collect();
        let item: RemoteTrailersDto = net::send_json(
            self.req(
                Method::GET,
                &format!("/Items/{}", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid()), ("fields", "RemoteTrailers")]),
        )
        .await?;
        out.extend(
            item.remote_trailers
                .into_iter()
                .filter(|t| !t.url.is_empty())
                .map(|t| Trailer {
                    title: t.name.unwrap_or_else(|| i18n::tr!("Trailer")),
                    item_id: None,
                    url: Some(t.url),
                }),
        );
        Ok(out)
    }

    async fn theme_song(&self, item_id: &str) -> Result<Option<HttpRequest>> {
        let r: QueryResult = net::send_json(
            self.req(
                Method::GET,
                &format!("/Items/{}/ThemeSongs", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid()), ("inheritFromParent", "true")]),
        )
        .await?;
        Ok(r.items.into_iter().next().map(|song| HttpRequest {
            url: format!(
                "{}/Audio/{}/stream?static=true",
                self.session.base_url,
                net::percent_encode(&song.id)
            ),
            headers: self.auth_headers(),
        }))
    }

    async fn watchlist(&self, _plex_tv_token: Option<&str>) -> Result<Vec<Item>> {
        let mut p = self.list_params();
        p.push(("filters", "IsFavorite".into()));
        p.push(("recursive", "true".into()));
        p.push(("includeItemTypes", "Movie,Series".into()));
        p.push(("sortBy", "DateCreated".into()));
        p.push(("sortOrder", "Descending".into()));
        Ok(self.convert_all(self.query("/Items", &p).await?.items))
    }

    async fn set_watchlisted(
        &self,
        item_id: &str,
        on: bool,
        _plex_tv_token: Option<&str>,
    ) -> Result<()> {
        let method = if on { Method::POST } else { Method::DELETE };
        net::send(
            self.req(
                method,
                &format!("/UserFavoriteItems/{}", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid())]),
        )
        .await?;
        Ok(())
    }

    async fn set_played(&self, item_id: &str, played: bool) -> Result<()> {
        let method = if played { Method::POST } else { Method::DELETE };
        net::send(
            self.req(
                method,
                &format!("/UserPlayedItems/{}", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid())]),
        )
        .await?;
        Ok(())
    }

    async fn download_source(&self, item_id: &str) -> Result<DownloadSource> {
        let d: BaseItemDto = net::send_json(
            self.req(
                Method::GET,
                &format!("/Items/{}", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid()), ("fields", "MediaSources,Path")]),
        )
        .await?;
        let source = d.media_sources.first();
        let name = source
            .and_then(|s| s.path.as_deref())
            .or(d.path.as_deref())
            .and_then(file_name)
            .unwrap_or_else(|| {
                format!(
                    "{item_id}.{}",
                    source.and_then(|s| s.container.as_deref()).unwrap_or("mkv")
                )
            });
        Ok(DownloadSource {
            request: HttpRequest {
                url: format!(
                    "{}/Items/{}/Download",
                    self.session.base_url,
                    net::percent_encode(item_id)
                ),
                headers: self.auth_headers(),
            },
            size: source.and_then(|s| s.size),
            file_name: name,
        })
    }

    async fn watch_history(&self) -> Result<Vec<Item>> {
        const PAGE: u32 = 500;
        let mut out: Vec<Item> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for filter in ["IsPlayed", "IsResumable"] {
            // A page at a time: a whole history in one response can outlast the read timeout.
            let mut start = 0;
            loop {
                let p = [
                    ("userId", self.uid().to_owned()),
                    ("recursive", "true".into()),
                    ("includeItemTypes", "Movie,Episode".into()),
                    ("filters", filter.into()),
                    ("fields", "ProviderIds".into()),
                    ("enableImages", "false".into()),
                    ("enableUserData", "true".into()),
                    ("enableTotalRecordCount", "false".into()),
                    ("sortBy", "SortName".into()),
                    ("startIndex", start.to_string()),
                    ("limit", PAGE.to_string()),
                ];
                let page = self.convert_all(self.query("/Items", &p).await?.items);
                let n = page.len() as u32;
                out.extend(page.into_iter().filter(|i| seen.insert(i.id.clone())));
                start += n;
                if n < PAGE {
                    break;
                }
            }
        }
        Ok(out)
    }

    async fn set_watch_state(&self, item_id: &str, state: &WatchState) -> Result<()> {
        let mut body = serde_json::json!({
            "Played": state.played,
            "PlaybackPositionTicks": if state.played { 0 } else { state.position_ms as i64 * 10_000 },
        });
        if state.played {
            body["PlayCount"] = 1.into();
        }
        if let Some(at) = state.last_played_at {
            body["LastPlayedDate"] = unix_to_iso(at).into();
        }
        net::send(
            self.req(
                Method::POST,
                &format!("/UserItems/{}/UserData", net::percent_encode(item_id)),
            )
            .query(&[("userId", self.uid())])
            .json(&body),
        )
        .await?;
        Ok(())
    }

    async fn provider_index(&self, kind: ItemKind) -> Result<Vec<Item>> {
        let t = match kind {
            ItemKind::Movie => "Movie",
            ItemKind::Series => "Series",
            _ => {
                return Err(Error::invalid(
                    "provider index supports movies and series only",
                ));
            }
        };
        let p = [
            ("userId", self.uid().to_owned()),
            ("recursive", "true".into()),
            ("includeItemTypes", t.into()),
            ("fields", "ProviderIds".into()),
            ("enableImages", "false".into()),
            ("enableUserData", "false".into()),
            ("enableTotalRecordCount", "false".into()),
        ];
        Ok(self.convert_all(self.query("/Items", &p).await?.items))
    }

    fn image_request(&self, _item_id: &str, image: &ImageRef, max_width: u32) -> HttpRequest {
        HttpRequest {
            url: format!(
                "{}{}&fillWidth={max_width}&quality=90",
                self.session.base_url, image.path
            ),
            headers: self.auth_headers(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct SegmentsDto {
    items: Vec<SegmentDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct SegmentDto {
    #[serde(rename = "Type")]
    kind: String,
    start_ticks: i64,
    end_ticks: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct ChaptersDto {
    chapters: Vec<ChapterDto>,
    run_time_ticks: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct ChapterDto {
    start_position_ticks: i64,
    name: Option<String>,
}

/// What a chapter's name says it is, if anything (English and French names; whole words, so
/// "Operation" is not an "op").
fn chapter_kind(name: &str) -> Option<SegmentKind> {
    let folded: String = name
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' => 'a',
            'ô' => 'o',
            'û' | 'ù' => 'u',
            'î' | 'ï' => 'i',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    let words: Vec<&str> = folded.split_whitespace().collect();
    let has = |w: &str| words.contains(&w);
    let phrase = |p: &str| format!(" {} ", words.join(" ")).contains(&format!(" {p} "));
    // "Credits End" marks where the credits stop (an after-credits scene may follow).
    if has("credits") && has("end") && !phrase("end credits") {
        return None;
    }
    if has("recap") || has("previously") || has("precedemment") || phrase("resume des episodes") {
        Some(SegmentKind::Recap)
    } else if has("preview")
        || phrase("next time")
        || has("prochainement")
        || has("next") && has("episode")
    {
        Some(SegmentKind::Preview)
    } else if has("credits") && !has("opening")
        || has("ending")
        || has("outro")
        || has("ed")
        || has("generique") && (has("fin") || has("final"))
        || phrase("end credits")
    {
        Some(SegmentKind::Credits)
    } else if has("intro") || has("opening") || has("op") || has("generique") || has("titles") {
        Some(SegmentKind::Intro)
    } else {
        None
    }
}

/// Skippable spans from named chapters: a chapter runs until the next one (credits until the
/// end). Implausible ones (a 20-minute "intro") are left out.
fn segments_from_chapters(chapters: &[ChapterDto], runtime_ticks: Option<i64>) -> Vec<Segment> {
    let end_of_file = runtime_ticks.map(ticks_to_ms);
    let mut out = Vec::new();
    for (i, c) in chapters.iter().enumerate() {
        let Some(kind) = c.name.as_deref().and_then(chapter_kind) else {
            continue;
        };
        let start_ms = ticks_to_ms(c.start_position_ticks.max(0));
        let end_ms = match chapters.get(i + 1) {
            Some(n) => ticks_to_ms(n.start_position_ticks.max(0)),
            None => match end_of_file {
                Some(e) => e,
                None => continue,
            },
        };
        let len = end_ms.saturating_sub(start_ms);
        let max = match kind {
            SegmentKind::Credits => 15 * 60_000,
            SegmentKind::Commercial => 10 * 60_000,
            _ => 5 * 60_000,
        };
        if len >= 5_000 && len <= max {
            out.push(Segment {
                kind,
                start_ms,
                end_ms,
            });
        }
    }
    out
}

fn convert_segments(list: Vec<SegmentDto>) -> Vec<Segment> {
    let mut out: Vec<Segment> = list
        .into_iter()
        .filter(|s| s.end_ticks > s.start_ticks && s.start_ticks >= 0)
        .filter_map(|s| {
            let kind = match s.kind.as_str() {
                "Intro" => SegmentKind::Intro,
                "Outro" => SegmentKind::Credits,
                "Recap" => SegmentKind::Recap,
                "Preview" => SegmentKind::Preview,
                "Commercial" => SegmentKind::Commercial,
                _ => return None,
            };
            Some(Segment {
                kind,
                start_ms: ticks_to_ms(s.start_ticks),
                end_ms: ticks_to_ms(s.end_ticks),
            })
        })
        .collect();
    out.sort_by_key(|s| s.start_ms);
    out
}

#[cfg(test)]
mod chapter_tests {
    use super::*;

    fn ch(min: f64, name: &str) -> ChapterDto {
        ChapterDto {
            start_position_ticks: (min * 60_000.0) as i64 * TICKS_PER_MS,
            name: Some(name.into()),
        }
    }

    #[test]
    fn chapter_names_are_recognised() {
        assert_eq!(chapter_kind("Opening"), Some(SegmentKind::Intro));
        assert_eq!(chapter_kind("OP"), Some(SegmentKind::Intro));
        assert_eq!(chapter_kind("Générique"), Some(SegmentKind::Intro));
        assert_eq!(chapter_kind("Générique de fin"), Some(SegmentKind::Credits));
        assert_eq!(chapter_kind("End Credits"), Some(SegmentKind::Credits));
        assert_eq!(chapter_kind("Ending"), Some(SegmentKind::Credits));
        assert_eq!(chapter_kind("Previously on"), Some(SegmentKind::Recap));
        assert_eq!(chapter_kind("Preview"), Some(SegmentKind::Preview));
        assert_eq!(chapter_kind("Opening Credits"), Some(SegmentKind::Intro));
        assert_eq!(chapter_kind("Credits Start"), Some(SegmentKind::Credits));
        assert_eq!(chapter_kind("Credits End"), None);
        assert_eq!(chapter_kind("Chapter 2"), None);
        assert_eq!(chapter_kind("Operation Overlord"), None);
    }

    #[test]
    fn spans_run_to_the_next_chapter_or_the_end() {
        let list = [
            ch(0.0, "Prologue"),
            ch(1.5, "Opening"),
            ch(3.0, "Part A"),
            ch(21.0, "Ending"),
            ch(22.5, "Preview"),
        ];
        let s = segments_from_chapters(&list, Some(23 * 60_000 * TICKS_PER_MS));
        assert_eq!(s.len(), 3);
        assert_eq!(
            (s[0].kind, s[0].start_ms, s[0].end_ms),
            (SegmentKind::Intro, 90_000, 180_000)
        );
        assert_eq!(s[1].kind, SegmentKind::Credits);
        assert_eq!(
            (s[2].kind, s[2].end_ms),
            (SegmentKind::Preview, 23 * 60_000)
        );
        // A whole act named like an intro is not one.
        let long = [ch(0.0, "Intro"), ch(25.0, "Chapter 2")];
        assert!(segments_from_chapters(&long, None).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trip() {
        for t in [0, 1_700_000_000, 1_795_000_123, 951_782_400] {
            assert_eq!(iso_to_unix(&unix_to_iso(t)), Some(t), "{t}");
        }
        assert_eq!(unix_to_iso(1_700_000_000), "2023-11-14T22:13:20.0000000Z");
    }

    #[test]
    fn iso_parsing() {
        assert_eq!(iso_to_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            iso_to_unix("2026-11-19T13:09:56.1234567Z"),
            Some(1_795_093_796)
        );
        assert_eq!(iso_to_unix("garbage"), None);
    }

    #[test]
    fn hdr_ranges() {
        assert_eq!(hdr_from_range(Some("DOVIWithHDR10")), HdrKind::DolbyVision);
        assert_eq!(hdr_from_range(Some("HDR10Plus")), HdrKind::Hdr10Plus);
        assert_eq!(hdr_from_range(None), HdrKind::Sdr);
    }

    #[test]
    fn auth_header_encodes_values_and_appends_token() {
        let c = ClientInfo {
            product: "AlwyWatch".into(),
            version: "0.1.0".into(),
            device_name: "Living \"room\" TV".into(),
            device_id: "abc".into(),
            platform: "Android TV".into(),
        };
        let h = auth_header(&c, Some("tok"));
        assert!(h.starts_with("MediaBrowser Client=\"AlwyWatch\""));
        assert!(h.contains("Device=\"Living%20%22room%22%20TV\""));
        assert!(h.ends_with(", Token=\"tok\""));
        assert!(!auth_header(&c, None).contains("Token"));
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn eight_bit_only_codecs_get_a_bit_depth_condition() {
        let p = DeviceProfile {
            max_bitrate: None,
            containers: vec!["mkv".into()],
            video_codecs: vec!["h264".into(), "hevc".into()],
            video_10bit_codecs: vec![],
            audio_codecs: vec!["aac".into()],
            subtitle_codecs: vec![],
            max_width: Some(1920),
            max_height: Some(1080),
            hdr: vec![],
        };
        let j = jellyfin_profile(&p);
        let cps = j["CodecProfiles"].as_array().unwrap();
        assert_eq!(cps.len(), 3, "h264 + hevc bit depth, one size limit");
        assert_eq!(cps[1]["Codec"], "hevc");
        assert_eq!(cps[1]["Conditions"][0]["Property"], "VideoBitDepth");
        assert_eq!(cps[2]["Conditions"][0]["Value"], "1920");
        assert_eq!(j["TranscodingProfiles"][0]["VideoCodec"], "h264,hevc");
    }
}
