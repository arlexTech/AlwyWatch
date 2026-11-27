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
/// plex.tv's catalogue service: the account's watchlist.
pub const PLEX_DISCOVER: &str = "https://discover.provider.plex.tv";
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

/// A member of a Plex Home (several people on one Plex account).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct PlexHomeUser {
    pub uuid: String,
    pub title: String,
    /// Avatar URL on plex.tv.
    pub thumb: Option<String>,
    pub admin: bool,
    /// Switching to this user needs its PIN.
    pub protected: bool,
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

    /// The people of the account's Plex Home (empty or one: not a Plex Home).
    pub async fn home_users(&self, account_token: &str) -> Result<Vec<PlexHomeUser>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct HomeDto {
            users: Vec<HomeUserDto>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct HomeUserDto {
            uuid: String,
            title: String,
            username: Option<String>,
            thumb: Option<String>,
            admin: bool,
            protected: bool,
        }
        let home: HomeDto =
            net::send_json(self.req(Method::GET, "/api/v2/home/users", Some(account_token)))
                .await?;
        Ok(home
            .users
            .into_iter()
            .map(|u| PlexHomeUser {
                title: if u.title.is_empty() {
                    u.username.unwrap_or_default()
                } else {
                    u.title
                },
                uuid: u.uuid,
                thumb: u.thumb.filter(|t| !t.is_empty()),
                admin: u.admin,
                protected: u.protected,
            })
            .collect())
    }

    /// Switches to another member of the Plex Home; returns that user's token (it replaces the
    /// account token from then on: their servers, watch state and permissions).
    pub async fn switch_home_user(
        &self,
        account_token: &str,
        uuid: &str,
        pin: Option<&str>,
    ) -> Result<String> {
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct SwitchDto {
            auth_token: String,
        }
        let mut req = self.req(
            Method::POST,
            &format!("/api/v2/home/users/{}/switch", net::percent_encode(uuid)),
            Some(account_token),
        );
        if let Some(pin) = pin.filter(|p| !p.is_empty()) {
            req = req.query(&[("pin", pin)]);
        }
        let r: SwitchDto = net::send_json(req).await?;
        if r.auth_token.is_empty() {
            return Err(Error::invalid(i18n::tr!(
                "Plex did not return a token for this user"
            )));
        }
        Ok(r.auth_token)
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
struct RoleDto {
    tag: String,
    role: Option<String>,
    thumb: Option<String>,
}

fn convert_roles(m: &mut MetadataDto) -> Vec<Person> {
    let mut out = Vec::new();
    for (list, kind) in [
        (std::mem::take(&mut m.roles), PersonKind::Actor),
        (std::mem::take(&mut m.directors), PersonKind::Director),
        (std::mem::take(&mut m.writers), PersonKind::Writer),
    ] {
        for r in list.into_iter().filter(|r| !r.tag.is_empty()) {
            out.push(Person {
                name: r.tag,
                role: r.role.filter(|x| !x.is_empty()),
                kind,
                // Photos from plex.tv's metadata servers are public; server paths need a token.
                photo_url: r.thumb.filter(|t| t.starts_with("http")),
            });
        }
    }
    out
}

/// The plex.tv catalogue id of a title ("plex://movie/5d77…" → "5d77…").
fn discover_key(plex_guid: &str) -> Option<&str> {
    plex_guid
        .strip_prefix("plex://")
        .and_then(|rest| rest.rsplit('/').next())
        .filter(|k| !k.is_empty())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GuidDto {
    id: String,
}

/// Intro and credits markers (`includeMarkers=1`), detected by the server.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct MarkerDto {
    #[serde(rename = "type")]
    kind: String,
    start_time_offset: u64,
    end_time_offset: u64,
}

fn convert_markers(markers: Vec<MarkerDto>) -> Vec<Segment> {
    let mut out: Vec<Segment> = markers
        .into_iter()
        .filter(|m| m.end_time_offset > m.start_time_offset)
        .filter_map(|m| {
            let kind = match m.kind.as_str() {
                "intro" => SegmentKind::Intro,
                "credits" => SegmentKind::Credits,
                "commercial" => SegmentKind::Commercial,
                _ => return None,
            };
            Some(Segment {
                kind,
                start_ms: m.start_time_offset,
                end_ms: m.end_time_offset,
            })
        })
        .collect();
    out.sort_by_key(|s| s.start_ms);
    out
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
    /// Theme song path (shows).
    theme: Option<String>,
    /// Extras: 1 = trailer.
    extra_type: Option<u32>,
    subtype: Option<String>,
    /// "plex://movie/5d7768…": the title in plex.tv's catalogue (watchlist).
    #[serde(rename = "guid")]
    plex_guid: Option<String>,
    #[serde(rename = "Role")]
    roles: Vec<RoleDto>,
    #[serde(rename = "Director")]
    directors: Vec<RoleDto>,
    #[serde(rename = "Writer")]
    writers: Vec<RoleDto>,
    thumb: Option<String>,
    art: Option<String>,
    parent_thumb: Option<String>,
    grandparent_thumb: Option<String>,
    grandparent_art: Option<String>,
    parent_rating_key: Option<String>,
    grandparent_rating_key: Option<String>,
    grandparent_title: Option<String>,
    #[serde(rename = "Marker")]
    marker: Vec<MarkerDto>,
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
    id: serde_json::Value,
    /// Only with `checkFiles=1`: false (or 0) when the file is gone from the disk.
    exists: Option<serde_json::Value>,
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

/// A part's id: its `id`, else the number in its key (`/library/parts/{id}/…`).
fn part_id(p: &PartDto) -> Option<String> {
    match &p.id {
        serde_json::Value::Null => p
            .key
            .strip_prefix("/library/parts/")?
            .split('/')
            .next()
            .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
            .map(str::to_owned),
        v => Some(id_string(v)),
    }
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
    discover: String,
}

impl PlexClient {
    pub fn new(session: Session, client: ClientInfo, http: Client) -> Self {
        let mut headers = plex_headers(&client);
        headers.push(header("X-Plex-Token", &session.token));
        Self {
            session,
            http,
            headers,
            discover: PLEX_DISCOVER.into(),
        }
    }

    /// For tests: point at a mock plex.tv catalogue.
    pub fn with_discover(mut self, url: &str) -> Self {
        self.discover = url.trim_end_matches('/').to_owned();
        self
    }

    /// A plex.tv catalogue request with the account token (not the server's).
    fn discover_req(&self, method: Method, path: &str, token: &str) -> RequestBuilder {
        let headers: Vec<Header> = self
            .headers
            .iter()
            .filter(|h| h.name != "X-Plex-Token")
            .cloned()
            .chain(std::iter::once(header("X-Plex-Token", token)))
            .collect();
        apply(
            self.http
                .request(method, format!("{}{path}", self.discover)),
            &headers,
        )
    }

    async fn metadata(&self, id: &str) -> Result<MetadataDto> {
        let mc = self
            .get(
                &format!("/library/metadata/{}", net::percent_encode(id)),
                &[("includeGuids", "1".into())],
            )
            .await?;
        mc.metadata.into_iter().next().ok_or(Error::NotFound)
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        apply(
            self.http
                .request(method, net::join(&self.session.base_url, path)),
            &self.headers,
        )
    }

    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<MediaContainer> {
        let e: Envelope = net::send_json_strict(self.req(Method::GET, path).query(params)).await?;
        Ok(e.mc)
    }

    /// Every item of a large listing, a page at a time (one response for a whole library of
    /// episodes can take longer than the read timeout).
    async fn get_all(&self, path: &str, params: &[(&str, String)]) -> Result<Vec<MetadataDto>> {
        const PAGE: u32 = 400;
        let mut out = Vec::new();
        let mut start = 0;
        loop {
            let mut p = params.to_vec();
            p.push(("X-Plex-Container-Start", start.to_string()));
            p.push(("X-Plex-Container-Size", PAGE.to_string()));
            let mc = self.get(path, &p).await?;
            let n = mc.metadata.len() as u32;
            out.extend(mc.metadata);
            start += n;
            if n < PAGE || mc.total_size.is_some_and(|t| start >= t) {
                return Ok(out);
            }
        }
    }

    fn convert_all(&self, list: Vec<MetadataDto>) -> Vec<Item> {
        list.into_iter().map(convert_item).collect()
    }

    fn convert_source(
        &self,
        rating_key: &str,
        index: usize,
        m: MediaDto,
        profile: &DeviceProfile,
    ) -> Option<MediaSource> {
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
        let session = play_session(rating_key, &id_string(&m.id));
        let transcode = transcode_url(&self.session.base_url, rating_key, index, &session, profile);
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

    /// Makes `selection` the account's audio and subtitles for this version, where it has
    /// those streams: Plex's converter takes the streams selected on the part (the same
    /// persistent, per-user choice Plex's own apps make). Returns the conversion's
    /// `subtitles=` value: "burn" for a chosen subtitle (once selected), "none" for off.
    async fn select_streams(
        &self,
        m: &MediaDto,
        selection: &TrackSelection,
    ) -> Option<&'static str> {
        if selection
            .source_id
            .as_ref()
            .is_some_and(|id| *id != id_string(&m.id))
        {
            return None;
        }
        let part = m.part.first()?;
        let has = |id: i64, kind: u8| {
            part.stream
                .iter()
                .any(|s| s.id == id && s.stream_type == kind)
        };
        let mut query = vec![];
        if let Some(a) = selection.audio_index.filter(|a| has(*a, 2)) {
            query.push(("audioStreamID", a.to_string()));
        }
        let subs = match selection.subtitle {
            SubtitleSelection::Off => {
                query.push(("subtitleStreamID", "0".into()));
                Some("none")
            }
            SubtitleSelection::Stream { index } if has(index, 3) => {
                query.push(("subtitleStreamID", index.to_string()));
                Some("burn")
            }
            _ => None,
        };
        if query.is_empty() {
            return None;
        }
        query.push(("allParts", "1".into()));
        let saved = match part_id(part) {
            Some(id) => net::send_strict(
                self.req(Method::PUT, &format!("/library/parts/{id}"))
                    .query(&query),
            )
            .await
            .is_ok(),
            None => false,
        };
        // Not saved: Plex would burn its own pick, so burn nothing rather than the wrong one.
        match subs {
            Some("burn") if !saved => None,
            other => other,
        }
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
    let file_name = m
        .media
        .iter()
        .flat_map(|x| &x.part)
        .find_map(|p| p.file.as_deref())
        .and_then(crate::file_name);
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
    if kind == ItemKind::Episode {
        push(ImageKind::SeriesPrimary, m.grandparent_thumb.as_ref());
    }
    push(
        ImageKind::Backdrop,
        m.art.as_ref().or(m.grandparent_art.as_ref()),
    );
    let mut ids = ProviderIds::default();
    for g in &m.guid {
        if let Some((scheme, v)) = g.id.split_once("://") {
            // Plex can list several ids of one kind (a show merged from older entries): the
            // first is the title's own, the later ones would match other titles.
            let slot = match scheme {
                "tmdb" => &mut ids.tmdb,
                "tvdb" => &mut ids.tvdb,
                "imdb" => &mut ids.imdb,
                _ => continue,
            };
            slot.get_or_insert_with(|| v.to_owned());
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
        file_name,
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
        if let Some(c) = &q.collection_id {
            let p = [
                ("X-Plex-Container-Start", q.start.to_string()),
                ("X-Plex-Container-Size", q.limit.to_string()),
                ("includeGuids", "1".into()),
            ];
            let mc = self
                .get(
                    &format!("/library/collections/{}/children", net::percent_encode(c)),
                    &p,
                )
                .await?;
            let items = self.convert_all(mc.metadata);
            return Ok(Page {
                start: mc.offset.unwrap_or(q.start),
                total: mc.total_size.unwrap_or(items.len() as u32),
                items,
            });
        }
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
        let mut sort = sort_param(q.sort_by, q.descending);
        // Recently added shows: by their newest episode, not by when the show first arrived
        // (the field only exists for show libraries, hence the look-up).
        if q.sort_by == SortBy::DateAdded
            && self.libraries().await.is_ok_and(|l| {
                l.iter()
                    .any(|x| x.id == *lib && x.kind == LibraryKind::Shows)
            })
        {
            sort = sort.replacen("addedAt", "episode.addedAt", 1);
        }
        let mut p = vec![
            ("X-Plex-Container-Start", q.start.to_string()),
            ("X-Plex-Container-Size", q.limit.to_string()),
            ("sort", sort),
            ("includeGuids", "1".into()),
        ];
        if q.unplayed_only {
            p.push(("unwatched", "1".into()));
        }
        if let Some(g) = &q.genre_id {
            p.push(("genre", g.clone()));
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

    async fn shows_last_watched(
        &self,
        series_ids: &[String],
    ) -> Result<std::collections::HashMap<String, i64>> {
        // A show's lastViewedAt: when one of its episodes was last watched to the end. Plex
        // takes several ids in one request.
        if series_ids.is_empty() {
            return Ok(Default::default());
        }
        let ids: Vec<String> = series_ids.iter().map(|i| net::percent_encode(i)).collect();
        let mc = self
            .get(&format!("/library/metadata/{}", ids.join(",")), &[])
            .await?;
        Ok(mc
            .metadata
            .into_iter()
            .filter_map(|m| Some((m.rating_key, m.last_viewed_at?)))
            .collect())
    }

    async fn remove_from_continue_watching(&self, item_id: &str) -> Result<bool> {
        // Plex's own "Remove from Continue Watching" (in progress and on deck alike).
        net::send_strict(
            self.req(Method::PUT, "/actions/removeFromContinueWatching")
                .query(&[("ratingKey", item_id)]),
        )
        .await?;
        Ok(true)
    }

    async fn genres(&self, library_id: &str) -> Result<Vec<Genre>> {
        let mc = self
            .get(
                &format!(
                    "/library/sections/{}/genre",
                    net::percent_encode(library_id)
                ),
                &[],
            )
            .await?;
        Ok(mc
            .directory
            .into_iter()
            .map(|d| Genre {
                id: d.key,
                name: d.title,
            })
            .collect())
    }

    async fn collections(&self, library_id: &str) -> Result<Vec<Item>> {
        let mc = self
            .get(
                &format!(
                    "/library/sections/{}/collections",
                    net::percent_encode(library_id)
                ),
                &[("includeGuids", "1".into())],
            )
            .await?;
        Ok(self.convert_all(mc.metadata))
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

    async fn playback_info_with(
        &self,
        item_id: &str,
        profile: &DeviceProfile,
        selection: &TrackSelection,
    ) -> Result<PlaybackInfo> {
        let mc = self
            .get(
                &format!("/library/metadata/{}", net::percent_encode(item_id)),
                &[("checkFiles", "1".into()), ("includeGuids", "1".into())],
            )
            .await?;
        let meta = mc.metadata.into_iter().next().ok_or(Error::NotFound)?;
        // The `subtitles=` of each version's conversion; nothing (Plex decides) by default.
        let mut subtitles = vec![None; meta.media.len()];
        // A chosen quality: converted to it (bitrate and height), never the original.
        let profile = &selection.profile(profile);
        if selection.has_tracks() {
            for (m, subs) in meta.media.iter().zip(subtitles.iter_mut()) {
                *subs = self.select_streams(m, selection).await;
            }
        }
        let sources = meta
            .media
            .into_iter()
            .zip(subtitles)
            .enumerate()
            .filter_map(|(i, (m, subs))| {
                let mut src = self.convert_source(item_id, i, m, profile)?;
                if selection.quality.is_some() {
                    src.supports_direct_play = false;
                }
                if let (Some(subs), Some(t)) = (subs, src.transcode.as_mut()) {
                    t.url.push_str("&subtitles=");
                    t.url.push_str(subs);
                }
                Some(src)
            })
            .collect();
        Ok(PlaybackInfo {
            item_id: item_id.to_owned(),
            play_session_id: None,
            sources,
        })
    }

    async fn segments(&self, item_id: &str) -> Result<Vec<Segment>> {
        let mc = self
            .get(
                &format!("/library/metadata/{}", net::percent_encode(item_id)),
                &[("includeMarkers", "1".into())],
            )
            .await?;
        Ok(mc
            .metadata
            .into_iter()
            .next()
            .map(|m| convert_markers(m.marker))
            .unwrap_or_default())
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
        if let Some(media) = &r.media_source_id {
            p.push(("X-Plex-Session-Identifier", play_session(&r.item_id, media)));
        }
        net::send_strict(self.req(Method::GET, "/:/timeline").query(&p)).await?;
        Ok(())
    }

    async fn people(&self, item_id: &str) -> Result<Vec<Person>> {
        let mut m = self.metadata(item_id).await?;
        Ok(convert_roles(&mut m))
    }

    async fn trailers(&self, item_id: &str) -> Result<Vec<Trailer>> {
        let mc = self
            .get(
                &format!("/library/metadata/{}/extras", net::percent_encode(item_id)),
                &[],
            )
            .await?;
        Ok(mc
            .metadata
            .into_iter()
            .filter(|m| m.extra_type == Some(1) || m.subtype.as_deref() == Some("trailer"))
            .map(|m| Trailer {
                title: m.title,
                item_id: Some(m.rating_key),
                url: None,
            })
            .collect())
    }

    async fn theme_song(&self, item_id: &str) -> Result<Option<HttpRequest>> {
        let m = self.metadata(item_id).await?;
        Ok(m.theme.filter(|t| t.starts_with('/')).map(|t| HttpRequest {
            url: net::join(&self.session.base_url, &t),
            headers: self.headers.clone(),
        }))
    }

    async fn watchlist(&self, plex_tv_token: Option<&str>) -> Result<Vec<Item>> {
        let Some(token) = plex_tv_token else {
            return Ok(vec![]);
        };
        let e: Envelope = net::send_json_strict(
            self.discover_req(Method::GET, "/library/sections/watchlist/all", token)
                .query(&[("includeCollections", "1"), ("includeExternalMedia", "1")]),
        )
        .await?;
        let mut out = Vec::new();
        // Only what this server has, in watchlist order.
        for w in e.mc.metadata {
            let Some(guid) = w.plex_guid.filter(|g| g.starts_with("plex://")) else {
                continue;
            };
            let found = self
                .get(
                    "/library/all",
                    &[("guid", guid.clone()), ("includeGuids", "1".into())],
                )
                .await;
            if let Ok(mc) = found
                && let Some(m) = mc.metadata.into_iter().next()
            {
                out.push(convert_item(m));
            }
        }
        Ok(out)
    }

    async fn set_watchlisted(
        &self,
        item_id: &str,
        on: bool,
        plex_tv_token: Option<&str>,
    ) -> Result<()> {
        let token = plex_tv_token.ok_or(Error::Unauthorized)?;
        let m = self.metadata(item_id).await?;
        let key = m
            .plex_guid
            .as_deref()
            .and_then(discover_key)
            .ok_or(Error::NotFound)?
            .to_owned();
        let action = if on {
            "/actions/addToWatchlist"
        } else {
            "/actions/removeFromWatchlist"
        };
        net::send(
            self.discover_req(Method::PUT, action, token)
                .query(&[("ratingKey", key)]),
        )
        .await?;
        Ok(())
    }

    async fn set_played(&self, item_id: &str, played: bool) -> Result<()> {
        let path = if played {
            "/:/scrobble"
        } else {
            "/:/unscrobble"
        };
        net::send_strict(
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

    async fn watch_history(&self) -> Result<Vec<Item>> {
        let mut out = Vec::new();
        for lib in self.libraries().await? {
            // Movies, or every episode of a show library; then only what was watched or started.
            let kind = match lib.kind {
                LibraryKind::Movies => "1",
                LibraryKind::Shows => "4",
                _ => continue,
            };
            // Episodes are matched by show and number: their own guids aren't needed.
            let mut p = vec![("type", kind.to_owned())];
            if kind == "1" {
                p.push(("includeGuids", "1".into()));
            }
            let all = self
                .get_all(
                    &format!("/library/sections/{}/all", net::percent_encode(&lib.id)),
                    &p,
                )
                .await?;
            out.extend(
                self.convert_all(all)
                    .into_iter()
                    .filter(|i| i.user.played || i.user.position_ms > 0),
            );
        }
        Ok(out)
    }

    async fn files_gone(&self, item_ids: &[String]) -> Result<Vec<String>> {
        let mut gone = Vec::new();
        for id in item_ids {
            // checkFiles: each file part says whether it still exists on the disk.
            let mc = self
                .get(
                    &format!("/library/metadata/{}", net::percent_encode(id)),
                    &[("checkFiles", "1".into())],
                )
                .await;
            let Ok(mc) = mc else { continue };
            if mc.metadata.first().is_some_and(all_files_missing) {
                gone.push(id.clone());
            }
        }
        Ok(gone)
    }

    async fn set_watch_state(&self, item_id: &str, state: &WatchState) -> Result<()> {
        // Watched or not first (a sync may unmark it; marking clears the resume point), then
        // Plex's own resume point (no session needed): on a watched title, a rewatch in
        // progress. Its last-viewed date can't be set.
        self.set_played(item_id, state.played).await?;
        if state.position_ms == 0 {
            return Ok(());
        }
        net::send_strict(self.req(Method::GET, "/:/progress").query(&[
            ("key", item_id.to_owned()),
            ("identifier", LIBRARY_IDENTIFIER.to_owned()),
            ("time", state.position_ms.to_string()),
            ("state", "stopped".to_owned()),
        ]))
        .await?;
        Ok(())
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
            ImageKind::Primary | ImageKind::SeriesPrimary => max_width * 3 / 2,
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

/// Names one playback of one version of a title. The conversion and the progress reports
/// carry the same name, so Plex ties them together: it shows the session as converted, and
/// stops converting when playback is reported stopped.
fn play_session(rating_key: &str, media_id: &str) -> String {
    format!("{rating_key}-{media_id}")
}

/// The HLS conversion of a file. Plex only converts for clients it has a profile for; for
/// any other it answers 400 unless the request describes the client: the "Generic" profile
/// plus what this device plays (`X-Plex-Client-Profile-Extra`). Streams the device plays
/// are copied (`directStream`), the others converted to H.264 / AAC.
fn transcode_url(
    base_url: &str,
    rating_key: &str,
    index: usize,
    session: &str,
    profile: &DeviceProfile,
) -> String {
    // HEVC is only kept as is where MPEG-TS plays natively (the apps' players, not browsers).
    let ts = profile
        .containers
        .iter()
        .any(|c| c == "ts" || c == "mpegts");
    let mut video = vec!["h264"];
    if ts && profile.video_codecs.iter().any(|c| c == "hevc") {
        video.push("hevc");
    }
    let mut audio = vec!["aac"];
    for c in ["ac3", "eac3", "mp3"] {
        if profile.audio_codecs.iter().any(|a| a == c) {
            audio.push(c);
        }
    }
    let mut extra = vec![format!(
        "add-transcode-target(type=videoProfile&context=streaming&protocol=hls&container=mpegts&videoCodec={}&audioCodec={})",
        video.join(","),
        audio.join(",")
    )];
    if let Some(w) = profile.max_width {
        extra.push(format!(
            "add-limitation(scope=videoCodec&scopeName=*&type=upperBound&name=video.width&value={w})"
        ));
    }
    if let Some(h) = profile.max_height {
        extra.push(format!(
            "add-limitation(scope=videoCodec&scopeName=*&type=upperBound&name=video.height&value={h})"
        ));
    }
    // 10-bit only where the device says so: otherwise the copy would not play.
    for codec in &video {
        if !profile.video_10bit_codecs.iter().any(|c| c == codec) {
            extra.push(format!(
                "add-limitation(scope=videoCodec&scopeName={codec}&type=upperBound&name=video.bitDepth&value=8)"
            ));
        }
    }
    // The bitrate cap, in kbit/s (a lower one than the file's makes Plex convert the video).
    let max = profile
        .max_bitrate
        .map(|b| format!("&maxVideoBitrate={}", b / 1000))
        .unwrap_or_default();
    format!(
        "{base_url}/video/:/transcode/universal/start.m3u8?hasMDE=1&path={}&mediaIndex={index}&partIndex=0&protocol=hls&fastSeek=1&copyts=1&directPlay=0&directStream=1&directStreamAudio=1&videoQuality=100&mediaBufferSize=102400&session={}&X-Plex-Session-Identifier={}&X-Plex-Client-Profile-Name=Generic&X-Plex-Client-Profile-Extra={}{max}",
        net::percent_encode(&format!("/library/metadata/{rating_key}")),
        net::percent_encode(session),
        net::percent_encode(session),
        net::percent_encode(&extra.join("+")),
    )
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

/// Every file of the title is gone from the disk (as `checkFiles=1` reports it); a title
/// without that information is not counted as gone.
fn all_files_missing(m: &MetadataDto) -> bool {
    let missing = |v: &Option<serde_json::Value>| match v {
        Some(serde_json::Value::Bool(b)) => !b,
        Some(serde_json::Value::Number(n)) => n.as_u64() == Some(0),
        Some(serde_json::Value::String(s)) => s == "0" || s == "false",
        _ => false,
    };
    let mut parts = m.media.iter().flat_map(|x| &x.part).peekable();
    parts.peek().is_some() && parts.all(|p| missing(&p.exists))
}

#[cfg(test)]
mod transcode_tests {
    use super::*;

    #[test]
    fn part_id_from_its_id_else_its_key() {
        let part = |id: serde_json::Value, key: &str| PartDto {
            id,
            key: key.into(),
            ..Default::default()
        };
        let key = "/library/parts/555/1690000/file.mkv";
        assert_eq!(part_id(&part(serde_json::json!(7), key)), Some("7".into()));
        assert_eq!(
            part_id(&part(serde_json::Value::Null, key)),
            Some("555".into())
        );
        assert_eq!(part_id(&part(serde_json::Value::Null, "/other/1")), None);
    }

    #[test]
    fn transcode_request_describes_the_device_so_plex_accepts_unknown_clients() {
        let mut p = empty_profile();
        p.containers = vec!["mkv".into(), "ts".into()];
        p.video_codecs = vec!["h264".into(), "hevc".into()];
        p.video_10bit_codecs = vec!["hevc".into()];
        p.audio_codecs = vec!["aac".into(), "eac3".into()];
        p.max_width = Some(3840);
        let url = transcode_url("https://plex.example", "42", 0, "42-7", &p);
        assert!(url.starts_with("https://plex.example/video/:/transcode/universal/start.m3u8?"));
        assert!(url.contains("X-Plex-Client-Profile-Name=Generic"));
        assert!(url.contains("&session=42-7&X-Plex-Session-Identifier=42-7&"));
        let extra = url.split("X-Plex-Client-Profile-Extra=").nth(1).unwrap();
        assert!(
            !extra.contains('(') && !extra.contains('&'),
            "encoded as one value"
        );
        let extra = extra
            .replace("%28", "(")
            .replace("%29", ")")
            .replace("%3D", "=")
            .replace("%26", "&")
            .replace("%2C", ",")
            .replace("%2B", "+");
        assert!(
            extra.contains("videoCodec=h264,hevc&audioCodec=aac,eac3)"),
            "{extra}"
        );
        assert!(extra.contains("name=video.width&value=3840"));
        // H.264 10-bit is not something this device plays: copies are limited to 8 bits.
        assert!(extra.contains("scopeName=h264&type=upperBound&name=video.bitDepth&value=8"));
        assert!(!extra.contains("scopeName=hevc&type=upperBound&name=video.bitDepth"));
        // A browser-like profile (no MPEG-TS): everything goes to H.264 / AAC.
        let web = transcode_url("https://plex.example", "42", 0, "42-7", &empty_profile());
        assert!(
            web.contains("videoCodec%3Dh264%26audioCodec%3Daac%29"),
            "{web}"
        );
    }
}
