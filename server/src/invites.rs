// SPDX-License-Identifier: AGPL-3.0-or-later
//! Invitations: a link (and code) that creates an AlwyWatch account, optionally emailed to the
//! person and optionally making their Jellyfin login at the same time, with the same username
//! and password and the access of a Jellyfin user the admin picks.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use i18n::tr_in;
use keep_service::ServerType;
use keep_service::account::{
    Invitations, Invite, InviteInfo, InviteItem, InviteRequest, JellyfinTarget, JellyfinUser,
};

use crate::accounts::{InviteExtra, InviteRow};
use crate::api::{Admin, Error};
use crate::jellyfin_admin::Jellyfin;
use crate::keep::Shared;
use crate::mail::esc;

fn bad(m: impl Into<String>) -> Error {
    Error(StatusCode::BAD_REQUEST, m.into())
}

fn internal(e: impl std::fmt::Display) -> Error {
    eprintln!("[invite] {e}");
    Error(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
}

/// The Jellyfin servers an invitation can make a login on: the server and its API key.
fn jellyfin_target<'a>(
    s: &'a Shared,
    id: &str,
) -> Option<(&'a crate::config::ServerConf, &'a str)> {
    let server = s
        .config
        .servers
        .iter()
        .find(|x| x.id == id && x.kind == ServerType::Jellyfin)?;
    Some((server, s.config.secrets.jellyfin_key(&server.id)?))
}

pub(crate) fn site(s: &Shared) -> Option<String> {
    s.config.accounts.rp.as_ref().map(|rp| rp.origin.clone())
}

/// `POST /v1/account/invites` (admins).
pub async fn create(
    State(s): State<Shared>,
    Admin(id): Admin,
    Json(r): Json<InviteRequest>,
) -> Result<Json<Invite>, Error> {
    let email = r
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_owned);
    // An emailed invitation is for one person.
    let (days, uses) = (
        r.days.clamp(1, 90),
        if email.is_some() {
            1
        } else {
            r.uses.clamp(1, 50)
        },
    );
    let jellyfin = r.jellyfin.clone().filter(|j| !j.is_empty());
    // A leaked link must not make dozens of Jellyfin logins.
    let uses = if jellyfin.is_some() {
        uses.min(10)
    } else {
        uses
    };
    if let Some(j) = &jellyfin
        && jellyfin_target(&s, j).is_none()
    {
        return Err(bad(
            "this Jellyfin server has no API key: add it again in Server settings",
        ));
    }
    let like = r
        .like
        .clone()
        .filter(|l| !l.is_empty() && jellyfin.is_some());
    // The access copied must be a plain user's on that server (never an admin's).
    if let (Some(l), Some((server, key))) = (
        &like,
        jellyfin.as_deref().and_then(|j| jellyfin_target(&s, j)),
    ) {
        let users = Jellyfin::new(&s.http, server, key)
            .users()
            .await
            .map_err(|e| {
                eprintln!("[invite] Jellyfin: {e}");
                Error(
                    StatusCode::BAD_GATEWAY,
                    format!("{} can't be reached right now", server.name),
                )
            })?;
        if !users.iter().any(|u| &u.id == l && u.is_regular()) {
            return Err(bad(
                "choose a Jellyfin user who is not an administrator to copy access from",
            ));
        }
    }
    let note: Option<String> = r
        .note
        .as_deref()
        .map(|n| n.trim().chars().take(500).collect::<String>())
        .filter(|n| !n.is_empty());
    let mailer = match &email {
        None => None,
        Some(e) => {
            if !crate::mail::valid_address(e) {
                return Err(bad(format!("'{e}' is not an email address")));
            }
            let m = s.config.accounts.mail.as_ref().ok_or(Error(
                StatusCode::NOT_IMPLEMENTED,
                "set up email first: Settings → Admin → Server settings → Email".into(),
            ))?;
            if site(&s).is_none() {
                return Err(Error(
                    StatusCode::NOT_IMPLEMENTED,
                    "set this site's address first (Server settings): the email links to it".into(),
                ));
            }
            if !s.limits.invite_emails.allow(&id.user_key) {
                return Err(Error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many invitations sent, try again in an hour".into(),
                ));
            }
            Some(m)
        }
    };
    let extra = InviteExtra {
        email: email.clone(),
        jellyfin: jellyfin.clone(),
        like_user: like,
        note: note.clone(),
    };
    let (code, row) =
        s.db.create_invite_with(&id.user_key, days, uses, &extra)
            .map_err(internal)?;
    let url = site(&s).map(|o| format!("{o}/join/{code}"));
    if let (Some(mailer), Some(to), Some(link)) = (mailer, &email, &url) {
        let inviter =
            s.db.person_name(&id.user_key)
                .ok()
                .flatten()
                .unwrap_or_else(|| "AlwyWatch".into());
        let server = jellyfin
            .as_deref()
            .and_then(|j| jellyfin_target(&s, j))
            .map(|(x, _)| x.name.clone());
        let lang = r.lang.as_deref().unwrap_or("en");
        let (subject, text, html) = invitation_email(
            lang,
            &inviter,
            server.as_deref(),
            note.as_deref(),
            link,
            days,
        );
        let sender = tr_in!(lang, "{name} via AlwyWatch", name = inviter);
        if let Err(e) = mailer.send(to, &sender, &subject, text, html).await {
            let _ = s.db.revoke_invite(row);
            eprintln!("[invite] email to an invitee failed: {e}");
            return Err(Error(
                StatusCode::BAD_GATEWAY,
                format!("The email could not be sent ({e}). Check Server settings → Email."),
            ));
        }
        // Sent: it replaces any invitation still waiting for this person.
        let _ = s.db.revoke_other_invites_to(to, row);
    }
    s.db.log(
        &id.user_key,
        "invite",
        &format!(
            "invitation {row}{}{}",
            if email.is_some() { ", emailed" } else { "" },
            if jellyfin.is_some() {
                ", with a Jellyfin login"
            } else {
                ""
            }
        ),
    );
    Ok(Json(Invite {
        url,
        code,
        expires_in_days: days,
        uses,
        sent_to: email,
    }))
}

