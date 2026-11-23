// SPDX-License-Identifier: AGPL-3.0-or-later
//! Seerr client (the Overseerr/Jellyseerr API, verified against Seerr 3.5.2's OpenAPI and code).
//!
//! A Seerr instance serves one media server type at a time, so sign-in follows what
//! `/settings/public` reports: Plex (plex.tv account token), Jellyfin/Emby (password or
//! QuickConnect) or a local account. The session is the `connect.sid` cookie, handled by hand
//! (no cookie-jar dependency) and kept by the platform in secure storage.

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

use i18n::tr;
use net::reqwest::{Client, Method, RequestBuilder, Response, header};
use net::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Title lookups in flight at once for the requests list.
const DETAIL_LOOKUPS: usize = 6;

/// Runs `futs` with at most `limit` in flight, results in the input order. No runtime needed
/// (works in a browser too): one future polls the running ones and starts the next as one
/// finishes.
async fn join_bounded<F: std::future::Future>(futs: Vec<F>, limit: usize) -> Vec<F::Output> {
    use std::pin::Pin;
    use std::task::Poll;
    let mut waiting = futs.into_iter().map(Box::pin).enumerate();
    let mut running: Vec<(usize, Pin<Box<F>>)> = Vec::new();
    let mut out: Vec<Option<F::Output>> = Vec::new();
    out.resize_with(waiting.len(), || None);
    std::future::poll_fn(|cx| {
        loop {
            while running.len() < limit.max(1) {
                match waiting.next() {
                    Some(f) => running.push(f),
                    None => break,
                }
            }
            let before = running.len();
            running.retain_mut(|(i, f)| match f.as_mut().poll(cx) {
                Poll::Ready(v) => {
                    out[*i] = Some(v);
                    false
                }
                Poll::Pending => true,
            });
            if running.is_empty() && waiting.len() == 0 {
                return Poll::Ready(());
            }
            // Nothing finished: wait to be woken. Otherwise start the next ones now.
            if running.len() == before {
                return Poll::Pending;
            }
        }
    })
    .await;
    out.into_iter().flatten().collect()
}

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
    /// Someone in the cast of a title.
    pub struct SeerrCast {
        pub name: String,
        pub character: Option<String>,
        pub photo_url: Option<String>,
    }
}

record! {
    /// A movie's saga (TMDB collection), to show its other parts.
    pub struct SeerrCollectionRef {
        pub id: i64,
        pub name: String,
    }
}

record! {
    /// Everything a title page shows (like Seerr's own movie/show page).
    pub struct SeerrDetails {
        pub tmdb_id: i64,
        pub media_type: SeerrMediaType,
        pub title: String,
        pub year: Option<String>,
        pub tagline: Option<String>,
        pub overview: Option<String>,
        /// Minutes (movies) or per episode (shows).
        pub runtime_min: Option<u32>,
        pub genres: Vec<String>,
        /// TMDB rating out of 10.
        pub rating: Option<f32>,
        pub poster_url: Option<String>,
        pub backdrop_url: Option<String>,
        pub availability: Availability,
        pub download_progress: Option<f32>,
        pub collection: Option<SeerrCollectionRef>,
        pub cast: Vec<SeerrCast>,
        /// Shows: how many seasons.
        pub seasons: Option<u32>,
    }
}

record! {
    pub struct SeerrCollection {
        pub id: i64,
        pub name: String,
        pub overview: Option<String>,
        pub backdrop_url: Option<String>,
        /// The films of the saga, in release order.
        pub parts: Vec<SeerrResult>,
    }
}

record! {
    pub struct SeerrSeason {
        pub number: u32,
        pub name: String,
        pub availability: Availability,
    }
}

simple_enum! {
    /// Where one season of a show stands, for choosing what to request.
    pub enum SeasonState {
        /// Every episode is on the media server.
        OnServer,
        /// Some episodes are on the media server (Seerr won't take a request for the rest).
        PartlyOnServer,
        /// Asked for already (waiting for approval, or approved and not on the server yet).
        Requested,
        /// Sonarr is downloading it.
        Downloading,
        /// Not on the server and not asked for: can be requested.
        Requestable,
        /// Blocklisted on Seerr: can't be requested.
        Blocked,
    }
}

