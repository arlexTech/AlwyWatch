// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch accounts in the browser (docs/keep-protocol.md "AlwyWatch accounts"): signing in
//! without a media server (password, passkey, Pocket ID), creating an account with an
//! invitation, the account page, and approving a Quick Connect code (`/q/123456`, the address
//! the apps' QR code opens).

use keep_service::KeepClient;
use keep_service::account::{self, AccountOptions, AccountView, LoginRequest, RegisterRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use serde::{Deserialize, Serialize};

use crate::state::{self, Ctx};
use crate::ui::ErrorText;

const ACCOUNT: &str = "aw.account";
const DEVICE: &str = "Web browser";

/// The AlwyWatch account this browser is signed in to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountLink {
    pub token: String,
    pub name: String,
    pub username: Option<String>,
    pub user_key: String,
}

pub fn load() -> Option<AccountLink> {
    state::load(ACCOUNT)
}

impl Ctx {
    /// A client for the account session, if signed in to one.
    pub fn account_client(&self) -> Option<KeepClient> {
        let a = self.account.get_untracked()?;
        Some(KeepClient::account(
            self.http.clone(),
            &Ctx::origin(),
            &a.token,
        ))
    }

    /// Whoever can act for me on the AlwyWatch server: the account session, else the linked
    /// media sign-in.
    pub fn person_client(&self) -> Option<KeepClient> {
        self.account_client().or_else(|| self.keep_client())
    }

    pub fn set_account(&self, token: &str, a: &AccountView) {
        let link = AccountLink {
            token: token.to_owned(),
            name: a.name.clone(),
            username: a.username.clone(),
            user_key: a.user_key.clone(),
        };
        state::save(ACCOUNT, &link);
        self.account.set(Some(link));
    }

    pub fn forget_account(&self) {
        state::remove(ACCOUNT);
        self.account.set(None);
    }
}

/// After signing in to a Jellyfin/Plex account while an AlwyWatch account is signed in: they
/// become one person (the server checks the media token, it doesn't keep it).
pub async fn link_media(c: &Ctx, server_id: &str, token: &str) {
    if let Some(k) = c.account_client()
        && let Ok(a) = k.account_link(server_id, token).await
    {
        let t = c
            .account
            .get_untracked()
            .map(|a| a.token)
            .unwrap_or_default();
        c.set_account(&t, &a);
    }
}

/// At start: refreshes the account (name), forgets it when its session ended.
pub async fn check(c: Ctx) {
    let Some(k) = c.account_client() else { return };
    match k.account_me().await {
        Ok(a) => {
            let t = c
                .account
                .get_untracked()
                .map(|a| a.token)
                .unwrap_or_default();
            c.set_account(&t, &a);
        }
        Err(net::Error::Unauthorized) => c.forget_account(),
        Err(_) => {}
    }
}

fn options_resource(c: &Ctx) -> LocalResource<Option<AccountOptions>> {
    let http = c.http.clone();
    LocalResource::new(move || {
        let http = http.clone();
        async move {
            KeepClient::account_options(&http, &Ctx::origin())
                .await
                .ok()
        }
    })
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_./".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn go(to: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.location().set_href(to);
    }
}