/// The invitation email: (subject, text, HTML), in the inviting admin's language.
fn invitation_email(
    lang: &str,
    inviter: &str,
    jellyfin: Option<&str>,
    note: Option<&str>,
    link: &str,
    days: i64,
) -> (String, String, String) {
    let subject = match jellyfin {
        Some(server) => tr_in!(
            lang,
            "{name} invites you to watch on {server}",
            name = inviter,
            server = server
        ),
        None => tr_in!(lang, "{name} invites you to AlwyWatch", name = inviter),
    };
    let intro = match jellyfin {
        Some(server) => tr_in!(
            lang,
            "{name} invites you to {server}, with AlwyWatch to watch it on your phone, TV, computer or browser.",
            name = inviter,
            server = server
        ),
        None => tr_in!(
            lang,
            "{name} invites you to AlwyWatch, to watch on your phone, TV, computer or browser.",
            name = inviter
        ),
    };
    let what = match jellyfin {
        Some(server) => tr_in!(
            lang,
            "Choose a username and a password: they work in AlwyWatch and in every Jellyfin app on {server}.",
            server = server
        ),
        None => tr_in!(
            lang,
            "Choose a username and a password to create your account."
        ),
    };
    let button = tr_in!(lang, "Create my account");
    let expiry = i18n::trn_in(
        lang,
        "This invitation works once and expires in {n} day.",
        "This invitation works once and expires in {n} days.",
        days as u64,
    );
    let ignore = tr_in!(lang, "Not expecting this? You can ignore this email.");
    let note_text = note
        .map(|n| format!("\n\n“{n}”\n— {inviter}"))
        .unwrap_or_default();
    let text = format!("{intro}{note_text}\n\n{what}\n\n{button}: {link}\n\n{expiry}\n{ignore}\n");
    let note_html = note
        .map(|n| {
            format!(
                "<blockquote style=\"margin:20px 0;padding:12px 16px;border-left:3px solid #7c6cf0;\
                 background:#f4f3fb;color:#333\">{}<br><span style=\"color:#777\">— {}</span></blockquote>",
                esc(n).replace('\n', "<br>"),
                esc(inviter)
            )
        })
        .unwrap_or_default();
    let html = format!(
        "<!doctype html><html><body style=\"margin:0;background:#f6f6f8;font-family:system-ui,\
         -apple-system,Segoe UI,Roboto,sans-serif\"><div style=\"max-width:520px;margin:0 auto;\
         padding:32px 20px\"><div style=\"background:#fff;border-radius:16px;padding:28px\">\
         <div style=\"font-size:22px;font-weight:700;margin-bottom:16px\">AlwyWatch</div>\
         <p style=\"font-size:16px;line-height:1.5;color:#222\">{}</p>{}\
         <p style=\"font-size:15px;line-height:1.5;color:#444\">{}</p>\
         <p style=\"margin:28px 0\"><a href=\"{}\" style=\"background:#7c6cf0;color:#fff;\
         text-decoration:none;padding:12px 22px;border-radius:10px;font-weight:600;\
         display:inline-block\">{}</a></p>\
         <p style=\"font-size:13px;color:#777;line-height:1.5\">{}<br>{}</p>\
         <p style=\"font-size:12px;color:#999;word-break:break-all\">{}</p>\
         </div></div></body></html>",
        esc(&intro),
        note_html,
        esc(&what),
        esc(link),
        esc(&button),
        esc(&expiry),
        esc(&ignore),
        esc(link),
    );
    (subject, text, html)
}

