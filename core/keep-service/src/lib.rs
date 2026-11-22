// SPDX-License-Identifier: AGPL-3.0-or-later
//! The keep-service protocol (shared by alwywatch-server and the apps) and its HTTP client.
//! Wire format: JSON, snake_case. Documented in `docs/keep-protocol.md`.
//!
//! Every authenticated call sends `Authorization: Bearer <token>` and `X-AlwyWatch-Server: <id>`:
//! for Jellyfin the token is the user's Jellyfin access token, for Plex the plex.tv account token.
//! The server validates it against the allow-listed media server and never stores it.

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

pub mod identity;
pub mod profile;
pub mod together;
pub mod transfer;

pub use identity::{LinkRequest, LinkedIdentity, MeWithIdentities};
pub use profile::{Avatar, AvatarChoice, AvatarPreset, DisplayNameRequest};

use net::Result;
use net::reqwest::{Client, Method, RequestBuilder};
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
        #[serde(rename_all = "snake_case")]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
        pub enum $name { $($body)* }
    };
}

pub const SERVER_HEADER: &str = "X-AlwyWatch-Server";

simple_enum! {
    pub enum ServerType { Jellyfin, Plex }
}

record! {
    /// An allow-listed media server (public, so apps can offer it without typing a URL).
    pub struct ServerEntry {
        pub id: String,
        pub kind: ServerType,
        pub name: String,
        pub url: String,
    }
}

simple_enum! {
    pub enum VoteKind {
        /// "I don't care": a signal for the admin, never deletes anything by itself.
        DontCare,
        /// Keep for N more days.
        KeepDays,
        /// Keep until I have watched it (with an optional reminder every N days).
        KeepUntilWatched,
        /// Keep forever ("it's a classic", "I love it").
        KeepForever,
    }
}

record! {
    pub struct VoteRequest {
        pub kind: VoteKind,
        /// KeepDays only.
        pub days: Option<u32>,
        /// KeepUntilWatched only: the app schedules a local reminder every N days.
        pub reminder_days: Option<u32>,
        /// KeepForever only, free text ("classic", "love it").
        pub reason: Option<String>,
    }
}

record! {
    pub struct MyVote {
        pub kind: VoteKind,
        /// KeepDays: unix seconds until which the item is kept.
        pub until: Option<i64>,
        pub reminder_days: Option<u32>,
        pub reason: Option<String>,
        pub created_at: i64,
    }
}

record! {
    #[derive(Default)]
    pub struct VoteCounts {
        pub forever: u32,
        pub until_watched: u32,
        pub days: u32,
        pub dont_care: u32,
    }
}

simple_enum! {
    pub enum Protection {
        /// Will be deleted at `deletion_date` (if the collection is active).
        None,
        /// Someone extended it: deleted at `effective_deletion_date`.
        Extended,
        /// Kept until everyone who asked has watched it.
        UntilWatched,
        Forever,
    }
}

record! {
    pub struct LeavingItem {
        /// Maintainerr item id (the Plex ratingKey in Plex mode). Stable across collections.
        pub key: String,
        pub title: String,
        pub year: Option<i32>,
        /// "movie" | "show"
        pub kind: String,
        pub poster_url: Option<String>,
        pub tmdb_id: Option<String>,
        pub tvdb_id: Option<String>,
        /// The same item on the caller's server, to open or play it.
        pub server_item_id: Option<String>,
        pub collection_id: Option<i64>,
        pub collection_title: Option<String>,
        /// False while the collection is inactive (nothing is deleted yet: review phase).
        pub collection_active: bool,
        /// Maintainerr's date (unix seconds), before any keep. May be in the past.
        pub deletion_date: Option<i64>,
        /// After extensions; None when protected (forever / until watched).
        pub effective_deletion_date: Option<i64>,
        pub protection: Protection,
        pub my_vote: Option<MyVote>,
        pub counts: VoteCounts,
    }
}

record! {
    pub struct Rights {
        pub can_keep_forever: bool,
        /// None = unlimited.
        pub max_forever: Option<u32>,
        /// Longest "keep N days" allowed; None = unlimited (Maintainerr caps at 3650).
        pub max_keep_days: Option<u32>,
    }
}

