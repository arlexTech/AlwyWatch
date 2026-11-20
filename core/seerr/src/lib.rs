// SPDX-License-Identifier: AGPL-3.0-or-later
//! Seerr client (the Overseerr/Jellyseerr API, verified against Seerr 3.5.2's OpenAPI and code).
//!
//! A Seerr instance serves one media server type at a time, so sign-in follows what
//! `/settings/public` reports: Plex (plex.tv account token), Jellyfin/Emby (password or
//! QuickConnect) or a local account. The session is the `connect.sid` cookie, handled by hand
//! (no cookie-jar dependency) and kept by the platform in secure storage.

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

use net::reqwest::{Client, Method, RequestBuilder, Response, header};
use net::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

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
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
        pub enum $name { $($body)* }
    };
}

simple_enum! {
    pub enum SeerrLogin { Plex, Jellyfin, Emby, Local, NotConfigured }
}

record! {
    pub struct SeerrInfo {
        pub base_url: String,
        pub title: String,
        /// Sign-in methods this instance accepts, best first.
        pub logins: Vec<SeerrLogin>,
        /// Seerr caches TMDB images and serves them from `/imageproxy`.
        pub cache_images: bool,
    }
}

record! {
    pub struct SeerrUser {
        pub id: i64,
        pub display_name: String,
        pub permissions: i64,
    }
}

record! {
    pub struct SeerrSession {
        pub base_url: String,
        /// `connect.sid=...`, sent back as the Cookie header.
        pub cookie: String,
        pub user: SeerrUser,
    }
}

simple_enum! {
    pub enum SeerrMediaType { Movie, Tv }
}

simple_enum! {
    /// Seerr `MediaStatus` (code values: 6 is BLOCKLISTED, 7 DELETED).
    pub enum Availability { Unknown, Pending, Processing, PartiallyAvailable, Available, Blocklisted, Deleted }
}

simple_enum! {
    pub enum RequestStatus { Pending, Approved, Declined, Failed, Completed }
}

record! {
    pub struct SeerrResult {
        pub tmdb_id: i64,
        pub media_type: SeerrMediaType,
        pub title: String,
        pub year: Option<String>,
        pub overview: Option<String>,
        /// Full poster URL (through Seerr's image cache when enabled).
        pub poster_url: Option<String>,
        pub availability: Availability,
        /// 0..1 while Radarr/Sonarr download it (None: not downloading).
        pub download_progress: Option<f32>,
    }
}

record! {
    pub struct SeerrSeason {
        pub number: u32,
        pub name: String,
        pub availability: Availability,
    }
}

record! {
    pub struct SeerrRequest {
        pub id: i64,
        pub status: RequestStatus,
        pub media_type: SeerrMediaType,
        pub tmdb_id: i64,
        pub availability: Availability,
        pub requested_by: String,
        /// ISO timestamp.
        pub created_at: String,
        /// Filled from the movie/TV details when available.
        pub title: Option<String>,
        pub poster_url: Option<String>,
    }
}

record! {
    pub struct QuickConnect {
        pub code: String,
        pub secret: String,
    }
}

fn availability(code: Option<i64>) -> Availability {
    match code {
        Some(2) => Availability::Pending,
        Some(3) => Availability::Processing,
        Some(4) => Availability::PartiallyAvailable,
        Some(5) => Availability::Available,
        Some(6) => Availability::Blocklisted,
        Some(7) => Availability::Deleted,
        _ => Availability::Unknown,
    }
}

fn request_status(code: i64) -> RequestStatus {
    match code {
        2 => RequestStatus::Approved,
        3 => RequestStatus::Declined,
        4 => RequestStatus::Failed,
        5 => RequestStatus::Completed,
        _ => RequestStatus::Pending,
    }
}

fn session_cookie(res: &Response) -> Option<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            c.split(';')
                .next()
                .filter(|kv| kv.trim_start().starts_with("connect.sid="))
        })
        .map(|kv| kv.trim().to_owned())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserDto {
    id: i64,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    plex_username: Option<String>,
    #[serde(default)]
    jellyfin_username: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    permissions: i64,
}

