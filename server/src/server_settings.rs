// SPDX-License-Identifier: AGPL-3.0-or-later
//! Setting the server up from the web app, as the *arr apps do: the first media server (with
//! the setup code from the log), then everything else by its admins (Settings → Admin →
//! Server settings). Saving starts the server again with the new settings.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use keep_service::ServerType;
use net::reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{Error, ServerAdmin};
use crate::config::{Config, ServerConf};
use crate::keep::Shared;
use crate::settings::{Stored, StoredServer};

fn bad(m: impl Into<String>) -> Error {
    Error(StatusCode::BAD_REQUEST, m.into())
}

/// Checks the new settings, saves them and switches to them (the site stays up: requests
/// already running finish with the old settings). `change` runs on the settings as saved, under a lock: two admins saving together can't
/// undo each other's change.
async fn save_and_reload(
    s: &Shared,
    change: impl FnOnce(&mut Stored) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut saved = s.stored.lock().await;
    let mut new = saved.clone();
    change(&mut new)?;
    Config::from_stored(&new, s.config.listen, s.config.data_dir.clone()).map_err(bad)?;
    new.save(&Stored::path(&s.config.data_dir))
        .map_err(|e| Error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    *saved = new;
    drop(saved);
    s.reload.notify_one();
    Ok(())
}

// ----- first start --------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SetupState {
    /// No media server yet: the web app shows the setup page.
    needed: bool,
}

/// `GET /v1/setup` (public).
pub async fn setup_state(State(s): State<Shared>) -> Json<SetupState> {
    Json(SetupState {
        needed: s.setup_code.is_some(),
    })
}

#[derive(Deserialize)]
pub struct SetupJellyfin {
    code: String,
    #[serde(flatten)]
    server: AddJellyfin,
}

#[derive(Deserialize)]
pub struct SetupPlex {
    code: String,
    #[serde(flatten)]
    server: AddPlex,
}

/// Letters and digits that can't be mixed up when copied from a log.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Wrong setup codes from everyone together before setup is locked until a restart.
const MAX_SETUP_FAILURES: u32 = 100;

/// A setup code like `K7QM-2XRA-9PDW` (about 59 bits): guessing it through the rate limits,
/// even from many addresses, is out of reach.
pub fn new_setup_code() -> String {
    let mut b = [0u8; 12];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b)
        .expect("the system random generator");
    let c: String = b
        .iter()
        .map(|x| CODE_CHARS[*x as usize % CODE_CHARS.len()] as char)
        .collect();
    format!("{}-{}-{}", &c[..4], &c[4..8], &c[8..])
}

fn normalize_code(c: &str) -> Vec<u8> {
    c.bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|b| b.to_ascii_uppercase())
        .collect()
}

fn check_code(s: &Shared, headers: &HeaderMap, code: &str) -> Result<(), Error> {
    use std::sync::atomic::Ordering;
    let Some(want) = &s.setup_code else {
        return Err(Error(
            StatusCode::CONFLICT,
            "this server is already set up: sign in".into(),
        ));
    };
    let ip = crate::limit::client(headers);
    if s.limits.setup_codes.blocked(&ip)
        || s.setup_failures.load(Ordering::Relaxed) >= MAX_SETUP_FAILURES
    {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong codes: restart the AlwyWatch server for a new one".into(),
        ));
    }
    let (a, b) = (normalize_code(code), normalize_code(want));
    let same = a.len() == b.len() && a.iter().zip(&b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0;
    if !same {
        s.limits.setup_codes.hit(&ip);
        s.setup_failures.fetch_add(1, Ordering::Relaxed);
        return Err(Error(
            StatusCode::FORBIDDEN,
            "wrong setup code: it is in the server's log (docker logs alwywatch-server)".into(),
        ));
    }
    Ok(())
}

