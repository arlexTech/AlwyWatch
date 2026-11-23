// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signing in with a Jellyfin or Plex account (the servers this AlwyWatch server serves are
//! listed, nothing to type), and the settings page (accounts, AlwyWatch server link).

use i18n::tr;
use keep_service::{KeepClient, ServerEntry, ServerType};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use media_api::jellyfin::JellyfinAuth;
use media_api::plex::PlexAuth;
use media_api::{Error, Session};

use crate::state::{self, Ctx, KeepLink};
use crate::ui::ErrorText;

/// After any sign-in: keep the account, and link it to the AlwyWatch server when that server
/// serves its media server (keep, requests and watch together then work).
async fn finish(c: &Ctx, s: Session, served: &[ServerEntry]) {
    c.add_account(s.clone());
    if c.expired.get_untracked().as_deref() == Some(state::account_key(&s).as_str()) {
        c.expired.set(None);
    }
    if c.keep.get_untracked().is_none() && served.iter().any(|e| e.id == s.server_id) {
        link(c, &s).await;
    }
    // Signed in to an AlwyWatch account too: this sign-in becomes part of it.
    if served.iter().any(|e| e.id == s.server_id)
        && let Some(t) = c.keep_token(&s)
    {
        crate::account::link_media(c, &s.server_id, &t).await;
    }
    // A new account may be the same person as the linked one.
    crate::links::refresh(true).await;
}

async fn link(c: &Ctx, s: &Session) -> bool {
    let Some(k) = c.keep_for(s) else {
        return false;
    };
    match k.me().await {
        Ok(me) => {
            c.set_keep(Some(KeepLink {
                account: state::account_key(s),
                user_name: me.name.clone(),
                is_admin: me.is_admin,
            }));
            got_me(c, me);
            true
        }
        Err(_) => false,
    }
}

/// My profile, fresh from the server: the welcome step while it was never done.
fn got_me(c: &Ctx, me: keep_service::Me) {
    c.welcome.set(!me.profile_set);
    c.set_me(me);
}

/// At start: refreshes the AlwyWatch server link (name, admin right; a refused account is
/// flagged), or makes it when an earlier sign-in couldn't (server unreachable then).
pub async fn check_keep(c: Ctx) {
    if let Some(k) = c.keep.get_untracked() {
        let Some(client) = c.session(&k.account).and_then(|s| c.keep_for(&s)) else {
            return c.set_keep(None);
        };
        match client.me().await {
            Ok(me) => {
                let fresh = KeepLink {
                    account: k.account.clone(),
                    user_name: me.name.clone(),
                    is_admin: me.is_admin,
                };
                if fresh != k {
                    c.set_keep(Some(fresh));
                }
                got_me(&c, me);
            }
            Err(e) => c.failed(&k.account, &e),
        }
        return;
    }
    if c.accounts.get_untracked().is_empty() {
        return;
    }
    let Ok(served) = KeepClient::servers(&c.http, &Ctx::origin()).await else {
        return;
    };
    for s in c.accounts.get_untracked() {
        if served.iter().any(|e| e.id == s.server_id) && link(&c, &s).await {
            return;
        }
    }
}

/// Where a signed-out visitor was heading (a device's code page), kept for a few minutes so
/// the sign-in brings them back there instead of to the home.
const RETURN_TO: &str = "aw.signin.return";

pub fn return_after_sign_in(path: &str) {
    state::save(RETURN_TO, &(path.to_owned(), js_sys::Date::now()));
}

fn after_sign_in() -> String {
    let kept = state::load::<(String, f64)>(RETURN_TO);
    state::remove(RETURN_TO);
    match kept {
        Some((path, at)) if path.starts_with("/q/") && js_sys::Date::now() - at < 900_000.0 => path,
        _ => "/".to_owned(),
    }
}

