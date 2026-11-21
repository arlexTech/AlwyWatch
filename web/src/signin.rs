// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signing in with a Jellyfin or Plex account (the servers this AlwyWatch server serves are
//! listed, nothing to type), and the settings page (accounts, AlwyWatch server link).

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
    if c.keep.get_untracked().is_none()
        && served.iter().any(|e| e.id == s.server_id)
        && let Ok(me) = c.keep_for(&s).me().await
    {
        c.set_keep(Some(KeepLink {
            account: state::account_key(&s),
            user_name: me.name,
            is_admin: me.is_admin,
        }));
    }
}

#[component]
pub fn SignIn() -> impl IntoView {
    let c = state::ctx();
    let http = c.http.clone();
    let served = LocalResource::new(move || {
        let http = http.clone();
        async move {
            KeepClient::servers(&http, &Ctx::origin())
                .await
                .unwrap_or_default()
        }
    });
    let picked = RwSignal::new(None::<ServerEntry>);
    view! {
        <h1>"Sign in"</h1>
        <p class="muted">"Use your Jellyfin or Plex account. Nothing else to create."</p>
        {move || match served.get() {
            None => view! { <crate::ui::Loading /> }.into_any(),
            Some(list) => {
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
                            id: String::new(), kind: ServerType::Jellyfin, name: "Another Jellyfin server".into(), url: String::new(),
                        }))>
                            <strong>"Another Jellyfin server"</strong>
                            <span class="muted">"By its address"</span>
                        </button>
                    </div>
                    {move || picked.get().map(|e| match e.kind {
                        ServerType::Jellyfin => view! { <JellyfinForm entry=e served=all.clone() /> }.into_any(),
                        ServerType::Plex => view! { <PlexForm entry=e served=all.clone() /> }.into_any(),
                    })}
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
                    navigate("/", Default::default());
                }
                Err(e) => error.set(Some(sign_in_error(&e))),
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
                Err(e) => return error.set(Some(sign_in_error(&e))),
            };
            code.set(Some(start.code.clone()));
            // Up to 5 minutes, every 2 s, while this form is shown.
            for _ in 0..150 {
                state::sleep(2000).await;
                if code.get_untracked().as_deref() != Some(start.code.as_str()) {
                    return;
                }
                match auth.quick_connect_poll(&base, &start.secret).await {
                    Ok(s) => {
                        finish(&c, s, &served).await;
                        return navigate("/", Default::default());
                    }
                    Err(Error::Pending) => {}
                    Err(e) => return error.set(Some(sign_in_error(&e))),
                }
            }
            code.set(None);
        });
    };
    view! {
        <form class="form" on:submit=submit>
            <h2>{entry.name.clone()}</h2>
            {ask_url.then(|| view! {
                <label>"Server address"<input type="text" inputmode="url" autocapitalize="off" spellcheck="false" placeholder="jellyfin.example.com"
                    prop:value=move || url.get() on:input=move |ev| url.set(event_target_value(&ev)) /></label>
            })}
            <label>"Username"<input autocomplete="username" prop:value=move || user.get()
                on:input=move |ev| user.set(event_target_value(&ev)) /></label>
            <label>"Password"<input type="password" autocomplete="current-password" prop:value=move || pass.get()
                on:input=move |ev| pass.set(event_target_value(&ev)) /></label>
            <div class="actions">
                <button class="primary" type="submit" disabled=move || busy.get()>"Sign in"</button>
                <button type="button" on:click=quick>"Use Quick Connect"</button>
            </div>
            {move || code.get().map(|k| view! {
                <p class="code-hint">"In a Jellyfin app where you're signed in, open Quick Connect and enter "
                    <strong class="code">{k}</strong></p>
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
            let result: Result<Session, String> = async {
                let pin = auth.create_pin(true).await.map_err(|e| sign_in_error(&e))?;
                match &win {
                    Some(w) => {
                        let _ = w.location().set_href(&pin.auth_url);
                    }
                    None => return Err("Allow pop-ups for this site, then try again.".into()),
                }
                let mut token = None;
                for _ in 0..150 {
                    state::sleep(2000).await;
                    match auth.check_pin(pin.id).await {
                        Ok(t) => {
                            token = Some(t);
                            break;
                        }
                        Err(Error::Pending) => {}
                        Err(e) => return Err(sign_in_error(&e)),
                    }
                }
                let token = token.ok_or("The Plex sign-in timed out.")?;
                let user = auth.user(&token).await.map_err(|e| sign_in_error(&e))?;
                let servers = auth.servers(&token).await.map_err(|e| sign_in_error(&e))?;
                let server = servers
                    .iter()
                    .find(|s| s.machine_id == entry.id)
                    .or_else(|| servers.first())
                    .ok_or("Your Plex account has no server.")?;
                let preferred = (!entry.url.is_empty()).then_some(entry.url.as_str());
                auth.connect(server, &user, preferred)
                    .await
                    .map_err(|e| sign_in_error(&e))
            }
            .await;
            if let Some(w) = win {
                let _ = w.close();
            }
            match result {
                Ok(s) => {
                    finish(&c, s, &served).await;
                    navigate("/", Default::default());
                }
                Err(e) => error.set(Some(e)),
            }
            waiting.set(false);
        });
    };
    view! {
        <div class="form">
            <h2>{title}</h2>
            <p class="muted">"Plex opens in a new tab: sign in there, then come back."</p>
            <div class="actions">
                <button class="primary" on:click=start disabled=move || waiting.get()>
                    {move || if waiting.get() { "Waiting for Plex…" } else { "Sign in with Plex" }}
                </button>
            </div>
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        </div>
    }
}

fn sign_in_error(e: &Error) -> String {
    match e {
        Error::Unauthorized => "Wrong username or password.".into(),
        Error::Network { .. } => "Can't reach the server.".into(),
        other => other.to_string(),
    }
}

#[component]
pub fn Settings() -> impl IntoView {
    let c = state::ctx();
    let c2 = c.clone();
    view! {
        <h1>"Settings"</h1>
        <section class="group">
            <h2>"Accounts"</h2>
            {move || {
                let c = c.clone();
                c.accounts.get().into_iter().map(|s| {
                    let key = state::account_key(&s);
                    let c = c.clone();
                    let kind = format!("{:?}", s.kind);
                    view! {
                        <div class="row">
                            <div><strong>{s.user_name.clone()}</strong>" on "{s.server_name.clone()}
                                <span class="muted">" · "{kind}</span></div>
                            <button on:click=move |_| c.remove_account(&key)>"Sign out"</button>
                        </div>
                    }
                }).collect_view()
            }}
            <leptos_router::components::A href="/signin" attr:class="button">"Add an account"</leptos_router::components::A>
        </section>
        <section class="group">
            <h2>"AlwyWatch server"</h2>
            {move || match c2.keep.get() {
                Some(k) => view! { <p>"Signed in as "<strong>{k.user_name}</strong>{k.is_admin.then_some(" · admin")}
                    " on "{Ctx::origin()}</p> }.into_any(),
                None => view! { <p class="muted">"None of your accounts is on a server this site serves."</p> }.into_any(),
            }}
        </section>
        <h2>"Playback"</h2>
        <crate::prefs::PlaybackSettings />
        <p class="muted small">"Accounts are kept in this browser only. Sign out on shared computers."</p>
    }
}