/// `POST /v1/setup/jellyfin`: the first server, with the setup code and a Jellyfin
/// administrator's login (whoever that is becomes this server's admin).
pub async fn setup_jellyfin(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(r): Json<SetupJellyfin>,
) -> Result<Json<Value>, Error> {
    check_code(&s, &headers, &r.code)?;
    let server = jellyfin_server(&s.http, &r.server).await?;
    save_and_reload(&s, |st| {
        st.ensure_webhook_secret();
        upsert(st, server)
    })
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// `POST /v1/setup/plex`: the first server, with the setup code and the Plex sign-in of the
/// server's owner.
pub async fn setup_plex(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(r): Json<SetupPlex>,
) -> Result<Json<Value>, Error> {
    check_code(&s, &headers, &r.code)?;
    let server = plex_server(&s, &r.server).await?;
    save_and_reload(&s, |st| {
        st.ensure_webhook_secret();
        upsert(st, server)
    })
    .await?;
    Ok(Json(json!({ "ok": true })))
}

// ----- admins -------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SettingsView {
    /// Secrets read as "••••" (send them back as they are to keep them).
    settings: Stored,
    /// Addresses to paste in Jellyfin's / Plex's / Tautulli's webhook settings (when this
    /// site's address is set).
    webhooks: Vec<(String, String)>,
    /// Why Maintainerr is not used although it is set up.
    maintainerr_problem: Option<String>,
}

/// `GET /v1/admin/settings`.
pub async fn get_settings(
    State(s): State<Shared>,
    ServerAdmin(_): ServerAdmin,
) -> Json<SettingsView> {
    let st = s.stored.lock().await.clone();
    let webhooks = match (&st.public_url, &st.webhook_secret) {
        (Some(url), Some(secret)) => ["jellyfin", "plex", "tautulli"]
            .iter()
            .map(|k| {
                (
                    (*k).to_owned(),
                    format!("{}/v1/hooks/{k}/{secret}", url.trim_end_matches('/')),
                )
            })
            .collect(),
        _ => vec![],
    };
    Json(SettingsView {
        settings: st.redacted(),
        webhooks,
        maintainerr_problem: s
            .maintainerr_problem
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
    })
}

