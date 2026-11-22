// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP side of AlwyWatch accounts (see `accounts.rs`, docs/keep-protocol.md "Accounts").
//!
//! An account session (`Authorization: Bearer aws_…`, no `X-AlwyWatch-Server`) reaches the
//! account itself, the person's profile and synced settings, and Quick Connect approvals;
//! keep votes, rooms and Seerr still need a media-server sign-in.

use axum::Json;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};

use crate::accounts::{self, AccountError, AccountRow, SESSION_PREFIX};
use crate::api::{Caller, Error};
use crate::auth::AuthError;
use crate::config::Registration;
use crate::keep::Shared;

const SERVER_HEADER: &str = "x-alwywatch-server";

fn internal(e: impl ToString) -> Error {
    let msg = e.to_string();
    eprintln!("[account] internal error: {msg}");
    Error(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
}

fn too_many() -> Error {
    Error(
        StatusCode::TOO_MANY_REQUESTS,
        "too many attempts, try again in a few minutes".into(),
    )
}

fn account_err(e: AccountError) -> Error {
    match e {
        AccountError::Invalid(m) => Error(StatusCode::BAD_REQUEST, m.into()),
        AccountError::UsernameTaken => Error(StatusCode::CONFLICT, "this username is taken".into()),
        AccountError::Db(e) => internal(e),
    }
}

fn bearer(h: &HeaderMap) -> Option<&str> {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
}

/// A signed-in AlwyWatch account session.
pub struct Session {
    pub account: AccountRow,
    /// The session's id (see `accounts::SessionRow::id`).
    pub id: String,
}

impl FromRequestParts<Shared> for Session {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, s: &Shared) -> Result<Self, Error> {
        let token = bearer(&parts.headers)
            .filter(|t| t.starts_with(SESSION_PREFIX))
            .ok_or(Error(
                StatusCode::UNAUTHORIZED,
                "sign in to your AlwyWatch account".into(),
            ))?;
        let ip = crate::limit::client(&parts.headers);
        if s.limits.failed_auth.blocked(&ip) {
            return Err(too_many());
        }
        match s.db.session(token).map_err(internal)? {
            Some((account, id)) => Ok(Session { account, id }),
            None => {
                s.limits.failed_auth.hit(&ip);
                Err(Error(
                    StatusCode::UNAUTHORIZED,
                    "this sign-in has ended, sign in again".into(),
                ))
            }
        }
    }
}

/// Whoever is signed in, with a media-server sign-in or an account session: the person.
pub struct Person {
    pub user_key: String,
    /// The account of the session, or the person's account (if any) for a media sign-in.
    pub account: Option<AccountRow>,
    /// The media sign-in, when the request came with one.
    pub media: Option<crate::auth::Identity>,
}

impl FromRequestParts<Shared> for Person {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, s: &Shared) -> Result<Self, Error> {
        if parts.headers.contains_key(SERVER_HEADER) {
            let Caller(id) = Caller::from_request_parts(parts, s).await?;
            let account = s.db.account_of_person(&id.user_key).map_err(internal)?;
            return Ok(Person {
                user_key: id.user_key.clone(),
                account,
                media: Some(id),
            });
        }
        let Session { account, .. } = Session::from_request_parts(parts, s).await?;
        Ok(Person {
            user_key: account.person.clone(),
            account: Some(account),
            media: None,
        })
    }
}

// ----- what the server offers ---------------------------------------------------------------

#[derive(Serialize)]
pub struct Options {
    /// "invite", "open" or "closed".
    registration: &'static str,
    passkeys: bool,
    pocketid: bool,
}

pub async fn options(State(s): State<Shared>) -> Json<Options> {
    let a = &s.config.accounts;
    Json(Options {
        registration: match a.registration {
            Registration::Invite => "invite",
            Registration::Open => "open",
            Registration::Closed => "closed",
        },
        passkeys: a.rp.is_some(),
        pocketid: a.pocketid.is_some(),
    })
}

// ----- sign-up and sign-in ------------------------------------------------------------------

#[derive(Serialize)]
pub struct SignedIn {
    pub token: String,
    pub account: AccountView,
}

