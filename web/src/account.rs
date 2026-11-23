// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch accounts in the browser (docs/keep-protocol.md "AlwyWatch accounts"): signing in
//! without a media server (password, passkey, Pocket ID), creating an account with an
//! invitation, the account page, and approving a Quick Connect code (`/q/123456`, the address
//! the apps' QR code opens).

use i18n::tr;
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
        // Signing in again after "Sign out of AlwyWatch": this profile is on AlwyWatch again,
        // its logins too.
        if self.keep_off() {
            self.set_keep_off(false);
            spawn_local(crate::signin::check_keep(self.clone()));
        }
        crate::profiles::adopt_name(&a.name);
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
                    error.set(Some(tr!("This passkey isn't known here.")))
                }
                Err(e) => error.set(Some(account::explain(&e, true))),
            }
        });
    };
    let next3 = next.clone();
    view! {
        <form class="form" on:submit=submit>
            <h2>{tr!("AlwyWatch account")}</h2>
            <label>{tr!("Username")}<input autocomplete="username webauthn" autocapitalize="off" spellcheck="false"
                prop:value=move || user.get() on:input=move |ev| user.set(event_target_value(&ev)) /></label>
            <label>{tr!("Password")}<input type="password" autocomplete="current-password" prop:value=move || pass.get()
                on:input=move |ev| pass.set(event_target_value(&ev)) /></label>
            <div class="actions">
                <button class="primary" type="submit" disabled=move || busy.get()>{tr!("Sign in")}</button>
                {move || {
                    let o = options.get().flatten();
                    let passkeys = o.as_ref().is_some_and(|o| o.passkeys) && crate::passkey::supported();
                    let pocket = o.as_ref().is_some_and(|o| o.pocketid);
                    let to = format!("/v1/account/oidc/start?next={}", pct(&next3));
                    view! {
                        {passkeys.then(|| view! { <button type="button" on:click=with_passkey.clone()>{tr!("Use a passkey")}</button> })}
                        {pocket.then(|| view! { <button type="button" on:click=move |_| go(&to)>{tr!("Sign in with Pocket ID")}</button> })}
                    }
                }}
            </div>
            {move || options.get().flatten().filter(|o| o.registration != "closed").map(|_| view! {
                <p class="muted small">{tr!("No account yet?")}" "<A href="/join">{tr!("Create one with an invitation")}</A></p>
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
            return error.set(Some(tr!("The two passwords differ.")));
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
                    // Next: a Jellyfin or Plex login to watch with.
                    navigate("/signin", Default::default());
                }
                Err(net::Error::Unauthorized) => error.set(Some(tr!(
                    "This invitation is unknown, used up or expired. Ask for a new one."
                ))),
                Err(e) => error.set(Some(account::explain(&e, false))),
            }
        });
    };
    view! {
        <h1>{tr!("Create an AlwyWatch account")}</h1>
        <p class="muted">{tr!("An account keeps your profile, settings and server list, and signs your devices in. To watch, add your Jellyfin or Plex server afterwards.")}</p>
        {move || options.get().flatten().map(|o| {
            let pocket = o.pocketid;
            let closed = o.registration == "closed";
            let invite_needed = o.registration == "invite";
            view! {
                {pocket.then(|| view! {
                    <p><button class="primary" on:click=move |_| go("/v1/account/oidc/start?next=/signin")>{tr!("Continue with Pocket ID")}</button>
                        " "<span class="muted small">{tr!("No invitation needed.")}</span></p>
                })}
                {closed.then(|| view! { <p class="muted">{tr!("This server doesn't accept new accounts without an invitation. Ask its owner for one, or sign in with your Jellyfin or Plex login.")}</p> })}
                {(!closed).then(|| view! {
                    <form class="form" on:submit=submit.clone()>
                        {invite_needed.then(|| view! {
                            <label>{tr!("Invitation code")}<input autocapitalize="off" spellcheck="false" prop:value=move || invite.get()
                                on:input=move |ev| invite.set(event_target_value(&ev)) /></label>
                        })}
                        <label>{tr!("Username")}<input autocomplete="username" autocapitalize="off" spellcheck="false" minlength="3" maxlength="32"
                            prop:value=move || user.get() on:input=move |ev| user.set(event_target_value(&ev)) /></label>
                        <label>{tr!("Password")}<input type="password" autocomplete="new-password" minlength="8"
                            prop:value=move || pass.get() on:input=move |ev| pass.set(event_target_value(&ev)) /></label>
                        <label>{tr!("Password again")}<input type="password" autocomplete="new-password" minlength="8"
                            prop:value=move || pass2.get() on:input=move |ev| pass2.set(event_target_value(&ev)) /></label>
                        <div class="actions">
                            <button class="primary" type="submit" disabled=move || busy.get()>{tr!("Create the account")}</button>
                        </div>
                        <p class="muted small">{tr!("You can add a passkey once signed in.")}</p>
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
        <h1>{tr!("Signing in…")}</h1>
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
                        error.set(Some(tr!("This code was already used.")))
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
        <h1>{tr!("Sign a device in")}</h1>
        <p class="muted">{
            let (before, after) = crate::ui::around(&tr!("Code {code}"), "{code}");
            view! { {before}<strong class="code">{shown}</strong>{after} }
        }</p>
        {move || if done.get() {
            view! { <p>{tr!("Done. The device signs itself in within a few seconds.")}</p> }.into_any()
        } else if !signed_in() {
            view! {
                <p class="muted">{tr!("Sign in first: the device is then signed in as you.")}</p>
                <p><A href="/signin?step=signin" attr:class="button primary" on:click={let n = next.clone(); move |_| crate::signin::return_after_sign_in(&n)}>{tr!("Sign in with Jellyfin or Plex")}</A></p>
                <p class="muted small">{tr!("You'll come straight back here. Or use your AlwyWatch account, if you have one:")}</p>
                <AccountSignIn next=next.clone() />
            }.into_any()
        } else {
            let approve = approve.clone();
            view! {
                {move || device.get().map(|d| view! {
                    <p>{
                        let (before, after) = crate::ui::around(&tr!("Sign {device} in to your account?"), "{device}");
                        view! { {before}<strong>{d}</strong>{after} }
                    }</p>
                    <p class="muted small">{tr!("Only approve a device you are setting up yourself, right now.")}</p>
                    <div class="actions">
                        <button class="primary" on:click=approve.clone() disabled=move || busy.get()>{tr!("Approve")}</button>
                        <A href="/" attr:class="button">{tr!("Cancel")}</A>
                    </div>
                })}
            }.into_any()
        }}
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
    }
}

fn day(t: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t as f64 * 1000.0));
    d.to_locale_date_string(i18n::language(), &wasm_bindgen::JsValue::UNDEFINED)
        .as_string()
        .unwrap_or_default()
}

/// Settings → Account → Password & passkeys (`/settings/account/password`, formerly
/// `/account`): ways to sign in, devices, deleting the account.
#[component]
pub fn PasswordAndPasskeys() -> impl IntoView {
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
            .map(|_| tr!("Pocket ID is linked.")),
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
                error.set(Some(tr!(
                    "Sign in to your AlwyWatch account, then link Pocket ID again."
                )));
                return;
            };
            match k.oidc_link(&id).await {
                Ok(()) => {
                    note.set(Some(tr!("Pocket ID is linked.")));
                    page.version.update(|v| *v += 1);
                }
                Err(net::Error::Status { status: 409 }) => error.set(Some(tr!(
                    "This Pocket ID is already used by another account."
                ))),
                Err(_) => error.set(Some(tr!(
                    "Pocket ID could not be linked. Try again from this page."
                ))),
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
    let explain = |e: net::Error| account::explain(&e, false);
    view! {
        <p><A href="/settings/account" attr:class="button small subtle">"‹ "{tr!("Account")}</A></p>
        <h2>{tr!("Password & passkeys")}</h2>
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        {move || note.get().map(|n| view! { <p class="note">{n}</p> })}
        {move || match account.get() {
            None => view! {
                <p class="muted">{tr!("Not signed in to an AlwyWatch account in this browser.")}</p>
                <AccountSignIn next="/settings/account/password" />
            }.into_any(),
            Some(a) => view! {
                <section class="group">
                    <h2>{tr!("Signed in")}</h2>
                    <div class="row">
                        <div><strong>{a.name.clone()}</strong>{a.username.clone().map(|u| view! { <span class="muted">" · "{u}</span> })}</div>
                    </div>
                    {move || info.get().map(|i| i.linked).filter(|l| !l.is_empty()).map(|l| view! {
                        <p class="muted small">{tr!("Also you: {list}", list = l.iter().map(|x| format!("{} ({})", x.name, x.kind)).collect::<Vec<_>>().join(", "))}</p>
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
                    <h2>{tr!("Passkeys")}</h2>
                    <p class="muted small">{tr!("Sign in with your fingerprint, face or device PIN instead of a password.")}</p>
                    {i.passkeys.iter().map(|p| {
                        let (id, c) = (p.id.clone(), c.clone());
                        view! {
                            <div class="row">
                                <div><strong>{p.name.clone()}</strong>
                                    <span class="muted small">" · "{match p.last_used {
                                        Some(t) => tr!("added {date}, last used {used}", date = day(p.created), used = day(t)),
                                        None => tr!("added {date}", date = day(p.created)),
                                    }}</span></div>
                                <button on:click=move |_| {
                                    let (id, c) = (id.clone(), c.clone());
                                    page.run(Box::pin(async move {
                                        let k = c.account_client().ok_or_else(|| tr!("Signed out."))?;
                                        k.passkey_delete(&id).await.map_err(|e| match e {
                                            net::Error::Status { status: 409 } => tr!("This is your only way to sign in: add a password or Pocket ID first."),
                                            e => explain(e),
                                        })
                                    }), tr!("Passkey removed."))
                                }>{tr!("Remove")}</button>
                            </div>
                        }
                    }).collect_view()}
                    {(o.as_ref().is_some_and(|o| o.passkeys) && crate::passkey::supported()).then(|| {
                        let c = c.clone();
                        view! {
                            <button on:click=move |_| {
                                let c = c.clone();
                                page.run(Box::pin(async move {
                                    let k = c.account_client().ok_or_else(|| tr!("Signed out."))?;
                                    let start = k.passkey_register_start().await.map_err(explain)?;
                                    let cred = crate::passkey::create(&start["options"]).await?;
                                    let flow = start["flow_id"].as_str().unwrap_or_default();
                                    k.passkey_register_finish(flow, &passkey_name(), &cred).await.map_err(explain)?;
                                    Ok(())
                                }), tr!("Passkey added."))
                            }>{tr!("Add a passkey")}</button>
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
                                <div class="muted">{if linked { tr!("Linked: you can sign in with Pocket ID.") } else { tr!("Pocket ID is this server's single sign-on. Link it to sign in without a password.") }}</div>
                                {if linked {
                                    view! { <button on:click=move |_| {
                                        let c = c.clone();
                                        page.run(Box::pin(async move {
                                            let k = c.account_client().ok_or_else(|| tr!("Signed out."))?;
                                            k.oidc_unlink().await.map_err(|e| match e {
                                                net::Error::Status { status: 409 } => tr!("This is your only way to sign in: add a password or passkey first."),
                                                e => explain(e),
                                            })
                                        }), tr!("Pocket ID unlinked."))
                                    }>{tr!("Unlink")}</button> }.into_any()
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
                                    }>{tr!("Link Pocket ID")}</button> }.into_any()
                                }}
                            </div>
                        </section>
                    }
                })}
                <section class="group">
                    <h2>{tr!("Devices")}</h2>
                    {i.sessions.iter().map(|s| {
                        let (id, c, current) = (s.id.clone(), c.clone(), s.current);
                        view! {
                            <div class="row">
                                <div><strong>{if s.device.is_empty() { tr!("Device") } else { s.device.clone() }}</strong>
                                    <span class="muted small">" · "{if current { tr!("this browser") } else { tr!("last seen {date}", date = day(s.last_seen)) }}</span></div>
                                {(!current).then(|| view! { <button on:click=move |_| {
                                    let (id, c) = (id.clone(), c.clone());
                                    page.run(Box::pin(async move {
                                        let k = c.account_client().ok_or_else(|| tr!("Signed out."))?;
                                        k.account_end_session(&id).await.map_err(explain)
                                    }), tr!("Signed out."))
                                }>{tr!("Sign out")}</button> })}
                            </div>
                        }
                    }).collect_view()}
                    <p class="muted small">{tr!("To sign a TV or another device in, choose “Sign in with a code” on it and open the address it shows (or scan its QR code).")}</p>
                </section>
                <DeleteAccount info=i.clone() />
            }
        })}
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
    .map(|v| v.to_owned())
    .unwrap_or_else(|| tr!("This device"));
    tr!("Passkey ({device})", device = os)
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
    fn run(self, job: Job, ok: String) {
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            match job.await {
                Ok(()) => {
                    if !ok.is_empty() {
                        self.note.set(Some(ok));
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
                let k = c.account_client().ok_or_else(|| tr!("Signed out."))?;
                k.account_set_password(has.then_some(cur.as_str()), &nw)
                    .await
                    .map_err(|e| match e {
                        net::Error::Unauthorized => tr!("The current password is wrong."),
                        e => account::explain(&e, false),
                    })
            }),
            tr!("Password saved. Your other devices stay signed in only if they use a passkey or Pocket ID."),
        );
        open.set(false);
    };
    view! {
        <section class="group">
            <h2>{tr!("Password")}</h2>
            {move || if !has_username {
                view! { <p class="muted small">{tr!("This account has no username yet, so it can't have a password. Add a passkey below to sign in without Jellyfin or Plex.")}</p> }.into_any()
            } else if open.get() {
                view! {
                    <form class="form" on:submit=submit.clone()>
                        {has.then(|| view! { <label>{tr!("Current password")}<input type="password" autocomplete="current-password"
                            prop:value=move || current.get() on:input=move |ev| current.set(event_target_value(&ev)) /></label> })}
                        <label>{tr!("New password")}<input type="password" autocomplete="new-password" minlength="8"
                            prop:value=move || new.get() on:input=move |ev| new.set(event_target_value(&ev)) /></label>
                        <div class="actions"><button class="primary" type="submit">{tr!("Save")}</button>
                            <button type="button" on:click=move |_| open.set(false)>{tr!("Cancel")}</button></div>
                    </form>
                }.into_any()
            } else {
                view! { <div class="row"><div class="muted">{if has { tr!("Set.") } else { tr!("None yet.") }}</div>
                    <button on:click=move |_| open.set(true)>{if has { tr!("Change") } else { tr!("Set a password") }}</button></div> }.into_any()
            }}
        </section>
    }
}

/// Deleting the account, behind a confirmation (and the password when there is one).
#[component]
fn DeleteAccount(info: AccountView) -> impl IntoView {
    let c = state::ctx();
    let open = RwSignal::new(false);
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let has = info.has_password;
    let media = !info.linked.is_empty();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let c = c.clone();
        let pw = password.get_untracked();
        spawn_local(async move {
            let Some(k) = c.account_client() else { return };
            match k.account_delete(has.then_some(pw.as_str())).await {
                Ok(()) => c.forget_account(),
                Err(net::Error::Status { status: 403 }) => {
                    error.set(Some(tr!("The password is wrong.")))
                }
                Err(e) => error.set(Some(account::explain(&e, false))),
            }
        });
    };
    view! {
        <section class="group">
            <h2>{tr!("Delete account")}</h2>
            {move || if open.get() {
                view! {
                    <form class="form" on:submit=submit.clone()>
                        <p>{if media {
                            tr!("This deletes your AlwyWatch account: its username, password, passkeys and the devices signed in with it. You keep using AlwyWatch with your Plex or Jellyfin sign-in, and your profile and keep requests stay with it.")
                        } else {
                            tr!("This deletes your AlwyWatch account and everything stored for it: username, password, passkeys, profile, picture and synced settings. Devices signed in with it are signed out.")
                        }}</p>
                        <p class="muted small">{tr!("It can't be undone.")}</p>
                        {has.then(|| view! { <label>{tr!("Your password")}<input type="password" autocomplete="current-password"
                            prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev)) /></label> })}
                        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                        <div class="actions"><button class="danger" type="submit">{tr!("Delete my account")}</button>
                            <button type="button" on:click=move |_| open.set(false)>{tr!("Cancel")}</button></div>
                    </form>
                }.into_any()
            } else {
                view! { <div class="row"><div class="muted">{tr!("Removes the account for good.")}</div>
                    <button on:click=move |_| open.set(true)>{tr!("Delete account…")}</button></div> }.into_any()
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
                let k = c.account_client().ok_or_else(|| tr!("Signed out."))?;
                k.account_media_sign_in(to).await.map_err(|e| match e {
                    net::Error::Status { status: 409 } => {
                        tr!("Add a password, passkey or Pocket ID first: it would be your only way in.")
                    }
                    e => account::explain(&e, false),
                })
            }),
            if to {
                tr!("Plex and Jellyfin sign-ins can change this account again.")
            } else {
                tr!("Done. Only this account's own sign-in changes it now.")
            },
        );
    };
    let keep_on = move |_| {
        state::save(MEDIA_SIGN_IN_ASKED, &true);
        asked.set(true);
    };
    view! {
        <section class="group">
            <h2>{tr!("Plex and Jellyfin sign-ins")}</h2>
            {move || if !allowed {
                view! {
                    <div class="row">
                        <div class="muted">{tr!("Turned off: a Plex or Jellyfin sign-in can't set this account's password or sign a new device in to it. You still watch, vote and join rooms with them.")}</div>
                        <button on:click=move |_| set(true)>{tr!("Turn back on")}</button>
                    </div>
                }.into_any()
            } else if asked.get() {
                view! {
                    <div class="row">
                        <div class="muted">{tr!("On: whoever is signed in to your Plex or Jellyfin account can set this account's password and sign new devices in to it.")}</div>
                        <button on:click=move |_| set(false)>{tr!("Turn off")}</button>
                    </div>
                }.into_any()
            } else {
                view! {
                    <p>{tr!("This account has its own way to sign in now. Do you want to stop Plex and Jellyfin sign-ins from changing it?")}</p>
                    <p class="muted small">{tr!("Today, whoever is signed in to your Plex or Jellyfin account can set this account's password and sign new devices in to it. Turned off, only this account's own sign-in can. Watching, votes and rooms keep working with Plex and Jellyfin either way, and you can turn it back on here.")}</p>
                    <div class="actions">
                        <button class="primary" on:click=move |_| set(false)>{tr!("Turn off")}</button>
                        <button on:click=keep_on>{tr!("Keep it on")}</button>
                    </div>
                }.into_any()
            }}
        </section>
    }
    .into_any()
}

/// Settings → Admin → Invitations: a code (and its link) to create an account.
#[component]
pub fn Invitations() -> impl IntoView {
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
                <h2>{tr!("Invitations")}</h2>
                <p class="muted small">{tr!("Someone without Jellyfin or Plex can create an AlwyWatch account with an invitation (works once, for 7 days).")}</p>
                <button on:click=make.clone()>{tr!("New invitation")}</button>
                {move || made.get().map(|i| view! {
                    <p>{
                        let (before, after) = crate::ui::around(&tr!("Invitation code: {code}"), "{code}");
                        view! { {before}<strong class="code">{i.code.clone()}</strong>{after} }
                    }</p>
                    {i.url.clone().map(|url| view! {
                        <p>{
                            let (before, after) = crate::ui::around(&tr!("Or send this link: {link}"), "{link}");
                            view! { {before}<a class="accent" href=url.clone()>{url.clone()}</a>{after} }
                        }</p>
                    })}
                })}
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
            </section>
        </Show>
    }
}

