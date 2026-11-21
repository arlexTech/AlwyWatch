// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP API (`/v1`), see docs/keep-protocol.md.

use axum::body::Bytes;
use axum::extract::{FromRequestParts, Path, Query, RawQuery, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post, put};
use axum::{Json, Router};
use keep_service::{
    ApiError, CountdownRequest, Me, Rights, SERVER_HEADER, ServerEntry, VoteRequest,
};
use serde::Deserialize;

use crate::auth::{AuthError, Identity};
use crate::keep::{Shared, VoteError};
use crate::seerr::{BridgeError, allowed};

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/servers", get(servers))
        .route("/v1/me", get(me))
        .route("/v1/me/settings", get(my_settings).put(set_my_settings))
        .route("/v1/leaving-soon", get(leaving_soon))
        .route("/v1/triage", get(triage))
        .route("/v1/votes/{key}", put(vote).delete(unvote))
        .route("/v1/admin/users", get(admin_users))
        .route("/v1/admin/users/{user}/rights", put(admin_rights))
        .route("/v1/admin/users/{user}/seerr", put(admin_link_seerr))
        .route("/v1/admin/seerr-users", get(admin_seerr_users))
        .route("/v1/admin/forever", get(admin_forever))
        .route(
            "/v1/admin/collections/{id}/countdown",
            post(admin_countdown),
        )
        .route("/v1/seerr", get(seerr_info))
        .route("/v1/seerr/api/v1/{*path}", any(seerr_forward))
        .route("/v1/hooks/{source}/{secret}", post(hook))
        .route("/v1/together", get(together_rooms).post(together_create))
        .route("/v1/together/{code}", put(together_update))
        .route("/v1/together/{code}/knock", post(together_knock))
        .route(
            "/v1/together/{code}/knock/{id}/wait",
            get(together_knock_wait),
        )
        .route(
            "/v1/together/{code}/knock/{id}",
            axum::routing::delete(together_knock_cancel),
        )
        .route("/v1/together/{code}/requests/{id}", post(together_answer))
        .route("/v1/together/{code}/wait", get(together_wait))
        .route(
            "/v1/together/{code}/members/{member}",
            axum::routing::delete(together_leave),
        )
        .route("/v1/transfer", post(transfer_start))
        .route("/v1/transfer/poll/{handle}", get(transfer_poll))
        .route("/v1/transfer/{code}", get(transfer_peer).put(transfer_send))
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
        let _ = state
            .db
            .touch_user(&id.user_key, &id.name, &id.server_id, id.is_admin);
        Ok(Caller(id))
    }
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
        E::Forbidden => Error(StatusCode::FORBIDDEN, "not allowed in this room".into()),
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
        .create(c, &id.name, &id.user_key)
        .map(Json)
        .map_err(together_error)
}

async fn together_rooms(
    State(s): State<Shared>,
    Caller(id): Caller,
) -> Json<Vec<keep_service::together::TogetherRoom>> {
    Json(s.rooms.list(&id.user_key))
}

async fn together_knock(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
) -> Result<Json<keep_service::together::TogetherKnockState>, Error> {
    if !s.limits.together_knocks.allow(&id.user_key) {
        return Err(too_many());
    }
    s.rooms
        .knock(&code, &id.name, &id.user_key)
        .map(Json)
        .map_err(together_error)
}

async fn together_knock_wait(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path((code, req)): Path<(String, String)>,
) -> Result<Json<keep_service::together::TogetherKnockState>, Error> {
    s.rooms
        .knock_wait(&code, &req, &id.user_key)
        .await
        .map(Json)
        .map_err(together_error)
}

async fn together_knock_cancel(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path((code, req)): Path<(String, String)>,
) -> Result<StatusCode, Error> {
    s.rooms
        .knock_cancel(&code, &req, &id.user_key)
        .map_err(together_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn together_answer(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path((code, req)): Path<(String, String)>,
    Json(a): Json<keep_service::together::TogetherAnswer>,
) -> Result<Json<keep_service::together::TogetherState>, Error> {
    s.rooms
        .answer(&code, &req, &a.member_id, a.accept, &id.user_key)
        .map(Json)
        .map_err(together_error)
}

async fn together_update(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
    Json(u): Json<keep_service::together::TogetherUpdate>,
) -> Result<Json<keep_service::together::TogetherState>, Error> {
    s.rooms
        .update(&code, u, &id.user_key)
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

fn transfer_error(e: crate::transfer::TransferError) -> Error {
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
    Caller(_): Caller,
    Path(code): Path<String>,
) -> Result<Json<keep_service::transfer::TransferPeer>, Error> {
    s.transfers.peer(&code).map(Json).map_err(transfer_error)
}

async fn transfer_send(
    State(s): State<Shared>,
    Caller(id): Caller,
    Path(code): Path<String>,
    Json(sealed): Json<keep_service::transfer::TransferSealed>,
) -> Result<StatusCode, Error> {
    s.transfers.send(&code, sealed).map_err(transfer_error)?;
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

async fn me(State(s): State<Shared>, Caller(id): Caller) -> Result<Json<Me>, Error> {
    Ok(Json(Me {
        rights: s.rights_of(&id),
        forever_count: s.db.forever_count(&id.user_key).map_err(internal)?,
        user_key: id.user_key,
        name: id.name,
        server_id: id.server_id,
        is_admin: id.is_admin,
    }))
}

/// Limits of the settings a user stores (they are small: languages, choices per title).
const MAX_SETTINGS: u32 = 20_000;
const MAX_SETTING_KEY: usize = 256;
const MAX_SETTING_VALUE: usize = 4096;

async fn my_settings(
    State(s): State<Shared>,
    Caller(id): Caller,
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
    Caller(id): Caller,
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
        .filter(|i| i.my_vote.is_none() && i.effective_deletion_date.is_some())
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

async fn admin_forever(
    State(s): State<Shared>,
    Admin(_): Admin,
) -> Result<impl IntoResponse, Error> {
    let names =
        s.db.users()
            .map_err(internal)?
            .into_iter()
            .map(|u| (u.user_key, u.name))
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
    if !s.db.user_exists(&user).map_err(internal)? {
        return Err(Error(StatusCode::NOT_FOUND, "unknown user".into()));
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
