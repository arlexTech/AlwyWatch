// SPDX-License-Identifier: AGPL-3.0-or-later
//! The first step (sign in to AlwyWatch or create an account), signing in with a Jellyfin or
//! Plex login (the servers this AlwyWatch server serves are listed, nothing to type), and the
//! settings page with its sections (docs/settings.md).

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
pub async fn finish(c: &Ctx, s: Session, served: &[ServerEntry]) {
    c.add_account(s.clone());
    if c.expired.get_untracked().as_deref() == Some(state::account_key(&s).as_str()) {
        c.expired.set(None);
    }
    if !c.keep_off()
        && c.keep.get_untracked().is_none()
        && served.iter().any(|e| e.id == s.server_id)
    {
        link(c, &s).await;
    }
    // Signed in to an AlwyWatch account too: this sign-in becomes part of it.
    if served.iter().any(|e| e.id == s.server_id)
        && let Some(t) = c.keep_token(&s)
    {
        crate::account::link_media(c, &s.server_id, &t).await;
    }
    // A profile added without a name takes the person's.
    let name = c.me.get_untracked().map(|m| m.name).unwrap_or(s.user_name);
    crate::profiles::adopt_name(&name);
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
    if c.accounts.get_untracked().is_empty() || c.keep_off() {
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

/// The first step of a profile (first visit, "Add a profile"): sign in or create an account.
#[component]
pub fn StartCards(
    on_sign_in: Callback<()>,
    on_create: Callback<()>,
    on_media: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="choices">
            <button class="choice" on:click=move |_| on_sign_in.run(())>
                <strong>{tr!("Sign in to AlwyWatch")}</strong>
                <span class="muted">{tr!("Your name, settings and servers come with you")}</span>
            </button>
            <button class="choice" on:click=move |_| on_create.run(())>
                <strong>{tr!("Create an AlwyWatch account")}</strong>
                <span class="muted">{tr!("If someone invited you")}</span>
            </button>
            <button class="choice" on:click=move |_| on_media.run(())>
                <strong>{tr!("Jellyfin or Plex only")}</strong>
                <span class="muted">{tr!("No AlwyWatch account: your logins and settings stay in this browser")}</span>
            </button>
        </div>
    }
}

/// `/signin`, and every page while this profile has no login. A profile with nothing yet
/// starts with the two cards ("Welcome to AlwyWatch"); `?step=signin` goes straight to the
/// ways to sign in. With a login or an account already, it adds a server.
#[component]
pub fn SignIn() -> impl IntoView {
    let c = state::ctx();
    let fresh = c.accounts.get_untracked().is_empty() && c.account.get_untracked().is_none();
    let step = leptos_router::hooks::use_query_map().with_untracked(|q| q.get("step"));
    let asked = step.as_deref() == Some("signin");
    let media_only = RwSignal::new(step.as_deref() == Some("media"));
    let signing_in = RwSignal::new(asked || media_only.get_untracked());
    let navigate = use_navigate();
    let create = Callback::new(move |()| navigate("/join", Default::default()));
    let sign_in = Callback::new(move |()| signing_in.set(true));
    let c2 = c.clone();
    let media = Callback::new(move |()| {
        // Never linked to the AlwyWatch server by itself ("Connect to AlwyWatch" does it).
        c2.set_keep_off(true);
        media_only.set(true);
        signing_in.set(true);
    });
    move || {
        if fresh && !signing_in.get() {
            view! {
                <h1>{tr!("Welcome to AlwyWatch")}</h1>
                <StartCards on_sign_in=sign_in on_create=create on_media=media />
            }
            .into_any()
        } else {
            let media = media_only.get();
            let to_alwywatch = (fresh || asked) && !media;
            let c = c.clone();
            view! {
                <h1>{if to_alwywatch { tr!("Sign in to AlwyWatch") } else if media { tr!("Sign in with Jellyfin or Plex") } else { tr!("Add a server") }}</h1>
                {(fresh && !asked).then(|| view! {
                    <p><button class="small subtle" on:click=move |_| {
                        if media_only.get_untracked() {
                            c.set_keep_off(false);
                            media_only.set(false);
                        }
                        signing_in.set(false)
                    }>"‹ "{tr!("Back")}</button></p>
                })}
                <SignInWays to_alwywatch=to_alwywatch />
            }
            .into_any()
        }
    }
}

/// The servers to sign in to with a Jellyfin or Plex login; `to_alwywatch`: the AlwyWatch
/// account's own ways (password, passkey, Pocket ID) too.
#[component]
fn SignInWays(to_alwywatch: bool) -> impl IntoView {
    let c = state::ctx();
    let http = c.http.clone();
    let served = LocalResource::new(move || {
        let http = http.clone();
        async move { KeepClient::servers(&http, &Ctx::origin()).await.ok() }
    });
    let picked = RwSignal::new(None::<ServerEntry>);
    let account = c.account;
    view! {
        <p class="muted">{if to_alwywatch {
            tr!("With your Jellyfin or Plex login, or below with your AlwyWatch account.")
        } else {
            tr!("Sign in with your Jellyfin or Plex login.")
        }}</p>
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
                }.into_any()
            }
        }}
        {move || match account.get() {
            Some(a) => view! {
                <p class="muted small">{
                    let (before, after) = crate::ui::around(&tr!("AlwyWatch account: {name}."), "{name}");
                    view! { {before}<strong>{a.name}</strong>{after} }
                }" "
                    <leptos_router::components::A href="/settings/account">{tr!("Manage")}</leptos_router::components::A></p>
            }.into_any(),
            None => to_alwywatch.then(|| view! {
                <crate::account::AccountSignIn next="/" />
            }).into_any(),
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
        let win = open_plex_tab();
        spawn_local(async move {
            let result: Result<Option<(Session, String)>, String> = async {
                let Some(token) =
                    plex_tv_token(&c, win.as_ref(), || waiting.try_get_untracked().is_some())
                        .await?
                else {
                    return Ok(None);
                };
                let s = plex_session(&c, &token, &entry.id, &entry.url).await?;
                Ok(Some((s, token)))
            }
            .await;
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

/// The Plex login on the server `id` (else the account's first one) for a plex.tv token,
/// reached at `url` when given.
pub async fn plex_session(c: &Ctx, token: &str, id: &str, url: &str) -> Result<Session, String> {
    let auth = PlexAuth::new(c.http.clone(), c.client.clone());
    let plain = |e: Error| sign_in_error(&e, "");
    let user = auth.user(token).await.map_err(plain)?;
    let servers = auth.servers(token).await.map_err(plain)?;
    let server = servers
        .iter()
        .find(|s| s.machine_id == id)
        .or_else(|| servers.first())
        .ok_or_else(|| tr!("Your Plex account has no server."))?;
    let preferred = (!url.is_empty()).then_some(url);
    auth.connect(server, &user, preferred).await.map_err(plain)
}

/// A tab for plex.tv's sign-in, opened right away (inside the click) so pop-up blockers allow
/// it; [`plex_tv_token`] fills it once the PIN exists.
pub fn open_plex_tab() -> Option<web_sys::Window> {
    web_sys::window().and_then(|w| {
        w.open_with_url_and_target("about:blank", "_blank")
            .ok()
            .flatten()
    })
}

/// Signs in to plex.tv in `win` (from [`open_plex_tab`]) with a PIN: the account's token, or
/// None when `alive` says the page asking is gone. The tab is closed at the end.
pub async fn plex_tv_token(
    c: &Ctx,
    win: Option<&web_sys::Window>,
    alive: impl Fn() -> bool,
) -> Result<Option<String>, String> {
    let auth = PlexAuth::new(c.http.clone(), c.client.clone());
    let plain = |e: Error| sign_in_error(&e, "");
    let result = async {
        let Some(w) = win else {
            return Err(tr!("Allow pop-ups for this site, then try again."));
        };
        let pin = auth.create_pin(true).await.map_err(plain)?;
        let _ = w.location().set_href(&pin.auth_url);
        // Once the Plex tab is closed, one last look (it may close right after saying yes),
        // then stop. Leaving the page stops too.
        let mut closed = false;
        for _ in 0..150 {
            state::sleep(2000).await;
            if !alive() {
                return Ok(None);
            }
            match auth.check_pin(pin.id).await {
                Ok(t) => return Ok(Some(t)),
                Err(Error::Pending) if closed => {
                    return Err(tr!("The Plex tab was closed before signing in."));
                }
                Err(Error::Pending) => closed = w.closed().unwrap_or(false),
                Err(e) => return Err(plain(e)),
            }
        }
        Err(tr!("The Plex sign-in timed out."))
    }
    .await;
    if let Some(w) = win {
        let _ = w.close();
    }
    result
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
/// (docs/settings.md). The web has no "Menu & pages" nor "Downloads"; "Admin" is for admins.
const SECTIONS: [&str; 7] = [
    "account", "servers", "requests", "playback", "audio", "device", "admin",
];

/// A section's title, in the selected language.
fn section_title(id: &str) -> String {
    match id {
        "account" => tr!("Account"),
        "servers" => tr!("Servers"),
        "requests" => tr!("Requests"),
        "playback" => tr!("Playback"),
        "audio" => tr!("Audio & subtitles"),
        "admin" => tr!("Admin"),
        _ => tr!("This device"),
    }
}

/// Section ids of earlier versions (bookmarks, links in the apps) → today's.
fn current_section(id: &str) -> &str {
    match id {
        "alwywatch" | "services" => "account",
        "profiles" | "about" => "device",
        other => other,
    }
}

/// `/settings`, `/settings/:section` and `/settings/account/password`: the sections on the
/// left, one shown at a time.
#[component]
pub fn Settings() -> impl IntoView {
    let c = state::ctx();
    let (keep, me) = (c.keep, c.me);
    let admin = Memo::new(move |_| {
        keep.with(|k| k.as_ref().is_some_and(|k| k.is_admin))
            || me.with(|m| m.as_ref().is_some_and(|m| m.is_admin))
    });
    let params = leptos_router::hooks::use_params_map();
    let navigate = use_navigate();
    // An old section's address shows its new one.
    Effect::new(move |_| {
        let Some(id) = params.read().get("section") else {
            return;
        };
        let now = current_section(&id);
        if now != id {
            navigate(
                &format!("/settings/{now}"),
                leptos_router::NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            );
        }
    });
    let section = Memo::new(move |_| {
        params
            .read()
            .get("section")
            .map(|s| current_section(&s).to_owned())
            .filter(|s| SECTIONS.contains(&s.as_str()) && (s != "admin" || admin.get()))
            .unwrap_or_else(|| "account".into())
    });
    let sub = Memo::new(move |_| params.read().get("sub"));
    view! {
        <h1>{tr!("Settings")}</h1>
        <div class="settings">
            <nav class="settings-nav" aria-label=tr!("Settings sections")>
                {SECTIONS.iter().map(|id| {
                    let id = *id;
                    view! {
                        <Show when=move || id != "admin" || admin.get()>
                            <a href=format!("/settings/{id}") aria-current=move || (section.get() == id).then_some("page")>{section_title(id)}</a>
                        </Show>
                    }
                }).collect_view()}
            </nav>
            <div class="settings-body">
                {move || match (section.get().as_str(), sub.get().as_deref()) {
                    ("account", Some("password")) => view! { <crate::account::PasswordAndPasskeys /> }.into_any(),
                    ("account", _) => view! { <crate::account::AccountSection /> }.into_any(),
                    ("servers", _) => view! { <ServersSection /> }.into_any(),
                    ("requests", _) => view! { <crate::request::RequestsSettings /> }.into_any(),
                    ("playback", _) => view! { <crate::prefs::PlaybackSettings /> }.into_any(),
                    ("audio", _) => view! { <crate::prefs::AudioSettings /> }.into_any(),
                    ("admin", Some("server")) => view! { <crate::admin::ServerSettingsPage /> }.into_any(),
                    ("admin", _) => view! {
                        <section class="group">
                            <leptos_router::components::A href="/settings/admin/server" attr:class="row link-row">
                                <div><strong>{tr!("Server settings")}</strong>
                                    <div class="muted small">{tr!("Media servers, Maintainerr, Seerr, Tautulli, email, sign-up")}</div></div>
                                <span>"›"</span>
                            </leptos_router::components::A>
                        </section>
                        <crate::keep::Admin />
                        <crate::admin::Invitations />
                        <crate::keep::ServerStatusView />
                    }.into_any(),
                    _ => view! {
                        <crate::profiles::ProfilesSettings />
                        <crate::account::SignInAnotherDevice />
                        <crate::lang::LanguageSetting />
                        <section class="group">
                            <h2>{tr!("Storage")}</h2>
                            <p class="muted small">{tr!("Accounts and settings are kept in this browser only. Sign out on shared computers.")}</p>
                        </section>
                        <section class="group">
                            <h2>{tr!("About")}</h2>
                            <p>{format!("AlwyWatch {}", env!("CARGO_PKG_VERSION"))}</p>
                            <p class="muted small">{tr!("Free software under the GNU AGPL v3 or later.")}</p>
                        </section>
                    }.into_any(),
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
