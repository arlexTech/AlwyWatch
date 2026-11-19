// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plex client: plex.tv PIN sign-in and resource discovery, then the Plex Media Server API (JSON).
//!
//! The token always travels in the `X-Plex-Token` header (never in URLs).

use std::time::Duration;

use net::reqwest::{Client, Method, RequestBuilder};
use serde::{Deserialize, Serialize};

use crate::model::*;
use crate::{Error, MediaServer, Result, header};

pub const PLEX_TV: &str = "https://plex.tv";
const LIBRARY_IDENTIFIER: &str = "com.plexapp.plugins.library";

fn plex_headers(client: &ClientInfo) -> Vec<Header> {
    vec![
        header("X-Plex-Product", &client.product),
        header("X-Plex-Version", &client.version),
        header("X-Plex-Client-Identifier", &client.device_id),
        header("X-Plex-Device-Name", &client.device_name),
        header("X-Plex-Platform", &client.platform),
        header("X-Plex-Device", &client.platform),
    ]
}

fn apply(mut req: RequestBuilder, headers: &[Header]) -> RequestBuilder {
    for h in headers {
        req = req.header(&h.name, &h.value);
    }
    req.header("Accept", "application/json")
}

// ---------------------------------------------------------------------------------------------
// plex.tv sign-in and discovery
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct PlexPin {
    pub id: u64,
    /// Short code to type at plex.tv/link (TV) or the long code embedded in `auth_url` (phone).
    pub code: String,
    /// URL to open in a browser (phone/desktop). Empty for short TV codes.
    pub auth_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct PlexUser {
    pub id: String,
    pub username: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct PlexConnection {
    pub uri: String,
    pub local: bool,
    pub relay: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct PlexServerResource {
    pub name: String,
    pub machine_id: String,
    pub owned: bool,
    /// Server-specific token (differs from the account token for shared users).
    pub access_token: String,
    pub connections: Vec<PlexConnection>,
}

#[derive(Debug, Clone)]
pub struct PlexAuth {
    http: Client,
    client: ClientInfo,
    plex_tv: String,
}

#[derive(Deserialize)]
struct PinDto {
    id: u64,
    code: String,
    #[serde(rename = "authToken")]
    auth_token: Option<String>,
}

#[derive(Deserialize)]
struct UserDto {
    id: u64,
    #[serde(default)]
    username: String,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceDto {
    name: String,
    client_identifier: String,
    #[serde(default)]
    provides: String,
    #[serde(default)]
    owned: bool,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    connections: Vec<ConnectionDto>,
}

#[derive(Deserialize)]
struct ConnectionDto {
    uri: String,
    #[serde(default)]
    local: bool,
    #[serde(default)]
    relay: bool,
}

impl PlexAuth {
    pub fn new(http: Client, client: ClientInfo) -> Self {
        Self::with_plex_tv(http, client, PLEX_TV)
    }

    /// For tests: point at a mock plex.tv.
    pub fn with_plex_tv(http: Client, client: ClientInfo, plex_tv: &str) -> Self {
        Self {
            http,
            client,
            plex_tv: plex_tv.trim_end_matches('/').to_owned(),
        }
    }

    fn req(&self, method: Method, path: &str, token: Option<&str>) -> RequestBuilder {
        let mut headers = plex_headers(&self.client);
        if let Some(t) = token {
            headers.push(header("X-Plex-Token", t));
        }
        apply(
            self.http.request(method, format!("{}{path}", self.plex_tv)),
            &headers,
        )
    }

    /// `strong = false` gives a 4-character code for plex.tv/link (best on TV);
    /// `strong = true` gives a long code used through `auth_url` (phone, desktop).
    pub async fn create_pin(&self, strong: bool) -> Result<PlexPin> {
        let pin: PinDto = net::send_json(
            self.req(Method::POST, "/api/v2/pins", None)
                .query(&[("strong", if strong { "true" } else { "false" })]),
        )
        .await?;
        let auth_url = if strong {
            format!(
                "https://app.plex.tv/auth#?clientID={}&code={}&context%5Bdevice%5D%5Bproduct%5D={}",
                net::percent_encode(&self.client.device_id),
                net::percent_encode(&pin.code),
                net::percent_encode(&self.client.product)
            )
        } else {
            String::new()
        };
        Ok(PlexPin {
            id: pin.id,
            code: pin.code,
            auth_url,
        })
    }

    /// Returns the account token once the PIN is linked, [`Error::Pending`] before.
    pub async fn check_pin(&self, pin_id: u64) -> Result<String> {
        let pin: PinDto =
            net::send_json(self.req(Method::GET, &format!("/api/v2/pins/{pin_id}"), None)).await?;
        pin.auth_token
            .filter(|t| !t.is_empty())
            .ok_or(Error::Pending)
    }

    pub async fn user(&self, account_token: &str) -> Result<PlexUser> {
        let u: UserDto =
            net::send_json(self.req(Method::GET, "/api/v2/user", Some(account_token))).await?;
        Ok(PlexUser {
            id: u.id.to_string(),
            title: if u.title.is_empty() {
                u.username.clone()
            } else {
                u.title
            },
            username: u.username,
        })
    }

    /// Media servers this account can access.
    pub async fn servers(&self, account_token: &str) -> Result<Vec<PlexServerResource>> {
        let list: Vec<ResourceDto> = net::send_json(
            self.req(Method::GET, "/api/v2/resources", Some(account_token))
                .query(&[("includeHttps", "1"), ("includeRelay", "1")]),
        )
        .await?;
        Ok(list
            .into_iter()
            .filter(|r| r.provides.split(',').any(|p| p == "server"))
            .map(|r| PlexServerResource {
                name: r.name,
                machine_id: r.client_identifier,
                owned: r.owned,
                access_token: r.access_token.unwrap_or_else(|| account_token.to_owned()),
                connections: r
                    .connections
                    .into_iter()
                    .map(|c| PlexConnection {
                        uri: c.uri,
                        local: c.local,
                        relay: c.relay,
                    })
                    .collect(),
            })
            .collect())
    }

    /// Picks the first connection that answers `/identity` with the expected machine id.
    /// Order: remote direct, local, relay (relays are bandwidth-limited by Plex).
    pub async fn connect(
        &self,
        server: &PlexServerResource,
        user: &PlexUser,
        preferred_uri: Option<&str>,
    ) -> Result<Session> {
        let mut candidates: Vec<&str> = preferred_uri.into_iter().collect();
        let mut conns: Vec<&PlexConnection> = server.connections.iter().collect();
        conns.sort_by_key(|c| (c.relay, c.local));
        candidates.extend(conns.iter().map(|c| c.uri.as_str()));
        for uri in candidates {
            let base = uri.trim_end_matches('/');
            if let Ok(id) = self.identity(base, &server.access_token).await
                && id == server.machine_id
            {
                return Ok(Session {
                    kind: ServerKind::Plex,
                    base_url: base.to_owned(),
                    server_id: server.machine_id.clone(),
                    server_name: server.name.clone(),
                    user_id: user.id.clone(),
                    user_name: user.title.clone(),
                    is_admin: server.owned,
                    token: server.access_token.clone(),
                });
            }
        }
        Err(Error::Network {
            message: format!("no reachable connection for Plex server {}", server.name),
        })
    }

    async fn identity(&self, base: &str, token: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Id {
            #[serde(rename = "MediaContainer")]
            mc: IdMc,
        }
        #[derive(Deserialize)]
        struct IdMc {
            #[serde(rename = "machineIdentifier")]
            machine_identifier: String,
        }
        let mut headers = plex_headers(&self.client);
        headers.push(header("X-Plex-Token", token));
        let req = apply(self.http.get(format!("{base}/identity")), &headers)
            .timeout(Duration::from_secs(4));
        let id: Id = net::send_json(req).await?;
        Ok(id.mc.machine_identifier)
    }
}

// ---------------------------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct Envelope {
    #[serde(rename = "MediaContainer", default)]
    mc: MediaContainer,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MediaContainer {
    #[serde(rename = "totalSize")]
    total_size: Option<u32>,
    offset: Option<u32>,
    #[serde(rename = "Metadata")]
    metadata: Vec<MetadataDto>,
    #[serde(rename = "Directory")]
    directory: Vec<DirectoryDto>,
    #[serde(rename = "Hub")]
    hub: Vec<HubDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DirectoryDto {
    key: String,
    title: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct HubDto {
    #[serde(rename = "Metadata")]
    metadata: Vec<MetadataDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Tag {
    tag: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GuidDto {
    id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct MetadataDto {
    rating_key: String,
    #[serde(rename = "type")]
    kind: String,
    title: String,
    title_sort: Option<String>,
    summary: Option<String>,
    year: Option<i32>,
    duration: Option<u64>,
    audience_rating: Option<f32>,
    rating: Option<f32>,
    content_rating: Option<String>,
    #[serde(rename = "Genre")]
    genre: Vec<Tag>,
    #[serde(rename = "Guid")]
    guid: Vec<GuidDto>,
    thumb: Option<String>,
    art: Option<String>,
    parent_thumb: Option<String>,
    grandparent_thumb: Option<String>,
    grandparent_art: Option<String>,
    parent_rating_key: Option<String>,
    grandparent_rating_key: Option<String>,
    grandparent_title: Option<String>,
    parent_title: Option<String>,
    parent_index: Option<u32>,
    index: Option<u32>,
    view_offset: Option<u64>,
    view_count: Option<u32>,
    last_viewed_at: Option<i64>,
    leaf_count: Option<u32>,
    viewed_leaf_count: Option<u32>,
    child_count: Option<u32>,
    originally_available_at: Option<String>,
    #[serde(rename = "Media")]
    media: Vec<MediaDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct MediaDto {
    id: serde_json::Value,
    duration: Option<u64>,
    /// kbit/s
    bitrate: Option<u64>,
    container: Option<String>,
    #[serde(rename = "Part")]
    part: Vec<PartDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PartDto {
    key: String,
    file: Option<String>,
    size: Option<u64>,
    container: Option<String>,
    #[serde(rename = "Stream")]
    stream: Vec<StreamDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct StreamDto {
    id: i64,
    stream_type: u8,
    codec: Option<String>,
    profile: Option<String>,
    language: Option<String>,
    language_code: Option<String>,
    title: Option<String>,
    display_title: Option<String>,
    extended_display_title: Option<String>,
    channels: Option<u32>,
    audio_channel_layout: Option<String>,
    bit_depth: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    color_trc: Option<String>,
    #[serde(rename = "DOVIPresent")]
    dovi_present: Option<bool>,
    #[serde(rename = "DOVIProfile")]
    dovi_profile: Option<u32>,
    default: Option<bool>,
    forced: Option<bool>,
    key: Option<String>,
}

fn id_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PlexClient {
    session: Session,
    http: Client,
    headers: Vec<Header>,
}

impl PlexClient {
    pub fn new(session: Session, client: ClientInfo, http: Client) -> Self {
        let mut headers = plex_headers(&client);
        headers.push(header("X-Plex-Token", &session.token));
        Self {
            session,
            http,
            headers,
        }
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        apply(
            self.http
                .request(method, net::join(&self.session.base_url, path)),
            &self.headers,
        )
    }

    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<MediaContainer> {
        let e: Envelope = net::send_json(self.req(Method::GET, path).query(params)).await?;
        Ok(e.mc)
    }

    fn convert_all(&self, list: Vec<MetadataDto>) -> Vec<Item> {
        list.into_iter().map(convert_item).collect()
    }

    fn convert_source(&self, rating_key: &str, index: usize, m: MediaDto) -> Option<MediaSource> {
        let part = m.part.into_iter().next()?;
        let streams = part
            .stream
            .into_iter()
            .filter_map(|s| {
                let kind = match s.stream_type {
                    1 => StreamKind::Video,
                    2 => StreamKind::Audio,
                    3 => StreamKind::Subtitle,
                    _ => return None,
                };
                let hdr = (kind == StreamKind::Video).then(|| {
                    if s.dovi_present.unwrap_or(false) {
                        HdrKind::DolbyVision
                    } else {
                        match s.color_trc.as_deref() {
                            Some("smpte2084") => HdrKind::Hdr10,
                            Some("arib-std-b67") => HdrKind::Hlg,
                            _ => HdrKind::Sdr,
                        }
                    }
                });
                let external = s
                    .key
                    .as_ref()
                    .filter(|_| kind == StreamKind::Subtitle)
                    .map(|k| HttpRequest {
                        url: net::join(&self.session.base_url, k),
                        headers: self.headers.clone(),
                    });
                Some(MediaStream {
                    index: s.id,
                    kind,
                    codec: s.codec.unwrap_or_default().to_ascii_lowercase(),
                    profile: s.profile,
                    language: s.language_code.or(s.language),
                    title: s.title,
                    display_title: s.extended_display_title.or(s.display_title),
                    channels: s.channels,
                    channel_layout: s.audio_channel_layout,
                    bit_depth: s.bit_depth,
                    width: s.width,
                    height: s.height,
                    hdr,
                    dv_profile: s.dovi_profile,
                    is_default: s.default.unwrap_or(false),
                    is_forced: s.forced.unwrap_or(false),
                    is_external: external.is_some(),
                    external,
                })
            })
            .collect::<Vec<_>>();
        let default_audio = streams
            .iter()
            .find(|s| s.kind == StreamKind::Audio && s.is_default)
            .or_else(|| streams.iter().find(|s| s.kind == StreamKind::Audio))
            .map(|s| s.index);
        let default_sub = streams
            .iter()
            .find(|s| s.kind == StreamKind::Subtitle && (s.is_default || s.is_forced))
            .map(|s| s.index);
        let transcode = format!(
            "{}/video/:/transcode/universal/start.m3u8?path={}&mediaIndex={index}&partIndex=0&protocol=hls&fastSeek=1&directPlay=0&directStream=1&directStreamAudio=1&session={}",
            self.session.base_url,
            net::percent_encode(&format!("/library/metadata/{rating_key}")),
            net::percent_encode(&format!("{rating_key}-{index}"))
        );
        Some(MediaSource {
            id: id_string(&m.id),
            container: part.container.or(m.container),
            size: part.size,
            bitrate: m.bitrate.map(|k| k * 1000),
            duration_ms: m.duration,
            file_name: part
                .file
                .as_deref()
                .and_then(|f| f.rsplit(['/', '\\']).next())
                .map(str::to_owned),
            streams,
            direct: HttpRequest {
                url: net::join(&self.session.base_url, &part.key),
                headers: self.headers.clone(),
            },
            transcode: Some(HttpRequest {
                url: transcode,
                headers: self.headers.clone(),
            }),
            // Plex does not decide for us: the platform checks codecs against its own profile.
            supports_direct_play: true,
            default_audio_index: default_audio,
            default_subtitle_index: default_sub,
        })
    }

    async fn on_deck(&self, limit: u32) -> Result<Vec<MetadataDto>> {
        Ok(self
            .get(
                "/library/onDeck",
                &[
                    ("X-Plex-Container-Size", (limit * 2).to_string()),
                    ("includeGuids", "1".into()),
                ],
            )
            .await?
            .metadata)
    }
}

fn kind_of(t: &str) -> ItemKind {
    match t {
        "movie" => ItemKind::Movie,
        "show" => ItemKind::Series,
        "season" => ItemKind::Season,
        "episode" => ItemKind::Episode,
        "collection" => ItemKind::Collection,
        _ => ItemKind::Other,
    }
}

fn convert_item(m: MetadataDto) -> Item {
    let kind = kind_of(&m.kind);
    let mut images = Vec::new();
    let mut push = |kind, path: Option<&String>| {
        if let Some(p) = path.filter(|p| !p.is_empty()) {
            images.push(ImageRef {
                kind,
                path: p.clone(),
                blurhash: None,
            });
        }
    };
    let poster = match kind {
        ItemKind::Season => m.thumb.as_ref().or(m.parent_thumb.as_ref()),
        _ => m.thumb.as_ref().or(m.grandparent_thumb.as_ref()),
    };
    push(ImageKind::Primary, poster);
    push(
        ImageKind::Backdrop,
        m.art.as_ref().or(m.grandparent_art.as_ref()),
    );
    let mut ids = ProviderIds::default();
    for g in &m.guid {
        if let Some((scheme, v)) = g.id.split_once("://") {
            let v = Some(v.to_owned());
            match scheme {
                "tmdb" => ids.tmdb = v,
                "tvdb" => ids.tvdb = v,
                "imdb" => ids.imdb = v,
                _ => {}
            }
        }
    }
    let (series_id, season_id) = match kind {
        ItemKind::Episode => (
            m.grandparent_rating_key.clone(),
            m.parent_rating_key.clone(),
        ),
        ItemKind::Season => (m.parent_rating_key.clone(), None),
        _ => (None, None),
    };
    let unplayed = m
        .leaf_count
        .map(|l| l.saturating_sub(m.viewed_leaf_count.unwrap_or(0)));
    let played = match kind {
        ItemKind::Series | ItemKind::Season => unplayed == Some(0) && m.leaf_count.unwrap_or(0) > 0,
        _ => m.view_count.unwrap_or(0) > 0,
    };
    Item {
        id: m.rating_key,
        kind,
        title: m.title,
        sort_title: m.title_sort,
        overview: m.summary.filter(|s| !s.is_empty()),
        year: m.year,
        runtime_ms: m.duration,
        community_rating: m.audience_rating.or(m.rating),
        official_rating: m.content_rating,
        genres: m.genre.into_iter().map(|g| g.tag).collect(),
        provider_ids: ids,
        series_id,
        series_title: match kind {
            ItemKind::Episode => m.grandparent_title,
            ItemKind::Season => m.parent_title,
            _ => None,
        },
        season_id,
        season_number: match kind {
            ItemKind::Episode => m.parent_index,
            ItemKind::Season => m.index,
            _ => None,
        },
        episode_number: (kind == ItemKind::Episode).then_some(m.index).flatten(),
        child_count: m.child_count.or(m.leaf_count),
        premiere_date: m.originally_available_at,
        images,
        user: UserData {
            played,
            position_ms: m.view_offset.unwrap_or(0),
            play_count: m.view_count.unwrap_or(0),
            favorite: false,
            last_played_at: m.last_viewed_at,
            unplayed_count: unplayed,
        },
    }
}

fn sort_param(s: SortBy, desc: bool) -> String {
    let field = match s {
        SortBy::Title => "titleSort",
        SortBy::DateAdded => "addedAt",
        SortBy::ReleaseDate => "originallyAvailableAt",
        SortBy::Rating => "audienceRating",
        SortBy::LastPlayed => "lastViewedAt",
        SortBy::Random => return "random".into(),
    };
    format!("{field}:{}", if desc { "desc" } else { "asc" })
}

impl MediaServer for PlexClient {
    fn session(&self) -> &Session {
        &self.session
    }

    async fn libraries(&self) -> Result<Vec<MediaLibrary>> {
        Ok(self
            .get("/library/sections", &[])
            .await?
            .directory
            .into_iter()
            .map(|d| MediaLibrary {
                kind: match d.kind.as_str() {
                    "movie" => LibraryKind::Movies,
                    "show" => LibraryKind::Shows,
                    _ => LibraryKind::Other,
                },
                id: d.key,
                title: d.title,
            })
            .collect())
    }

    async fn items(&self, q: &ItemQuery) -> Result<Page> {
        let Some(lib) = &q.library_id else {
            // Plex has no cross-library listing; search covers the "all" case.
            let items = self
                .search(q.search.as_deref().unwrap_or(""), q.limit)
                .await?;
            return Ok(Page {
                total: items.len() as u32,
                start: 0,
                items,
            });
        };
        let mut p = vec![
            ("X-Plex-Container-Start", q.start.to_string()),
            ("X-Plex-Container-Size", q.limit.to_string()),
            ("sort", sort_param(q.sort_by, q.descending)),
            ("includeGuids", "1".into()),
        ];
        if q.unplayed_only {
            p.push(("unwatched", "1".into()));
        }
        if let Some(s) = q.search.as_ref().filter(|s| !s.is_empty()) {
            p.push(("title", s.clone()));
        }
        let mc = self
            .get(
                &format!("/library/sections/{}/all", net::percent_encode(lib)),
                &p,
            )
            .await?;
        let items = self.convert_all(mc.metadata);
        Ok(Page {
            start: mc.offset.unwrap_or(q.start),
            total: mc.total_size.unwrap_or(items.len() as u32),
            items,
        })
    }

    async fn item(&self, id: &str) -> Result<Item> {
        let mc = self
            .get(
                &format!("/library/metadata/{}", net::percent_encode(id)),
                &[("includeGuids", "1".into())],
            )
            .await?;
        mc.metadata
            .into_iter()
            .next()
            .map(convert_item)
            .ok_or(Error::NotFound)
    }

    async fn seasons(&self, series_id: &str) -> Result<Vec<Item>> {
        let mc = self
            .get(
                &format!(
                    "/library/metadata/{}/children",
                    net::percent_encode(series_id)
                ),
                &[],
            )
            .await?;
        Ok(self.convert_all(mc.metadata))
    }

    async fn episodes(&self, _series_id: &str, season_id: &str) -> Result<Vec<Item>> {
        let mc = self
            .get(
                &format!(
                    "/library/metadata/{}/children",
                    net::percent_encode(season_id)
                ),
                &[],
            )
            .await?;
        Ok(self.convert_all(mc.metadata))
    }

    async fn resume(&self, limit: u32) -> Result<Vec<Item>> {
        let list = self.on_deck(limit).await?;
        Ok(self
            .convert_all(
                list.into_iter()
                    .filter(|m| m.view_offset.unwrap_or(0) > 0)
                    .collect(),
            )
            .into_iter()
            .take(limit as usize)
            .collect())
    }

    async fn next_up(&self, limit: u32) -> Result<Vec<Item>> {
        let list = self.on_deck(limit).await?;
        Ok(self
            .convert_all(
                list.into_iter()
                    .filter(|m| m.kind == "episode" && m.view_offset.unwrap_or(0) == 0)
                    .collect(),
            )
            .into_iter()
            .take(limit as usize)
            .collect())
    }

    async fn latest(&self, library_id: &str, limit: u32) -> Result<Vec<Item>> {
        let mc = self
            .get(
                &format!(
                    "/library/sections/{}/recentlyAdded",
                    net::percent_encode(library_id)
                ),
                &[
                    ("X-Plex-Container-Size", limit.to_string()),
                    ("includeGuids", "1".into()),
                ],
            )
            .await?;
        Ok(self.convert_all(mc.metadata))
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Item>> {
        let mc = self
            .get(
                "/hubs/search",
                &[
                    ("query", query.to_owned()),
                    ("limit", limit.to_string()),
                    ("includeGuids", "1".into()),
                ],
            )
            .await?;
        Ok(self.convert_all(
            mc.hub
                .into_iter()
                .flat_map(|h| h.metadata)
                .filter(|m| matches!(m.kind.as_str(), "movie" | "show" | "episode"))
                .collect(),
        ))
    }

    async fn playback_info(&self, item_id: &str, _profile: &DeviceProfile) -> Result<PlaybackInfo> {
        let mc = self
            .get(
                &format!("/library/metadata/{}", net::percent_encode(item_id)),
                &[("checkFiles", "1".into()), ("includeGuids", "1".into())],
            )
            .await?;
        let meta = mc.metadata.into_iter().next().ok_or(Error::NotFound)?;
        let sources = meta
            .media
            .into_iter()
            .enumerate()
            .filter_map(|(i, m)| self.convert_source(item_id, i, m))
            .collect();
        Ok(PlaybackInfo {
            item_id: item_id.to_owned(),
            play_session_id: None,
            sources,
        })
    }

    async fn report_progress(&self, r: &ProgressReport) -> Result<()> {
        let state = match r.event {
            PlaybackEvent::Start | PlaybackEvent::Progress => "playing",
            PlaybackEvent::Pause => "paused",
            PlaybackEvent::Stop => "stopped",
        };
        let mut p = vec![
            ("ratingKey", r.item_id.clone()),
            ("key", format!("/library/metadata/{}", r.item_id)),
            ("identifier", LIBRARY_IDENTIFIER.into()),
            ("state", state.into()),
            ("time", r.position_ms.to_string()),
        ];
        if let Some(d) = r.duration_ms {
            p.push(("duration", d.to_string()));
        }
        net::send(self.req(Method::GET, "/:/timeline").query(&p)).await?;
        Ok(())
    }

    async fn set_played(&self, item_id: &str, played: bool) -> Result<()> {
        let path = if played {
            "/:/scrobble"
        } else {
            "/:/unscrobble"
        };
        net::send(
            self.req(Method::GET, path)
                .query(&[("key", item_id), ("identifier", LIBRARY_IDENTIFIER)]),
        )
        .await?;
        Ok(())
    }

    async fn download_source(&self, item_id: &str) -> Result<DownloadSource> {
        let info = self.playback_info(item_id, &empty_profile()).await?;
        let src = info.sources.into_iter().next().ok_or(Error::NotFound)?;
        let mut req = src.direct;
        req.url.push_str(if req.url.contains('?') {
            "&download=1"
        } else {
            "?download=1"
        });
        Ok(DownloadSource {
            file_name: src.file_name.unwrap_or_else(|| {
                format!("{item_id}.{}", src.container.as_deref().unwrap_or("mkv"))
            }),
            size: src.size,
            request: req,
        })
    }

    async fn provider_index(&self, kind: ItemKind) -> Result<Vec<Item>> {
        let want = match kind {
            ItemKind::Movie => LibraryKind::Movies,
            ItemKind::Series => LibraryKind::Shows,
            _ => {
                return Err(Error::invalid(
                    "provider index supports movies and series only",
                ));
            }
        };
        let mut out = Vec::new();
        for lib in self
            .libraries()
            .await?
            .into_iter()
            .filter(|l| l.kind == want)
        {
            let mc = self
                .get(
                    &format!("/library/sections/{}/all", net::percent_encode(&lib.id)),
                    &[("includeGuids", "1".into())],
                )
                .await?;
            out.extend(self.convert_all(mc.metadata));
        }
        Ok(out)
    }

    fn image_request(&self, _item_id: &str, image: &ImageRef, max_width: u32) -> HttpRequest {
        let height = match image.kind {
            ImageKind::Primary => max_width * 3 / 2,
            ImageKind::Backdrop | ImageKind::Thumb => max_width * 9 / 16,
            ImageKind::Logo => max_width / 2,
        };
        HttpRequest {
            url: format!(
                "{}/photo/:/transcode?width={max_width}&height={height}&minSize=1&upscale=1&url={}",
                self.session.base_url,
                net::percent_encode(&image.path)
            ),
            headers: self.headers.clone(),
        }
    }
}

fn empty_profile() -> DeviceProfile {
    DeviceProfile {
        max_bitrate: None,
        containers: vec![],
        video_codecs: vec![],
        video_10bit_codecs: vec![],
        audio_codecs: vec![],
        subtitle_codecs: vec![],
        max_width: None,
        max_height: None,
        hdr: vec![],
    }
}