/// `PUT /v1/admin/settings`: the whole settings, as `GET` gave them, changed.
pub async fn put_settings(
    State(s): State<Shared>,
    ServerAdmin(id): ServerAdmin,
    Json(new): Json<Stored>,
) -> Result<Json<Value>, Error> {
    save_and_reload(&s, |st| {
        let merged = st.merged_from(new);
        if merged.servers.is_empty() {
            return Err(bad(
                "keep at least one media server: without one, nobody can sign in",
            ));
        }
        *st = merged;
        Ok(())
    })
    .await?;
    s.db.log(&id.user_key, "settings", "server settings changed");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct AddJellyfin {
    url: String,
    #[serde(default)]
    internal_url: Option<String>,
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub struct AddPlex {
    url: String,
    #[serde(default)]
    internal_url: Option<String>,
    /// The Plex sign-in of the server's owner. Kept as the server's credential: with it
    /// AlwyWatch signs the users' new devices in (their server token from plex.tv, as Tautulli
    /// reads it), so nobody signs in to Plex again on each device.
    token: String,
}

/// `POST /v1/admin/settings/jellyfin`: adds a Jellyfin server (or renews its key) with an
/// administrator's login: AlwyWatch makes its own API key there.
pub async fn add_jellyfin(
    State(s): State<Shared>,
    ServerAdmin(id): ServerAdmin,
    Json(r): Json<AddJellyfin>,
) -> Result<Json<Value>, Error> {
    let server = jellyfin_server(&s.http, &r).await?;
    let name = server.name.clone();
    save_and_reload(&s, |st| upsert(st, server)).await?;
    s.db.log(
        &id.user_key,
        "settings",
        &format!("Jellyfin server {name} added"),
    );
    Ok(Json(json!({ "ok": true, "name": name })))
}

/// `POST /v1/admin/settings/plex`: adds a Plex server the signed-in Plex account owns.
pub async fn add_plex(
    State(s): State<Shared>,
    ServerAdmin(id): ServerAdmin,
    Json(r): Json<AddPlex>,
) -> Result<Json<Value>, Error> {
    let server = plex_server(&s, &r).await?;
    let name = server.name.clone();
    save_and_reload(&s, |st| upsert(st, server)).await?;
    s.db.log(
        &id.user_key,
        "settings",
        &format!("Plex server {name} added"),
    );
    Ok(Json(json!({ "ok": true, "name": name })))
}

/// The same server again (same id) replaces the old entry, keeping its key when the new one
/// has none.
/// Adds a server, or renews the key of the same server at the same address. A server answering
/// with the id of one already here, at another address or of another kind, is refused: moving
/// a server means removing it first (else any host could take an existing server's place).
fn upsert(st: &mut Stored, mut server: StoredServer) -> Result<(), Error> {
    if let Some(old) = st.servers.iter_mut().find(|x| x.id == server.id) {
        let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
        if old.kind != server.kind || !same(&old.public_url, &server.public_url) {
            return Err(Error(
                StatusCode::CONFLICT,
                format!(
                    "{} is already set up at {}: remove it first to move it",
                    old.name, old.public_url
                ),
            ));
        }
        if server.api_key.is_none() {
            server.api_key = old.api_key.take();
        }
        // Renewing a key from the server list sends no internal address: keep the one set.
        if server.internal_url.is_none() {
            server.internal_url = old.internal_url.take();
        }
        *old = server;
    } else {
        st.servers.push(server);
    }
    Ok(())
}

fn base(url: &str) -> Result<String, Error> {
    net::normalize_base_url(url).map_err(|e| bad(e.to_string()))
}

fn optional_base(url: &Option<String>) -> Result<Option<String>, Error> {
    match url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        None => Ok(None),
        Some(u) => base(u).map(Some),
    }
}

const JF_CLIENT: &str = "MediaBrowser Client=\"AlwyWatch\", Device=\"AlwyWatch server\", \
                         DeviceId=\"alwywatch-server-settings\", Version=\"1\"";

/// Reads the Jellyfin server, checks the login is an administrator's, and gets an API key
/// named "AlwyWatch" (the existing one, or a new one).
async fn jellyfin_server(http: &Client, r: &AddJellyfin) -> Result<StoredServer, Error> {
    let public = base(&r.url)?;
    let internal = optional_base(&r.internal_url)?;
    let at = internal.clone().unwrap_or_else(|| public.clone());
    let unreachable = |e: net::Error| {
        Error(
            StatusCode::BAD_GATEWAY,
            format!("Can't reach Jellyfin at {at}: {e}"),
        )
    };
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Info {
        id: String,
        server_name: String,
    }
    let info: Info = net::send_json(http.get(format!("{at}/System/Info/Public")))
        .await
        .map_err(unreachable)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct AuthUser {
        policy: Option<crate::jellyfin_admin::JfPolicy>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Auth {
        access_token: String,
        user: AuthUser,
    }
    let auth: Auth = net::send_json(
        http.post(format!("{at}/Users/AuthenticateByName"))
            .header("Authorization", JF_CLIENT)
            .json(&json!({ "Username": r.username.trim(), "Pw": r.password })),
    )
    .await
    .map_err(|e| match e {
        net::Error::Unauthorized => Error(
            StatusCode::UNAUTHORIZED,
            "Wrong Jellyfin username or password.".into(),
        ),
        e => unreachable(e),
    })?;
    let with_token = |m: Method, path: &str| {
        http.request(m, format!("{at}{path}")).header(
            "Authorization",
            format!("{JF_CLIENT}, Token=\"{}\"", auth.access_token),
        )
    };
    let result = async {
        if !auth.user.policy.is_some_and(|p| p.is_administrator) {
            return Err(Error(
                StatusCode::FORBIDDEN,
                "This Jellyfin login is not an administrator: use one that can manage the server."
                    .into(),
            ));
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Key {
            access_token: String,
            #[serde(default)]
            app_name: String,
            #[serde(default)]
            date_revoked: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Keys {
            items: Vec<Key>,
        }
        let find = |k: Keys| {
            k.items
                .into_iter()
                .rev()
                .find(|k| k.app_name == "AlwyWatch" && k.date_revoked.is_none())
                .map(|k| k.access_token)
        };
        let keys: Keys = net::send_json(with_token(Method::GET, "/Auth/Keys"))
            .await
            .map_err(unreachable)?;
        let key = match find(keys) {
            Some(k) => k,
            None => {
                net::send(with_token(Method::POST, "/Auth/Keys").query(&[("app", "AlwyWatch")]))
                    .await
                    .map_err(unreachable)?;
                let keys: Keys = net::send_json(with_token(Method::GET, "/Auth/Keys"))
                    .await
                    .map_err(unreachable)?;
                find(keys).ok_or(Error(
                    StatusCode::BAD_GATEWAY,
                    "Jellyfin did not make the API key".into(),
                ))?
            }
        };
        Ok(StoredServer {
            id: info.id.clone(),
            kind: Some(ServerType::Jellyfin),
            name: info.server_name.clone(),
            public_url: public.clone(),
            internal_url: internal.clone(),
            api_key: Some(key),
        })
    }
    .await;
    // The administrator's session ends here: only the API key stays.
    let _ = net::send(with_token(Method::POST, "/Sessions/Logout")).await;
    result
}

/// Reads the Plex server and checks the sign-in is its owner's.
async fn plex_server(s: &Shared, r: &AddPlex) -> Result<StoredServer, Error> {
    let public = base(&r.url)?;
    let internal = optional_base(&r.internal_url)?;
    let at = internal.clone().unwrap_or_else(|| public.clone());
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Root {
        #[serde(rename = "MediaContainer")]
        container: Container,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Container {
        machine_identifier: String,
        #[serde(default)]
        friendly_name: String,
    }
    let root: Root = net::send_json(
        s.http
            .get(format!("{at}/"))
            .header("X-Plex-Token", &r.token)
            .header("Accept", "application/json"),
    )
    .await
    .map_err(|e| {
        Error(
            StatusCode::BAD_GATEWAY,
            format!("Can't reach Plex at {at}: {e}"),
        )
    })?;
    let conf = ServerConf {
        id: root.container.machine_identifier.clone(),
        kind: ServerType::Plex,
        name: root.container.friendly_name.clone(),
        public_url: public.clone(),
        internal_url: internal.clone(),
    };
    let me = s.auth.identify(&conf, &r.token).await.map_err(|_| {
        Error(
            StatusCode::UNAUTHORIZED,
            "This Plex sign-in can't be checked: sign in to Plex again".into(),
        )
    })?;
    if !me.is_admin {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Only the Plex account that owns this server can add it.".into(),
        ));
    }
    Ok(StoredServer {
        id: conf.id,
        kind: Some(ServerType::Plex),
        name: conf.name,
        public_url: public,
        internal_url: internal,
        api_key: Some(r.token.clone()),
    })
}

// ----- Test buttons -------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TestRequest {
    /// "server", "maintainerr", "tautulli", "seerr", "email", "pocketid".
    what: String,
    /// The settings as edited (not saved yet), secrets as "••••" when unchanged.
    settings: Stored,
    /// "server": which one.
    #[serde(default)]
    server_id: Option<String>,
    /// "email": also send a test email there.
    #[serde(default)]
    email_to: Option<String>,
}

#[derive(Serialize)]
pub struct TestResult {
    ok: bool,
    message: String,
}

/// `POST /v1/admin/settings/test`: tries one connection with the settings being edited.
pub async fn test(
    State(s): State<Shared>,
    ServerAdmin(id): ServerAdmin,
    Json(r): Json<TestRequest>,
) -> Json<TestResult> {
    if !s.limits.settings_tests.allow(&id.user_key) {
        return Json(TestResult {
            ok: false,
            message: "too many tests: try again later".into(),
        });
    }
    let st = s.stored.lock().await.clone().merged_from(r.settings);
    let res = run_test(
        &s,
        &st,
        &r.what,
        r.server_id.as_deref(),
        r.email_to.as_deref(),
    )
    .await;
    Json(match res {
        Ok(message) => TestResult { ok: true, message },
        Err(message) => TestResult { ok: false, message },
    })
}

async fn run_test(
    s: &Shared,
    st: &Stored,
    what: &str,
    server_id: Option<&str>,
    email_to: Option<&str>,
) -> Result<String, String> {
    let http = &s.http;
    match what {
        "server" => {
            let x = st
                .servers
                .iter()
                .find(|x| Some(x.id.as_str()) == server_id)
                .ok_or("no such server")?;
            let at = x
                .internal_url
                .clone()
                .filter(|u| !u.trim().is_empty())
                .unwrap_or(x.public_url.clone());
            let at = at.trim_end_matches('/');
            match x.kind {
                Some(ServerType::Jellyfin) => {
                    let key = x
                        .api_key
                        .as_deref()
                        .ok_or("No API key: add the server again with an administrator's login")?;
                    let v: Value = net::send_json(
                        http.get(format!("{at}/System/Info"))
                            .header("Authorization", format!("MediaBrowser Token=\"{key}\"")),
                    )
                    .await
                    .map_err(|e| format!("Jellyfin at {at}: {e}"))?;
                    Ok(format!(
                        "Jellyfin {} answers, and the API key works",
                        v["Version"].as_str().unwrap_or("")
                    ))
                }
                _ => {
                    net::send(http.get(format!("{at}/identity")))
                        .await
                        .map_err(|e| format!("Plex at {at}: {e}"))?;
                    Ok("Plex answers".into())
                }
            }
        }
        "maintainerr" => {
            let m = st.maintainerr.as_ref().ok_or("Maintainerr is off")?;
            let v = maintainerr::Maintainerr::new(http.clone(), &m.url, m.api_key.clone())
                .version()
                .await
                .map_err(|e| format!("Maintainerr at {}: {e}", m.url))?;
            if v.starts_with("3.") {
                Ok(format!("Maintainerr {v} answers"))
            } else {
                Err(format!(
                    "Maintainerr {v} answers, but this version of AlwyWatch only works with \
                     Maintainerr 3.x"
                ))
            }
        }
        "tautulli" => {
            let t = st.tautulli.as_ref().ok_or("Tautulli is off")?;
            let key = t.api_key.as_deref().ok_or("Tautulli needs its API key")?;
            let v: Value = net::send_json(
                http.get(format!("{}/api/v2", t.url.trim_end_matches('/')))
                    .query(&[("apikey", key), ("cmd", "get_tautulli_info")]),
            )
            .await
            .map_err(|e| format!("Tautulli at {}: {e}", t.url))?;
            if v["response"]["result"] == "success" {
                Ok(format!(
                    "Tautulli {} answers, and the API key works",
                    v["response"]["data"]["tautulli_version"]
                        .as_str()
                        .unwrap_or("")
                ))
            } else {
                Err("Tautulli refused the API key".into())
            }
        }
        "seerr" => {
            let x = st
                .seerr
                .as_ref()
                .ok_or("Requests through AlwyWatch are off")?;
            let key = x.api_key.as_deref().ok_or("Seerr needs its API key")?;
            let v: Value = net::send_json(
                http.get(format!(
                    "{}/api/v1/settings/main",
                    x.url.trim_end_matches('/')
                ))
                .header("X-Api-Key", key),
            )
            .await
            .map_err(|e| match e {
                net::Error::Unauthorized => "Seerr refused the API key".to_owned(),
                e => format!("Seerr at {}: {e}", x.url),
            })?;
            let kind = match v["mediaServerType"].as_i64() {
                Some(1) => " (set up for Plex)",
                Some(2) => " (set up for Jellyfin)",
                Some(3) => " (set up for Emby)",
                _ => "",
            };
            Ok(format!("Seerr answers, and the API key works{kind}"))
        }
        "email" => {
            let m = st.smtp.as_ref().ok_or("Email is off")?;
            let mailer = crate::mail::Mailer::from_stored(m)?;
            mailer
                .test()
                .await
                .map_err(|e| format!("{}: {e}", mailer.describe))?;
            match email_to.map(str::trim).filter(|t| !t.is_empty()) {
                None => Ok(format!("{} accepts the sign-in", mailer.describe)),
                Some(to) => {
                    if !crate::mail::valid_address(to) {
                        return Err(format!("'{to}' is not an email address"));
                    }
                    mailer
                        .send(
                            to,
                            &mailer.from_name,
                            "AlwyWatch: test email",
                            "Emails from AlwyWatch reach you.".into(),
                            "<p>Emails from AlwyWatch reach you.</p>".into(),
                        )
                        .await
                        .map_err(|e| format!("Sending to {to}: {e}"))?;
                    Ok(format!("Sent a test email to {to}"))
                }
            }
        }
        "pocketid" => {
            let p = st.pocketid.as_ref().ok_or("Pocket ID is off")?;
            let v: Value = net::send_json(http.get(format!(
                "{}/.well-known/openid-configuration",
                p.issuer.trim().trim_end_matches('/')
            )))
            .await
            .map_err(|e| format!("Pocket ID at {}: {e}", p.issuer))?;
            if v["issuer"].is_string() {
                Ok("Pocket ID answers".into())
            } else {
                Err("This address doesn't look like Pocket ID".into())
            }
        }
        _ => Err("unknown test".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keep_service::ServerType;

    fn server(id: &str, url: &str) -> StoredServer {
        StoredServer {
            id: id.into(),
            kind: Some(ServerType::Jellyfin),
            name: "HomeJelly".into(),
            public_url: url.into(),
            internal_url: None,
            api_key: None,
        }
    }

    #[test]
    fn a_server_answering_with_another_ones_id_cant_take_its_place() {
        let mut st = Stored {
            servers: vec![StoredServer {
                api_key: Some("key".into()),
                internal_url: Some("http://jellyfin:8096".into()),
                ..server("abc", "https://jf.example.org")
            }],
            ..Default::default()
        };
        // Same server, same address: the key is renewed (or kept), the internal address too.
        assert!(upsert(&mut st, server("abc", "https://jf.example.org/")).is_ok());
        assert_eq!(st.servers[0].api_key.as_deref(), Some("key"));
        assert_eq!(
            st.servers[0].internal_url.as_deref(),
            Some("http://jellyfin:8096")
        );
        // Same id elsewhere, or as another kind: refused.
        assert!(upsert(&mut st, server("abc", "https://evil.example.org")).is_err());
        let mut plex = server("abc", "https://jf.example.org");
        plex.kind = Some(ServerType::Plex);
        assert!(upsert(&mut st, plex).is_err());
        assert!(
            st.servers[0]
                .public_url
                .starts_with("https://jf.example.org")
        );
        assert!(upsert(&mut st, server("other", "https://two.example.org")).is_ok());
        assert_eq!(st.servers.len(), 2);
    }

    #[test]
    fn setup_codes_are_long_and_forgiving_to_type() {
        let c = new_setup_code();
        assert_eq!(c.len(), 14, "{c}");
        assert_ne!(c, new_setup_code());
        assert_eq!(
            normalize_code(&c.to_lowercase().replace('-', " ")),
            normalize_code(&c)
        );
    }
}