#[derive(Serialize)]
pub struct AccountView {
    pub id: String,
    pub username: Option<String>,
    pub name: String,
    /// The person's key (as in rooms and profiles).
    pub user_key: String,
    pub has_password: bool,
    pub passkeys: Vec<PasskeyView>,
    pub pocketid: bool,
    pub sessions: Vec<SessionView>,
    /// Media-server sign-ins linked to this person.
    pub linked: Vec<LinkedView>,
}

#[derive(Serialize)]
pub struct PasskeyView {
    pub id: String,
    pub name: String,
    pub created: i64,
    pub last_used: Option<i64>,
}

#[derive(Serialize)]
pub struct SessionView {
    pub id: String,
    pub device: String,
    pub created: i64,
    pub last_seen: i64,
    pub current: bool,
}

#[derive(Serialize)]
pub struct LinkedView {
    pub server_id: String,
    pub kind: String,
    pub name: String,
}

fn view(s: &Shared, a: &AccountRow, current: Option<&str>) -> Result<AccountView, Error> {
    let db = &s.db;
    Ok(AccountView {
        id: a.id.clone(),
        username: a.username.clone(),
        name: db
            .person_name(&a.person)
            .map_err(internal)?
            .or_else(|| a.username.clone())
            .unwrap_or_default(),
        user_key: a.person.clone(),
        has_password: a.has_password,
        passkeys: db
            .passkeys(&a.id)
            .map_err(internal)?
            .into_iter()
            .map(|p| PasskeyView {
                id: p.cred_id,
                name: p.name,
                created: p.created,
                last_used: p.last_used,
            })
            .collect(),
        pocketid: db.has_oidc(&a.id).map_err(internal)?,
        sessions: db
            .sessions(&a.id)
            .map_err(internal)?
            .into_iter()
            .map(|x| SessionView {
                current: current == Some(x.id.as_str()),
                id: x.id,
                device: x.device,
                created: x.created,
                last_seen: x.last_seen,
            })
            .collect(),
        linked: db
            .identities_of(&a.person)
            .map_err(internal)?
            .into_iter()
            .map(|i| LinkedView {
                server_id: i.server_id,
                kind: format!("{:?}", i.kind).to_lowercase(),
                name: i.media_name,
            })
            .collect(),
    })
}

fn signed_in(s: &Shared, a: &AccountRow, device: &str) -> Result<Json<SignedIn>, Error> {
    let token = s.db.new_session(&a.id, device).map_err(internal)?;
    let current = accounts::sha256_hex(token.as_bytes())[..12].to_owned();
    Ok(Json(SignedIn {
        account: view(s, a, Some(&current))?,
        token,
    }))
}

#[derive(Deserialize)]
pub struct Register {
    username: String,
    password: String,
    #[serde(default)]
    invite: Option<String>,
    #[serde(default)]
    device: String,
}

/// `POST /v1/account/register`: a new account with a password (and a new person).
pub async fn register(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(r): Json<Register>,
) -> Result<Json<SignedIn>, Error> {
    let ip = crate::limit::client(&headers);
    if s.limits.account_signups.blocked(&ip) {
        return Err(too_many());
    }
    let username = r.username.trim();
    accounts::check_username(username).map_err(account_err)?;
    let hash = accounts::hash_password(&r.password).map_err(account_err)?;
    match s.config.accounts.registration {
        Registration::Closed => {
            return Err(Error(
                StatusCode::FORBIDDEN,
                "new accounts are not open on this server".into(),
            ));
        }
        Registration::Invite => {
            let code = r.invite.as_deref().unwrap_or("");
            if !s.db.use_invite(code).map_err(internal)? {
                s.limits.account_signups.hit(&ip);
                return Err(Error(
                    StatusCode::FORBIDDEN,
                    "this invitation is unknown, used up or expired".into(),
                ));
            }
        }
        Registration::Open => {}
    }
    s.limits.account_signups.hit(&ip);
    let a =
        s.db.create_account(Some(username), Some(&hash), None, username)
            .map_err(account_err)?;
    s.db.log(&a.person, "account", "created with a password");
    signed_in(&s, &a, &r.device)
}