#[component]
pub fn SignIn() -> impl IntoView {
    let c = state::ctx();
    let http = c.http.clone();
    let served = LocalResource::new(move || {
        let http = http.clone();
        async move { KeepClient::servers(&http, &Ctx::origin()).await.ok() }
    });
    let picked = RwSignal::new(None::<ServerEntry>);
    let account = c.account;
    view! {
        <h1>{tr!("Sign in")}</h1>
        <p class="muted">{tr!("Use your Jellyfin or Plex account.")}</p>
        {move || match served.get() {
            None => view! { <crate::ui::Loading /> }.into_any(),
            Some(None) => view! {
                <ErrorText text=tr!("Can't load the list of servers. Check your connection and reload the page.") />
            }.into_any(),
            Some(Some(list)) => {
                let all = list.clone();
                view! {
                    <div class="choices">
                        {list.into_iter().map(|e| {
                            let e2 = e.clone();
                            let kind = match e.kind { ServerType::Jellyfin => "Jellyfin", ServerType::Plex => "Plex" };
                            view! {
                                <button class="choice" on:click=move |_| picked.set(Some(e2.clone()))>
                                    <strong>{e.name.clone()}</strong>
                                    <span class="muted">{kind}</span>
                                </button>
                            }
                        }).collect_view()}
                        <button class="choice" on:click=move |_| picked.set(Some(ServerEntry {
                            id: String::new(), kind: ServerType::Jellyfin, name: tr!("Another Jellyfin server"), url: String::new(),
                        }))>
                            <strong>{tr!("Another Jellyfin server")}</strong>
                            <span class="muted">{tr!("By its address")}</span>
                        </button>
                    </div>
                    {move || picked.get().map(|e| match e.kind {
                        ServerType::Jellyfin => view! { <JellyfinForm entry=e served=all.clone() /> }.into_any(),
                        ServerType::Plex => view! { <PlexForm entry=e served=all.clone() /> }.into_any(),
                    })}
                    {move || match account.get() {
                        Some(a) => view! {
                            <p class="muted small">{
                                let (before, after) = crate::ui::around(&tr!("AlwyWatch account: {name}."), "{name}");
                                view! { {before}<strong>{a.name}</strong>{after} }
                            }" "
                                <leptos_router::components::A href="/account">{tr!("Manage")}</leptos_router::components::A></p>
                        }.into_any(),
                        None => view! {
                            <details>
                                <summary>{tr!("Have an AlwyWatch account instead? Sign in with it")}</summary>
                                <p class="muted small">{tr!("An AlwyWatch account is optional: your server's owner may have invited you to create one. It is not your Jellyfin or Plex login.")}</p>
                                <crate::account::AccountSignIn next="/settings" />
                            </details>
                        }.into_any(),
                    }}
                }.into_any()
            }
        }}
    }
}

