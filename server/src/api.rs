// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP API (`/v1`), see docs/keep-protocol.md.

use axum::body::Bytes;
use axum::extract::{FromRequestParts, Path, Query, RawQuery, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum::{Json, Router};
use keep_service::{
    ApiError, CountdownRequest, LinkRequest, LinkedIdentity, Me, MeWithIdentities, Rights,
    SERVER_HEADER, ServerEntry, ServerType, VoteRequest,
};
use serde::Deserialize;

use crate::account_api;
use crate::auth::{AuthError, Identity};
use crate::keep::{Shared, VoteError};
use crate::seerr::{BridgeError, allowed};

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/servers", get(servers))
        .route("/v1/me", get(me))
        .route("/v1/me/links", get(my_links).post(link_identity))
        .route(
            "/v1/me/links/{user}",
            axum::routing::delete(unlink_identity),
        )
        .route("/v1/me/settings", get(my_settings).put(set_my_settings))
        .route("/v1/me/name", put(set_my_name).delete(reset_my_name))
        .route("/v1/me/profile-done", post(profile_done))
        .route("/v1/me/avatar", put(set_my_avatar).delete(remove_my_avatar))
        .route("/v1/users/{user}/avatar", get(user_avatar))
        .route("/v1/leaving-soon", get(leaving_soon))
        .route("/v1/triage", get(triage))
        .route("/v1/votes/{key}", put(vote).delete(unvote))
        .route("/v1/keep-entry", post(keep_entry))
        .route("/v1/admin/users", get(admin_users))
        .route("/v1/admin/users/{user}/rights", put(admin_rights))
        .route("/v1/admin/users/{user}/seerr", put(admin_link_seerr))
        .route("/v1/admin/seerr-users", get(admin_seerr_users))
        .route("/v1/admin/forever", get(admin_forever))
        .route("/v1/admin/status", get(admin_status))
        .route("/v1/admin/dashboard/live", get(dashboard_live))
        .route("/v1/admin/dashboard/stats", get(dashboard_stats))
        .route("/v1/admin/dashboard/image", get(dashboard_image))
        .route("/v1/setup", get(crate::server_settings::setup_state))
        .route(
            "/v1/setup/jellyfin",
            post(crate::server_settings::setup_jellyfin),
        )
        .route("/v1/setup/plex", post(crate::server_settings::setup_plex))
        .route(
            "/v1/admin/settings",
            get(crate::server_settings::get_settings).put(crate::server_settings::put_settings),
        )
        .route(
            "/v1/admin/settings/test",
            post(crate::server_settings::test),
        )
        .route(
            "/v1/admin/settings/jellyfin",
            post(crate::server_settings::add_jellyfin),
        )
        .route(
            "/v1/admin/settings/plex",
            post(crate::server_settings::add_plex),
        )
        .route(
            "/v1/admin/collections/{id}/countdown",
            post(admin_countdown),
        )
        .route("/v1/seerr", get(seerr_info))
        .route("/v1/seerr/api/v1/{*path}", any(seerr_forward))
        .route("/v1/hooks/{source}/{secret}", post(hook))
        .route("/v1/together", get(together_rooms).post(together_create))
        .route("/v1/together/wait", get(together_rooms_wait))
        .route("/v1/remote/listen", get(remote_listen))
        .route("/v1/remote/devices", get(remote_devices))
        .route("/v1/remote/devices/{device}", delete(remote_leave))
        .route(
            "/v1/remote/devices/{device}/state",
            get(remote_state).put(remote_report),
        )
        .route("/v1/remote/devices/{device}/commands", post(remote_command))
        .route("/v1/together/people", get(together_people))
        .route("/v1/together/clock", get(together_clock))
        .route("/v1/together/{code}", put(together_update))
        .route("/v1/together/{code}/invites", post(together_invite))
        .route(
            "/v1/together/{code}/invites/{user}",
            axum::routing::delete(together_uninvite),
        )
        .route("/v1/together/{code}/join", post(together_join))
        .route("/v1/together/{code}/wait", get(together_wait))
        .route("/v1/together/{code}/ready", post(together_ready))
        .route(
            "/v1/together/{code}/members/{member}",
            axum::routing::delete(together_leave),
        )
        .route("/v1/transfer", post(transfer_start))
        .route("/v1/transfer/poll/{handle}", get(transfer_poll))
        .route("/v1/transfer/{code}", get(transfer_peer).put(transfer_send))
        .route(
            "/v1/transfer/{code}/approve",
            put(account_api::quick_approve),
        )
        .route("/v1/account", get(account_api::me))
        .route("/v1/account/options", get(account_api::options))
        .route("/v1/account/register", post(account_api::register))
        .route("/v1/account/login", post(account_api::login))
        .route("/v1/account/logout", post(account_api::logout))
        .route("/v1/account/delete", post(account_api::delete_account))
        .route("/v1/account/claim", post(account_api::claim))
        .route("/v1/account/session", post(account_api::session_from_media))
        .route("/v1/account/password", put(account_api::set_password))
        .route(
            "/v1/account/password/forgot",
            post(account_api::forgot_password),
        )
        .route(
            "/v1/account/password/reset",
            post(account_api::reset_password),
        )
        .route("/v1/account/email", put(account_api::set_email))
        .route("/v1/account/username", put(account_api::set_username))
        .route(
            "/v1/account/media-sign-in",
            put(account_api::set_media_sign_in),
        )
        .route(
            "/v1/account/sessions/{id}",
            delete(account_api::end_session),
        )
        .route("/v1/account/link", post(account_api::link_media))
        .route(
            "/v1/account/server-sign-in",
            post(crate::server_sign_in::server_sign_in),
        )
        .route(
            "/v1/account/invites",
            post(crate::invites::create).get(crate::invites::list),
        )
        .route("/v1/account/invites/{id}", delete(crate::invites::revoke))
        .route(
            "/v1/account/invites/check/{code}",
            get(crate::invites::check),
        )
        .route(
            "/v1/account/passkeys/register/start",
            post(account_api::passkey_register_start),
        )
        .route(
            "/v1/account/passkeys/register/finish",
            post(account_api::passkey_register_finish),
        )
        .route(
            "/v1/account/passkeys/login/start",
            post(account_api::passkey_login_start),
        )
        .route(
            "/v1/account/passkeys/login/finish",
            post(account_api::passkey_login_finish),
        )
        .route(
            "/v1/account/passkeys/{id}",
            put(account_api::passkey_rename).delete(account_api::passkey_delete),
        )
        .route("/v1/account/oidc", delete(account_api::oidc_unlink))
        .route("/v1/account/oidc/start", get(account_api::oidc_start))
        .route("/v1/account/oidc/ticket", post(account_api::oidc_ticket))
        .route("/v1/account/oidc/link", post(account_api::oidc_link))
        .route("/v1/account/oidc/callback", get(account_api::oidc_callback))
        .fallback(crate::web::serve)
        .with_state(state)
}