/// `GET /v1/account/invites` (admins).
pub async fn list(State(s): State<Shared>, Admin(id): Admin) -> Result<Json<Invitations>, Error> {
    let mut jellyfin = Vec::new();
    for x in &s.config.servers {
        let Some((server, key)) = jellyfin_target(&s, &x.id) else {
            continue;
        };
        let users = Jellyfin::new(&s.http, server, key)
            .users()
            .await
            .map(|u| {
                u.into_iter()
                    .filter(|u| u.is_regular())
                    .map(|u| JellyfinUser {
                        id: u.id,
                        name: u.name,
                    })
                    .collect()
            })
            .unwrap_or_default();
        jellyfin.push(JellyfinTarget {
            server_id: server.id.clone(),
            name: server.name.clone(),
            users,
            last_like: s.db.last_like(&id.user_key, &server.id).ok().flatten(),
        });
    }
    let names = |key: &str| {
        s.db.person_name(key)
            .ok()
            .flatten()
            .unwrap_or_else(|| "?".into())
    };
    let invites =
        s.db.invites()
            .map_err(internal)?
            .into_iter()
            .map(|i: InviteRow| InviteItem {
                id: i.id,
                created_by: names(&i.created_by),
                created: i.created,
                expires: i.expires,
                uses_left: i.uses_left,
                email: i.extra.email,
                jellyfin: i
                    .extra
                    .jellyfin
                    .as_deref()
                    .and_then(|j| s.config.server(j))
                    .map(|x| x.name.clone()),
                used_by: i.used_by,
            })
            .collect();
    Ok(Json(Invitations {
        mail: s.config.accounts.mail.is_some(),
        public_url: site(&s).is_some(),
        jellyfin,
        invites,
    }))
}

/// `DELETE /v1/account/invites/{id}` (admins).
pub async fn revoke(
    State(s): State<Shared>,
    Admin(id): Admin,
    Path(invite): Path<i64>,
) -> Result<StatusCode, Error> {
    if s.db.revoke_invite(invite).map_err(internal)? {
        s.db.log(
            &id.user_key,
            "invite",
            &format!("invitation {invite} ended"),
        );
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Error(StatusCode::NOT_FOUND, "no such invitation".into()))
    }
}

/// `GET /v1/account/invites/check/{code}` (public): what the sign-up page says.
pub async fn check(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<InviteInfo>, Error> {
    let ip = crate::limit::client(&headers);
    // Every look counts, on its own counter: wrong codes don't lock people on the same
    // address out of signing in.
    if !s.limits.invite_checks.allow(&ip) {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "too many attempts, try again in a few minutes".into(),
        ));
    }
    let Some(i) = s.db.invite(&code).map_err(internal)? else {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "this invitation is unknown, used up or expired".into(),
        ));
    };
    let target = i
        .extra
        .jellyfin
        .as_deref()
        .and_then(|j| jellyfin_target(&s, j));
    Ok(Json(InviteInfo {
        inviter: s
            .db
            .person_name(&i.created_by)
            .ok()
            .flatten()
            .unwrap_or_else(|| "AlwyWatch".into()),
        jellyfin: target.map(|(x, _)| x.name.clone()),
        email: i.extra.email,
        expires: i.expires,
    }))
}