#[derive(Deserialize)]
pub struct Login {
    username: String,
    password: String,
    #[serde(default)]
    device: String,
}

/// `POST /v1/account/login`.
pub async fn login(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(r): Json<Login>,
) -> Result<Json<SignedIn>, Error> {
    let ip = crate::limit::client(&headers);
    let user = format!("u:{}", r.username.trim().to_lowercase());
    if s.limits.account_logins.blocked(&ip) || s.limits.account_logins.blocked(&user) {
        return Err(too_many());
    }
    let found =
        s.db.account_by_username(r.username.trim())
            .map_err(internal)?;
    let ok = match &found {
        Some((_, Some(hash))) => accounts::verify_password(hash, &r.password),
        _ => {
            accounts::verify_nothing(&r.password);
            false
        }
    };
    let Some((a, _)) = found.filter(|_| ok) else {
        s.limits.account_logins.hit(&ip);
        s.limits.account_logins.hit(&user);
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "wrong username or password".into(),
        ));
    };
    signed_in(&s, &a, &r.device)
}

/// `POST /v1/account/logout`: ends this session.
pub async fn logout(State(s): State<Shared>, session: Session) -> Result<StatusCode, Error> {
    s.db.end_session(&session.account.id, &session.id)
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/account`.
pub async fn me(State(s): State<Shared>, session: Session) -> Result<Json<AccountView>, Error> {
    Ok(Json(view(&s, &session.account, Some(&session.id))?))
}

#[derive(Deserialize)]
pub struct Claim {
    username: String,
    password: String,
}

/// `POST /v1/account/claim` (media sign-in): adds a username and password to the person, so
/// they can also sign in without a media server (e.g. once Plex is gone).
pub async fn claim(
    State(s): State<Shared>,
    Caller(id): Caller,
    Json(r): Json<Claim>,
) -> Result<Json<AccountView>, Error> {
    let username = r.username.trim();
    accounts::check_username(username).map_err(account_err)?;
    let hash = accounts::hash_password(&r.password).map_err(account_err)?;
    let a = match s.db.account_of_person(&id.user_key).map_err(internal)? {
        Some(a) => {
            s.db.set_username(&a.id, username).map_err(account_err)?;
            s.db.set_password(&a.id, Some(&hash)).map_err(internal)?;
            s.db.account(&a.id)
                .map_err(internal)?
                .ok_or(Error(StatusCode::NOT_FOUND, "no such account".into()))?
        }
        None => {
            s.db.create_account(Some(username), Some(&hash), Some(&id.user_key), &id.name)
                .map_err(account_err)?
        }
    };
    Ok(Json(view(&s, &a, None)?))
}

#[derive(Deserialize)]
pub struct PasswordChange {
    #[serde(default)]
    current: Option<String>,
    new: String,
}

/// `PUT /v1/account/password`: sets or changes the password; other sessions end.
pub async fn set_password(
    State(s): State<Shared>,
    headers: HeaderMap,
    session: Session,
    Json(r): Json<PasswordChange>,
) -> Result<StatusCode, Error> {
    let a = &session.account;
    if let Some(hash) = s.db.password_hash(&a.id).map_err(internal)? {
        let ip = crate::limit::client(&headers);
        if s.limits.account_logins.blocked(&ip) {
            return Err(too_many());
        }
        if !accounts::verify_password(&hash, r.current.as_deref().unwrap_or("")) {
            s.limits.account_logins.hit(&ip);
            return Err(Error(
                StatusCode::FORBIDDEN,
                "the current password is wrong".into(),
            ));
        }
    }
    if a.username.is_none() {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "choose a username first".into(),
        ));
    }
    let hash = accounts::hash_password(&r.new).map_err(account_err)?;
    s.db.set_password(&a.id, Some(&hash)).map_err(internal)?;
    s.db.end_other_sessions(&a.id, Some(&session.id))
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct Username {
    username: String,
}

/// `PUT /v1/account/username`.
pub async fn set_username(
    State(s): State<Shared>,
    session: Session,
    Json(r): Json<Username>,
) -> Result<StatusCode, Error> {
    let u = r.username.trim();
    accounts::check_username(u).map_err(account_err)?;
    s.db.set_username(&session.account.id, u)
        .map_err(account_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /v1/account/sessions/{id}`: signs a device out.
pub async fn end_session(
    State(s): State<Shared>,
    session: Session,
    Path(id): Path<String>,
) -> Result<StatusCode, Error> {
    if s.db
        .end_session(&session.account.id, &id)
        .map_err(internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Error(StatusCode::NOT_FOUND, "no such session".into()))
    }
}