pub struct Error(pub StatusCode, pub String);

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(ApiError { error: self.1 })).into_response()
    }
}

/// Details go to the log, not to the client (they can name internal hosts).
fn internal(e: impl ToString) -> Error {
    eprintln!("[api] internal error: {}", e.to_string());
    Error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "server error, try again later".into(),
    )
}

fn too_many() -> Error {
    Error(
        StatusCode::TOO_MANY_REQUESTS,
        "too many attempts, try again in a few minutes".into(),
    )
}

/// The authenticated caller (Bearer token + X-AlwyWatch-Server).
pub struct Caller(pub Identity);

impl FromRequestParts<Shared> for Caller {
    type Rejection = Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Shared,
    ) -> Result<Self, Self::Rejection> {
        let h = &parts.headers;
        let token = h
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|t| !t.is_empty())
            .ok_or(Error(
                StatusCode::UNAUTHORIZED,
                "missing bearer token".into(),
            ))?;
        // An AlwyWatch session is never sent on to a media server (or plex.tv).
        if token.starts_with(crate::accounts::SESSION_PREFIX) {
            return Err(Error(
                StatusCode::UNAUTHORIZED,
                "invalid token or no access to this server".into(),
            ));
        }
        let server_id = h
            .get(SERVER_HEADER)
            .and_then(|v| v.to_str().ok())
            .ok_or(Error(
                StatusCode::UNAUTHORIZED,
                format!("missing {SERVER_HEADER}"),
            ))?;
        let server = state.config.server(server_id).ok_or(Error(
            StatusCode::FORBIDDEN,
            "this server is not served here".into(),
        ))?;
        // Too many rejected tokens from this address: refuse before asking the media server.
        let ip = crate::limit::client(h);
        if state.limits.failed_auth.blocked(&ip) {
            return Err(too_many());
        }
        let id = state
            .auth
            .identify(server, token)
            .await
            .map_err(|e| match e {
                AuthError::Denied => {
                    state.limits.failed_auth.hit(&ip);
                    Error(
                        StatusCode::UNAUTHORIZED,
                        "invalid token or no access to this server".into(),
                    )
                }
                AuthError::Unavailable => {
                    Error(StatusCode::BAD_GATEWAY, "media server unreachable".into())
                }
            })?;
        resolve(state, id).map(Caller)
    }
}

/// A stored admin flag of a linked sign-in counts this long after that sign-in was last used
/// (the media server decides again at its next sign-in).
const LINKED_ADMIN_FRESH: i64 = 30 * 86_400;

fn sign_in_of(id: &Identity) -> crate::db::SignIn<'_> {
    crate::db::SignIn {
        user_key: &id.signed_in_key,
        server_id: &id.server_id,
        kind: id.kind,
        user_id: &id.user_id,
        media_name: &id.media_name,
        is_admin: id.is_admin,
    }
}

/// The resolution point: a validated sign-in becomes its person (canonical user key, linked
/// identities, admin flag, profile). Everything after this sees one user per person.
fn resolve(state: &Shared, mut id: Identity) -> Result<Identity, Error> {
    let (person, ids) = state.db.sign_in(&sign_in_of(&id)).map_err(internal)?;
    let now = crate::db::now();
    // Admin of AlwyWatch only (never of a media server): this sign-in's own rule, or a linked
    // sign-in's last known one on a server still served.
    id.is_admin = id.is_admin
        || ids.iter().any(|i| {
            i.user_key != id.signed_in_key
                && i.is_admin
                && now - i.last_seen < LINKED_ADMIN_FRESH
                && state.config.server(&i.server_id).is_some()
        });
    id.user_key = person;
    id.identities = ids;
    // The media server's name is stored as such; others see the chosen one if any.
    if let Ok(p) = state
        .db
        .touch_person(&id.user_key, &sign_in_of(&id), id.is_admin)
    {
        id.name = p.name;
        id.avatar = p.avatar;
    }
    Ok(id)
}

pub struct Admin(pub Identity);

impl FromRequestParts<Shared> for Admin {
    type Rejection = Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Shared,
    ) -> Result<Self, Self::Rejection> {
        let Caller(id) = Caller::from_request_parts(parts, state).await?;
        if !id.is_admin {
            return Err(Error(StatusCode::FORBIDDEN, "administrators only".into()));
        }
        Ok(Admin(id))
    }
}

/// An admin whose own sign-in, right now, is a media server's administrator or owner (not
/// through a linked sign-in): the server settings, with every key in them, need this.
pub struct ServerAdmin(pub Identity);

impl FromRequestParts<Shared> for ServerAdmin {
    type Rejection = Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Shared,
    ) -> Result<Self, Self::Rejection> {
        let Admin(id) = Admin::from_request_parts(parts, state).await?;
        if !id
            .identities
            .iter()
            .any(|i| i.user_key == id.signed_in_key && i.is_admin)
        {
            return Err(Error(
                StatusCode::FORBIDDEN,
                "sign in with a Jellyfin administrator or the Plex owner's login for server settings"
                    .into(),
            ));
        }
        Ok(ServerAdmin(id))
    }
}

// ----- watch together -------------------------------------------------------------------------

fn together_error(e: crate::together::TogetherError) -> Error {
    use crate::together::TogetherError as E;
    match e {
        E::NotFound => Error(StatusCode::NOT_FOUND, "unknown or closed room".into()),
        E::Busy => Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many rooms open, try again later".into(),
        ),
        E::Full => Error(StatusCode::CONFLICT, "this room is full".into()),
        E::Forbidden => Error(
            StatusCode::FORBIDDEN,
            "only the host can do this, or not your seat".into(),
        ),
        // Not 403: clients read 401/403 as a lost sign-in.
        E::NotAllowed => Error(
            StatusCode::LOCKED,
            "the host doesn't let others do this in this room".into(),
        ),
        E::NoTitle => Error(StatusCode::CONFLICT, "choose what to watch first".into()),
    }
}