/// For `register`: the Jellyfin login an invitation makes, before the account exists. Returns
/// the server, its key, the new Jellyfin user id; the caller deletes it if the account fails.
pub async fn make_jellyfin_login<'a>(
    s: &'a Shared,
    invite: &InviteRow,
    username: &str,
    password: &str,
) -> Result<Option<(&'a crate::config::ServerConf, &'a str, String)>, Error> {
    let Some(j) = invite.extra.jellyfin.as_deref() else {
        return Ok(None);
    };
    let (server, key) = jellyfin_target(s, j).ok_or(Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "this invitation's Jellyfin server is not set up here any more: ask for a new one".into(),
    ))?;
    let jf = Jellyfin::new(&s.http, server, key);
    let unreachable = |e: net::Error| {
        eprintln!("[invite] Jellyfin: {e}");
        Error(
            StatusCode::BAD_GATEWAY,
            format!(
                "{} can't be reached right now: try again in a moment",
                server.name
            ),
        )
    };
    let users = jf.users().await.map_err(unreachable)?;
    if users.iter().any(|u| u.name.eq_ignore_ascii_case(username)) {
        return Err(Error(
            StatusCode::CONFLICT,
            format!("this username is taken on {}: choose another", server.name),
        ));
    }
    // The access to copy must still be a plain user's; otherwise nothing is made (a new
    // Jellyfin login with the defaults would see every library).
    let like = match &invite.extra.like_user {
        Some(l) => match users.iter().find(|u| &u.id == l) {
            Some(u) if u.is_regular() => Some(l),
            _ => {
                return Err(Error(
                    StatusCode::CONFLICT,
                    "this invitation's Jellyfin access can't be given any more: ask for a new one"
                        .into(),
                ));
            }
        },
        None => None,
    };
    let new = jf.create(username, password).await.map_err(unreachable)?;
    if let Some(like) = like
        && let Err(e) = jf.copy_policy(like, &new).await
    {
        eprintln!("[invite] could not copy a Jellyfin user's access: {e}");
        remove_jellyfin_login(s, server, key, &new).await;
        return Err(unreachable(e));
    }
    Ok(Some((server, key, new)))
}

/// Deletes a Jellyfin login an unfinished sign-up made (retried: it must not stay).
pub async fn remove_jellyfin_login(
    s: &Shared,
    server: &crate::config::ServerConf,
    key: &str,
    id: &str,
) {
    let jf = Jellyfin::new(&s.http, server, key);
    for wait in [0, 2, 10] {
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        match jf.delete(id).await {
            Ok(()) | Err(net::Error::Status { status: 404 }) => return,
            Err(e) => eprintln!("[invite] could not delete an unfinished Jellyfin login: {e}"),
        }
    }
}

/// After the account exists: the Jellyfin login joins it (one person), signing in once to
/// learn who it is the way every sign-in does.
pub async fn join_jellyfin_login(
    s: &Shared,
    server: &crate::config::ServerConf,
    key: &str,
    person: &str,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let jf = Jellyfin::new(&s.http, server, key);
    let token = jf
        .sign_in_once(username, password)
        .await
        .map_err(|e| e.to_string())?;
    let r = match s.auth.identify(server, &token).await {
        Ok(id) => crate::api::link_person(s, person, &id)
            .await
            .map(|_| ())
            .map_err(|e| e.1),
        Err(_) => Err("the new Jellyfin login was refused".into()),
    };
    jf.sign_out(&token).await;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_email_says_who_where_and_until_when() {
        let (subject, text, html) = invitation_email(
            "en",
            "Alex",
            Some("HomeJelly"),
            Some("Bienvenue <3"),
            "https://watch.example.org/join/abc",
            7,
        );
        assert_eq!(subject, "Alex invites you to watch on HomeJelly");
        assert!(text.contains("https://watch.example.org/join/abc"));
        assert!(text.contains("expires in 7 days"));
        assert!(html.contains("Bienvenue &lt;3") && !html.contains("<3"));
        let (fr, ..) = invitation_email("fr", "Alex", None, None, "x", 1);
        assert!(!fr.is_empty());
    }
}