#[component]
fn JellyfinForm(entry: ServerEntry, served: Vec<ServerEntry>) -> impl IntoView {
    let c = state::ctx();
    let navigate = use_navigate();
    let url = RwSignal::new(entry.url.clone());
    let user = RwSignal::new(String::new());
    let pass = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let code = RwSignal::new(None::<String>);
    let ask_url = entry.url.is_empty();
    let auth = JellyfinAuth::new(c.http.clone(), c.client.clone());
    let (c1, a1, s1, n1) = (c.clone(), auth.clone(), served.clone(), navigate.clone());
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let (c, auth, served, navigate) = (c1.clone(), a1.clone(), s1.clone(), n1.clone());
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let base = match net::normalize_base_url(&url.get_untracked()) {
                Ok(b) => b,
                Err(e) => {
                    busy.set(false);
                    return error.set(Some(e.to_string()));
                }
            };
            match auth
                .sign_in(&base, &user.get_untracked(), &pass.get_untracked())
                .await
            {
                Ok(s) => {
                    finish(&c, s, &served).await;
                    navigate(&after_sign_in(), Default::default());
                }
                Err(e) => error.set(Some(sign_in_error(&e, &base))),
            }
            busy.set(false);
        });
    };
    let quick = move |_| {
        let (c, auth, served, navigate) =
            (c.clone(), auth.clone(), served.clone(), navigate.clone());
        error.set(None);
        spawn_local(async move {
            let base = match net::normalize_base_url(&url.get_untracked()) {
                Ok(b) => b,
                Err(e) => {
                    busy.set(false);
                    return error.set(Some(e.to_string()));
                }
            };
            let start = match auth.quick_connect_start(&base).await {
                Ok(s) => s,
                // Jellyfin answers 401 when Quick Connect is turned off.
                Err(Error::Unauthorized) => {
                    return error.set(Some(tr!(
                        "Quick Connect is turned off on this server. Use your password."
                    )));
                }
                Err(e) => return error.set(Some(sign_in_error(&e, &base))),
            };
            code.set(Some(start.code.clone()));
            // Up to 5 minutes, every 2 s, while this form is shown (and this code is the one
            // shown: a new code or leaving the page stops it).
            let current =
                || code.try_get_untracked().flatten().as_deref() == Some(start.code.as_str());
            for _ in 0..150 {
                state::sleep(2000).await;
                if !current() {
                    return;
                }
                match auth.quick_connect_poll(&base, &start.secret).await {
                    Ok(s) => {
                        if !current() {
                            return;
                        }
                        finish(&c, s, &served).await;
                        return navigate(&after_sign_in(), Default::default());
                    }
                    Err(Error::Pending) => {}
                    Err(e) => {
                        code.try_set(None);
                        return error.set(Some(sign_in_error(&e, &base)));
                    }
                }
            }
            code.try_set(None);
            error.set(Some(tr!(
                "The Quick Connect code expired. Ask for a new one."
            )));
        });
    };
    view! {
        <form class="form" on:submit=submit>
            <h2>{entry.name.clone()}</h2>
            {ask_url.then(|| view! {
                <label>{tr!("Server address (ask the person who gave you your login)")}<input type="text" inputmode="url" autocapitalize="off" spellcheck="false" placeholder="jellyfin.example.org"
                    prop:value=move || url.get() on:input=move |ev| url.set(event_target_value(&ev)) /></label>
            })}
            <label>{tr!("Username")}<input autocomplete="username" prop:value=move || user.get()
                on:input=move |ev| user.set(event_target_value(&ev)) /></label>
            <label>{tr!("Password")}<input type="password" autocomplete="current-password" prop:value=move || pass.get()
                on:input=move |ev| pass.set(event_target_value(&ev)) /></label>
            <div class="actions">
                <button class="primary" type="submit" disabled=move || busy.get()>{tr!("Sign in")}</button>
                <button type="button" on:click=quick>{tr!("Use a code from another Jellyfin app (Quick Connect)")}</button>
            </div>
            {move || code.get().map(|k| view! {
                <p class="code-hint">{
                    let (before, after) = crate::ui::around(&tr!("In a Jellyfin app where you're signed in, open Quick Connect and enter {code}"), "{code}");
                    view! { {before}<strong class="code">{k}</strong>{after} }
                }</p>

            })}
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        </form>
    }
}