/// Signing in to an AlwyWatch account: password, passkey, Pocket ID. On the sign-in page and
/// on the Quick Connect page (`next` is where to go afterwards).
#[component]
pub fn AccountSignIn(#[prop(into)] next: String) -> impl IntoView {
    let c = state::ctx();
    let options = options_resource(&c);
    let user = RwSignal::new(String::new());
    let pass = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let navigate = use_navigate();
    let (c1, n1, next1) = (c.clone(), navigate.clone(), next.clone());
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let (c, navigate, next) = (c1.clone(), n1.clone(), next1.clone());
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let r = KeepClient::account_login(
                &c.http,
                &Ctx::origin(),
                &LoginRequest {
                    username: user.get_untracked().trim().to_owned(),
                    password: pass.get_untracked(),
                    device: DEVICE.into(),
                },
            )
            .await;
            busy.set(false);
            match r {
                Ok(s) => {
                    c.set_account(&s.token, &s.account);
                    navigate(&next, Default::default());
                }
                Err(e) => error.set(Some(account::explain(&e, true))),
            }
        });
    };
    let (c2, n2, next2) = (c.clone(), navigate.clone(), next.clone());
    let with_passkey = move |_| {
        let (c, navigate, next) = (c2.clone(), n2.clone(), next2.clone());
        error.set(None);
        spawn_local(async move {
            let base = Ctx::origin();
            let start = match KeepClient::passkey_login_start(&c.http, &base).await {
                Ok(s) => s,
                Err(e) => return error.set(Some(account::explain(&e, true))),
            };
            let flow = start["flow_id"].as_str().unwrap_or_default().to_owned();
            let cred = match crate::passkey::get(&start["options"]).await {
                Ok(c) => c,
                Err(e) => return error.set(Some(e)),
            };
            match KeepClient::passkey_login_finish(&c.http, &base, &flow, DEVICE, &cred).await {
                Ok(s) => {
                    c.set_account(&s.token, &s.account);
                    navigate(&next, Default::default());
                }
                Err(net::Error::Unauthorized) => {
                    error.set(Some("This passkey isn't known here.".into()))
                }
                Err(e) => error.set(Some(account::explain(&e, true))),
            }
        });
    };
    let next3 = next.clone();
    view! {
        <form class="form" on:submit=submit>
            <h2>"AlwyWatch account"</h2>
            <label>"Username"<input autocomplete="username webauthn" autocapitalize="off" spellcheck="false"
                prop:value=move || user.get() on:input=move |ev| user.set(event_target_value(&ev)) /></label>
            <label>"Password"<input type="password" autocomplete="current-password" prop:value=move || pass.get()
                on:input=move |ev| pass.set(event_target_value(&ev)) /></label>
            <div class="actions">
                <button class="primary" type="submit" disabled=move || busy.get()>"Sign in"</button>
                {move || {
                    let o = options.get().flatten();
                    let passkeys = o.as_ref().is_some_and(|o| o.passkeys) && crate::passkey::supported();
                    let pocket = o.as_ref().is_some_and(|o| o.pocketid);
                    let to = format!("/v1/account/oidc/start?next={}", pct(&next3));
                    view! {
                        {passkeys.then(|| view! { <button type="button" on:click=with_passkey.clone()>"Use a passkey"</button> })}
                        {pocket.then(|| view! { <button type="button" on:click=move |_| go(&to)>"Pocket ID"</button> })}
                    }
                }}
            </div>
            {move || options.get().flatten().filter(|o| o.registration != "closed").map(|_| view! {
                <p class="muted small">"No account yet? "<A href="/join">"Create one with an invitation"</A></p>
            })}
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        </form>
    }
}