record! {
    /// A season of a show as Seerr knows it: TMDB's facts plus what the media server and the
    /// open requests say about it (input of [`season_plan`]).
    pub struct SeerrSeasonInfo {
        pub number: u32,
        pub name: String,
        /// Episodes TMDB lists for it (0 for an announced season not aired yet).
        pub episodes: u32,
        pub availability: Availability,
        /// In an open request (pending, approved or failed: not declined or completed).
        pub requested: bool,
        /// Sonarr's queue holds episodes of it.
        pub downloading: bool,
    }
}

record! {
    /// One season in the "request more seasons" list.
    pub struct SeasonChoice {
        pub number: u32,
        pub name: String,
        pub state: SeasonState,
        /// The state in plain words ("On your server", "Requested"…).
        pub label: String,
    }
}

record! {
    /// What can be requested of a show, season by season.
    pub struct SeasonPlan {
        /// The seasons to list, in order (specials only when they are all that is missing).
        pub seasons: Vec<SeasonChoice>,
        /// The seasons that can be requested ("Select all missing"); empty = nothing to do.
        pub missing: Vec<u32>,
        /// Some season is already on the server or asked for: the main button reads "Request
        /// the missing seasons" rather than "Request all seasons".
        pub has_some: bool,
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
        /// Shows: the seasons this request asked for (a show often has several requests,
        /// one per batch of seasons). Empty for movies.
        #[serde(default)]
        pub seasons: Vec<u32>,
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
        .unwrap_or_else(|| tr!("user {id}", id = u.id));
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
            return Err(Error::invalid(tr!("this Seerr instance is not set up yet")));
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
    /// Requests for it (with the seasons each asked for), on the title endpoints.
    #[serde(default)]
    requests: Vec<MediaRequestDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaRequestDto {
    #[serde(default)]
    status: i64,
    #[serde(default)]
    is4k: bool,
    #[serde(default)]
    seasons: Vec<SeasonStatusDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueEpisodeDto {
    season_number: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadStatusDto {
    #[serde(default)]
    size: f64,
    #[serde(default)]
    size_left: f64,
    /// The episode it is (shows only, when Sonarr says).
    #[serde(default)]
    episode: Option<QueueEpisodeDto>,
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
    #[serde(default)]
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

    /// TMDB backdrop (wide), through Seerr's image cache when it has one.
    pub fn backdrop_url(&self, path: &str) -> String {
        if self.cache_images {
            format!("{}/imageproxy/tmdb/t/p/w1280{path}", self.base)
        } else {
            format!("https://image.tmdb.org/t/p/w1280{path}")
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
                    .unwrap_or_else(|| tr!("Season {number}", number = s.season_number)),
                availability: availability(status(s.season_number)),
            })
            .collect())
    }

    /// Every season of a show (specials included) with its state on the server, in requests
    /// and in Sonarr's queue: the input of [`season_plan`].
    pub async fn season_info(&self, tmdb_id: i64) -> Result<Vec<SeerrSeasonInfo>> {
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
            #[serde(default)]
            episode_count: Option<u32>,
        }
        let tv: Tv = net::send_json(self.req(Method::GET, &format!("/tv/{tmdb_id}"))).await?;
        let info = tv.media_info.as_ref();
        let status = |n: u32| {
            info.and_then(|m| m.seasons.iter().find(|s| s.season_number == n))
                .and_then(|s| s.status)
        };
        // Seerr counts a season as asked for while a (non-4K) request for it is neither
        // declined (3) nor completed (5); the season's own status in it can say the same.
        let requested = |n: u32| {
            info.is_some_and(|m| {
                m.requests.iter().any(|r| {
                    !r.is4k
                        && !matches!(r.status, 3 | 5)
                        && r.seasons
                            .iter()
                            .any(|s| s.season_number == n && !matches!(s.status, Some(3 | 5)))
                })
            })
        };
        let queue = info.map(|m| m.download_status.as_slice()).unwrap_or(&[]);
        let downloading = |n: u32, a: Availability| {
            queue.iter().any(|d| match &d.episode {
                Some(e) => e.season_number == n,
                // Sonarr didn't say which episode: the seasons being processed.
                None => a == Availability::Processing,
            })
        };
        Ok(tv
            .seasons
            .iter()
            .map(|s| {
                let n = s.season_number;
                let a = availability(status(n));
                SeerrSeasonInfo {
                    number: n,
                    name: s.name.clone().filter(|x| !x.is_empty()).unwrap_or_else(|| {
                        if n == 0 {
                            tr!("Specials")
                        } else {
                            tr!("Season {number}", number = n)
                        }
                    }),
                    episodes: s.episode_count.unwrap_or(0),
                    availability: a,
                    requested: requested(n),
                    downloading: downloading(n, a),
                }
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
            seasons: v
                .get("seasons")
                .and_then(|s| s.as_array())
                .map(|a| {
                    let mut n: Vec<u32> = a
                        .iter()
                        .filter_map(|s| s.get("seasonNumber")?.as_u64())
                        .map(|n| n as u32)
                        .collect();
                    n.sort_unstable();
                    n.dedup();
                    n
                })
                .unwrap_or_default(),
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
        let out: Vec<SeerrRequest> = p
            .results
            .iter()
            .filter_map(|v| self.convert_request(v))
            .collect();
        let mut out = group_requests(out);
        // Titles and posters come from the details endpoints (cached by Seerr), a few at once.
        let details = join_bounded(
            out.iter()
                .map(|r| self.details(r.media_type, r.tmdb_id))
                .collect(),
            DETAIL_LOOKUPS,
        )
        .await;
        for (r, d) in out.iter_mut().zip(details) {
            if let Some((title, poster)) = d {
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

    /// The page of a movie or a show: details, state, cast and saga.
    pub async fn title(&self, media_type: SeerrMediaType, tmdb_id: i64) -> Result<SeerrDetails> {
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct Named {
            name: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct CastDto {
            name: String,
            character: Option<String>,
            profile_path: Option<String>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct Credits {
            cast: Vec<CastDto>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct CollectionDto {
            id: i64,
            name: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct D {
            title: Option<String>,
            name: Option<String>,
            tagline: Option<String>,
            overview: Option<String>,
            release_date: Option<String>,
            first_air_date: Option<String>,
            runtime: Option<u32>,
            episode_run_time: Vec<u32>,
            genres: Vec<Named>,
            vote_average: Option<f32>,
            poster_path: Option<String>,
            backdrop_path: Option<String>,
            media_info: Option<MediaInfoDto>,
            collection: Option<CollectionDto>,
            belongs_to_collection: Option<CollectionDto>,
            credits: Credits,
            number_of_seasons: Option<u32>,
        }
        let path = match media_type {
            SeerrMediaType::Movie => format!("/movie/{tmdb_id}"),
            SeerrMediaType::Tv => format!("/tv/{tmdb_id}"),
        };
        let d: D = net::send_json(self.req(Method::GET, &path)).await?;
        let date = d.release_date.or(d.first_air_date);
        Ok(SeerrDetails {
            tmdb_id,
            media_type,
            title: d.title.or(d.name).unwrap_or_default(),
            year: date
                .and_then(|x| x.get(..4).map(str::to_owned))
                .filter(|y| !y.is_empty()),
            tagline: d.tagline.filter(|t| !t.is_empty()),
            overview: d.overview.filter(|o| !o.is_empty()),
            runtime_min: d
                .runtime
                .filter(|r| *r > 0)
                .or(d.episode_run_time.first().copied()),
            genres: d.genres.into_iter().map(|g| g.name).collect(),
            rating: d.vote_average.filter(|v| *v > 0.0),
            poster_url: d.poster_path.as_deref().map(|p| self.poster_url(p)),
            backdrop_url: d.backdrop_path.as_deref().map(|p| self.backdrop_url(p)),
            download_progress: d
                .media_info
                .as_ref()
                .and_then(MediaInfoDto::download_progress),
            availability: availability(d.media_info.and_then(|m| m.status)),
            collection: d
                .collection
                .or(d.belongs_to_collection)
                .filter(|c| c.id > 0)
                .map(|c| SeerrCollectionRef {
                    id: c.id,
                    name: c.name,
                }),
            cast: d
                .credits
                .cast
                .into_iter()
                .take(12)
                .map(|c| SeerrCast {
                    name: c.name,
                    character: c.character.filter(|x| !x.is_empty()),
                    photo_url: c.profile_path.as_deref().map(|p| self.poster_url(p)),
                })
                .collect(),
            seasons: d.number_of_seasons,
        })
    }

    /// A saga with its films (and whether each is on the server or requested).
    pub async fn collection(&self, id: i64) -> Result<SeerrCollection> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct D {
            #[serde(default)]
            name: String,
            #[serde(default)]
            overview: Option<String>,
            #[serde(default)]
            backdrop_path: Option<String>,
            #[serde(default)]
            parts: Vec<ResultDto>,
        }
        let d: D = net::send_json(self.req(Method::GET, &format!("/collection/{id}"))).await?;
        // A saga's parts are films (Seerr may leave their media type out).
        let mut parts: Vec<SeerrResult> = d
            .parts
            .into_iter()
            .filter_map(|mut p| {
                if p.media_type.is_empty() {
                    p.media_type = "movie".into();
                }
                self.convert(p)
            })
            .collect();
        parts.sort_by(|a, b| a.year.cmp(&b.year));
        Ok(SeerrCollection {
            id,
            name: d.name,
            overview: d.overview.filter(|o| !o.is_empty()),
            backdrop_url: d.backdrop_path.as_deref().map(|p| self.backdrop_url(p)),
            parts,
        })
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

// ---------------------------------------------------------------------------------------------
// Choosing seasons to request (shared by the apps: same rules, same words).

/// A season's state in plain words.
pub fn season_state_label(state: SeasonState) -> String {
    match state {
        SeasonState::OnServer => tr!("On your server"),
        SeasonState::PartlyOnServer => tr!("Partly on your server"),
        SeasonState::Requested => tr!("Requested"),
        SeasonState::Downloading => tr!("Being downloaded"),
        SeasonState::Requestable => tr!("Not on your server"),
        SeasonState::Blocked => tr!("Can't be requested"),
    }
}

fn season_state(s: &SeerrSeasonInfo, have: &[u32]) -> SeasonState {
    match s.availability {
        Availability::Available => SeasonState::OnServer,
        _ if s.downloading => SeasonState::Downloading,
        Availability::PartiallyAvailable => SeasonState::PartlyOnServer,
        _ if s.requested => SeasonState::Requested,
        Availability::Pending | Availability::Processing => SeasonState::Requested,
        Availability::Blocklisted => SeasonState::Blocked,
        // Seerr follows one media server; the one being browsed may have it anyway.
        _ if have.contains(&s.number) => SeasonState::OnServer,
        _ => SeasonState::Requestable,
    }
}

/// Each season of a show with its state, and what can be requested. `have`: the season
/// numbers the media server being browsed shows (Seerr follows only one media server, so a
/// season it doesn't know of can still be there). Specials (season 0) are listed only when
/// they are the only thing that can be requested.
pub fn season_plan(seasons: &[SeerrSeasonInfo], have: &[u32]) -> SeasonPlan {
    let mut list: Vec<SeasonChoice> = seasons
        .iter()
        .map(|s| {
            let state = season_state(s, have);
            SeasonChoice {
                number: s.number,
                name: s.name.clone(),
                state,
                label: season_state_label(state),
            }
        })
        .collect();
    list.sort_by_key(|c| c.number);
    let open = |c: &SeasonChoice| c.state == SeasonState::Requestable;
    let specials_only = !list.iter().any(|c| c.number > 0 && open(c));
    list.retain(|c| c.number > 0 || (specials_only && open(c)));
    SeasonPlan {
        missing: list.iter().filter(|c| open(c)).map(|c| c.number).collect(),
        has_some: list
            .iter()
            .any(|c| !matches!(c.state, SeasonState::Requestable | SeasonState::Blocked)),
        seasons: list,
    }
}

/// The confirmation once seasons are requested: "Requested: season 2.", "Requested: seasons
/// 2, 3, 5 and the specials." (whole sentences, for translation).
pub fn seasons_requested_message(numbers: &[u32]) -> String {
    let mut sorted = numbers.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let names: Vec<String> = sorted
        .iter()
        .filter(|n| **n > 0)
        .map(u32::to_string)
        .collect();
    let specials = sorted.contains(&0);
    match (names.as_slice(), specials) {
        ([one], false) => tr!(
            "Requested: season {number}. It'll appear here when it's ready.",
            number = one
        ),
        ([one], true) => tr!(
            "Requested: season {number} and the specials. They'll appear here when they're ready.",
            number = one
        ),
        ([], _) => tr!("Requested: the specials. They'll appear here when they're ready."),
        (many, false) => tr!(
            "Requested: seasons {numbers}. They'll appear here when they're ready.",
            numbers = many.join(", ")
        ),
        (many, true) => tr!(
            "Requested: seasons {numbers} and the specials. They'll appear here when they're ready.",
            numbers = many.join(", ")
        ),
    }
}

/// One card per title: a show is often requested several times (a batch of seasons each),
/// and several people may want the same film. Keeps the newest request's place (the list is
/// newest first), names everyone who asked, joins the seasons, and stays "pending" while any
/// request still waits for approval.
pub fn group_requests(list: Vec<SeerrRequest>) -> Vec<SeerrRequest> {
    let mut out: Vec<SeerrRequest> = Vec::with_capacity(list.len());
    for r in list {
        let Some(g) = out
            .iter_mut()
            .find(|g| g.media_type == r.media_type && g.tmdb_id == r.tmdb_id)
        else {
            out.push(r);
            continue;
        };
        if r.status == RequestStatus::Pending {
            g.status = RequestStatus::Pending;
        }
        if !r.requested_by.is_empty() && !g.requested_by.split(", ").any(|n| n == r.requested_by) {
            if !g.requested_by.is_empty() {
                g.requested_by.push_str(", ");
            }
            g.requested_by.push_str(&r.requested_by);
        }
        g.seasons.extend(r.seasons);
        g.seasons.sort_unstable();
        g.seasons.dedup();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_requested_several_times_is_one_card() {
        let req = |id, tmdb, status, by: &str, seasons: Vec<u32>| SeerrRequest {
            id,
            status,
            media_type: SeerrMediaType::Tv,
            tmdb_id: tmdb,
            availability: Availability::Processing,
            requested_by: by.into(),
            created_at: String::new(),
            title: None,
            poster_url: None,
            seasons,
        };
        let g = group_requests(vec![
            req(9, 1, RequestStatus::Approved, "Ann", vec![3]),
            req(8, 2, RequestStatus::Approved, "Bob", vec![1]),
            req(7, 1, RequestStatus::Pending, "Bob", vec![1, 2]),
            req(6, 1, RequestStatus::Approved, "Ann", vec![2]),
        ]);
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].id, g[0].tmdb_id), (9, 1));
        assert_eq!(g[0].status, RequestStatus::Pending);
        assert_eq!(g[0].requested_by, "Ann, Bob");
        assert_eq!(g[0].seasons, vec![1, 2, 3]);
        assert_eq!(g[1].tmdb_id, 2);
    }

    #[test]
    fn status_codes_follow_seerr_code_not_docs() {
        assert_eq!(availability(Some(6)), Availability::Blocklisted);
        assert_eq!(availability(Some(7)), Availability::Deleted);
        assert_eq!(availability(None), Availability::Unknown);
        assert_eq!(request_status(4), RequestStatus::Failed);
    }

    fn season(n: u32, a: Availability) -> SeerrSeasonInfo {
        SeerrSeasonInfo {
            number: n,
            name: if n == 0 {
                "Specials".into()
            } else {
                format!("Season {n}")
            },
            episodes: 8,
            availability: a,
            requested: false,
            downloading: false,
        }
    }

    #[test]
    fn plan_for_a_show_with_seasons_1_6_7() {
        use Availability::*;
        let mut s: Vec<_> = (0..=8).map(|n| season(n, Unknown)).collect();
        s[1].availability = Available;
        s[6].availability = Available;
        s[7].availability = PartiallyAvailable;
        s[2].requested = true;
        s[3].availability = Processing;
        s[4].availability = Processing;
        s[4].downloading = true;
        s[8].availability = Blocklisted;
        let p = season_plan(&s, &[1, 6, 7]);
        let states: Vec<_> = p.seasons.iter().map(|c| (c.number, c.state)).collect();
        assert_eq!(
            states,
            vec![
                (1, SeasonState::OnServer),
                (2, SeasonState::Requested),
                (3, SeasonState::Requested),
                (4, SeasonState::Downloading),
                (5, SeasonState::Requestable),
                (6, SeasonState::OnServer),
                (7, SeasonState::PartlyOnServer),
                (8, SeasonState::Blocked),
            ],
            "specials left out while other seasons can be requested"
        );
        assert_eq!(p.missing, vec![5]);
        assert!(p.has_some);
        assert_eq!(p.seasons[6].label, "Partly on your server");
        assert_eq!(p.seasons[3].label, "Being downloaded");
    }

    #[test]
    fn plan_lists_specials_only_when_they_are_all_that_is_missing() {
        use Availability::*;
        let s = vec![
            season(2, Available),
            season(0, Unknown),
            season(1, Available),
        ];
        let p = season_plan(&s, &[]);
        assert_eq!(
            p.seasons.iter().map(|c| c.number).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(p.missing, vec![0]);
        // Specials already there: not listed, nothing to do.
        let s = vec![season(0, Available), season(1, Available)];
        let p = season_plan(&s, &[]);
        assert_eq!(p.seasons.len(), 1);
        assert!(p.missing.is_empty());
    }

    #[test]
    fn plan_counts_what_the_browsed_server_has_and_fresh_shows() {
        use Availability::*;
        // Seerr follows another media server: the seasons this one shows are there.
        let s = vec![season(1, Unknown), season(2, Deleted), season(3, Unknown)];
        let p = season_plan(&s, &[1]);
        assert_eq!(p.seasons[0].state, SeasonState::OnServer);
        assert_eq!(p.missing, vec![2, 3]);
        assert!(p.has_some);
        // Nothing anywhere: everything requestable, "Request all seasons".
        let p = season_plan(&s, &[]);
        assert_eq!(p.missing, vec![1, 2, 3]);
        assert!(!p.has_some);
        assert!(season_plan(&[], &[]).missing.is_empty());
    }

    #[test]
    fn requested_message_in_plain_words() {
        assert_eq!(
            seasons_requested_message(&[5, 2, 3, 4]),
            "Requested: seasons 2, 3, 4, 5. They'll appear here when they're ready."
        );
        assert_eq!(
            seasons_requested_message(&[2]),
            "Requested: season 2. It'll appear here when it's ready."
        );
        assert_eq!(
            seasons_requested_message(&[0]),
            "Requested: the specials. They'll appear here when they're ready."
        );
        assert_eq!(
            seasons_requested_message(&[3, 0]),
            "Requested: season 3 and the specials. They'll appear here when they're ready."
        );
    }

    #[tokio::test]
    async fn bounded_join_keeps_order_and_the_limit() {
        use std::cell::Cell;
        /// Pending a given number of polls (waking itself), then ready.
        struct Yield(u32);
        impl std::future::Future for Yield {
            type Output = ();
            fn poll(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<()> {
                if self.0 == 0 {
                    return std::task::Poll::Ready(());
                }
                self.0 -= 1;
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        }
        let (now, most) = (Cell::new(0), Cell::new(0));
        let futs = (0..20u32)
            .map(|i| {
                let (now, most) = (&now, &most);
                async move {
                    now.set(now.get() + 1);
                    most.set(most.get().max(now.get()));
                    // Later ones finish sooner: order must still be the input's.
                    Yield(20 - i).await;
                    now.set(now.get() - 1);
                    i * 10
                }
            })
            .collect();
        let out = join_bounded(futs, 6).await;
        assert_eq!(out, (0..20).map(|i| i * 10).collect::<Vec<_>>());
        assert_eq!(most.get(), 6);
        assert!(join_bounded(Vec::<Yield>::new(), 6).await.is_empty());
    }
}