async fn together_create(
    State(s): State<Shared>,
    Caller(id): Caller,
    Json(c): Json<keep_service::together::TogetherCreate>,
) -> Result<Json<keep_service::together::TogetherJoined>, Error> {
    if !s.limits.together_creates.allow(&id.user_key) {
        return Err(too_many());
    }
    s.rooms
        .create(c, &id.name, &id.user_key, &id.avatar)
        .map(Json)
        .map_err(together_error)
}

// ----- Play on the TV (remote.rs) ---------------------------------------------------------

fn remote_error(e: crate::remote::RemoteError) -> Error {
    match e {
        crate::remote::RemoteError::NotFound => Error(
            StatusCode::NOT_FOUND,
            "this TV is not there any more".into(),
        ),
        crate::remote::RemoteError::Busy => Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many TVs, try again later".into(),
        ),
    }
}

#[derive(Deserialize)]
struct RemoteListenQuery {
    device: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    after: u64,
}

async fn remote_listen(
    State(s): State<Shared>,
    Caller(id): Caller,
    Query(q): Query<RemoteListenQuery>,
) -> Result<Json<keep_service::remote::RemoteCommands>, Error> {
    if q.device.trim().is_empty() {
        return Err(Error(StatusCode::BAD_REQUEST, "device is required".into()));
    }
    s.remotes
        .listen(&id.user_key, &q.device, &q.name, q.after)
        .await
        .map(Json)
        .map_err(remote_error)
}