// ----- linking a media-server sign-in -------------------------------------------------------

#[derive(Deserialize)]
pub struct LinkMedia {
    server_id: String,
    token: String,
}

/// `POST /v1/account/link`: proves a Jellyfin/Plex sign-in (its token is checked with that
/// server, never stored) and merges it into the account's person.
pub async fn link_media(
    State(s): State<Shared>,
    headers: HeaderMap,
    session: Session,
    Json(r): Json<LinkMedia>,
) -> Result<Json<AccountView>, Error> {
    let person = session.account.person.clone();
    if !s.limits.links.allow(&person) {
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
    let other = s
        .auth
        .identify(server, &r.token)
        .await
        .map_err(|e| match e {
            AuthError::Denied => {
                s.limits.failed_auth.hit(&ip);
                Error(
                    StatusCode::UNAUTHORIZED,
                    "that sign-in was refused (wrong or expired token)".into(),
                )
            }
            AuthError::Unavailable => {
                Error(StatusCode::BAD_GATEWAY, "media server unreachable".into())
            }
        })?;
    let outcome = crate::api::link_person(&s, &person, &other).await?;
    s.db.log(
        &outcome,
        "link",
        &format!("account {} + {}", session.account.id, other.signed_in_key),
    );
    let a =
        s.db.account(&session.account.id)
            .map_err(internal)?
            .ok_or(Error(StatusCode::NOT_FOUND, "no such account".into()))?;
    Ok(Json(view(&s, &a, Some(&session.id))?))
}

// ----- invitations --------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct NewInvite {
    #[serde(default = "seven")]
    days: i64,
    #[serde(default = "one")]
    uses: i64,
}

fn seven() -> i64 {
    7
}
fn one() -> i64 {
    1
}

#[derive(Serialize)]
pub struct Invite {
    code: String,
    /// The web page that signs up with it.
    url: Option<String>,
    expires_in_days: i64,
    uses: i64,
}

/// `POST /v1/account/invites` (AlwyWatch admin): an invitation to create an account.
pub async fn create_invite(
    State(s): State<Shared>,
    crate::api::Admin(id): crate::api::Admin,
    Json(r): Json<NewInvite>,
) -> Result<Json<Invite>, Error> {
    let days = r.days.clamp(1, 90);
    let uses = r.uses.clamp(1, 50);
    let code =
        s.db.create_invite(&id.user_key, days, uses)
            .map_err(internal)?;
    Ok(Json(Invite {
        url: s
            .config
            .accounts
            .rp
            .as_ref()
            .map(|rp| format!("{}/join/{code}", rp.origin)),
        code,
        expires_in_days: days,
        uses,
    }))
}

// ----- passkeys -----------------------------------------------------------------------------

fn rp(s: &Shared) -> Result<&crate::passkey::RelyingParty, Error> {
    s.config.accounts.rp.as_ref().ok_or(Error(
        StatusCode::NOT_IMPLEMENTED,
        "passkeys need ALWYWATCH_PUBLIC_URL on the server".into(),
    ))
}

fn passkey_err(e: crate::passkey::PasskeyError) -> Error {
    use crate::passkey::PasskeyError as E;
    match e {
        E::Expired => Error(
            StatusCode::BAD_REQUEST,
            "this took too long, try again".into(),
        ),
        E::Invalid(m) => Error(StatusCode::BAD_REQUEST, format!("passkey refused: {m}")),
        E::UnknownCredential => Error(
            StatusCode::UNAUTHORIZED,
            "this passkey is not known here".into(),
        ),
    }
}