/// `/join` and `/join/:code`: a new AlwyWatch account (username + password).
#[component]
pub fn Join() -> impl IntoView {
    let c = state::ctx();
    let options = options_resource(&c);
    let params = use_params_map();
    let invite = RwSignal::new(params.with_untracked(|p| p.get("code")).unwrap_or_default());
    let user = RwSignal::new(String::new());
    let pass = RwSignal::new(String::new());
    let pass2 = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let navigate = use_navigate();
    let c1 = c.clone();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if pass.get_untracked() != pass2.get_untracked() {
            return error.set(Some("The two passwords differ.".into()));
        }
        let (c, navigate) = (c1.clone(), navigate.clone());
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let code = invite.get_untracked().trim().to_owned();
            let r = KeepClient::account_register(
                &c.http,
                &Ctx::origin(),
                &RegisterRequest {
                    username: user.get_untracked().trim().to_owned(),
                    password: pass.get_untracked(),
                    invite: (!code.is_empty()).then_some(code),
                    device: DEVICE.into(),
                },
            )
            .await;
            busy.set(false);
            match r {
                Ok(s) => {
                    c.set_account(&s.token, &s.account);
                    navigate("/account", Default::default());
                }
                Err(net::Error::Unauthorized) => error.set(Some(
                    "This invitation is unknown, used up or expired.".into(),
                )),
                Err(e) => error.set(Some(account::explain(&e, false))),
            }
        });
    };
    view! {
        <h1>"Create an AlwyWatch account"</h1>
        <p class="muted">"An account keeps your profile, settings and server list, and signs your devices in. "
            "To watch, add your Jellyfin or Plex server afterwards."</p>
        {move || options.get().flatten().map(|o| {
            let pocket = o.pocketid;
            let closed = o.registration == "closed";
            let invite_needed = o.registration == "invite";
            view! {
                {pocket.then(|| view! {
                    <p><button class="primary" on:click=move |_| go("/v1/account/oidc/start?next=/account")>"Continue with Pocket ID"</button>
                        <span class="muted small">" No invitation needed."</span></p>
                })}
                {closed.then(|| view! { <p class="muted">"New accounts with a password are not open on this server."</p> })}
                {(!closed).then(|| view! {
                    <form class="form" on:submit=submit.clone()>
                        {invite_needed.then(|| view! {
                            <label>"Invitation code"<input autocapitalize="off" spellcheck="false" prop:value=move || invite.get()
                                on:input=move |ev| invite.set(event_target_value(&ev)) /></label>
                        })}
                        <label>"Username"<input autocomplete="username" autocapitalize="off" spellcheck="false" minlength="3" maxlength="32"
                            prop:value=move || user.get() on:input=move |ev| user.set(event_target_value(&ev)) /></label>
                        <label>"Password"<input type="password" autocomplete="new-password" minlength="8"
                            prop:value=move || pass.get() on:input=move |ev| pass.set(event_target_value(&ev)) /></label>
                        <label>"Password again"<input type="password" autocomplete="new-password" minlength="8"
                            prop:value=move || pass2.get() on:input=move |ev| pass2.set(event_target_value(&ev)) /></label>
                        <div class="actions">
                            <button class="primary" type="submit" disabled=move || busy.get()>"Create the account"</button>
                        </div>
                        <p class="muted small">"You can add a passkey once signed in."</p>
                        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                    </form>
                })}
            }
        })}
    }
}

/// `/account/signed-in#token=…&next=…`: back from Pocket ID with a session.
#[component]
pub fn SignedInPage() -> impl IntoView {
    let c = state::ctx();
    let error = RwSignal::new(None::<String>);
    let navigate = use_navigate();
    let hash = web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .unwrap_or_default();
    // The token leaves the address bar (and history) at once.
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ =
            h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/account/signed-in"));
    }
    let mut token = String::new();
    let mut next = "/".to_owned();
    for part in hash.trim_start_matches('#').split('&') {
        if let Some(v) = part.strip_prefix("token=") {
            token = v.to_owned();
        } else if let Some(v) = part.strip_prefix("next=") {
            let v = js_sys::decode_uri_component(v)
                .ok()
                .and_then(|s| s.as_string())
                .unwrap_or_default();
            if v.starts_with('/') && !v.starts_with("//") {
                next = v;
            }
        }
    }
    spawn_local(async move {
        let k = KeepClient::account(c.http.clone(), &Ctx::origin(), &token);
        match k.account_me().await {
            Ok(a) => {
                c.set_account(&token, &a);
                navigate(&next, Default::default());
            }
            Err(e) => error.set(Some(account::explain(&e, false))),
        }
    });
    view! {
        <h1>"Signing in…"</h1>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
    }
}

