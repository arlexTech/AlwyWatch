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
        .route("/v1/leaving-soon", get(leaving_soon))
        .route("/v1/triage", get(triage))
        .route("/v1/votes/{key}", put(vote).delete(unvote))
        .route("/v1/admin/users", get(admin_users))
        .route("/v1/admin/users/{user}/rights", put(admin_rights))
        .route(
            "/v1/admin/collections/{id}/countdown",
            post(admin_countdown),
        )
        .route("/v1/seerr", get(seerr_info))
        .route("/v1/seerr/api/v1/{*path}", any(seerr_forward))
        .route("/v1/hooks/{source}/{secret}", post(hook))
        .with_state(state)
}

pub struct Error(StatusCode, String);

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(ApiError { error: self.1 })).into_response()
    }
}

fn internal(e: impl ToString) -> Error {
    Error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
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
        let id = state
            .auth
            .identify(server, token)
            .await
            .map_err(|e| match e {
                AuthError::Denied => Error(
                    StatusCode::UNAUTHORIZED,
                    "invalid token or no access to this server".into(),
                ),
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