/// `POST /v1/account/passkeys/register/start`.
pub async fn passkey_register_start(
    State(s): State<Shared>,
    session: Session,
) -> Result<Json<serde_json::Value>, Error> {
    let rp = rp(&s)?;
    let a = &session.account;
    let exclude: Vec<String> =
        s.db.passkeys(&a.id)
            .map_err(internal)?
            .into_iter()
            .map(|p| p.cred_id)
            .collect();
    let name =
        s.db.person_name(&a.person)
            .map_err(internal)?
            .unwrap_or_default();
    let user = a.username.clone().unwrap_or_else(|| name.clone());
    Ok(Json(
        s.passkeys.register_start(rp, &a.id, &user, &name, &exclude),
    ))
}

#[derive(Deserialize)]
pub struct PasskeyRegister {
    flow_id: String,
    #[serde(default)]
    name: Option<String>,
    credential: crate::passkey::RegisterCredential,
}

/// `POST /v1/account/passkeys/register/finish`.
pub async fn passkey_register_finish(
    State(s): State<Shared>,
    session: Session,
    Json(r): Json<PasskeyRegister>,
) -> Result<Json<AccountView>, Error> {
    let rp = rp(&s)?;
    let (account, key) = s
        .passkeys
        .register_finish(rp, &r.flow_id, &r.credential)
        .map_err(passkey_err)?;
    if account != session.account.id {
        return Err(Error(StatusCode::FORBIDDEN, "not your passkey flow".into()));
    }
    let name: String = r
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Passkey")
        .chars()
        .take(60)
        .collect();
    s.db.add_passkey(&accounts::PasskeyRow {
        cred_id: key.cred_id,
        account,
        name,
        alg: key.alg,
        public_key: key.public_key,
        sign_count: key.sign_count,
        created: crate::db::now(),
        last_used: None,
    })
    .map_err(|_| {
        Error(
            StatusCode::CONFLICT,
            "this passkey is already registered".into(),
        )
    })?;
    Ok(Json(view(&s, &session.account, Some(&session.id))?))
}

/// `POST /v1/account/passkeys/login/start`.
pub async fn passkey_login_start(
    State(s): State<Shared>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(s.passkeys.login_start(rp(&s)?)))
}

#[derive(Deserialize)]
pub struct PasskeyLogin {
    flow_id: String,
    #[serde(default)]
    device: String,
    credential: crate::passkey::LoginCredential,
}

/// `POST /v1/account/passkeys/login/finish`.
pub async fn passkey_login_finish(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(r): Json<PasskeyLogin>,
) -> Result<Json<SignedIn>, Error> {
    let ip = crate::limit::client(&headers);
    if s.limits.account_logins.blocked(&ip) {
        return Err(too_many());
    }
    let rp = rp(&s)?;
    let stored = std::cell::RefCell::new(None);
    let result = s
        .passkeys
        .login_finish(rp, &r.flow_id, &r.credential, |id| {
            let p = s.db.passkey(id).ok().flatten()?;
            let out = (p.alg, p.public_key.clone(), p.sign_count);
            *stored.borrow_mut() = Some(p);
            Some(out)
        });
    let (cred_id, count) = match result {
        Ok(x) => x,
        Err(e) => {
            s.limits.account_logins.hit(&ip);
            return Err(passkey_err(e));
        }
    };
    let p = stored.into_inner().ok_or(Error(
        StatusCode::UNAUTHORIZED,
        "this passkey is not known here".into(),
    ))?;
    s.db.passkey_used(&cred_id, count).map_err(internal)?;
    let a =
        s.db.account(&p.account)
            .map_err(internal)?
            .ok_or(Error(StatusCode::UNAUTHORIZED, "no such account".into()))?;
    signed_in(&s, &a, &r.device)
}

#[derive(Deserialize)]
pub struct Rename {
    name: String,
}