/// `/q/:code`: Quick Connect, opened from an app's QR code (or typed): signs that device in.
#[component]
pub fn Approve() -> impl IntoView {
    let c = state::ctx();
    let code = use_params_map()
        .with_untracked(|p| p.get("code"))
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    let device = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let done = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let account = c.account;
    let keep = c.keep;
    let signed_in = move || account.get().is_some() || keep.get().is_some();
    {
        let (c, code) = (c.clone(), code.clone());
        Effect::new(move |_| {
            if !signed_in() || device.get_untracked().is_some() {
                return;
            }
            let (c, code) = (c.clone(), code.clone());
            spawn_local(async move {
                let Some(k) = c.person_client() else { return };
                match k.transfer_peer(&code).await {
                    Ok(p) => device.set(Some(p.device)),
                    Err(net::Error::Status { status: 409 }) => {
                        error.set(Some("This code was already used.".into()))
                    }
                    Err(e) => error.set(Some(account::explain(&e, false))),
                }
            });
        });
    }
    let (c1, code1) = (c.clone(), code.clone());
    let approve = move |_| {
        let (c, code) = (c1.clone(), code1.clone());
        busy.set(true);
        spawn_local(async move {
            let r = match c.person_client() {
                Some(k) => k.transfer_approve(&code).await.map(|_| ()),
                None => Err(net::Error::Unauthorized),
            };
            busy.set(false);
            match r {
                Ok(()) => done.set(true),
                Err(e) => error.set(Some(account::explain(&e, false))),
            }
        });
    };
    let shown = code.clone();
    let next = format!("/q/{code}");
    view! {
        <h1>"Sign a device in"</h1>
        <p class="muted">"Code "<strong class="code">{shown}</strong></p>
        {move || if done.get() {
            view! { <p>"Done. The device signs itself in within a few seconds."</p> }.into_any()
        } else if !signed_in() {
            view! {
                <p class="muted">"Sign in first: the device then gets your account."</p>
                <AccountSignIn next=next.clone() />
                <p class="muted small">"Or "<A href="/signin">"sign in with Jellyfin or Plex"</A>" and open this page again."</p>
            }.into_any()
        } else {
            let approve = approve.clone();
            view! {
                {move || device.get().map(|d| view! {
                    <p>"Sign "<strong>{d}</strong>" in to your account?"</p>
                    <p class="muted small">"Only approve a device you are setting up yourself, right now."</p>
                    <div class="actions">
                        <button class="primary" on:click=approve.clone() disabled=move || busy.get()>"Approve"</button>
                        <A href="/" attr:class="button">"Cancel"</A>
                    </div>
                })}
            }.into_any()
        }}
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
    }
}

fn day(t: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t as f64 * 1000.0));
    d.to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED)
        .as_string()
        .unwrap_or_default()
}