impl Default for Rights {
    fn default() -> Self {
        Self {
            can_keep_forever: true,
            max_forever: None,
            max_keep_days: None,
        }
    }
}

record! {
    pub struct Me {
        pub user_key: String,
        /// The name shown to others: the chosen display name, else the media-server name.
        pub name: String,
        /// The name from the media server (offer "Reset to <media_name>" when `name_is_custom`).
        #[serde(default)]
        pub media_name: String,
        /// A display name was chosen (`PUT /v1/me/name`).
        #[serde(default)]
        pub name_is_custom: bool,
        #[serde(default)]
        pub avatar: Avatar,
        /// The one-time welcome step (name + avatar) was done or skipped on some device: while
        /// false, apps offer it (see `docs/profile-apps.md`).
        #[serde(default)]
        pub profile_set: bool,
        pub server_id: String,
        pub is_admin: bool,
        pub rights: Rights,
        pub forever_count: u32,
        /// What works behind this server right now (apps hide the pages that don't). Missing
        /// from older servers: everything is assumed to work.
        #[serde(default)]
        pub services: Services,
    }
}

record! {
    /// The parts of the AlwyWatch server that depend on another service.
    pub struct Services {
        /// Leaving soon and keep choices: Maintainerr answers (its last sync worked).
        pub leaving_soon: bool,
        /// Requests through this server: its Seerr link is configured.
        pub requests: bool,
    }
}

impl Default for Services {
    fn default() -> Self {
        Self {
            leaving_soon: true,
            requests: true,
        }
    }
}

record! {
    pub struct AdminUser {
        pub user_key: String,
        /// Display name (chosen, else the media-server name).
        pub name: String,
        /// The media-server name (names are not unique: show it next to `name`).
        #[serde(default)]
        pub media_name: String,
        #[serde(default)]
        pub avatar: Avatar,
        pub server_id: String,
        pub is_admin: bool,
        pub rights: Rights,
        pub forever_count: u32,
        pub last_seen: i64,
        /// The Seerr user their requests go to (linked automatically or by an admin).
        #[serde(default)]
        pub seerr_user_id: Option<i64>,
    }
}

record! {
    /// A Seerr user, for admins linking AlwyWatch users to existing Seerr accounts.
    pub struct SeerrUser {
        pub id: i64,
        pub name: String,
    }
}

record! {
    /// `GET /v1/admin/forever`: who keeps what forever, and how much of it others keep too.
    pub struct ForeverStats {
        /// Distinct titles someone keeps forever.
        pub titles: u32,
        /// Of those, kept forever by two people or more.
        pub shared: u32,
        /// Most titles first.
        pub people: Vec<ForeverPerson>,
        /// Every title kept forever, most keepers first.
        pub items: Vec<ForeverTitle>,
    }
}

record! {
    pub struct ForeverPerson {
        pub user_key: String,
        /// Display name (chosen, else the media-server name).
        pub name: String,
        /// The media-server name, shown next to `name`.
        #[serde(default)]
        pub media_name: String,
        #[serde(default)]
        pub avatar: Avatar,
        pub count: u32,
        /// Also kept forever by someone else.
        pub shared: u32,
        /// Kept forever by them alone.
        pub alone: u32,
        /// Who they overlap with, most titles in common first.
        pub overlaps: Vec<ForeverOverlap>,
    }
}

record! {
    pub struct ForeverOverlap {
        pub user_key: String,
        pub name: String,
        pub both: u32,
    }
}

record! {
    pub struct ForeverTitle {
        pub key: String,
        pub title: String,
        pub year: Option<i32>,
        pub poster_url: Option<String>,
        /// user_keys of the people keeping it forever.
        pub keepers: Vec<String>,
    }
}

record! {
    /// `PUT /v1/admin/users/{user}/seerr`: link to this Seerr user, or unlink (None).
    pub struct SeerrLinkRequest {
        pub seerr_user_id: Option<i64>,
    }
}

record! {
    pub struct CountdownRequest {
        /// Spread new deletion dates over this many days (0 = all start today).
        pub stagger_days: u32,
    }
}

record! {
    pub struct CountdownResult {
        pub reset: u32,
        pub skipped: u32,
    }
}