/// `PUT /v1/account/passkeys/{id}`: renames.
pub async fn passkey_rename(
    State(s): State<Shared>,
    session: Session,
    Path(id): Path<String>,
    Json(r): Json<Rename>,
) -> Result<StatusCode, Error> {
    let name: String = r.name.trim().chars().take(60).collect();
    if name.is_empty() {
        return Err(Error(StatusCode::BAD_REQUEST, "give it a name".into()));
    }
    if s.db
        .rename_passkey(&session.account.id, &id, &name)
        .map_err(internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Error(StatusCode::NOT_FOUND, "no such passkey".into()))
    }
}

/// Refuses to remove the last way into an account that has no media sign-in either.
fn keep_one_way_in(s: &Shared, a: &AccountRow) -> Result<(), Error> {
    let methods = s.db.sign_in_methods(&a.id).map_err(internal)?;
    let media = !s.db.identities_of(&a.person).map_err(internal)?.is_empty();
    if methods <= 1 && !media {
        return Err(Error(
            StatusCode::CONFLICT,
            "this is your only way to sign in: add a password, passkey or Pocket ID first".into(),
        ));
    }
    Ok(())
}

/// `DELETE /v1/account/passkeys/{id}`.
pub async fn passkey_delete(
    State(s): State<Shared>,
    session: Session,
    Path(id): Path<String>,
) -> Result<StatusCode, Error> {
    keep_one_way_in(&s, &session.account)?;
    if s.db
        .delete_passkey(&session.account.id, &id)
        .map_err(internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Error(StatusCode::NOT_FOUND, "no such passkey".into()))
    }
}

// ----- Pocket ID ----------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct OidcStart {
    #[serde(default)]
    next: Option<String>,
    /// From `POST /v1/account/oidc/ticket`: link to that account instead of signing in.
    #[serde(default)]
    ticket: Option<String>,
}

fn to_web(path: &str) -> Redirect {
    Redirect::to(path)
}

fn web_error(msg: &str) -> Redirect {
    let enc: String = msg
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    to_web(&format!("/account?error={enc}"))
}

/// `GET /v1/account/oidc/start`: off to Pocket ID.
pub async fn oidc_start(State(s): State<Shared>, Query(q): Query<OidcStart>) -> Response {
    let Some(cfg) = s.config.accounts.pocketid.as_ref() else {
        return web_error("Pocket ID is not set up on this server").into_response();
    };
    let intent = match q.ticket.as_deref() {
        Some(t) => match s.oidc.take_ticket(t) {
            Some(account) => crate::oidc::Intent::Link { account },
            None => return web_error("this link has expired, try again").into_response(),
        },
        None => crate::oidc::Intent::Login {
            next: crate::oidc::safe_next(q.next.as_deref()),
        },
    };
    match s.oidc.start(&s.http, cfg, intent).await {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => web_error(&e.to_string()).into_response(),
    }
}

#[derive(Serialize)]
pub struct Ticket {
    ticket: String,
}

/// `POST /v1/account/oidc/ticket`: a one-time ticket to link Pocket ID in a browser tab.
pub async fn oidc_ticket(State(s): State<Shared>, session: Session) -> Json<Ticket> {
    Json(Ticket {
        ticket: s.oidc.ticket(&session.account.id),
    })
}