/// Settings → Account. With AlwyWatch: how others see you, the AlwyWatch server, password &
/// passkeys, linked logins, "Sign out of AlwyWatch". Without: this profile's name here, and
/// "Connect to AlwyWatch".
#[component]
pub fn AccountSection() -> impl IntoView {
    let c = state::ctx();
    let (keep, account) = (c.keep, c.account);
    let connected = move || keep.get().is_some() || account.get().is_some();
    move || {
        if connected() {
            view! { <OnAlwyWatch /> }.into_any()
        } else {
            view! { <ConnectToAlwyWatch /> }.into_any()
        }
    }
}

#[component]
fn OnAlwyWatch() -> impl IntoView {
    let c = state::ctx();
    let (keep, account, expired) = (c.keep, c.account, c.expired);
    let host = Ctx::origin()
        .split_once("://")
        .map(|(_, h)| h.to_owned())
        .unwrap_or_else(Ctx::origin);
    // The page comes from the AlwyWatch server itself: it is reachable; a refused sign-in
    // shows here.
    let status = move || {
        let refused = keep.with(|k| {
            k.as_ref()
                .is_some_and(|k| expired.get().as_deref() == Some(k.account.as_str()))
        });
        if refused {
            tr!("Sign-in expired")
        } else {
            tr!("Connected")
        }
    };
    let c_out = c.clone();
    let sign_out = move |_| {
        let sure = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&tr!(
                    "Sign out of AlwyWatch? This profile keeps its Jellyfin and Plex logins and becomes a profile without an AlwyWatch account: Leaving soon and watch together disappear from it."
                ))
                .ok()
            })
            .unwrap_or(false);
        if !sure {
            return;
        }
        let c = c_out.clone();
        spawn_local(async move {
            if let Some(k) = c.account_client() {
                let _ = k.account_logout().await;
            }
            c.forget_account();
            if c.keep.get_untracked().is_some() {
                c.disconnect_keep();
            }
            c.set_keep_off(true);
        });
    };
    view! {
        <section class="group">
            <h2>{tr!("How others see you")}</h2>
            {move || match (keep.get(), account.get()) {
                (Some(_), _) => view! {
                    <crate::links::AlsoSignedIn />
                    <crate::profile::AccountProfile />
                }.into_any(),
                (None, Some(a)) => view! {
                    <div class="profile-head">
                        <crate::profile::Avatar user_key=a.user_key.clone() name=a.name.clone() avatar=Default::default() size=64 />
                        <strong class="profile-name">{a.name.clone()}</strong>
                    </div>
                    <p class="muted small">{tr!("To change your name and picture, add a Jellyfin or Plex login: Servers → Add a server.")}</p>
                }.into_any(),
                (None, None) => ().into_any(),
            }}
        </section>
        <section class="group">
            <h2>{tr!("AlwyWatch")}</h2>
            <div class="row">
                <div><strong>{tr!("AlwyWatch server")}</strong>
                    <div class="muted small">{host}" · "{status}</div></div>
            </div>
            <A href="/settings/account/password" attr:class="row link-row">
                <div><strong>{tr!("Password & passkeys")}</strong>
                    <div class="muted small">{tr!("Ways to sign in, devices signed in, deleting the account")}</div></div>
                <span>"›"</span>
            </A>
            <Show when=move || keep.get().is_some()><crate::links::LinkedSignins /></Show>
            <div class="actions">
                <button on:click=sign_out>{tr!("Sign out of AlwyWatch")}</button>
            </div>
        </section>
    }
}