record! {
    /// `GET /v1/seerr`: the caller's Seerr account behind the bridge.
    pub struct SeerrBridgeInfo {
        pub seerr_user_id: i64,
        pub user_name: String,
        /// Seerr's own web UI, when the operator published it.
        pub public_url: Option<String>,
        /// Whether Seerr serves images through its cache (`/imageproxy`).
        pub cache_images: bool,
    }
}

/// Seerr API through the bridge: `{keep base}{SEERR_BRIDGE_PATH}/api/v1/...`, same paths and
/// bodies as Seerr itself (an allow-listed subset), authenticated like every keep call.
pub const SEERR_BRIDGE_PATH: &str = "/v1/seerr";

record! {
    pub struct ApiError {
        pub error: String,
    }
}

// ---------------------------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct KeepClient {
    http: Client,
    base: String,
    token: String,
    server_id: String,
}

impl KeepClient {
    pub fn new(http: Client, base_url: &str, server_id: &str, token: &str) -> Self {
        Self {
            http,
            base: base_url.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            server_id: server_id.to_owned(),
        }
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}/v1{path}", self.base))
            .bearer_auth(&self.token)
            .header(SERVER_HEADER, &self.server_id)
            .header("Accept", "application/json")
    }

    /// Public: the allow-listed servers.
    pub async fn servers(http: &Client, base_url: &str) -> Result<Vec<ServerEntry>> {
        net::send_json(http.get(format!("{}/v1/servers", base_url.trim_end_matches('/')))).await
    }

    /// New device: registers its transfer key (no sign-in needed).
    pub async fn transfer_start(
        http: &Client,
        base_url: &str,
        start: &transfer::TransferStart,
    ) -> Result<transfer::TransferStarted> {
        net::send_json(
            http.post(format!("{}/v1/transfer", base_url.trim_end_matches('/')))
                .json(start),
        )
        .await
    }

    /// New device: the sealed bundle once a signed-in device sent it (None while waiting).
    pub async fn transfer_poll(
        http: &Client,
        base_url: &str,
        handle: &str,
    ) -> Result<Option<transfer::TransferSealed>> {
        let res = net::send(http.get(format!(
            "{}/v1/transfer/poll/{}",
            base_url.trim_end_matches('/'),
            net::percent_encode(handle)
        )))
        .await?;
        if res.status() == net::reqwest::StatusCode::ACCEPTED {
            return Ok(None);
        }
        let bytes = res.bytes().await?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| net::Error::decode(e.to_string()))
    }

    /// Signed-in device: who is asking for a transfer with this code.
    pub async fn transfer_peer(&self, code: &str) -> Result<transfer::TransferPeer> {
        net::send_json(self.req(
            Method::GET,
            &format!("/transfer/{}", net::percent_encode(code)),
        ))
        .await
    }

    /// Signed-in device: hands over the bundle sealed to the new device's key.
    pub async fn transfer_send(&self, code: &str, sealed: &transfer::TransferSealed) -> Result<()> {
        net::send(
            self.req(
                Method::PUT,
                &format!("/transfer/{}", net::percent_encode(code)),
            )
            .json(sealed),
        )
        .await?;
        Ok(())
    }

    /// Watch together: opens a lobby with this title (the caller is its host; nothing plays
    /// until [`KeepClient::together_start`]).
    pub async fn together_create(
        &self,
        create: &together::TogetherCreate,
    ) -> Result<together::TogetherJoined> {
        net::send_json(self.req(Method::POST, "/together").json(create)).await
    }

    /// Watch together: my rooms and the rooms I am invited to (nobody sees other rooms).
    pub async fn together_rooms(&self) -> Result<together::TogetherRooms> {
        net::send_json(self.req(Method::GET, "/together")).await
    }

    /// My rooms and invitations once their `version` differs from `after`, or after about 25 s
    /// (a long poll: call it in a loop with the last `version` to learn about invitations).
    pub async fn together_rooms_wait(&self, after: u64) -> Result<together::TogetherRooms> {
        net::send_json(
            self.req(Method::GET, &format!("/together/wait?after={after}"))
                .timeout(std::time::Duration::from_secs(together::WAIT_SECS + 15)),
        )
        .await
    }

    /// Everyone I can invite: recent watch-together partners first, then by name.
    pub async fn together_people(&self) -> Result<Vec<together::TogetherPerson>> {
        net::send_json(self.req(Method::GET, "/together/people")).await
    }

    /// The host invites people to the room.
    pub async fn together_invite(
        &self,
        code: &str,
        invite: &together::TogetherInvite,
    ) -> Result<together::TogetherState> {
        net::send_json(
            self.req(
                Method::POST,
                &format!("/together/{}/invites", net::percent_encode(code)),
            )
            .json(invite),
        )
        .await
    }

    /// The host withdraws an invitation (`member` is one of the host's member ids).
    pub async fn together_uninvite(
        &self,
        code: &str,
        member: &str,
        user_key: &str,
    ) -> Result<together::TogetherState> {
        net::send_json(self.req(
            Method::DELETE,
            &format!(
                "/together/{}/invites/{}?member={}",
                net::percent_encode(code),
                net::percent_encode(user_key),
                net::percent_encode(member)
            ),
        ))
        .await
    }

    /// An invited user says "not now": the invitation goes away.
    pub async fn together_decline(&self, code: &str) -> Result<()> {
        net::send(self.req(
            Method::DELETE,
            &format!("/together/{}/invites/me", net::percent_encode(code)),
        ))
        .await?;
        Ok(())
    }

    /// Takes a seat in a room I am invited to (or already in on another device).
    pub async fn together_join(&self, code: &str) -> Result<together::TogetherJoined> {
        net::send_json(self.req(
            Method::POST,
            &format!("/together/{}/join", net::percent_encode(code)),
        ))
        .await
    }

    /// Play/pause/seek, a new title, or (host, `started: true`) the start of a lobby.
    pub async fn together_update(
        &self,
        code: &str,
        update: &together::TogetherUpdate,
    ) -> Result<together::TogetherState> {
        net::send_json(
            self.req(
                Method::PUT,
                &format!("/together/{}", net::percent_encode(code)),
            )
            .json(update),
        )
        .await
    }

    /// The host starts the lobby: playing from `position_ms` for everyone.
    pub async fn together_start(
        &self,
        code: &str,
        member: &str,
        position_ms: u64,
    ) -> Result<together::TogetherState> {
        self.together_update(
            code,
            &together::TogetherUpdate {
                member_id: member.to_owned(),
                playing: true,
                position_ms,
                media: None,
                started: true,
            },
        )
        .await
    }

    /// The room once it changed past `after`, or after about 25 s (a long poll: give the
    /// request a longer timeout). It is also the member's heartbeat.
    pub async fn together_wait(
        &self,
        code: &str,
        member: &str,
        after: u64,
    ) -> Result<together::TogetherState> {
        net::send_json(
            self.req(
                Method::GET,
                &format!(
                    "/together/{}/wait?after={after}&member={}",
                    net::percent_encode(code),
                    net::percent_encode(member)
                ),
            )
            .timeout(std::time::Duration::from_secs(together::WAIT_SECS + 15)),
        )
        .await
    }

    /// Leaves the room for good (rejoining needs a new invitation unless another of my
    /// devices is still in it).
    pub async fn together_leave(&self, code: &str, member: &str) -> Result<()> {
        net::send(self.req(
            Method::DELETE,
            &format!(
                "/together/{}/members/{}",
                net::percent_encode(code),
                net::percent_encode(member)
            ),
        ))
        .await?;
        Ok(())
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Links (or creates) the caller's Seerr account. 404 when the bridge is not configured.
    pub async fn seerr_bridge(&self) -> Result<SeerrBridgeInfo> {
        net::send_json(self.req(Method::GET, "/seerr")).await
    }

    pub async fn me(&self) -> Result<Me> {
        net::send_json(self.req(Method::GET, "/me")).await
    }

    /// `me` with the account's linked sign-ins ("Also signed in as …").
    pub async fn me_with_identities(&self) -> Result<MeWithIdentities> {
        net::send_json(self.req(Method::GET, "/me")).await
    }

    /// The sign-ins of my AlwyWatch account (one entry: nothing linked).
    pub async fn links(&self) -> Result<Vec<LinkedIdentity>> {
        net::send_json(self.req(Method::GET, "/me/links")).await
    }

    /// Makes my account and the account of `token` on `server_id` (another served server, the
    /// same person) one AlwyWatch account. Idempotent. Afterwards read [`KeepClient::me`]
    /// again: `user_key` may have changed (see `docs/identity-apps.md`).
    pub async fn link(&self, server_id: &str, token: &str) -> Result<Vec<LinkedIdentity>> {
        net::send_json(self.req(Method::POST, "/me/links").json(&LinkRequest {
            server_id: server_id.to_owned(),
            token: token.to_owned(),
        }))
        .await
    }

    /// Separates one of my sign-ins (not the canonical one): it becomes a new, empty account.
    pub async fn unlink(&self, user_key: &str) -> Result<Vec<LinkedIdentity>> {
        net::send_json(self.req(
            Method::DELETE,
            &format!("/me/links/{}", net::percent_encode(user_key)),
        ))
        .await
    }

    /// My settings shared across my devices (opaque key/values, see the core's settings sync).
    pub async fn settings(&self) -> Result<std::collections::HashMap<String, String>> {
        net::send_json(self.req(Method::GET, "/me/settings")).await
    }

    /// Upserts some of my shared settings.
    pub async fn set_settings(
        &self,
        values: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        net::send(self.req(Method::PUT, "/me/settings").json(values)).await?;
        Ok(())
    }

    /// Chooses my display name (validated by the server like
    /// [`profile::clean_display_name`]); returns my updated profile.
    pub async fn set_display_name(&self, name: &str) -> Result<Me> {
        net::send_json(self.req(Method::PUT, "/me/name").json(&DisplayNameRequest {
            name: name.to_owned(),
        }))
        .await
    }

    /// Ends the one-time welcome step for this account (Continue or Skip); idempotent.
    pub async fn mark_profile_set(&self) -> Result<Me> {
        net::send_json(self.req(Method::POST, "/me/profile-done")).await
    }

    /// Back to my media-server name.
    pub async fn reset_display_name(&self) -> Result<Me> {
        net::send_json(self.req(Method::DELETE, "/me/name")).await
    }

    /// Chooses a built-in avatar (an id of [`profile::avatar_presets`]).
    pub async fn set_avatar_preset(&self, preset: &str) -> Result<Me> {
        net::send_json(self.req(Method::PUT, "/me/avatar").json(&AvatarChoice {
            preset: preset.to_owned(),
        }))
        .await
    }

    /// Uploads my custom avatar: a square picture (512×512 at most, 512 KiB at most) of
    /// `content_type` `image/jpeg`, `image/png` or `image/webp`.
    pub async fn upload_avatar(&self, bytes: Vec<u8>, content_type: &str) -> Result<Me> {
        net::send_json(
            self.req(Method::PUT, "/me/avatar")
                .header("Content-Type", content_type)
                .body(bytes),
        )
        .await
    }

    /// Back to the default avatar (my initial on a colour).
    pub async fn remove_avatar(&self) -> Result<Me> {
        net::send_json(self.req(Method::DELETE, "/me/avatar")).await
    }

    /// URL of a user's custom avatar (`Avatar::custom_version`); fetch it with
    /// [`KeepClient::auth_headers`] (it is not public).
    pub fn avatar_url(&self, user_key: &str, version: u32) -> String {
        format!(
            "{}/v1/users/{}/avatar?v={version}",
            self.base,
            net::percent_encode(user_key)
        )
    }

    /// The headers every signed-in call sends (name, value): for fetching avatars.
    pub fn auth_headers(&self) -> Vec<(String, String)> {
        vec![
            ("Authorization".into(), format!("Bearer {}", self.token)),
            (SERVER_HEADER.into(), self.server_id.clone()),
        ]
    }

    pub async fn leaving_soon(&self) -> Result<Vec<LeavingItem>> {
        net::send_json(self.req(Method::GET, "/leaving-soon")).await
    }

    /// Most urgent items I have not voted on yet (swipe mode).
    pub async fn triage(&self, limit: u32) -> Result<Vec<LeavingItem>> {
        net::send_json(self.req(Method::GET, &format!("/triage?limit={limit}"))).await
    }

    pub async fn vote(&self, key: &str, vote: &VoteRequest) -> Result<LeavingItem> {
        net::send_json(
            self.req(Method::PUT, &format!("/votes/{}", net::percent_encode(key)))
                .json(vote),
        )
        .await
    }

    pub async fn unvote(&self, key: &str) -> Result<LeavingItem> {
        net::send_json(self.req(
            Method::DELETE,
            &format!("/votes/{}", net::percent_encode(key)),
        ))
        .await
    }

    pub async fn admin_users(&self) -> Result<Vec<AdminUser>> {
        net::send_json(self.req(Method::GET, "/admin/users")).await
    }

    /// Seerr's users (admins), to link AlwyWatch users to them.
    pub async fn admin_seerr_users(&self) -> Result<Vec<SeerrUser>> {
        net::send_json(self.req(Method::GET, "/admin/seerr-users")).await
    }

    /// Admins: keep-forever counts per person and their overlaps.
    pub async fn admin_forever(&self) -> Result<ForeverStats> {
        net::send_json(self.req(Method::GET, "/admin/forever")).await
    }

    /// Links a user's requests to an existing Seerr user (None: back to automatic).
    pub async fn admin_link_seerr(
        &self,
        user_key: &str,
        seerr_user_id: Option<i64>,
    ) -> Result<AdminUser> {
        net::send_json(
            self.req(
                Method::PUT,
                &format!("/admin/users/{}/seerr", net::percent_encode(user_key)),
            )
            .json(&SeerrLinkRequest { seerr_user_id }),
        )
        .await
    }

    pub async fn admin_set_rights(&self, user_key: &str, rights: &Rights) -> Result<AdminUser> {
        net::send_json(
            self.req(
                Method::PUT,
                &format!("/admin/users/{}/rights", net::percent_encode(user_key)),
            )
            .json(rights),
        )
        .await
    }

    pub async fn admin_countdown(
        &self,
        collection_id: i64,
        req: &CountdownRequest,
    ) -> Result<CountdownResult> {
        net::send_json(
            self.req(
                Method::POST,
                &format!("/admin/collections/{collection_id}/countdown"),
            )
            .json(req),
        )
        .await
    }
}