#[derive(Deserialize)]
pub struct OidcCallback {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `GET /v1/account/oidc/callback`: back from Pocket ID; on to the web app.
pub async fn oidc_callback(State(s): State<Shared>, Query(q): Query<OidcCallback>) -> Redirect {
    let Some(cfg) = s.config.accounts.pocketid.as_ref() else {
        return web_error("Pocket ID is not set up on this server");
    };
    if let Some(e) = q.error {
        return web_error(&format!("Pocket ID said: {e}"));
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return web_error("Pocket ID's answer was incomplete");
    };
    let (intent, user) = match s.oidc.finish(&s.http, cfg, &code, &state).await {
        Ok(x) => x,
        Err(e) => return web_error(&e.to_string()),
    };
    let found = match s.db.oidc_account(&cfg.issuer, &user.sub) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[account] {e}");
            return web_error("something went wrong");
        }
    };
    match intent {
        crate::oidc::Intent::Link { account } => {
            if found.as_deref().is_some_and(|a| a != account) {
                return web_error("this Pocket ID is already used by another account");
            }
            if let Err(e) = s.db.link_oidc(&cfg.issuer, &user.sub, &account) {
                eprintln!("[account] {e}");
                return web_error("something went wrong");
            }
            to_web("/account?linked=pocketid")
        }
        crate::oidc::Intent::Login { next } => {
            let account = match found {
                Some(id) => s.db.account(&id).ok().flatten(),
                None => {
                    // First sign-in: Pocket ID users are the owner's own, no invitation needed.
                    let name = user
                        .name
                        .clone()
                        .or(user.username.clone())
                        .unwrap_or_else(|| "AlwyWatch user".into());
                    let username = user
                        .username
                        .as_deref()
                        .filter(|u| accounts::check_username(u).is_ok())
                        .filter(|u| matches!(s.db.account_by_username(u), Ok(None)));
                    match s.db.create_account(username, None, None, &name) {
                        Ok(a) => {
                            let _ = s.db.link_oidc(&cfg.issuer, &user.sub, &a.id);
                            s.db.log(&a.person, "account", "created with Pocket ID");
                            Some(a)
                        }
                        Err(e) => {
                            eprintln!("[account] {e:?}");
                            None
                        }
                    }
                }
            };
            let Some(a) = account else {
                return web_error("something went wrong");
            };
            match s.db.new_session(&a.id, "Web browser") {
                Ok(token) => to_web(&format!(
                    "/account/signed-in#token={token}&next={}",
                    next.replace('&', "%26").replace('#', "%23")
                )),
                Err(e) => {
                    eprintln!("[account] {e}");
                    web_error("something went wrong")
                }
            }
        }
    }
}

/// `DELETE /v1/account/oidc`: unlinks Pocket ID.
pub async fn oidc_unlink(State(s): State<Shared>, session: Session) -> Result<StatusCode, Error> {
    if !s.db.has_oidc(&session.account.id).map_err(internal)? {
        return Ok(StatusCode::NO_CONTENT);
    }
    keep_one_way_in(&s, &session.account)?;
    s.db.unlink_oidc(&session.account.id).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

// ----- Quick Connect ------------------------------------------------------------------------

/// The person's account, made without a way to sign in if they have none yet (a media
/// sign-in approving a new device: the device gets the same person).
fn account_for(s: &Shared, p: &Person) -> Result<AccountRow, Error> {
    if let Some(a) = &p.account {
        return Ok(a.clone());
    }
    let name =
        s.db.person_name(&p.user_key)
            .map_err(internal)?
            .unwrap_or_default();
    s.db.create_account(None, None, Some(&p.user_key), &name)
        .map_err(account_err)
}

/// Makes the session a Quick Connect approval hands to the new device.
pub fn grant_for(s: &Shared, p: &Person, device: &str) -> Result<String, Error> {
    let a = account_for(s, p)?;
    s.db.new_session(&a.id, device).map_err(internal)
}

#[derive(Serialize)]
pub struct Approved {
    device: String,
}

/// `PUT /v1/transfer/{code}/approve`: signs the device showing `code` in to my account (the
/// web, or a device with no media sign-ins to send).
pub async fn quick_approve(
    State(s): State<Shared>,
    person: Person,
    Path(code): Path<String>,
) -> Result<Json<Approved>, Error> {
    if !s.limits.transfer_codes.allow(&person.user_key) {
        return Err(too_many());
    }
    let peer = s
        .transfers
        .peer(code.trim())
        .map_err(crate::api::transfer_error)?;
    let token = grant_for(&s, &person, &peer.device)?;
    s.transfers
        .send(code.trim(), None, Some(token))
        .map_err(crate::api::transfer_error)?;
    eprintln!("[transfer] {} approved a device", person.user_key);
    Ok(Json(Approved {
        device: peer.device,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_say_what_happened() {
        assert_eq!(
            account_err(AccountError::UsernameTaken).0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            web_error("a b").into_response().status(),
            StatusCode::SEE_OTHER
        );
    }
}