#[component]
fn PlexForm(entry: ServerEntry, served: Vec<ServerEntry>) -> impl IntoView {
    let c = state::ctx();
    let navigate = use_navigate();
    let error = RwSignal::new(None::<String>);
    let waiting = RwSignal::new(false);
    let title = entry.name.clone();
    let start = move |_| {
        let (c, served, navigate, entry) =
            (c.clone(), served.clone(), navigate.clone(), entry.clone());
        error.set(None);
        waiting.set(true);
        // Opened right away (inside the click) so pop-up blockers allow it; filled once the
        // PIN exists.
        let win = web_sys::window().and_then(|w| {
            w.open_with_url_and_target("about:blank", "_blank")
                .ok()
                .flatten()
        });
        spawn_local(async move {
            let auth = PlexAuth::new(c.http.clone(), c.client.clone());
            let plain = |e: Error| sign_in_error(&e, "");
            let result: Result<Option<(Session, String)>, String> = async {
                let Some(w) = &win else {
                    return Err(tr!("Allow pop-ups for this site, then try again."));
                };
                let pin = auth.create_pin(true).await.map_err(plain)?;
                let _ = w.location().set_href(&pin.auth_url);
                let mut token = None;
                // Once the Plex tab is closed, one last look (it may close right after
                // saying yes), then stop. Leaving this page stops too.
                let mut closed = false;
                for _ in 0..150 {
                    state::sleep(2000).await;
                    if waiting.try_get_untracked().is_none() {
                        return Ok(None);
                    }
                    match auth.check_pin(pin.id).await {
                        Ok(t) => {
                            token = Some(t);
                            break;
                        }
                        Err(Error::Pending) if closed => {
                            return Err(tr!("The Plex tab was closed before signing in."));
                        }
                        Err(Error::Pending) => closed = w.closed().unwrap_or(false),
                        Err(e) => return Err(plain(e)),
                    }
                }
                let token = token.ok_or_else(|| tr!("The Plex sign-in timed out."))?;
                let user = auth.user(&token).await.map_err(plain)?;
                let servers = auth.servers(&token).await.map_err(plain)?;
                let server = servers
                    .iter()
                    .find(|s| s.machine_id == entry.id)
                    .or_else(|| servers.first())
                    .ok_or_else(|| tr!("Your Plex account has no server."))?;
                let preferred = (!entry.url.is_empty()).then_some(entry.url.as_str());
                let s = auth
                    .connect(server, &user, preferred)
                    .await
                    .map_err(plain)?;
                Ok(Some((s, token)))
            }
            .await;
            if let Some(w) = win {
                let _ = w.close();
            }
            match result {
                // This page is gone: nothing to show it on.
                Ok(None) => return,
                Ok(Some((s, plex_tv))) => {
                    // The watchlist (My list) lives on plex.tv, under the account's own token.
                    c.set_plex_tv_token(&state::account_key(&s), &plex_tv);
                    finish(&c, s, &served).await;
                    navigate(&after_sign_in(), Default::default());
                }
                Err(e) => error.set(Some(e)),
            }
            waiting.set(false);
        });
    };
    view! {
        <div class="form">
            <h2>{title}</h2>
            <p class="muted">{tr!("Plex opens in a new tab: sign in there, then come back.")}</p>
            <div class="actions">
                <button class="primary" on:click=start disabled=move || waiting.get()>
                    {move || if waiting.get() { tr!("Waiting for Plex…") } else { tr!("Sign in with Plex") }}
                </button>
            </div>
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        </div>
    }
}

/// `base`: the server's address, when known (to explain a blocked http one).
fn sign_in_error(e: &Error, base: &str) -> String {
    match e {
        Error::Unauthorized => tr!("Wrong username or password."),
        Error::Network { .. } if base.starts_with("http://") && page_is_https() => {
            tr!(
                "This site is secure (https), so the browser won't talk to a server on plain http. Use its https address."
            )
        }
        Error::Network { .. } => {
            tr!("Can't reach the server. Check the address and your connection.")
        }

        other => state::plain(other),
    }
}

fn page_is_https() -> bool {
    Ctx::origin().starts_with("https://")
}

/// The sections of Settings: the same, in the same order, on every platform
/// (docs/settings.md). `(id, title)`.
const SECTIONS: [&str; 6] = [
    "servers",
    "alwywatch",
    "profiles",
    "playback",
    "audio",
    "about",
];

/// A section's title, in the selected language.
fn section_title(id: &str) -> String {
    match id {
        "servers" => tr!("Servers"),
        "alwywatch" => tr!("AlwyWatch & requests"),
        "profiles" => tr!("Profiles"),
        "playback" => tr!("Playback"),
        "audio" => tr!("Audio & subtitles"),
        _ => tr!("Language & about"),
    }
}