/// A profile without an AlwyWatch account: its name in this browser, and the way back.
#[component]
fn ConnectToAlwyWatch() -> impl IntoView {
    let c = state::ctx();
    let navigate = use_navigate();
    // Plex logins from before the plex.tv token was kept can't sign in to AlwyWatch.
    let old_plex = c
        .accounts
        .get_untracked()
        .iter()
        .any(|s| s.kind == media_api::ServerKind::Plex && c.keep_token(s).is_none());
    let (c1, n1) = (c.clone(), navigate.clone());
    let sign_in = move |_| {
        let (c, navigate) = (c1.clone(), n1.clone());
        c.set_keep_off(false);
        spawn_local(async move {
            // A login on this site's servers connects at once; otherwise, sign in.
            crate::signin::check_keep(c.clone()).await;
            if c.keep.get_untracked().is_none() {
                navigate("/signin?step=signin", Default::default());
            }
        });
    };
    view! {
        <section class="group">
            <h2>{tr!("How others see you")}</h2>
            <crate::profiles::LocalProfile />
        </section>
        <section class="group">
            <h2>{tr!("Connect to AlwyWatch")}</h2>
            <p class="muted">{tr!("Your name, settings and servers come with you, and you get Leaving soon and watch together.")}</p>
            <div class="actions">
                <button class="primary" on:click=sign_in>{tr!("Sign in")}</button>
                <button on:click=move |_| navigate("/join", Default::default())>{tr!("Create an account")}</button>
            </div>
            {old_plex.then(|| view! { <p class="muted small">{tr!("Plex account: sign out and sign in again to use requests, Leaving soon and watch together.")}</p> })}
        </section>
    }
}

/// Settings → This device → Sign in another device: the code a new device shows ("Sign in
/// with a code") is approved here.
#[component]
pub fn SignInAnotherDevice() -> impl IntoView {
    let c = state::ctx();
    let (keep, account) = (c.keep, c.account);
    let code = RwSignal::new(String::new());
    let navigate = use_navigate();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let digits: String = code
            .get_untracked()
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        if !digits.is_empty() {
            navigate(&format!("/q/{digits}"), Default::default());
        }
    };
    view! {
        <Show when=move || keep.get().is_some() || account.get().is_some()>
            <section class="group">
                <h2>{tr!("Sign in another device")}</h2>
                <p class="muted small">{tr!("On the other device, choose “Sign in with a code” and type the code it shows here (or scan its QR code with your phone).")}</p>
                <form class="name-form" on:submit=submit.clone()>
                    <input aria-label=tr!("Code") inputmode="numeric" autocomplete="off" maxlength="12"
                        prop:value=move || code.get() on:input=move |ev| code.set(event_target_value(&ev)) />
                    <button class="small primary" type="submit">{tr!("Continue")}</button>
                </form>
            </section>
        </Show>
    }
}