/// `/account`: the AlwyWatch account: ways to sign in, devices, invitations (admins).
#[component]
pub fn AccountPage() -> impl IntoView {
    let c = state::ctx();
    let info = RwSignal::new(None::<AccountView>);
    let error = RwSignal::new(use_query_map().with_untracked(|q| q.get("error")).map(|e| {
        js_sys::decode_uri_component(&e)
            .ok()
            .and_then(|s| s.as_string())
            .unwrap_or(e)
    }));
    let note = RwSignal::new(
        use_query_map()
            .with_untracked(|q| q.get("linked"))
            .map(|_| "Pocket ID is linked.".to_owned()),
    );
    let page = Page {
        error,
        note,
        version: RwSignal::new(0),
    };
    // Back from Pocket ID to link it: this account's session confirms, only then is it linked.
    if let Some(id) = use_query_map().with_untracked(|q| q.get("link")) {
        let c = c.clone();
        spawn_local(async move {
            let Some(k) = c.account_client() else {
                error.set(Some(
                    "Sign in to your AlwyWatch account, then link Pocket ID again.".to_owned(),
                ));
                return;
            };
            match k.oidc_link(&id).await {
                Ok(()) => {
                    note.set(Some("Pocket ID is linked.".to_owned()));
                    page.version.update(|v| *v += 1);
                }
                Err(net::Error::Status { status: 409 }) => error.set(Some(
                    "This Pocket ID is already used by another account.".to_owned(),
                )),
                Err(_) => error.set(Some(
                    "Pocket ID could not be linked. Try again from this page.".to_owned(),
                )),
            }
        });
    }
    {
        let c = c.clone();
        Effect::new(move |_| {
            page.version.get();
            let c = c.clone();
            spawn_local(async move {
                let Some(k) = c.account_client() else { return };
                match k.account_me().await {
                    Ok(a) => {
                        let t = c
                            .account
                            .get_untracked()
                            .map(|a| a.token)
                            .unwrap_or_default();
                        c.set_account(&t, &a);
                        info.set(Some(a));
                    }
                    Err(net::Error::Unauthorized) => c.forget_account(),
                    Err(e) => error.set(Some(account::explain(&e, false))),
                }
            });
        });
    }
    let options = options_resource(&c);
    let account = c.account;
    let c_out = c.clone();
    let sign_out = move |_| {
        let c = c_out.clone();
        spawn_local(async move {
            if let Some(k) = c.account_client() {
                let _ = k.account_logout().await;
            }
            c.forget_account();
        });
    };
    let explain = |e: net::Error| account::explain(&e, false);
    view! {
        <h1>"AlwyWatch account"</h1>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        {move || note.get().map(|n| view! { <p class="note">{n}</p> })}
        {move || match account.get() {
            None => view! {
                <p class="muted">"Not signed in to an AlwyWatch account in this browser."</p>
                <AccountSignIn next="/account" />
            }.into_any(),
            Some(a) => view! {
                <section class="group">
                    <h2>"Signed in"</h2>
                    <div class="row">
                        <div><strong>{a.name.clone()}</strong>{a.username.clone().map(|u| view! { <span class="muted">" · "{u}</span> })}</div>
                        <button on:click=sign_out.clone()>"Sign out"</button>
                    </div>
                    {move || info.get().map(|i| i.linked).filter(|l| !l.is_empty()).map(|l| view! {
                        <p class="muted small">"Also you: "{l.iter().map(|x| format!("{} ({})", x.name, x.kind)).collect::<Vec<_>>().join(", ")}</p>
                    })}
                </section>
            }.into_any(),
        }}
        {move || info.get().map(|i| {
                        let c = c.clone();
            let o = options.get().flatten();
            view! {
                <Password info=i.clone() page=page />
                <MediaSignIn info=i.clone() page=page />
                <section class="group">
                    <h2>"Passkeys"</h2>
                    {i.passkeys.iter().map(|p| {
                        let (id, c) = (p.id.clone(), c.clone());
                        view! {
                            <div class="row">
                                <div><strong>{p.name.clone()}</strong>
                                    <span class="muted small">" · added "{day(p.created)}
                                    {p.last_used.map(|t| format!(", last used {}", day(t)))}</span></div>
                                <button on:click=move |_| {
                                    let (id, c) = (id.clone(), c.clone());
                                    page.run(Box::pin(async move {
                                        let k = c.account_client().ok_or("Signed out.")?;
                                        k.passkey_delete(&id).await.map_err(|e| match e {
                                            net::Error::Status { status: 409 } => "This is your only way to sign in: add a password or Pocket ID first.".into(),
                                            e => explain(e),
                                        })
                                    }), "Passkey removed.")
                                }>"Remove"</button>
                            </div>
                        }
                    }).collect_view()}
                    {(o.as_ref().is_some_and(|o| o.passkeys) && crate::passkey::supported()).then(|| {
                        let c = c.clone();
                        view! {
                            <button on:click=move |_| {
                                let c = c.clone();
                                page.run(Box::pin(async move {
                                    let k = c.account_client().ok_or("Signed out.")?;
                                    let start = k.passkey_register_start().await.map_err(explain)?;
                                    let cred = crate::passkey::create(&start["options"]).await?;
                                    let flow = start["flow_id"].as_str().unwrap_or_default();
                                    k.passkey_register_finish(flow, &passkey_name(), &cred).await.map_err(explain)?;
                                    Ok(())
                                }), "Passkey added.")
                            }>"Add a passkey"</button>
                        }
                    })}
                </section>
                {o.as_ref().is_some_and(|o| o.pocketid).then(|| {
                    let (c, linked) = (c.clone(), i.pocketid);
                    let c2 = c.clone();
                    view! {
                        <section class="group">
                            <h2>"Pocket ID"</h2>
                            <div class="row">
                                <div class="muted">{if linked { "Linked: you can sign in with Pocket ID." } else { "Not linked." }}</div>
                                {if linked {
                                    view! { <button on:click=move |_| {
                                        let c = c.clone();
                                        page.run(Box::pin(async move {
                                            let k = c.account_client().ok_or("Signed out.")?;
                                            k.oidc_unlink().await.map_err(|e| match e {
                                                net::Error::Status { status: 409 } => "This is your only way to sign in: add a password or passkey first.".into(),
                                                e => explain(e),
                                            })
                                        }), "Pocket ID unlinked.")
                                    }>"Unlink"</button> }.into_any()
                                } else {
                                    view! { <button on:click=move |_| {
                                        let c = c2.clone();
                                        spawn_local(async move {
                                            let Some(k) = c.account_client() else { return };
                                            match k.oidc_ticket().await {
                                                Ok(t) => go(&format!("/v1/account/oidc/start?ticket={t}")),
                                                Err(e) => error.set(Some(explain(e))),
                                            }
                                        });
                                    }>"Link Pocket ID"</button> }.into_any()
                                }}
                            </div>
                        </section>
                    }
                })}
                <section class="group">
                    <h2>"Devices"</h2>
                    {i.sessions.iter().map(|s| {
                        let (id, c, current) = (s.id.clone(), c.clone(), s.current);
                        view! {
                            <div class="row">
                                <div><strong>{if s.device.is_empty() { "Device".to_owned() } else { s.device.clone() }}</strong>
                                    <span class="muted small">{if current { " · this browser".to_owned() } else { format!(" · last seen {}", day(s.last_seen)) }}</span></div>
                                {(!current).then(|| view! { <button on:click=move |_| {
                                    let (id, c) = (id.clone(), c.clone());
                                    page.run(Box::pin(async move {
                                        let k = c.account_client().ok_or("Signed out.")?;
                                        k.account_end_session(&id).await.map_err(explain)
                                    }), "Signed out.")
                                }>"Sign out"</button> })}
                            </div>
                        }
                    }).collect_view()}
                    <p class="muted small">"To sign a TV or another device in, choose “Sign in with a code” on it and open the address it shows (or scan its QR code)."</p>
                </section>
            }
        })}
        <Invitations />
    }
}