/// `/settings` and `/settings/:section`: the sections on the left, one shown at a time.
#[component]
pub fn Settings() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let section = Memo::new(move |_| {
        params
            .read()
            .get("section")
            .filter(|s| SECTIONS.iter().any(|id| id == s))
            .unwrap_or_else(|| "servers".into())
    });
    view! {
        <h1>{tr!("Settings")}</h1>
        <div class="settings">
            <nav class="settings-nav" aria-label=tr!("Settings sections")>
                {SECTIONS.iter().map(|id| {
                    let id = *id;
                    view! {
                        <a href=format!("/settings/{id}") aria-current=move || (section.get() == id).then_some("page")>{section_title(id)}</a>
                    }
                }).collect_view()}
            </nav>
            <div class="settings-body">
                {move || match section.get().as_str() {
                    "alwywatch" => view! { <AlwyWatchSection /> }.into_any(),
                    "profiles" => view! { <crate::profiles::ProfilesSettings /> }.into_any(),
                    "playback" => view! { <crate::prefs::PlaybackSettings /> }.into_any(),
                    "audio" => view! { <crate::prefs::AudioSettings /> }.into_any(),
                    "about" => view! {
                        <crate::lang::LanguageSetting />
                        <section class="group">
                            <h2>{tr!("About")}</h2>
                            <p>{format!("AlwyWatch {}", env!("CARGO_PKG_VERSION"))}</p>
                            <p class="muted small">{tr!("Free software under the GNU AGPL v3 or later.")}</p>
                        </section>
                        <section class="group">
                            <h2>{tr!("Storage")}</h2>
                            <p class="muted small">{tr!("Accounts and settings are kept in this browser only. Sign out on shared computers.")}</p>
                        </section>
                    }.into_any(),
                    _ => view! { <ServersSection /> }.into_any(),
                }}
            </div>
        </div>
    }
}

#[component]
fn ServersSection() -> impl IntoView {
    let c = state::ctx();
    view! {
        <section class="group">
            <h2>{tr!("Your Jellyfin and Plex servers")}</h2>
            {move || {
                let c = c.clone();
                c.accounts.get().into_iter().map(|s| {
                    let key = state::account_key(&s);
                    let c = c.clone();
                    let kind = format!("{:?}", s.kind);
                    let (before, after) = crate::ui::around(&tr!("{name} on {server}", server = s.server_name.clone()), "{name}");
                    view! {
                        <div class="row">
                            <div>{before}<strong>{s.user_name.clone()}</strong>{after}
                                <span class="muted">" · "{kind}</span></div>
                            <button on:click=move |_| c.remove_account(&key)>{tr!("Sign out")}</button>
                        </div>
                    }
                }).collect_view()
            }}
            <leptos_router::components::A href="/signin" attr:class="button">{tr!("Add a server")}</leptos_router::components::A>
        </section>
    }
}

#[component]
fn AlwyWatchSection() -> impl IntoView {
    let c2 = state::ctx();
    let c3 = c2.clone();
    view! {
        <section class="group">
            <h2>{tr!("You on AlwyWatch")}</h2>
            {move || match c2.keep.get() {
                Some(k) => view! {
                    <p class="muted small">{
                        let (before, after) = crate::ui::around(&tr!("Signed in as {name} on {site}. Others see this name and avatar.", site = Ctx::origin()), "{name}");
                        view! { {before}<strong>{k.user_name}</strong>{after} }
                    }</p>
                    <crate::links::AlsoSignedIn />
                    <crate::profile::AccountProfile />
                    <crate::links::LinkedSignins />
                }.into_any(),
                None => {
                    // Plex accounts from before the plex.tv token was kept can't sign in here.
                    let old_plex = c2.accounts.get().iter().any(|s| s.kind == media_api::ServerKind::Plex && c2.keep_token(s).is_none());
                    view! {
                        <p class="muted">{tr!("None of your logins is on a server this site works with. Add one: Servers → Add a server.")}</p>
                        {old_plex.then(|| view! { <p class="muted small">{tr!("Plex account: sign out and sign in again to use requests, Leaving soon and watch together.")}</p> })}
                    }.into_any()
                }
            }}
        </section>
        <section class="group">
            <h2>{tr!("Connection and devices")}</h2>
            <div class="row">
                <div class="muted small">{move || match c3.account.get() {
                    Some(a) => tr!("Signed in to the account of {name}. Password, passkeys, Pocket ID and your devices.", name = a.name),
                    None => tr!("Sign in without a media server, with a password, passkey or Pocket ID; sign a TV in with a code."),

                }}</div>
                <leptos_router::components::A href="/account" attr:class="button">{tr!("Account")}</leptos_router::components::A>
            </div>
        </section>
    }
}