impl From<UserDto> for SeerrUser {
    fn from(u: UserDto) -> Self {
        let name = [
            u.display_name,
            u.username,
            u.plex_username,
            u.jellyfin_username,
            u.email,
        ]
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| format!("user {}", u.id));
        SeerrUser {
            id: u.id,
            display_name: name,
            permissions: u.permissions,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SeerrAuth {
    http: Client,
}

impl SeerrAuth {
    pub fn new(http: Client) -> Self {
        Self { http }
    }

    fn req(&self, method: Method, base: &str, path: &str) -> RequestBuilder {
        self.http
            .request(
                method,
                format!("{}/api/v1{path}", base.trim_end_matches('/')),
            )
            .header(header::ACCEPT, "application/json")
    }

    pub async fn info(&self, base_url: &str) -> Result<SeerrInfo> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Public {
            #[serde(default)]
            initialized: bool,
            #[serde(default)]
            application_title: Option<String>,
            #[serde(default)]
            local_login: bool,
            #[serde(default)]
            media_server_login: Option<bool>,
            #[serde(default)]
            media_server_type: Option<i64>,
            #[serde(default)]
            cache_images: bool,
        }
        let p: Public = net::send_json(self.req(Method::GET, base_url, "/settings/public")).await?;
        if !p.initialized {
            return Err(Error::invalid("this Seerr instance is not set up yet"));
        }
        let mut logins = Vec::new();
        if p.media_server_login.unwrap_or(true) {
            match p.media_server_type {
                Some(1) => logins.push(SeerrLogin::Plex),
                Some(2) => logins.push(SeerrLogin::Jellyfin),
                Some(3) => logins.push(SeerrLogin::Emby),
                _ => {}
            }
        }
        if p.local_login {
            logins.push(SeerrLogin::Local);
        }
        if logins.is_empty() {
            logins.push(SeerrLogin::NotConfigured);
        }
        Ok(SeerrInfo {
            base_url: base_url.trim_end_matches('/').to_owned(),
            title: p
                .application_title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Seerr".into()),
            logins,
            cache_images: p.cache_images,
        })
    }

    async fn finish(&self, base_url: &str, res: Response) -> Result<SeerrSession> {
        if !res.status().is_success() {
            return Err(net::status_error(res.status()));
        }
        let cookie = session_cookie(&res)
            .ok_or_else(|| Error::decode("Seerr did not return a session cookie"))?;
        let user: UserDto = res.json().await.map_err(Error::from)?;
        Ok(SeerrSession {
            base_url: base_url.trim_end_matches('/').to_owned(),
            cookie,
            user: user.into(),
        })
    }

    /// `plex_account_token` is the plex.tv account token (not a server access token).
    pub async fn sign_in_plex(
        &self,
        base_url: &str,
        plex_account_token: &str,
    ) -> Result<SeerrSession> {
        let res = self
            .req(Method::POST, base_url, "/auth/plex")
            .json(&json!({ "authToken": plex_account_token }))
            .send()
            .await?;
        self.finish(base_url, res).await
    }

    pub async fn sign_in_jellyfin(
        &self,
        base_url: &str,
        username: &str,
        password: &str,
    ) -> Result<SeerrSession> {
        let res = self
            .req(Method::POST, base_url, "/auth/jellyfin")
            .json(&json!({ "username": username, "password": password }))
            .send()
            .await?;
        self.finish(base_url, res).await
    }

    pub async fn sign_in_local(
        &self,
        base_url: &str,
        email: &str,
        password: &str,
    ) -> Result<SeerrSession> {
        let res = self
            .req(Method::POST, base_url, "/auth/local")
            .json(&json!({ "email": email, "password": password }))
            .send()
            .await?;
        self.finish(base_url, res).await
    }

    pub async fn quick_connect_start(&self, base_url: &str) -> Result<QuickConnect> {
        net::send_json(self.req(
            Method::POST,
            base_url,
            "/auth/jellyfin/quickconnect/initiate",
        ))
        .await
    }

    /// [`Error::Pending`] until approved in a Jellyfin app.
    pub async fn quick_connect_poll(&self, base_url: &str, secret: &str) -> Result<SeerrSession> {
        #[derive(Deserialize)]
        struct Check {
            #[serde(default)]
            authenticated: bool,
        }
        let c: Check = net::send_json(
            self.req(Method::GET, base_url, "/auth/jellyfin/quickconnect/check")
                .query(&[("secret", secret)]),
        )
        .await?;
        if !c.authenticated {
            return Err(Error::Pending);
        }
        let res = self
            .req(
                Method::POST,
                base_url,
                "/auth/jellyfin/quickconnect/authenticate",
            )
            .json(&json!({ "secret": secret }))
            .send()
            .await?;
        self.finish(base_url, res).await
    }
}

// ---------------------------------------------------------------------------------------------
// Signed-in client
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SeerrClient {
    http: Client,
    base: String,
    /// Session cookie, or the AlwyWatch server's auth headers when bridged.
    headers: Vec<(String, String)>,
    cache_images: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaInfoDto {
    #[serde(default)]
    status: Option<i64>,
    #[serde(default)]
    seasons: Vec<SeasonStatusDto>,
    /// Radarr/Sonarr queue entries (only while actually downloading).
    #[serde(default)]
    download_status: Vec<DownloadStatusDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadStatusDto {
    #[serde(default)]
    size: f64,
    #[serde(default)]
    size_left: f64,
}

impl MediaInfoDto {
    /// 0..1 when Radarr/Sonarr are downloading it, else None.
    fn download_progress(&self) -> Option<f32> {
        let size: f64 = self.download_status.iter().map(|d| d.size).sum();
        let left: f64 = self.download_status.iter().map(|d| d.size_left).sum();
        (size > 0.0).then(|| ((size - left) / size).clamp(0.0, 1.0) as f32)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeasonStatusDto {
    season_number: u32,
    #[serde(default)]
    status: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResultDto {
    id: i64,
    media_type: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    media_info: Option<MediaInfoDto>,
}

impl SeerrClient {
    pub fn new(http: Client, session_base_url: &str, cookie: &str, cache_images: bool) -> Self {
        Self {
            http,
            base: session_base_url.trim_end_matches('/').to_owned(),
            headers: vec![(header::COOKIE.as_str().to_owned(), cookie.to_owned())],
            cache_images,
        }
    }

    /// Seerr through the AlwyWatch server's bridge (`{keep}/v1/seerr`), for users whose media
    /// server is not the type Seerr runs on. Same API; posters come straight from TMDB.
    pub fn bridged(http: Client, bridge_base_url: &str, headers: Vec<(String, String)>) -> Self {
        Self {
            http,
            base: bridge_base_url.trim_end_matches('/').to_owned(),
            headers,
            cache_images: false,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn req(&self, method: Method, path_and_query: &str) -> RequestBuilder {
        let mut r = self
            .http
            .request(method, format!("{}/api/v1{path_and_query}", self.base))
            .header(header::ACCEPT, "application/json");
        for (k, v) in &self.headers {
            r = r.header(k.as_str(), v.as_str());
        }
        r
    }

    /// TMDB poster through Seerr's image cache when it has one (public, no auth needed).
    pub fn poster_url(&self, path: &str) -> String {
        if self.cache_images {
            format!("{}/imageproxy/tmdb/t/p/w300_and_h450_face{path}", self.base)
        } else {
            format!("https://image.tmdb.org/t/p/w300_and_h450_face{path}")
        }
    }

    pub async fn me(&self) -> Result<SeerrUser> {
        let u: UserDto = net::send_json(self.req(Method::GET, "/auth/me")).await?;
        Ok(u.into())
    }

    pub async fn search(&self, query: &str, page: u32) -> Result<Vec<SeerrResult>> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            results: Vec<serde_json::Value>,
        }
        // Seerr rejects '+' for spaces: encode the query strictly (RFC 3986).
        let path = format!(
            "/search?query={}&page={}",
            net::percent_encode(query),
            page.max(1)
        );
        let p: Page = net::send_json(self.req(Method::GET, &path)).await?;
        Ok(p.results
            .into_iter()
            // People results have a different shape; skip anything that is not a movie/show.
            .filter_map(|v| serde_json::from_value::<ResultDto>(v).ok())
            .filter_map(|r| self.convert(r))
            .collect())
    }

    fn convert(&self, r: ResultDto) -> Option<SeerrResult> {
        let media_type = match r.media_type.as_str() {
            "movie" => SeerrMediaType::Movie,
            "tv" => SeerrMediaType::Tv,
            _ => return None,
        };
        let date = r.release_date.or(r.first_air_date);
        Some(SeerrResult {
            tmdb_id: r.id,
            media_type,
            title: r.title.or(r.name).unwrap_or_default(),
            year: date
                .and_then(|d| d.get(..4).map(str::to_owned))
                .filter(|y| !y.is_empty()),
            overview: r.overview.filter(|o| !o.is_empty()),
            poster_url: r.poster_path.as_deref().map(|p| self.poster_url(p)),
            download_progress: r
                .media_info
                .as_ref()
                .and_then(MediaInfoDto::download_progress),
            availability: availability(r.media_info.and_then(|m| m.status)),
        })
    }

    /// Seasons of a show with their availability (to pick what to request).
    pub async fn seasons(&self, tmdb_id: i64) -> Result<Vec<SeerrSeason>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Tv {
            #[serde(default)]
            seasons: Vec<SeasonDto>,
            #[serde(default)]
            media_info: Option<MediaInfoDto>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct SeasonDto {
            season_number: u32,
            #[serde(default)]
            name: Option<String>,
        }
        let tv: Tv = net::send_json(self.req(Method::GET, &format!("/tv/{tmdb_id}"))).await?;
        let status = |n: u32| {
            tv.media_info
                .as_ref()
                .and_then(|m| m.seasons.iter().find(|s| s.season_number == n))
                .and_then(|s| s.status)
        };
        Ok(tv
            .seasons
            .iter()
            // Season 0 holds specials, which Seerr hides unless enabled.
            .filter(|s| s.season_number > 0)
            .map(|s| SeerrSeason {
                number: s.season_number,
                name: s
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Season {}", s.season_number)),
                availability: availability(status(s.season_number)),
            })
            .collect())
    }

    /// Requests a movie, or a show (`seasons` empty = every season).
    pub async fn request(
        &self,
        media_type: SeerrMediaType,
        tmdb_id: i64,
        seasons: &[u32],
    ) -> Result<SeerrRequest> {
        let mut body = json!({
            "mediaType": match media_type { SeerrMediaType::Movie => "movie", SeerrMediaType::Tv => "tv" },
            "mediaId": tmdb_id,
            "is4k": false,
        });
        if media_type == SeerrMediaType::Tv {
            body["seasons"] = if seasons.is_empty() {
                json!("all")
            } else {
                json!(seasons)
            };
        }
        let v: serde_json::Value =
            net::send_json(self.req(Method::POST, "/request").json(&body)).await?;
        self.convert_request(&v)
            .ok_or_else(|| Error::decode("unexpected request response"))
    }

    fn convert_request(&self, v: &serde_json::Value) -> Option<SeerrRequest> {
        let media = v.get("media")?;
        let media_type = match media.get("mediaType").and_then(|t| t.as_str()) {
            Some("tv") => SeerrMediaType::Tv,
            _ => SeerrMediaType::Movie,
        };
        let by = v
            .get("requestedBy")
            .cloned()
            .and_then(|u| serde_json::from_value::<UserDto>(u).ok());
        Some(SeerrRequest {
            id: v.get("id")?.as_i64()?,
            status: request_status(v.get("status")?.as_i64()?),
            media_type,
            tmdb_id: media.get("tmdbId")?.as_i64()?,
            availability: availability(media.get("status").and_then(|s| s.as_i64())),
            requested_by: by
                .map(|u| SeerrUser::from(u).display_name)
                .unwrap_or_default(),
            created_at: v
                .get("createdAt")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_owned(),
            title: None,
            poster_url: None,
        })
    }

    /// The signed-in user's requests (all requests for managers), newest first, with titles.
    /// Recent requests: the user's own, or everyone's when `all` (needs the right to view
    /// requests in Seerr, else `Unauthorized`).
    pub async fn requests(&self, take: u32, all: bool) -> Result<Vec<SeerrRequest>> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            results: Vec<serde_json::Value>,
        }
        let mut path = format!("/request?take={take}&skip=0&sort=added&sortDirection=desc");
        if !all {
            let me = self.me().await?;
            path.push_str(&format!("&requestedBy={}", me.id));
        }
        let p: Page = net::send_json(self.req(Method::GET, &path)).await?;
        let mut out: Vec<SeerrRequest> = p
            .results
            .iter()
            .filter_map(|v| self.convert_request(v))
            .collect();
        // Titles and posters come from the details endpoints (cached by Seerr).
        for r in out.iter_mut() {
            if let Some((title, poster)) = self.details(r.media_type, r.tmdb_id).await {
                r.title = Some(title);
                r.poster_url = poster;
            }
        }
        Ok(out)
    }

    async fn details(
        &self,
        media_type: SeerrMediaType,
        tmdb_id: i64,
    ) -> Option<(String, Option<String>)> {
        let path = match media_type {
            SeerrMediaType::Movie => format!("/movie/{tmdb_id}"),
            SeerrMediaType::Tv => format!("/tv/{tmdb_id}"),
        };
        let v: serde_json::Value = net::send_json(self.req(Method::GET, &path)).await.ok()?;
        let title = v
            .get("title")
            .or_else(|| v.get("name"))?
            .as_str()?
            .to_owned();
        let poster = v
            .get("posterPath")
            .and_then(|p| p.as_str())
            .map(|p| self.poster_url(p));
        Some((title, poster))
    }

    /// Cancels one of the user's pending requests.
    pub async fn cancel(&self, request_id: i64) -> Result<()> {
        net::send(self.req(Method::DELETE, &format!("/request/{request_id}"))).await?;
        Ok(())
    }

    /// Looks up a TMDB id's current state (e.g. right after a request).
    pub async fn availability(
        &self,
        media_type: SeerrMediaType,
        tmdb_id: i64,
    ) -> Result<Availability> {
        let path = match media_type {
            SeerrMediaType::Movie => format!("/movie/{tmdb_id}"),
            SeerrMediaType::Tv => format!("/tv/{tmdb_id}"),
        };
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct D {
            #[serde(default)]
            media_info: Option<MediaInfoDto>,
        }
        let d: D = net::send_json(self.req(Method::GET, &path)).await?;
        Ok(availability(d.media_info.and_then(|m| m.status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_follow_seerr_code_not_docs() {
        assert_eq!(availability(Some(6)), Availability::Blocklisted);
        assert_eq!(availability(Some(7)), Availability::Deleted);
        assert_eq!(availability(None), Availability::Unknown);
        assert_eq!(request_status(4), RequestStatus::Failed);
    }
}