fn passkey_name() -> String {
    let ua = web_sys::window()
        .and_then(|w| w.navigator().user_agent().ok())
        .unwrap_or_default();
    let os = [
        ("Android", "Android"),
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Mac OS", "Mac"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(k, _)| ua.contains(k))
    .map(|(_, v)| *v)
    .unwrap_or("This device");
    format!("Passkey ({os})")
}

type Job = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>;

/// The account page's messages, and a reload after each change.
#[derive(Clone, Copy)]
struct Page {
    error: RwSignal<Option<String>>,
    note: RwSignal<Option<String>>,
    version: RwSignal<u32>,
}

impl Page {
    fn run(self, job: Job, ok: &'static str) {
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            match job.await {
                Ok(()) => {
                    if !ok.is_empty() {
                        self.note.set(Some(ok.into()));
                    }
                    self.version.update(|v| *v += 1);
                }
                Err(e) => self.error.set(Some(e)),
            }
        });
    }
}

#[component]
fn Password(info: AccountView, page: Page) -> impl IntoView {
    let c = state::ctx();
    let current = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    let open = RwSignal::new(false);
    let has = info.has_password;
    let has_username = info.username.is_some();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let c = c.clone();
        let (cur, nw) = (current.get_untracked(), new.get_untracked());
        page.run(
            Box::pin(async move {
                let k = c.account_client().ok_or("Signed out.")?;
                k.account_set_password(has.then_some(cur.as_str()), &nw)
                    .await
                    .map_err(|e| match e {
                        net::Error::Unauthorized => "The current password is wrong.".into(),
                        e => account::explain(&e, false),
                    })
            }),
            "Password saved. Your other devices stay signed in only if they use a passkey or Pocket ID.",
        );
        open.set(false);
    };
    view! {
        <section class="group">
            <h2>"Password"</h2>
            {move || if !has_username {
                view! { <p class="muted small">"This account was made by signing a device in; it has no username, so no password."</p> }.into_any()
            } else if open.get() {
                view! {
                    <form class="form" on:submit=submit.clone()>
                        {has.then(|| view! { <label>"Current password"<input type="password" autocomplete="current-password"
                            prop:value=move || current.get() on:input=move |ev| current.set(event_target_value(&ev)) /></label> })}
                        <label>"New password"<input type="password" autocomplete="new-password" minlength="8"
                            prop:value=move || new.get() on:input=move |ev| new.set(event_target_value(&ev)) /></label>
                        <div class="actions"><button class="primary" type="submit">"Save"</button>
                            <button type="button" on:click=move |_| open.set(false)>"Cancel"</button></div>
                    </form>
                }.into_any()
            } else {
                view! { <div class="row"><div class="muted">{if has { "Set." } else { "None yet." }}</div>
                    <button on:click=move |_| open.set(true)>{if has { "Change" } else { "Set a password" }}</button></div> }.into_any()
            }}
        </section>
    }
}