async fn remote_report(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(device): Path<String>,
    Json(r): Json<keep_service::remote::RemoteReport>,
) -> Result<StatusCode, Error> {
    s.remotes
        .report(&id.user_key, &device, r)
        .map_err(remote_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remote_leave(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(device): Path<String>,
) -> StatusCode {
    s.remotes.leave(&id.user_key, &device);
    StatusCode::NO_CONTENT
}

async fn remote_devices(
    State(s): State<Shared>,
    Caller(id): Caller,
) -> Json<Vec<keep_service::remote::RemoteDevice>> {
    Json(s.remotes.devices(&id.user_key))
}

async fn remote_command(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(device): Path<String>,
    Json(c): Json<keep_service::remote::RemoteCommand>,
) -> Result<StatusCode, Error> {
    s.remotes
        .command(&id.user_key, &device, c)
        .map_err(remote_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remote_state(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(device): Path<String>,
    Query(q): Query<ListWaitQuery>,
) -> Result<Json<keep_service::remote::RemoteState>, Error> {
    s.remotes
        .state(&id.user_key, &device, q.after)
        .await
        .map(Json)
        .map_err(remote_error)
}

async fn together_rooms(
    State(s): State<Shared>,
    Caller(id): Caller,
) -> Json<keep_service::together::TogetherRooms> {
    Json(s.rooms.list(&id.user_key))
}

#[derive(Deserialize)]
struct ListWaitQuery {
    #[serde(default)]
    after: u64,
}

async fn together_rooms_wait(
    State(s): State<Shared>,
    Caller(id): Caller,
    Query(q): Query<ListWaitQuery>,
) -> Json<keep_service::together::TogetherRooms> {
    Json(s.rooms.list_wait(&id.user_key, q.after).await)
}

/// Everyone who signed in to this AlwyWatch server (on a server still served), but the caller:
/// recent watch-together partners first, then by name.
async fn together_people(
    State(s): State<Shared>,
    Caller(id): Caller,
) -> Result<Json<Vec<keep_service::together::TogetherPerson>>, Error> {
    let identities = s.db.all_identities().map_err(internal)?;
    let mut people: Vec<keep_service::together::TogetherPerson> =
        s.db.together_people(&id.user_key)
            .map_err(internal)?
            .into_iter()
            .filter_map(|(user_key, server_id, last, p)| {
                // A person shows once, named after their canonical server if still served,
                // else another served sign-in; hidden when none is served.
                let server = s.config.server(&server_id).or_else(|| {
                    identities
                        .get(&user_key)?
                        .iter()
                        .find_map(|i| s.config.server(&i.server_id))
                })?;
                Some(keep_service::together::TogetherPerson {
                    user_key,
                    name: crate::together::clean_name(&p.name),
                    media_name: crate::together::clean_name(&p.media_name),
                    avatar: p.avatar,
                    server_name: server.name.clone(),
                    last_together_at: last,
                })
            })
            .collect();
    people.sort_by(|a, b| {
        let key = |p: &keep_service::together::TogetherPerson| {
            // None sorts below any time, so Reverse puts the never-met last.
            (
                std::cmp::Reverse(p.last_together_at),
                p.name.to_lowercase(),
                p.user_key.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    Ok(Json(people))
}

/// Users invited at once at most.
const MAX_INVITES_AT_ONCE: usize = 20;

async fn together_invite(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
    Json(inv): Json<keep_service::together::TogetherInvite>,
) -> Result<Json<keep_service::together::TogetherState>, Error> {
    if inv.user_keys.is_empty() || inv.user_keys.len() > MAX_INVITES_AT_ONCE {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            format!("invite 1 to {MAX_INVITES_AT_ONCE} people at a time"),
        ));
    }
    for _ in &inv.user_keys {
        if !s.limits.together_invites.allow(&id.user_key) {
            return Err(too_many());
        }
    }
    let mut people = Vec::with_capacity(inv.user_keys.len());
    for key in &inv.user_keys {
        let unknown = || Error(StatusCode::NOT_FOUND, "unknown user".into());
        // An alias key (a list fetched before a link) invites the person.
        let person = served_person(&s, key)?.ok_or_else(unknown)?;
        let (p, _) =
            s.db.user_profile(&person)
                .map_err(internal)?
                .ok_or_else(unknown)?;
        people.push(keep_service::together::TogetherInvitee {
            user_key: person,
            name: p.name,
            avatar: p.avatar,
        });
    }
    s.rooms
        .invite(&code, &inv.member_id, &id.user_key, &people)
        .map(Json)
        .map_err(together_error)
}

#[derive(Deserialize)]
struct MemberQuery {
    member: Option<String>,
}

/// `user` = "me": the caller declines their invitation (204). Otherwise the host withdraws
/// that user's invitation (`?member=` one of the host's member ids) and gets the room back.
async fn together_uninvite(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path((code, user)): Path<(String, String)>,
    Query(q): Query<MemberQuery>,
) -> Result<Response, Error> {
    if user == "me" {
        s.rooms
            .decline(&code, &id.user_key)
            .map_err(together_error)?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let member = q.member.unwrap_or_default();
    let user = s.db.person_of(&user).map_err(internal)?;
    s.rooms
        .uninvite(&code, &member, &id.user_key, &user)
        .map(|st| Json(st).into_response())
        .map_err(together_error)
}

/// Notes who just watched together (the people list's order); never fails the call.
fn record_met(s: &Shared, met: &crate::together::Met) {
    if !met.is_empty()
        && let Err(e) = s.db.record_together(met, crate::db::now())
    {
        eprintln!("[together] could not record history: {e}");
    }
}

async fn together_join(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
) -> Result<Json<keep_service::together::TogetherJoined>, Error> {
    let (joined, met) = s
        .rooms
        .join(&code, &id.name, &id.user_key, &id.avatar)
        .map_err(together_error)?;
    record_met(&s, &met);
    Ok(Json(joined))
}

async fn together_update(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
    Json(u): Json<keep_service::together::TogetherUpdate>,
) -> Result<Json<keep_service::together::TogetherState>, Error> {
    let (state, met) = s
        .rooms
        .update(&code, u, &id.user_key)
        .map_err(together_error)?;
    record_met(&s, &met);
    Ok(Json(state))
}

/// The server clock, for the apps' clock sync (no sign-in: it says nothing but the time).
async fn together_clock() -> Json<keep_service::together::TogetherClock> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Json(keep_service::together::TogetherClock { now_ms })
}

async fn together_ready(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
    Json(r): Json<keep_service::together::TogetherReady>,
) -> Result<Json<keep_service::together::TogetherState>, Error> {
    s.rooms
        .ready(&code, &r.member_id, &id.user_key, r.round)
        .map(Json)
        .map_err(together_error)
}

#[derive(Deserialize)]
struct WaitQuery {
    after: u64,
    member: String,
}

async fn together_wait(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
    Query(q): Query<WaitQuery>,
) -> Result<Json<keep_service::together::TogetherState>, Error> {
    s.rooms
        .wait(&code, &q.member, &id.user_key, q.after)
        .await
        .map(Json)
        .map_err(together_error)
}

async fn together_leave(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path((code, member)): Path<(String, String)>,
) -> Result<StatusCode, Error> {
    s.rooms
        .leave(&code, &member, &id.user_key)
        .map_err(together_error)?;
    Ok(StatusCode::NO_CONTENT)
}

// ----- device-to-device transfer ----------------------------------------------------------------

pub(crate) fn transfer_error(e: crate::transfer::TransferError) -> Error {
    use crate::transfer::TransferError as E;
    match e {
        E::Invalid(m) => Error(StatusCode::BAD_REQUEST, m.into()),
        E::Busy => Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many transfers in progress, try again later".into(),
        ),
        E::NotFound => Error(StatusCode::NOT_FOUND, "unknown or expired code".into()),
        E::AlreadySent => Error(StatusCode::CONFLICT, "this code was already used".into()),
    }
}

async fn transfer_start(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(start): Json<keep_service::transfer::TransferStart>,
) -> Result<Json<keep_service::transfer::TransferStarted>, Error> {
    // No sign-in here: a few per address, so nobody can fill the pending slots.
    if !s
        .limits
        .transfer_starts
        .allow(&crate::limit::client(&headers))
    {
        return Err(too_many());
    }
    s.transfers.start(start).map(Json).map_err(transfer_error)
}

async fn transfer_poll(State(s): State<Shared>, Path(handle): Path<String>) -> Response {
    match s.transfers.poll(&handle) {
        crate::transfer::Poll::Waiting => StatusCode::ACCEPTED.into_response(),
        crate::transfer::Poll::Ready(b) => Json(b).into_response(),
        crate::transfer::Poll::Gone => {
            transfer_error(crate::transfer::TransferError::NotFound).into_response()
        }
    }
}

/// Signed in: only users of a served media server can approve transfers.
async fn transfer_peer(
    State(s): State<Shared>,
    id: crate::account_api::Person,
    Path(code): Path<String>,
) -> Result<Json<keep_service::transfer::TransferPeer>, Error> {
    // Every send starts by looking the code up here: a handful of tries per user is plenty,
    // and stops someone from guessing (and spoiling) another person's pending transfer.
    if !s.limits.transfer_codes.allow(&id.user_key) {
        return Err(too_many());
    }
    s.transfers.peer(&code).map(Json).map_err(transfer_error)
}

async fn transfer_send(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
    Json(sealed): Json<keep_service::transfer::TransferSealed>,
) -> Result<StatusCode, Error> {
    if !s.limits.transfer_codes.allow(&id.user_key) {
        return Err(too_many());
    }
    // The new device also joins this person's AlwyWatch account (settings, Quick Connect).
    let peer = s.transfers.peer(code.trim()).map_err(transfer_error)?;
    let person = crate::account_api::Person {
        account: s.db.account_of_person(&id.user_key).map_err(internal)?,
        user_key: id.user_key.clone(),
        media: None,
    };
    // Unless they turned that off: then the media sign-ins move, the account does not.
    let grant = if person.account.as_ref().is_some_and(|a| a.media_locked) {
        None
    } else {
        Some(crate::account_api::grant_for(&s, &person, &peer.device)?)
    };
    s.transfers
        .send(code.trim(), Some(sealed.sealed), grant)
        .map_err(transfer_error)?;
    eprintln!("[transfer] {} sent a device transfer", id.user_key);
    Ok(StatusCode::NO_CONTENT)
}

async fn servers(State(s): State<Shared>) -> Json<Vec<ServerEntry>> {
    Json(
        s.config
            .servers
            .iter()
            .map(|c| ServerEntry {
                id: c.id.clone(),
                kind: c.kind,
                name: c.name.clone(),
                url: c.public_url.clone(),
            })
            .collect(),
    )
}

async fn me(State(s): State<Shared>, Caller(id): Caller) -> Result<Json<MeWithIdentities>, Error> {
    me_of(&s, id).map(Json)
}

/// The caller's `Me` (with their linked sign-ins), read again from the database (after a
/// profile change).
/// After a profile change: the media sign-in's profile, or nothing for an account session.
fn profile_answer(s: &Shared, p: crate::account_api::Person) -> Result<Response, Error> {
    match p.media {
        Some(id) => Ok(Json(me_of(s, id)?).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

fn me_of(s: &Shared, id: Identity) -> Result<MeWithIdentities, Error> {
    let p =
        s.db.profile(&id.user_key)
            .map_err(internal)?
            .unwrap_or_default();
    let identities = links_view(s, &id.identities, &id.signed_in_key);
    Ok(MeWithIdentities {
        me: Me {
            rights: s.rights_of(&id),
            forever_count: s.db.forever_count(&id.user_key).map_err(internal)?,
            name: if p.name.is_empty() { id.name } else { p.name },
            media_name: if p.media_name.is_empty() {
                id.media_name
            } else {
                p.media_name
            },
            name_is_custom: p.name_is_custom,
            avatar: p.avatar,
            profile_set: p.profile_set,
            services: keep_service::Services {
                // Until the first sync has run, assume it works (no flicker at start).
                leaving_soon: s
                    .maintainerr_state
                    .load(std::sync::atomic::Ordering::Relaxed)
                    != 2,
                requests: s.seerr.is_some(),
            },
            user_key: id.user_key,
            server_id: id.server_id,
            is_admin: id.is_admin,
        },
        identities,
    })
}

// ----- linked identities ------------------------------------------------------------------------

fn server_name(s: &Shared, server_id: &str) -> String {
    s.config
        .server(server_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| server_id.to_owned())
}

fn links_view(s: &Shared, ids: &[crate::db::IdentityRow], current: &str) -> Vec<LinkedIdentity> {
    let mut out: Vec<LinkedIdentity> = ids
        .iter()
        .map(|i| LinkedIdentity {
            user_key: i.user_key.clone(),
            server_name: server_name(s, &i.server_id),
            server_id: i.server_id.clone(),
            kind: i.kind,
            media_name: crate::together::clean_name(&i.media_name),
            canonical: i.user_key == i.person,
            current: i.user_key == current,
        })
        .collect();
    // Canonical first, then oldest link first (as stored).
    out.sort_by_key(|i| !i.canonical);
    out
}

async fn my_links(State(s): State<Shared>, Caller(id): Caller) -> Json<Vec<LinkedIdentity>> {
    Json(links_view(&s, &id.identities, &id.signed_in_key))
}

/// Proves a second sign-in of the same person (its token is validated like any sign-in,
/// never stored nor logged) and makes both one AlwyWatch account.
async fn link_identity(
    State(s): State<Shared>,
    headers: HeaderMap,
    Caller(id): Caller,
    Json(r): Json<LinkRequest>,
) -> Result<Json<Vec<LinkedIdentity>>, Error> {
    if !s.limits.links.allow(&id.user_key) {
        return Err(too_many());
    }
    let server = s.config.server(&r.server_id).ok_or(Error(
        StatusCode::FORBIDDEN,
        "this server is not served here".into(),
    ))?;
    let ip = crate::limit::client(&headers);
    if s.limits.failed_auth.blocked(&ip) {
        return Err(too_many());
    }
    let refused = || {
        Error(
            StatusCode::UNAUTHORIZED,
            "the other sign-in was refused (wrong or expired token, or no access to that server)"
                .into(),
        )
    };
    if r.token.is_empty() {
        return Err(refused());
    }
    let other = s
        .auth
        .identify(server, &r.token)
        .await
        .map_err(|e| match e {
            AuthError::Denied => {
                s.limits.failed_auth.hit(&ip);
                refused()
            }
            AuthError::Unavailable => {
                Error(StatusCode::BAD_GATEWAY, "media server unreachable".into())
            }
        })?;
    let canonical = link_person(&s, &id.user_key, &other).await?;
    let ids = s.db.identities_of(&canonical).map_err(internal)?;
    Ok(Json(links_view(&s, &ids, &id.signed_in_key)))
}

/// Merges a validated sign-in (`other`) into `person` (an identity link, or an AlwyWatch
/// account adding a media sign-in): the canonical person key.
pub(crate) async fn link_person(
    s: &Shared,
    person: &str,
    other: &Identity,
) -> Result<String, Error> {
    // Jellyfin (open source, the server everyone moves to) holds the data when it can, so
    // stopping Plex later never strands an account.
    let prefer = |i: &crate::db::IdentityRow| {
        i.kind == ServerType::Jellyfin && s.config.server(&i.server_id).is_some()
    };
    let outcome =
        s.db.link(person, &sign_in_of(other), &prefer)
            .map_err(|e| match e {
                crate::db::LinkError::SameServer(server) => Error(
                    StatusCode::CONFLICT,
                    format!(
                        "your AlwyWatch account already has another sign-in on {}; unlink it first",
                        server_name(s, &server)
                    ),
                ),
                crate::db::LinkError::Db(e) => internal(e),
            })?;
    if !outcome.merged.is_empty() {
        if let Some(from) = &outcome.avatar_from {
            let to = avatar_path(s, &outcome.canonical);
            if let Err(e) = tokio::fs::rename(avatar_path(s, from), &to).await {
                eprintln!("[link] could not move a custom avatar: {e}");
            }
        }
        for old in &outcome.merged {
            forget_avatar_file(s, old).await;
        }
        s.rooms.rekey(&outcome.merged, &outcome.canonical);
        s.remotes.rekey(&outcome.merged, &outcome.canonical);
        s.db.log(
            &outcome.canonical,
            "link",
            &format!(
                "{person} + {} (merged {})",
                other.signed_in_key,
                outcome.merged.join(", ")
            ),
        );
        // Votes may have merged: re-apply.
        s.reconcile.notify_one();
    }
    Ok(outcome.canonical)
}

/// Separates one sign-in from the caller's account: it starts over as its own empty account.
async fn unlink_identity(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(user): Path<String>,
) -> Result<Json<Vec<LinkedIdentity>>, Error> {
    use crate::db::UnlinkError as E;
    s.db.unlink(&id.user_key, &user).map_err(|e| match e {
        E::NotFound => Error(StatusCode::NOT_FOUND, "not one of your sign-ins".into()),
        E::Canonical => Error(
            StatusCode::CONFLICT,
            "this sign-in holds your AlwyWatch account's data and cannot be unlinked; unlink the other sign-ins instead"
                .into(),
        ),
        E::Db(e) => internal(e),
    })?;
    s.db.log(&id.user_key, "unlink", &user);
    let ids = s.db.identities_of(&id.user_key).map_err(internal)?;
    Ok(Json(links_view(&s, &ids, &id.signed_in_key)))
}

/// The person behind a user key (an alias resolves to its canonical key) if they can be
/// reached: one of their sign-ins is on a server still served.
fn served_person(s: &Shared, key: &str) -> Result<Option<String>, Error> {
    let person = s.db.person_of(key).map_err(internal)?;
    let ids = s.db.identities_of(&person).map_err(internal)?;
    let served = if ids.is_empty() {
        s.db.user_profile(&person)
            .map_err(internal)?
            .is_some_and(|(_, server)| s.config.server(&server).is_some())
    } else {
        ids.iter().any(|i| s.config.server(&i.server_id).is_some())
    };
    Ok(served.then_some(person))
}

// ----- profile: display name and avatar ---------------------------------------------------------

async fn set_my_name(
    State(s): State<Shared>,
    id: crate::account_api::Person,
    Json(r): Json<keep_service::DisplayNameRequest>,
) -> Result<Response, Error> {
    let name = keep_service::profile::clean_display_name(&r.name)
        .map_err(|m| Error(StatusCode::BAD_REQUEST, m))?;
    if !s.limits.profile_changes.allow(&id.user_key) {
        return Err(too_many());
    }
    s.db.set_custom_name(&id.user_key, Some(&name))
        .map_err(internal)?;
    profile_answer(&s, id)
}

/// The one-time welcome step (name + avatar) was done or skipped, for this account on every
/// device. Idempotent; names and avatars set on their own do not count.
async fn profile_done(
    State(s): State<Shared>,
    id: crate::account_api::Person,
) -> Result<Response, Error> {
    s.db.mark_profile_set(&id.user_key).map_err(internal)?;
    profile_answer(&s, id)
}

async fn reset_my_name(
    State(s): State<Shared>,
    id: crate::account_api::Person,
) -> Result<Response, Error> {
    if !s.limits.profile_changes.allow(&id.user_key) {
        return Err(too_many());
    }
    s.db.set_custom_name(&id.user_key, None).map_err(internal)?;
    profile_answer(&s, id)
}

/// Where a user's custom avatar lives: under the data dir, named by a hash of the user key
/// (no user text in paths).
fn avatar_path(s: &Shared, user_key: &str) -> std::path::PathBuf {
    let d = ring::digest::digest(&ring::digest::SHA256, user_key.as_bytes());
    let name: String = d.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    s.config.data_dir.join("avatars").join(name)
}

pub(crate) async fn forget_avatar_file(s: &Shared, user_key: &str) {
    let _ = tokio::fs::remove_file(avatar_path(s, user_key)).await;
}

/// JSON `{preset}` picks a built-in avatar; a raw JPEG/PNG/WebP body uploads a custom one.
async fn set_my_avatar(
    State(s): State<Shared>,
    id: crate::account_api::Person,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, Error> {
    let bad = |m: &str| Error(StatusCode::BAD_REQUEST, m.into());
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let picture = keep_service::profile::AVATAR_TYPES.contains(&ct.as_str());
    if ct != "application/json" && !picture {
        return Err(bad(
            "send a JPEG, PNG or WebP picture, or a built-in avatar",
        ));
    }
    if !s.limits.profile_changes.allow(&id.user_key) {
        return Err(too_many());
    }
    if !picture {
        let bytes = axum::body::to_bytes(body, 4096)
            .await
            .map_err(|_| bad("invalid request"))?;
        let choice: keep_service::AvatarChoice =
            serde_json::from_slice(&bytes).map_err(|_| bad("invalid request"))?;
        if !keep_service::profile::is_avatar_preset(&choice.preset) {
            return Err(bad("unknown built-in avatar"));
        }
        s.db.set_avatar(&id.user_key, Some(&choice.preset), None)
            .map_err(internal)?;
        forget_avatar_file(&s, &id.user_key).await;
        return profile_answer(&s, id);
    }
    // Read at most the limit: a bigger body is cut off, not buffered.
    let bytes = axum::body::to_bytes(body, keep_service::profile::AVATAR_MAX_BYTES)
        .await
        .map_err(|_| {
            Error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the picture is too large (512 KiB at most)".into(),
            )
        })?;
    if !keep_service::profile::avatar_bytes_match(&ct, &bytes) {
        return Err(bad("this file is not the picture type it claims to be"));
    }
    let path = avatar_path(&s, &id.user_key);
    let tmp = path.with_extension("part");
    let write = async {
        tokio::fs::create_dir_all(path.parent().expect("has a parent")).await?;
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, &path).await
    };
    write.await.map_err(internal)?;
    s.db.set_avatar(&id.user_key, None, Some(&ct))
        .map_err(internal)?;
    profile_answer(&s, id)
}

async fn remove_my_avatar(
    State(s): State<Shared>,
    id: crate::account_api::Person,
) -> Result<Response, Error> {
    if !s.limits.profile_changes.allow(&id.user_key) {
        return Err(too_many());
    }
    s.db.set_avatar(&id.user_key, None, None)
        .map_err(internal)?;
    forget_avatar_file(&s, &id.user_key).await;
    profile_answer(&s, id)
}

/// A user's custom avatar, to signed-in users. The version is in the URL, so it is cached for
/// good; the bytes only ever go out as the type they were checked to be.
async fn user_avatar(
    State(s): State<Shared>,
    _who: crate::account_api::Person,
    Path(user): Path<String>,
) -> Result<Response, Error> {
    let none = || Error(StatusCode::NOT_FOUND, "no picture".into());
    let user = s.db.person_of(&user).map_err(internal)?;
    let (ct, _) =
        s.db.avatar_custom(&user)
            .map_err(internal)?
            .ok_or_else(none)?;
    if !keep_service::profile::AVATAR_TYPES.contains(&ct.as_str()) {
        return Err(none());
    }
    let bytes = tokio::fs::read(avatar_path(&s, &user))
        .await
        .map_err(|_| none())?;
    use axum::http::header;
    Ok((
        [
            (header::CONTENT_TYPE, ct),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".into(),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'".into()),
        ],
        bytes,
    )
        .into_response())
}

/// Limits of the settings a user stores (they are small: languages, choices per title).
const MAX_SETTINGS: u32 = 20_000;
const MAX_SETTING_KEY: usize = 256;
const MAX_SETTING_VALUE: usize = 4096;

async fn my_settings(
    State(s): State<Shared>,
    id: crate::account_api::Person,
) -> Result<Json<std::collections::HashMap<String, String>>, Error> {
    Ok(Json(
        s.db.settings(&id.user_key)
            .map_err(internal)?
            .into_iter()
            .collect(),
    ))
}

async fn set_my_settings(
    State(s): State<Shared>,
    id: crate::account_api::Person,
    Json(values): Json<std::collections::HashMap<String, String>>,
) -> Result<StatusCode, Error> {
    let bad = |m: &str| Error(StatusCode::BAD_REQUEST, m.into());
    if values
        .iter()
        .any(|(k, v)| k.is_empty() || k.len() > MAX_SETTING_KEY || v.len() > MAX_SETTING_VALUE)
    {
        return Err(bad("setting too long"));
    }
    let have = s.db.settings_count(&id.user_key).map_err(internal)?;
    if have + values.len() as u32 > MAX_SETTINGS {
        return Err(bad("too many settings"));
    }
    let values: Vec<(String, String)> = values.into_iter().collect();
    s.db.set_settings(&id.user_key, &values).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn leaving_soon(
    State(s): State<Shared>,
    Caller(id): Caller,
) -> Result<impl IntoResponse, Error> {
    Ok(Json(s.leaving_soon(&id).await.map_err(internal)?))
}

#[derive(Deserialize)]
struct Limit {
    limit: Option<usize>,
}

async fn triage(
    State(s): State<Shared>,
    Caller(id): Caller,
    Query(q): Query<Limit>,
) -> Result<impl IntoResponse, Error> {
    let list: Vec<_> = s
        .leaving_soon(&id)
        .await
        .map_err(internal)?
        .into_iter()
        .filter(keep_service::needs_review)
        .take(q.limit.unwrap_or(20).min(200))
        .collect();
    Ok(Json(list))
}

fn vote_err(e: VoteError) -> Error {
    match e {
        VoteError::NotFound => Error(StatusCode::NOT_FOUND, "no such item".into()),
        VoteError::Forbidden(m) => Error(StatusCode::FORBIDDEN, m),
        VoteError::Invalid(m) => Error(StatusCode::BAD_REQUEST, m),
    }
}

async fn vote(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(key): Path<String>,
    Json(req): Json<VoteRequest>,
) -> Result<impl IntoResponse, Error> {
    s.vote(&id, &key, &req).map_err(vote_err)?;
    Ok(Json(s.item_for(&id, &key).await.map_err(internal)?))
}

async fn keep_entry(
    State(s): State<Shared>,
    Caller(id): Caller,
    Json(req): Json<keep_service::TitleLookup>,
) -> Result<impl IntoResponse, Error> {
    Ok(Json(s.keep_entry(&id, &req).await.map_err(
        |e| match e {
            VoteError::NotFound => Error(
                StatusCode::NOT_FOUND,
                "this title isn't in the library that deletions are managed for".into(),
            ),
            other => vote_err(other),
        },
    )?))
}

async fn unvote(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(key): Path<String>,
) -> Result<impl IntoResponse, Error> {
    s.unvote(&id, &key).map_err(vote_err)?;
    Ok(Json(s.item_for(&id, &key).await.map_err(|_| {
        Error(StatusCode::NOT_FOUND, "no such item".into())
    })?))
}

async fn admin_users(State(s): State<Shared>, Admin(_): Admin) -> Result<impl IntoResponse, Error> {
    Ok(Json(s.db.users().map_err(internal)?))
}

#[derive(Deserialize)]
struct DashQuery {
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    days: Option<u32>,
    /// The viewer's offset from UTC in minutes (east positive), for the days of the history.
    #[serde(default)]
    tz: Option<i32>,
    #[serde(default)]
    img: Option<String>,
}

/// `GET /v1/admin/dashboard/live?server=`: what plays now and the last minutes' graphs.
async fn dashboard_live(
    State(s): State<Shared>,
    Admin(_): Admin,
    Query(q): Query<DashQuery>,
) -> Json<keep_service::dashboard::Live> {
    Json(s.dash_live(q.server.as_deref().unwrap_or("all")).await)
}

/// `GET /v1/admin/dashboard/stats?server=&days=&tz=`: plays, top people and titles.
async fn dashboard_stats(
    State(s): State<Shared>,
    Admin(_): Admin,
    Query(q): Query<DashQuery>,
) -> Json<keep_service::dashboard::Stats> {
    let tz = q.tz.unwrap_or(0).clamp(-14 * 60, 14 * 60);
    Json(
        s.dash_stats(
            q.server.as_deref().unwrap_or("all"),
            q.days.unwrap_or(30),
            tz,
        )
        .await,
    )
}

/// `GET /v1/admin/dashboard/image?server=&img=`: a poster or avatar the dashboard shows.
async fn dashboard_image(
    State(s): State<Shared>,
    Admin(_): Admin,
    Query(q): Query<DashQuery>,
) -> Response {
    let (Some(server), Some(img)) = (q.server, q.img) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match s.dash_image(&server, &img).await {
        Some((mime, bytes)) => (
            [
                (axum::http::header::CONTENT_TYPE, mime),
                (
                    axum::http::header::CACHE_CONTROL,
                    "private, max-age=3600".into(),
                ),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /v1/admin/status`: what this server is connected to, for the apps' admin page.
async fn admin_status(
    State(s): State<Shared>,
    Admin(_): Admin,
) -> Json<keep_service::ServerStatus> {
    use std::sync::atomic::Ordering::Relaxed;
    let c = &s.config;
    let (maintainerr, version) = if !c.keep_on() {
        ("off", None)
    } else {
        match s.maintainerr_state.load(Relaxed) {
            0 => ("starting", None),
            _ => match s.maintainerr.version().await {
                Ok(v) => ("ok", Some(v)),
                Err(_) => ("unreachable", None),
            },
        }
    };
    let last = s.last_sync.load(Relaxed);
    Json(keep_service::ServerStatus {
        version: env!("CARGO_PKG_VERSION").into(),
        maintainerr: maintainerr.into(),
        maintainerr_version: version,
        last_sync: (last > 0).then_some(last),
        leaving_items: s.last_sync_items.load(Relaxed) as u32,
        seerr_bridge: s.seerr.is_some(),
        tautulli: c.tautulli.is_some() && c.secrets.tautulli_api_key.is_some(),
        jellyfin_key: c
            .servers
            .iter()
            .any(|x| c.secrets.jellyfin_key(&x.id).is_some()),
        webhooks: c.secrets.webhook_secret.is_some(),
        passkeys: c.accounts.rp.is_some(),
        pocketid: c.accounts.pocketid.is_some(),
        registration: match c.accounts.registration {
            crate::config::Registration::Invite => "invite",
            crate::config::Registration::Open => "open",
            crate::config::Registration::Closed => "closed",
        }
        .into(),
        dry_run: c.dry_run,
    })
}

async fn admin_forever(
    State(s): State<Shared>,
    Admin(_): Admin,
) -> Result<impl IntoResponse, Error> {
    let names =
        s.db.users()
            .map_err(internal)?
            .into_iter()
            .map(|u| (u.user_key, (u.name, u.media_name, u.avatar)))
            .collect();
    let votes = s.db.forever_votes().map_err(internal)?;
    Ok(Json(crate::keep::forever_stats(&votes, &names)))
}

async fn admin_seerr_users(
    State(s): State<Shared>,
    Admin(_): Admin,
) -> Result<impl IntoResponse, Error> {
    let bridge = s
        .seerr
        .as_ref()
        .ok_or(BridgeError::Disabled)
        .map_err(bridge_err)?;
    Ok(Json(bridge.users().await.map_err(bridge_err)?))
}

/// Links a user's requests to an existing Seerr user (the person already has one), or back to
/// automatic linking. Only the AlwyWatch database changes: nothing is written to Seerr.
async fn admin_link_seerr(
    State(s): State<Shared>,
    Admin(admin): Admin,
    Path(user): Path<String>,
    Json(r): Json<keep_service::SeerrLinkRequest>,
) -> Result<impl IntoResponse, Error> {
    let user = s.db.person_of(&user).map_err(internal)?;
    if !s.db.user_exists(&user).map_err(internal)? {
        return Err(Error(StatusCode::NOT_FOUND, "unknown user".into()));
    }
    // Requests made as a Seerr manager carry its rights: only an admin who may see every key
    // anyway (server settings) links someone to one.
    if let Some(sid) = r.seerr_user_id {
        let bridge = s
            .seerr
            .as_ref()
            .ok_or(BridgeError::Disabled)
            .map_err(bridge_err)?;
        let own_admin = admin
            .identities
            .iter()
            .any(|i| i.user_key == admin.signed_in_key && i.is_admin);
        if bridge.is_manager(sid).await.map_err(bridge_err)? && !own_admin {
            return Err(Error(
                StatusCode::FORBIDDEN,
                "this Seerr user manages Seerr: sign in with a Jellyfin administrator or the Plex owner's login to link someone to it".into(),
            ));
        }
    }
    match r.seerr_user_id {
        Some(id) => s.db.set_seerr_link(&user, id, false).map_err(internal)?,
        None => s.db.forget_seerr_link(&user).map_err(internal)?,
    }
    s.db.log(
        &admin.user_key,
        "seerr-link",
        &format!("{user} -> {:?} (by admin)", r.seerr_user_id),
    );
    let u =
        s.db.users()
            .map_err(internal)?
            .into_iter()
            .find(|u| u.user_key == user)
            .ok_or_else(|| internal("vanished"))?;
    Ok(Json(u))
}

async fn admin_rights(
    State(s): State<Shared>,
    Admin(admin): Admin,
    Path(user): Path<String>,
    Json(r): Json<Rights>,
) -> Result<impl IntoResponse, Error> {
    let user = s.db.person_of(&user).map_err(internal)?;
    if !s.db.user_exists(&user).map_err(internal)? {
        return Err(Error(StatusCode::NOT_FOUND, "unknown user".into()));
    }
    s.db.set_rights(&user, &r).map_err(internal)?;
    s.db.log(&admin.user_key, "rights", &format!("{user} {r:?}"));
    let u =
        s.db.users()
            .map_err(internal)?
            .into_iter()
            .find(|u| u.user_key == user)
            .ok_or_else(|| internal("vanished"))?;
    Ok(Json(u))
}

async fn admin_countdown(
    State(s): State<Shared>,
    Admin(admin): Admin,
    Path(id): Path<i64>,
    Json(r): Json<CountdownRequest>,
) -> Result<impl IntoResponse, Error> {
    Ok(Json(
        s.countdown(&admin, id, r.stagger_days.min(365))
            .await
            .map_err(internal)?,
    ))
}

fn bridge_err(e: BridgeError) -> Error {
    match e {
        BridgeError::Disabled => Error(StatusCode::NOT_FOUND, "Seerr is not available here".into()),
        BridgeError::NoAccount => Error(
            StatusCode::FORBIDDEN,
            "no Seerr account for you; ask the administrator".into(),
        ),
        BridgeError::DryRun => Error(
            StatusCode::FORBIDDEN,
            "this AlwyWatch server is in read-only test mode".into(),
        ),
        BridgeError::Unavailable(m) => Error(StatusCode::BAD_GATEWAY, format!("Seerr: {m}")),
    }
}

async fn seerr_info(
    State(s): State<Shared>,
    Caller(id): Caller,
) -> Result<impl IntoResponse, Error> {
    let bridge = s
        .seerr
        .as_ref()
        .ok_or(BridgeError::Disabled)
        .map_err(bridge_err)?;
    Ok(Json(bridge.info(&s.db, &id).await.map_err(bridge_err)?))
}

async fn seerr_forward(
    State(s): State<Shared>,
    Caller(id): Caller,
    method: Method,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Result<Response, Error> {
    let bridge = s
        .seerr
        .as_ref()
        .ok_or(BridgeError::Disabled)
        .map_err(bridge_err)?;
    if !allowed(&method, &path) {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "not available through the bridge".into(),
        ));
    }
    bridge
        .forward(&s.db, &id, method, &path, query.as_deref(), body)
        .await
        .map_err(bridge_err)
}

/// Tautulli ("watched" notification, JSON) and the Jellyfin webhook plugin. Either just wakes
/// the watched checker, so payload formats do not matter beyond being authenticated.
async fn hook(
    State(s): State<Shared>,
    Path((source, secret)): Path<(String, String)>,
    _h: HeaderMap,
) -> Result<StatusCode, Error> {
    let expected = s
        .config
        .secrets
        .webhook_secret
        .as_deref()
        .ok_or(Error(StatusCode::NOT_FOUND, "webhooks disabled".into()))?;
    if !constant_time_eq(secret.as_bytes(), expected.as_bytes())
        || !matches!(source.as_str(), "tautulli" | "jellyfin" | "plex")
    {
        return Err(Error(StatusCode::NOT_FOUND, "not found".into()));
    }
    s.watch.notify_one();
    Ok(StatusCode::NO_CONTENT)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