/// The Leaving soon entry of a title page's item: the same item on the caller's server, else
/// the same TMDB or TVDB id with the same kind ("movie" or "show"). Items not in the list are
/// not scheduled for deletion.
pub fn leaving_entry<'a>(
    list: &'a [LeavingItem],
    item_id: &str,
    kind: &str,
    tmdb: Option<&str>,
    tvdb: Option<&str>,
) -> Option<&'a LeavingItem> {
    let same = |a: &Option<String>, b: Option<&str>| matches!((a.as_deref(), b), (Some(x), Some(y)) if !x.is_empty() && x == y);
    list.iter()
        .find(|i| i.server_item_id.as_deref() == Some(item_id))
        .or_else(|| {
            list.iter()
                .find(|i| i.kind == kind && (same(&i.tmdb_id, tmdb) || same(&i.tvdb_id, tvdb)))
        })
}

#[cfg(test)]
mod leaving_entry_tests {
    use super::*;

    fn item(key: &str, kind: &str, server: Option<&str>, tmdb: Option<&str>) -> LeavingItem {
        LeavingItem {
            key: key.into(),
            title: key.into(),
            year: None,
            kind: kind.into(),
            poster_url: None,
            tmdb_id: tmdb.map(Into::into),
            tvdb_id: None,
            server_item_id: server.map(Into::into),
            collection_id: None,
            collection_title: None,
            collection_active: false,
            deletion_date: None,
            effective_deletion_date: None,
            protection: Protection::None,
            my_vote: None,
            counts: VoteCounts::default(),
        }
    }

    #[test]
    fn by_server_item_then_provider_id_of_the_same_kind() {
        let list = vec![
            item("a", "movie", Some("jf1"), Some("10")),
            item("b", "show", None, Some("20")),
            item("c", "movie", None, Some("20")),
        ];
        assert_eq!(
            leaving_entry(&list, "jf1", "movie", None, None)
                .unwrap()
                .key,
            "a"
        );
        assert_eq!(
            leaving_entry(&list, "x", "movie", Some("20"), None)
                .unwrap()
                .key,
            "c"
        );
        assert_eq!(
            leaving_entry(&list, "x", "show", Some("20"), None)
                .unwrap()
                .key,
            "b"
        );
        assert!(leaving_entry(&list, "x", "movie", Some(""), None).is_none());
        assert!(leaving_entry(&list, "x", "movie", None, None).is_none());
    }
}