/// Kept once the proposal to turn media sign-ins off was answered with "keep it on".
const MEDIA_SIGN_IN_ASKED: &str = "aw.account.media-asked";

/// Plex/Jellyfin sign-ins acting on the account: proposed to be turned off once the account
/// has a way of its own to sign in (a password, a passkey, Pocket ID), never forced.
#[component]
fn MediaSignIn(info: AccountView, page: Page) -> impl IntoView {
    let own_way = info.has_password || !info.passkeys.is_empty() || info.pocketid;
    let allowed = info.media_sign_in;
    // Nothing to offer before the account can be signed in to on its own.
    if !own_way && allowed {
        return ().into_any();
    }
    let asked = RwSignal::new(state::load::<bool>(MEDIA_SIGN_IN_ASKED).unwrap_or(false));
    let set = move |to: bool| {
        let c = state::ctx();
        page.run(
            Box::pin(async move {
                let k = c.account_client().ok_or("Signed out.")?;
                k.account_media_sign_in(to).await.map_err(|e| match e {
                    net::Error::Status { status: 409 } => {
                        "Add a password, passkey or Pocket ID first: it would be your only way in."
                            .into()
                    }
                    e => account::explain(&e, false),
                })
            }),
            if to {
                "Plex and Jellyfin sign-ins can change this account again."
            } else {
                "Done. Only this account's own sign-in changes it now."
            },
        );
    };
    let keep_on = move |_| {
        state::save(MEDIA_SIGN_IN_ASKED, &true);
        asked.set(true);
    };
    view! {
        <section class="group">
            <h2>"Plex and Jellyfin sign-ins"</h2>
            {move || if !allowed {
                view! {
                    <div class="row">
                        <div class="muted">"Turned off: a Plex or Jellyfin sign-in can't set this account's password or sign a new device in to it. You still watch, vote and join rooms with them."</div>
                        <button on:click=move |_| set(true)>"Turn back on"</button>
                    </div>
                }.into_any()
            } else if asked.get() {
                view! {
                    <div class="row">
                        <div class="muted">"On: whoever is signed in to your Plex or Jellyfin account can set this account's password and sign new devices in to it."</div>
                        <button on:click=move |_| set(false)>"Turn off"</button>
                    </div>
                }.into_any()
            } else {
                view! {
                    <p>"This account has its own way to sign in now. Do you want to stop Plex and Jellyfin sign-ins from changing it?"</p>
                    <p class="muted small">"Today, whoever is signed in to your Plex or Jellyfin account can set this account's password and sign new devices in to it. Turned off, only this account's own sign-in can. Watching, votes and rooms keep working with Plex and Jellyfin either way, and you can turn it back on here."</p>
                    <div class="actions">
                        <button class="primary" on:click=move |_| set(false)>"Turn off"</button>
                        <button on:click=keep_on>"Keep it on"</button>
                    </div>
                }.into_any()
            }}
        </section>
    }
    .into_any()
}

/// Admins: invitations to create an account.
#[component]
fn Invitations() -> impl IntoView {
    let c = state::ctx();
    let keep = c.keep;
    let made = RwSignal::new(None::<account::Invite>);
    let error = RwSignal::new(None::<String>);
    let make = move |_| {
        let c = c.clone();
        spawn_local(async move {
            let Some(k) = c.keep_client() else { return };
            match k.account_invite(7, 1).await {
                Ok(i) => made.set(Some(i)),
                Err(e) => error.set(Some(account::explain(&e, false))),
            }
        });
    };
    view! {
        <Show when=move || keep.get().is_some_and(|k| k.is_admin)>
            <section class="group">
                <h2>"Invitations"</h2>
                <p class="muted small">"Someone without Jellyfin or Plex can create an AlwyWatch account with an invitation (one use, 7 days)."</p>
                <button on:click=make.clone()>"New invitation"</button>
                {move || made.get().map(|i| view! {
                    <p>"Send this link: "<strong class="code">{i.url.clone().unwrap_or_else(|| i.code.clone())}</strong></p>
                })}
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
            </section>
        </Show>
    }
}
