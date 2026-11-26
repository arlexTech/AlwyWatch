// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /v1/account/server-sign-in` (account session): a new device signed in to my AlwyWatch
//! account signs in to my linked media servers without typing anything (docs/keep-protocol.md, "Signing devices in").
//!
//! Only server-level credentials are used, and no user token is ever stored:
//! - Jellyfin: the device starts Quick Connect, this server approves the code as my Jellyfin
//!   user there with its API key, the device polls and gets its own token.
//! - Plex: the owner's plex.tv token (kept when the owner added the server) reads my server
//!   token from plex.tv (`shared_servers`, as Tautulli does); the owner gets their own token.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use keep_service::{ServerSignIn, ServerSignInRequest, ServerType};

use crate::account_api::Session;
use crate::api::Error;
use crate::config::ServerConf;
use crate::db::IdentityRow;
use crate::keep::Shared;

pub async fn server_sign_in(
    State(s): State<Shared>,
    session: Session,
    Json(r): Json<ServerSignInRequest>,
) -> Result<Json<ServerSignIn>, Error> {
    let person = &session.account.person;
    if !s.limits.server_sign_ins.allow(person) {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "too many attempts, try again in a few minutes".into(),
        ));
    }
    let server = s.config.server(&r.server_id).ok_or(Error(
        StatusCode::FORBIDDEN,
        "this server is not served here".into(),
    ))?;
    // The most recently used of my sign-ins there.
    let identities = s.db.identities_of(person).map_err(|e| {
        eprintln!("[sign-in] {e}");
        unavailable("Server error")
    })?;
    let me = identities
        .iter()
        .filter(|i| i.server_id == server.id)
        .max_by_key(|i| i.last_seen)
        .ok_or(Error(
            StatusCode::NOT_FOUND,
            "no sign-in of yours is linked on this server".into(),
        ))?;
    let token = match server.kind {
        ServerType::Jellyfin => {
            jellyfin(&s, server, me, r.code.as_deref().unwrap_or_default()).await?;
            None
        }
        ServerType::Plex => Some(plex(&s, server, me).await?),
    };
    s.db.log(
        person,
        "sign-in",
        &format!("new device signed in to {} from the account", server.name),
    );
    Ok(Json(ServerSignIn {
        kind: server.kind,
        server_id: server.id.clone(),
        server_name: server.name.clone(),
        url: server.public_url.trim_end_matches('/').to_owned(),
        user_id: me.user_id.clone(),
        user_name: crate::together::clean_name(&me.media_name),
        is_admin: me.is_admin,
        token,
    }))
}

fn unavailable(what: &str) -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        format!("{what}: sign in on this device instead"),
    )
}

async fn jellyfin(
    s: &Shared,
    server: &ServerConf,
    me: &IdentityRow,
    code: &str,
) -> Result<(), Error> {
    let code = code.trim();
    if code.is_empty() || code.len() > 32 || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "a Quick Connect code is needed".into(),
        ));
    }
    let key = s
        .config
        .secrets
        .jellyfin_key(&server.id)
        .ok_or_else(|| unavailable("This server has no API key"))?;
    let res = s
        .http
        .post(format!("{}/QuickConnect/Authorize", server.internal()))
        .query(&[("code", code), ("userId", me.user_id.as_str())])
        .header("Authorization", format!("MediaBrowser Token=\"{key}\""))
        .send()
        .await
        .map_err(|e| {
            eprintln!("[sign-in] Jellyfin {}: {e}", server.id);
            unavailable("Jellyfin can't be reached")
        })?;
    match res.status() {
        st if st.is_success() => Ok(()),
        StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST => Err(Error(
            StatusCode::CONFLICT,
            "the Quick Connect code expired, try again".into(),
        )),
        st => {
            eprintln!(
                "[sign-in] Jellyfin {} refused Quick Connect: {st}",
                server.id
            );
            Err(unavailable("Jellyfin refused (is Quick Connect on?)"))
        }
    }
}

async fn plex(s: &Shared, server: &ServerConf, me: &IdentityRow) -> Result<String, Error> {
    let owner = s
        .config
        .secrets
        .plex_owner_token(&server.id)
        .ok_or_else(|| unavailable("The owner has not signed this server in to AlwyWatch"))?;
    if me.is_admin {
        return Ok(owner.to_owned());
    }
    let res = s
        .http
        .get(format!(
            "{}/api/servers/{}/shared_servers",
            s.auth.plex_tv(),
            net::percent_encode(&server.id)
        ))
        .header("X-Plex-Token", owner)
        .header("X-Plex-Client-Identifier", "alwywatch-server")
        .header("X-Plex-Product", "AlwyWatch Server")
        .send()
        .await
        .map_err(|e| {
            eprintln!("[sign-in] plex.tv: {e}");
            unavailable("plex.tv can't be reached")
        })?;
    if !res.status().is_success() {
        eprintln!("[sign-in] plex.tv shared_servers: {}", res.status());
        return Err(unavailable("plex.tv refused the owner's sign-in"));
    }
    let xml = res
        .text()
        .await
        .map_err(|_| unavailable("plex.tv can't be reached"))?;
    shared_token(&xml, &me.user_id).ok_or(Error(
        StatusCode::NOT_FOUND,
        "this server is no longer shared with your Plex account".into(),
    ))
}

/// The `accessToken` of the `<SharedServer>` of plex.tv user `user_id`.
fn shared_token(xml: &str, user_id: &str) -> Option<String> {
    xml.split("<SharedServer")
        .skip(1)
        .map(|el| &el[..el.find('>').unwrap_or(el.len())])
        .find(|el| attr(el, "userID").as_deref() == Some(user_id))
        .and_then(|el| attr(el, "accessToken"))
        .filter(|t| !t.is_empty())
}

fn attr(el: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=\"");
    let at = el.find(&needle)? + needle.len();
    let end = el[at..].find('"')?;
    Some(
        el[at..at + end]
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_shared_token_of_a_user() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<MediaContainer friendlyName="x" identifier="com.plexapp.plugins.myplex" machineIdentifier="m" size="2">
<SharedServer id="1" username="ann" email="a@example.org" userID="11" accessToken="tok-ann" name="HomePlex" acceptedAt="1" invitedAt="1" allowSync="1">
<Section id="1" key="1" title="Films" type="movie" shared="1"/>
</SharedServer>
<SharedServer id="2" username="bob" userID="22" accessToken="tok&amp;bob" name="HomePlex"/>
</MediaContainer>"#;
        assert_eq!(shared_token(xml, "11").as_deref(), Some("tok-ann"));
        assert_eq!(shared_token(xml, "22").as_deref(), Some("tok&bob"));
        assert_eq!(shared_token(xml, "1"), None);
        assert_eq!(shared_token(xml, "33"), None);
    }
}
