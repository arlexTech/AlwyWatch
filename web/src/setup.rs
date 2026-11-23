// SPDX-License-Identifier: AGPL-3.0-or-later
//! The first start of an AlwyWatch server (`GET /v1/setup` says `needed`): every address
//! shows this page until a first media server is set, with the setup code from the server's
//! log and a Jellyfin administrator's login or the Plex owner's sign-in. That person then
//! lands signed in, as this server's admin.

use i18n::tr;
use keep_service::KeepClient;
use keep_service::settings::{AddJellyfin, AddPlex};
use leptos::prelude::*;
use leptos::task::spawn_local;
use media_api::jellyfin::JellyfinAuth;

use crate::admin::wait_for_restart;
use crate::state::{self, Ctx};
use crate::ui::ErrorText;

/// Whether this server still needs setting up (checked once, in the background).
pub fn needed() -> RwSignal<bool> {
    let needed = RwSignal::new(false);
    let http = state::ctx().http;
    spawn_local(async move {
        if let Ok(s) = KeepClient::setup_state(&http, &Ctx::origin()).await {
            needed.set(s.needed);
        }
    });
    needed
}

fn go(to: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.location().set_href(to);
    }
}

fn explain(e: &net::Error) -> String {
    match e {
        net::Error::Unauthorized => tr!(
            "Wrong setup code. It is in the AlwyWatch server's log: docker logs alwywatch-server"
        ),
        net::Error::Status { status: 409 } => tr!("This server is already set up: sign in."),
        net::Error::Invalid { message } => message.clone(),
        other => state::plain_keep(other),
    }
}

/// Signed in with the login that set the server up, once it runs with it.
async fn sign_in_after(
    c: &Ctx,
    jellyfin: Option<(String, String, String)>,
    plex: Option<(String, String)>,
) {
    let served = KeepClient::servers(&c.http, &Ctx::origin())
        .await
        .unwrap_or_default();
    let session = match (jellyfin, plex) {
        (Some((url, user, pass)), _) => JellyfinAuth::new(c.http.clone(), c.client.clone())
            .sign_in(&url, &user, &pass)
            .await
            .ok(),
        (None, Some((url, token))) => {
            match crate::signin::plex_session(c, &token, "", &url).await {
                Ok(s) => {
                    c.set_plex_tv_token(&state::account_key(&s), &token);
                    Some(s)
                }
                Err(_) => None,
            }
        }
        _ => None,
    };
    match session {
        Some(s) => {
            crate::signin::finish(c, s, &served).await;
            go("/");
        }
        None => go("/signin?step=signin"),
    }
}

#[component]
pub fn Setup() -> impl IntoView {
    let code = RwSignal::new(String::new());
    let kind = RwSignal::new(None::<&'static str>);
    let url = RwSignal::new(String::new());
    let internal = RwSignal::new(String::new());
    let user = RwSignal::new(String::new());
    let pass = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let done = RwSignal::new(false);
    let internal_url = move || {
        let v = internal.get_untracked().trim().to_owned();
        (!v.is_empty()).then_some(v)
    };
    let base = move || net::normalize_base_url(&url.get_untracked()).map_err(|e| e.to_string());
    let started = move |c: Ctx,
                        jellyfin: Option<(String, String, String)>,
                        plex: Option<(String, String)>| {
        done.set(true);
        spawn_local(async move {
            if wait_for_restart(&c).await {
                sign_in_after(&c, jellyfin, plex).await;
            } else {
                done.try_set(false);
                busy.try_set(false);
                error.try_set(Some(tr!(
                    "The server hasn't come back yet. Reload this page in a moment."
                )));
            }
        });
    };
    let jellyfin = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        error.set(None);
        let url = match base() {
            Ok(u) => u,
            Err(e) => return error.set(Some(e)),
        };
        busy.set(true);
        spawn_local(async move {
            let c = state::ctx();
            let (u, p) = (user.get_untracked().trim().to_owned(), pass.get_untracked());
            let r = KeepClient::setup_jellyfin(
                &c.http,
                &Ctx::origin(),
                &code.get_untracked(),
                &AddJellyfin {
                    url: url.clone(),
                    internal_url: internal_url(),
                    username: u.clone(),
                    password: p.clone(),
                },
            )
            .await;
            match r {
                Ok(()) => started(c, Some((url, u, p)), None),
                Err(e) => {
                    busy.try_set(false);
                    error.try_set(Some(explain(&e)));
                }
            }
        });
    };
    let plex = move |_| {
        error.set(None);
        let url = match base() {
            Ok(u) => u,
            Err(e) => return error.set(Some(e)),
        };
        busy.set(true);
        let win = crate::signin::open_plex_tab();
        spawn_local(async move {
            let c = state::ctx();
            let token = match crate::signin::plex_tv_token(&c, win.as_ref(), || {
                busy.try_get_untracked().is_some()
            })
            .await
            {
                Ok(Some(t)) => t,
                Ok(None) => return,
                Err(e) => {
                    busy.try_set(false);
                    error.try_set(Some(e));
                    return;
                }
            };
            let r = KeepClient::setup_plex(
                &c.http,
                &Ctx::origin(),
                &code.get_untracked(),
                &AddPlex {
                    url: url.clone(),
                    internal_url: internal_url(),
                    token: token.clone(),
                },
            )
            .await;
            match r {
                Ok(()) => started(c, None, Some((url, token))),
                Err(e) => {
                    busy.try_set(false);
                    error.try_set(Some(explain(&e)));
                }
            }
        });
    };
    view! {
        <main class="page setup">
            <h1>{tr!("Set up AlwyWatch")}</h1>
            {move || if done.get() {
                view! {
                    <p>{tr!("Done. AlwyWatch is starting with your server and signs you in…")}</p>
                    <crate::ui::Loading />
                }.into_any()
            } else {
                view! {
                    <p class="muted">{tr!("Welcome. AlwyWatch needs a Jellyfin or Plex server to start: you do this once. Everything else is set later in Settings → Admin → Server settings.")}</p>
                    <div class="form">
                        <label>{tr!("Setup code")}
                            <input type="text" autocapitalize="characters" autocomplete="one-time-code" spellcheck="false" maxlength="20" placeholder="XXXX-XXXX-XXXX" prop:value=move || code.get()
                                on:input=move |ev| code.set(event_target_value(&ev)) />
                            <span class="hint">{tr!("It is in the AlwyWatch server's log, on a line with “setup code” (docker logs alwywatch-server).")}</span>
                        </label>
                    </div>
                    <h2>{tr!("Your media server")}</h2>
                    <div class="choices">
                        <button class="choice" class:selected=move || kind.get() == Some("jellyfin") on:click=move |_| kind.set(Some("jellyfin"))>
                            <strong>"Jellyfin"</strong>
                            <span class="muted">{tr!("With an administrator's login")}</span>
                        </button>
                        <button class="choice" class:selected=move || kind.get() == Some("plex") on:click=move |_| kind.set(Some("plex"))>
                            <strong>"Plex"</strong>
                            <span class="muted">{tr!("With the server owner's Plex sign-in")}</span>
                        </button>
                    </div>
                    {move || kind.get().map(|k| {
                        let jf = k == "jellyfin";
                        let address = view! {
                            <label>{tr!("Address")}
                                <input type="text" inputmode="url" autocapitalize="off" spellcheck="false"
                                    placeholder=if jf { "https://jellyfin.example.org" } else { "https://plex.example.org" }
                                    prop:value=move || url.get() on:input=move |ev| url.set(event_target_value(&ev)) />
                                <span class="hint">{if jf { tr!("The address you open Jellyfin at.") } else { tr!("The address you open Plex at.") }}</span>
                            </label>
                            <details class="advanced">
                                <summary>{tr!("Advanced: address from this server")}</summary>
                                <label>{tr!("Address the AlwyWatch server uses")}
                                    <input type="text" autocapitalize="off" spellcheck="false"
                                        placeholder=if jf { "http://jellyfin:8096" } else { "http://plex:32400" }
                                        prop:value=move || internal.get() on:input=move |ev| internal.set(event_target_value(&ev)) />
                                    <span class="hint">{tr!("When it can't use the public address, e.g. http://jellyfin:8096 when both run in Docker on the same network. Empty: the public address.")}</span>
                                </label>
                            </details>
                        };
                        if jf {
                            view! {
                                <form class="form" on:submit=jellyfin>
                                    {address}
                                    <label>{tr!("Administrator's username")}<input autocomplete="username" autocapitalize="off" spellcheck="false"
                                        prop:value=move || user.get() on:input=move |ev| user.set(event_target_value(&ev)) /></label>
                                    <label>{tr!("Password")}<input type="password" autocomplete="current-password" prop:value=move || pass.get()
                                        on:input=move |ev| pass.set(event_target_value(&ev)) />
                                        <span class="hint">{tr!("AlwyWatch uses it once to make its own API key, and doesn't keep it. You'll be this AlwyWatch server's admin.")}</span></label>
                                    <div class="actions">
                                        <button class="primary" type="submit" disabled=move || busy.get()>{tr!("Set up")}</button>
                                    </div>
                                </form>
                            }.into_any()
                        } else {
                            view! {
                                <div class="form">
                                    {address}
                                    <p class="muted small">{tr!("Plex opens in a new tab: sign in there as the server's owner, then come back. You'll be this AlwyWatch server's admin.")}</p>
                                    <div class="actions">
                                        <button class="primary" on:click=plex disabled=move || busy.get()>
                                            {move || if busy.get() { tr!("Waiting for Plex…") } else { tr!("Sign in with Plex and set up") }}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    })}
                    {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                }.into_any()
            }}
        </main>
    }
}
