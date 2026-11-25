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
use media_api::jellyfin::JellyfinAuth;
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

/// The browser's own yes/no question (as "Sign out of AlwyWatch" asks): false when it can't
/// be asked.
pub fn confirm(question: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(question).ok())
        .unwrap_or(false)
}

/// What the server's Pocket ID round trip put in `?error=`, in the selected language. Only its
/// own messages are recognized: anything else in the address (a crafted link) is not shown.
fn oidc_error(raw: &str) -> String {
    match raw.trim() {
        "too many attempts, try again in a few minutes" => {
            tr!("Too many attempts. Try again in a few minutes.")
        }
        "Pocket ID is not set up on this server" => tr!("Pocket ID isn't set up on this server."),
        "this link has expired, try again" | "this sign-in took too long, try again" => {
            tr!("This sign-in took too long. Try again.")
        }
        "Pocket ID can't be reached right now, try again later"
        | "Pocket ID's answer could not be used, try again later" => {
            tr!("Pocket ID can't be used right now. Try again later.")
        }
        "Pocket ID refused the sign-in" => tr!("Pocket ID refused the sign-in."),
        "Pocket ID's answer was incomplete" => tr!("Pocket ID's answer was incomplete. Try again."),
        "this sign-in was started in another browser, start again here" => {
            tr!("This sign-in was started in another browser. Start again here.")
        }
        "this Pocket ID is already used by another account" => {
            tr!("This Pocket ID is already used by another account.")
        }
        _ => tr!("Signing in with Pocket ID didn't work. Try again."),
    }
}

fn go(to: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.location().set_href(to);
    }
}

/// Set when this tab starts a Pocket ID sign-in: `/account/signed-in` only takes a session
/// that comes back to a tab that asked for one.
const POCKET_ID_STARTED: &str = "pocketid-started";
/// The server's own limit on a Pocket ID round trip.
const POCKET_ID_MS: f64 = 600_000.0;

/// Starts a Pocket ID sign-in in this tab.
fn start_pocket_id(to: &str) {
    state::tab_save(POCKET_ID_STARTED, Some(&js_sys::Date::now()));
    go(to);
}

/// Whether this tab started a Pocket ID sign-in a moment ago (forgotten once asked).
fn pocket_id_was_started() -> bool {
    let at: Option<f64> = state::tab_load(POCKET_ID_STARTED);
    state::tab_save::<f64>(POCKET_ID_STARTED, None);
    at.is_some_and(|at| (0.0..POCKET_ID_MS).contains(&(js_sys::Date::now() - at)))
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
        // One browser prompt at a time.
        if busy.get_untracked() {
            return;
        }
        let (c, navigate, next) = (c2.clone(), n2.clone(), next2.clone());
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let r: Result<(), String> = async {
                let base = Ctx::origin();
                let start = KeepClient::passkey_login_start(&c.http, &base)
                    .await
                    .map_err(|e| account::explain(&e, true))?;
                let flow = start["flow_id"].as_str().unwrap_or_default().to_owned();
                let cred = crate::passkey::get(&start["options"]).await?;
                match KeepClient::passkey_login_finish(&c.http, &base, &flow, DEVICE, &cred).await {
                    Ok(s) => {
                        c.set_account(&s.token, &s.account);
                        navigate(&next, Default::default());
                        Ok(())
                    }
                    Err(net::Error::Unauthorized) => Err(tr!("This passkey isn't known here.")),
                    Err(e) => Err(account::explain(&e, true)),
                }
            }
            .await;
            busy.try_set(false);
            if let Err(e) = r {
                error.try_set(Some(e));
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
                        {passkeys.then(|| view! { <button type="button" on:click=with_passkey.clone() disabled=move || busy.get()>{tr!("Use a passkey")}</button> })}
                        {pocket.then(|| view! { <button type="button" on:click=move |_| start_pocket_id(&to)>{tr!("Sign in with Pocket ID")}</button> })}
                    }
                }}
            </div>
            {move || options.get().flatten().filter(|o| o.password_reset).map(|_| view! {
                <p class="muted small"><A href="/forgot">{tr!("Forgot password?")}</A></p>
            })}
            {move || options.get().flatten().filter(|o| o.registration != "closed").map(|_| view! {
                <p class="muted small">{tr!("No account yet?")}" "<A href="/join">{tr!("Create one with an invitation")}</A></p>
            })}
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        </form>
    }
}

/// `/join` and `/join/:code`: a new AlwyWatch account (username + password). With an
/// invitation's code in the address: who invited, and the Jellyfin login it makes too.
#[component]
pub fn Join() -> impl IntoView {
    let c = state::ctx();
    let code = use_params_map()
        .with_untracked(|p| p.get("code"))
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty());
    match code {
        Some(code) => {
            let http = c.http.clone();
            let check = code.clone();
            let info = LocalResource::new(move || {
                let (http, check) = (http.clone(), check.clone());
                async move { KeepClient::account_invite_info(&http, &Ctx::origin(), &check).await }
            });
            view! {
                {move || match info.get() {
                    None => view! { <crate::ui::Loading /> }.into_any(),
                    Some(Ok(i)) => view! { <JoinForm invite=Some((code.clone(), i)) /> }.into_any(),
                    Some(Err(net::Error::NotFound)) => view! {
                        <h1>{tr!("This invitation can't be used")}</h1>
                        <p class="muted">{tr!("It was already used, ended or expired. Ask the person who invited you for a new one.")}</p>
                    }.into_any(),
                    Some(Err(e)) => view! {
                        <h1>{tr!("Create an AlwyWatch account")}</h1>
                        <ErrorText text=account::explain(&e, false) />
                    }.into_any(),
                }}
            }
            .into_any()
        }
        None => view! { <JoinForm invite=None /> }.into_any(),
    }
}

#[component]
fn JoinForm(invite: Option<(String, account::InviteInfo)>) -> impl IntoView {
    let c = state::ctx();
    let options = options_resource(&c);
    let from_link = invite.is_some();
    let info = invite.as_ref().map(|(_, i)| i.clone());
    let jellyfin = info.as_ref().and_then(|i| i.jellyfin.clone());
    let inviter = info.as_ref().map(|i| i.inviter.clone());
    let code = RwSignal::new(invite.as_ref().map(|(c, _)| c.clone()).unwrap_or_default());
    // The username is theirs to choose; the invitation's address is offered for "forgot
    // password".
    let user = RwSignal::new(String::new());
    let email = RwSignal::new(
        info.as_ref()
            .and_then(|i| i.email.clone())
            .unwrap_or_default(),
    );
    let pass = RwSignal::new(String::new());
    let pass2 = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let navigate = use_navigate();
    let (c1, jf1, inviter1) = (c.clone(), jellyfin.clone(), inviter.clone());
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if pass.get_untracked() != pass2.get_untracked() {
            return error.set(Some(tr!("The two passwords differ.")));
        }
        let (c, navigate, jf, inviter) =
            (c1.clone(), navigate.clone(), jf1.clone(), inviter1.clone());
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let invite = code.get_untracked().trim().to_owned();
            let password = pass.get_untracked();
            let r = KeepClient::account_register(
                &c.http,
                &Ctx::origin(),
                &RegisterRequest {
                    username: user.get_untracked().trim().to_owned(),
                    password: password.clone(),
                    invite: (!invite.is_empty()).then_some(invite),
                    email: Some(email.get_untracked().trim().to_owned()).filter(|e| !e.is_empty()),
                    device: DEVICE.into(),
                },
            )
            .await;
            match r {
                Ok(s) => {
                    c.set_account(&s.token, &s.account);
                    // The invitation made a Jellyfin login too: signed in to it here, so the
                    // library is ready.
                    if let Some(j) = s.jellyfin
                        && let Ok(session) = JellyfinAuth::new(c.http.clone(), c.client.clone())
                            .sign_in(&j.url, &j.username, &password)
                            .await
                    {
                        let served = KeepClient::servers(&c.http, &Ctx::origin())
                            .await
                            .unwrap_or_default();
                        crate::signin::finish(&c, session, &served).await;
                        busy.try_set(false);
                        return navigate("/", Default::default());
                    }
                    busy.try_set(false);
                    // Next: a Jellyfin or Plex login to watch with.
                    navigate("/signin", Default::default());
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(match e {
                        net::Error::Unauthorized => match &inviter {
                            Some(name) => tr!(
                                "This invitation can't be used any more. Ask {name} for a new one.",
                                name = name.clone()
                            ),
                            None => tr!(
                                "This invitation is unknown, used up or expired. Ask for a new one."
                            ),
                        },
                        net::Error::Status { status: 409 } => match &jf {
                            Some(server) => tr!(
                                "This username is taken, here or on {server}: choose another.",
                                server = server.clone()
                            ),
                            None => account::explain(&e, false),
                        },
                        e => account::explain(&e, false),
                    }));
                }
            }
        });
    };
    let form = move |invite_needed: bool| {
        let submit = submit.clone();
        let jellyfin = jellyfin.clone();
        view! {
            <form class="form" on:submit=submit>
                {(invite_needed && !from_link).then(|| view! {
                    <label>{tr!("Invitation code")}<input autocapitalize="off" spellcheck="false" prop:value=move || code.get()
                        on:input=move |ev| code.set(event_target_value(&ev)) /></label>
                })}
                <label>{tr!("Username")}<input autocomplete="username" autocapitalize="off" spellcheck="false" minlength="3" maxlength="32"
                    prop:value=move || user.get() on:input=move |ev| user.set(event_target_value(&ev)) /></label>
                <label>{tr!("Password")}<input type="password" autocomplete="new-password" minlength="8"
                    prop:value=move || pass.get() on:input=move |ev| pass.set(event_target_value(&ev)) /></label>
                <label>{tr!("Password again")}<input type="password" autocomplete="new-password" minlength="8"
                    prop:value=move || pass2.get() on:input=move |ev| pass2.set(event_target_value(&ev)) /></label>
                <label>{tr!("Email (to reset a forgotten password)")}<input type="email" autocomplete="email" autocapitalize="off" spellcheck="false"
                    prop:value=move || email.get() on:input=move |ev| email.set(event_target_value(&ev)) /></label>
                {jellyfin.map(|server| view! {
                    <p class="muted small">{tr!("This username and password will also be your {server} (Jellyfin) login.", server = server)}</p>
                })}
                <div class="actions">
                    <button class="primary" type="submit" disabled=move || busy.get()>{tr!("Create the account")}</button>
                </div>
                <p class="muted small">{tr!("You can add a passkey once signed in.")}</p>
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
            </form>
        }
    };
    match info {
        Some(i) => view! {
            <h1>{tr!("{name} invited you", name = i.inviter.clone())}</h1>
            <p class="muted">{match i.jellyfin.clone() {
                Some(server) => tr!("Create your AlwyWatch account: it comes with a login on {server}, to watch right away.", server = server),
                None => tr!("Create your AlwyWatch account. It keeps your profile, settings and server list, and signs your devices in."),
            }}</p>
            {form(true)}
        }
        .into_any(),
        None => view! {
            <h1>{tr!("Create an AlwyWatch account")}</h1>
            <p class="muted">{tr!("An account keeps your profile, settings and server list, and signs your devices in. To watch, add your Jellyfin or Plex server afterwards.")}</p>
            {move || options.get().flatten().map(|o| {
                let pocket = o.pocketid;
                let closed = o.registration == "closed";
                let invite_needed = o.registration == "invite";
                view! {
                    {pocket.then(|| view! {
                        <p><button class="primary" on:click=move |_| start_pocket_id("/v1/account/oidc/start?next=/signin")>{tr!("Continue with Pocket ID")}</button>
                            " "<span class="muted small">{tr!("No invitation needed.")}</span></p>
                    })}
                    {closed.then(|| view! { <p class="muted">{tr!("This server doesn't accept new accounts without an invitation. Ask its owner for one, or sign in with your Jellyfin or Plex login.")}</p> })}
                    {(!closed).then(|| form(invite_needed))}
                }
            })}
        }
        .into_any(),
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
            if v.starts_with('/') && !v.starts_with("//") && !v.contains('\\') {
                next = v;
            }
        }
    }
    // A link made by someone else (with their own session in it) must not sign this browser in
    // to their account: the next Jellyfin/Plex sign-in here would join it, and they would act
    // as this person. Only a Pocket ID sign-in this tab started comes back here legitimately.
    if !pocket_id_was_started() {
        token.clear();
    }
    spawn_local(async move {
        if token.is_empty() {
            error.set(Some(tr!(
                "This sign-in link wasn't started here: sign in again."
            )));
            return;
        }
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
        let e = js_sys::decode_uri_component(&e)
            .ok()
            .and_then(|s| s.as_string())
            .unwrap_or(e);
        oidc_error(&e)
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
    // Signed in with Jellyfin or Plex but not to the account in this browser: that sign-in opens
    // the account here (the sign-in form is only for when the owner turned that off).
    let opening = RwSignal::new(c.account.get_untracked().is_none());
    if opening.get_untracked() {
        let c = c.clone();
        spawn_local(async move {
            if let Some(k) = c.keep_client()
                && let Ok(s) = k.account_session(DEVICE).await
            {
                c.set_account(&s.token, &s.account);
                page.version.update(|v| *v += 1);
            }
            opening.set(false);
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
            None if opening.get() => view! { <p class="muted">{tr!("Loading…")}</p> }.into_any(),
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
                <Email info=i.clone() page=page />
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
                                    if !confirm(&tr!("Remove this passkey? It can't sign you in any more.")) {
                                        return;
                                    }
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
                                    if !confirm(&tr!("Sign this device out of your account?")) {
                                        return;
                                    }
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
                <DeleteAccount info=i.clone() page=page shown=info />
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
        self.run_and(job, ok, || {});
    }

    /// [`Page::run`], then `done` on success only (a form closes once its answer is good).
    fn run_and(self, job: Job, ok: String, done: impl FnOnce() + 'static) {
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            match job.await {
                Ok(()) => {
                    done();
                    if !ok.is_empty() {
                        self.note.try_set(Some(ok));
                    }
                    self.version.try_update(|v| *v += 1);
                }
                Err(e) => {
                    self.error.try_set(Some(e));
                }
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
    let busy = RwSignal::new(false);
    let has = info.has_password;
    let has_username = info.username.is_some();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let c = c.clone();
        let (cur, nw) = (current.get_untracked(), new.get_untracked());
        busy.set(true);
        let job = Box::pin(async move {
            let r = match c.account_client() {
                Some(k) => k
                    .account_set_password(has.then_some(cur.as_str()), &nw)
                    .await
                    .map_err(|e| match e {
                        net::Error::Unauthorized if has => tr!("The current password is wrong."),
                        e => account::explain(&e, false),
                    }),
                None => Err(tr!("Signed out.")),
            };
            busy.try_set(false);
            r
        });
        // Open until the answer is good: a wrong password is fixed in place, not retyped.
        page.run_and(
            job,
            tr!("Password saved. Your other devices stay signed in only if they use a passkey or Pocket ID."),
            move || {
                open.try_set(false);
                current.try_set(String::new());
                new.try_set(String::new());
            },
        );
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
                        <label>{tr!("New password")}<input type="password" autocomplete="new-password" minlength="8" required
                            prop:value=move || new.get() on:input=move |ev| new.set(event_target_value(&ev)) /></label>
                        <div class="actions"><button class="primary" type="submit" disabled=move || busy.get()>{tr!("Save")}</button>
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

/// Where "forgot password" sends its link. Changing it asks for the password (when there is
/// one): a borrowed session must not be enough to take the account.
#[component]
fn Email(info: AccountView, page: Page) -> impl IntoView {
    let c = state::ctx();
    let open = RwSignal::new(false);
    let current = RwSignal::new(String::new());
    let email = RwSignal::new(info.email.clone().unwrap_or_default());
    let busy = RwSignal::new(false);
    let has = info.has_password;
    let shown = info.email.clone();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let c = c.clone();
        let (cur, e) = (current.get_untracked(), email.get_untracked());
        busy.set(true);
        let job = Box::pin(async move {
            let e = e.trim();
            let r = match c.account_client() {
                Some(k) => k
                    .account_set_email((!e.is_empty()).then_some(e), has.then_some(cur.as_str()))
                    .await
                    .map_err(|e| match e {
                        // The server's 403 for a wrong password reaches here as Unauthorized.
                        net::Error::Unauthorized if has => tr!("The current password is wrong."),
                        e => account::explain(&e, false),
                    }),
                None => Err(tr!("Signed out.")),
            };
            busy.try_set(false);
            r
        });
        page.run_and(job, tr!("Email saved."), move || {
            open.try_set(false);
            current.try_set(String::new());
        });
    };
    view! {
        <section class="group">
            <h2>{tr!("Email")}</h2>
            {move || if open.get() {
                view! {
                    <form class="form" on:submit=submit.clone()>
                        <label>{tr!("Email")}<input type="email" autocomplete="email" autocapitalize="off" spellcheck="false"
                            prop:value=move || email.get() on:input=move |ev| email.set(event_target_value(&ev)) /></label>
                        {has.then(|| view! { <label>{tr!("Current password")}<input type="password" autocomplete="current-password"
                            prop:value=move || current.get() on:input=move |ev| current.set(event_target_value(&ev)) /></label> })}
                        <div class="actions"><button class="primary" type="submit" disabled=move || busy.get()>{tr!("Save")}</button>
                            <button type="button" on:click=move |_| open.set(false)>{tr!("Cancel")}</button></div>
                    </form>
                }.into_any()
            } else {
                let shown = shown.clone();
                view! { <div class="row"><div class="muted">{shown.clone().unwrap_or_else(|| tr!("None: a forgotten password can't be reset by email."))}</div>
                    <button on:click=move |_| open.set(true)>{if shown.is_some() { tr!("Change") } else { tr!("Add an email") }}</button></div> }.into_any()
            }}
        </section>
    }
}

/// `/forgot`: emails a link to set a new password.
#[component]
pub fn Forgot() -> impl IntoView {
    let c = state::ctx();
    let login = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let sent = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let c = c.clone();
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let r = KeepClient::password_forgot(
                &c.http,
                &Ctx::origin(),
                &account::ForgotRequest {
                    login: login.get_untracked().trim().to_owned(),
                    lang: i18n::language().to_owned(),
                },
            )
            .await;
            busy.set(false);
            match r {
                Ok(()) => sent.set(true),
                Err(net::Error::Status { status: 501 }) => error.set(Some(tr!(
                    "This server can't send emails. Ask the person who runs it to set a new password for you."
                ))),
                Err(e) => error.set(Some(account::explain(&e, false))),
            }
        });
    };
    view! {
        <h1>{tr!("Forgot password")}</h1>
        {move || if sent.get() {
            view! {
                <p>{tr!("If this account has an email address, a link to set a new password is on its way. It works for an hour.")}</p>
                <p class="muted small">{tr!("Nothing after a few minutes? Check your spam folder, or ask the person who runs your server.")}</p>
                <p><A href="/signin?step=signin" attr:class="button">{tr!("Back to sign in")}</A></p>
            }.into_any()
        } else {
            view! {
                <form class="form" on:submit=submit.clone()>
                    <p class="muted">{tr!("Type your username or the email address of your AlwyWatch account.")}</p>
                    <label>{tr!("Username or email")}<input autocomplete="username" autocapitalize="off" spellcheck="false" required
                        prop:value=move || login.get() on:input=move |ev| login.set(event_target_value(&ev)) /></label>
                    <div class="actions"><button class="primary" type="submit" disabled=move || busy.get()>{tr!("Send me a link")}</button></div>
                    {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                </form>
            }.into_any()
        }}
    }
}

/// `/reset/:token`: the emailed link: a new password, then signing in with it.
#[component]
pub fn Reset() -> impl IntoView {
    let c = state::ctx();
    let token = use_params_map()
        .with_untracked(|p| p.get("token"))
        .unwrap_or_default();
    let pass = RwSignal::new(String::new());
    let pass2 = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let done = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if pass.get_untracked() != pass2.get_untracked() {
            return error.set(Some(tr!("The two passwords differ.")));
        }
        let (c, token) = (c.clone(), token.clone());
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let r = KeepClient::password_reset(
                &c.http,
                &Ctx::origin(),
                &account::ResetRequest {
                    token,
                    password: pass.get_untracked(),
                },
            )
            .await;
            busy.set(false);
            match r {
                Ok(()) => done.set(true),
                Err(net::Error::Status { status: 410 }) => error.set(Some(tr!(
                    "This link was already used or has expired. Ask for a new one."
                ))),
                Err(e) => error.set(Some(account::explain(&e, false))),
            }
        });
    };
    view! {
        <h1>{tr!("Set a new password")}</h1>
        {move || if done.get() {
            view! {
                <p>{tr!("Your new password is set. Devices signed in to your account were signed out: sign in again with it.")}</p>
                <p><A href="/signin?step=signin" attr:class="button primary">{tr!("Sign in")}</A></p>
            }.into_any()
        } else {
            view! {
                <form class="form" on:submit=submit.clone()>
                    <label>{tr!("New password")}<input type="password" autocomplete="new-password" minlength="8" required
                        prop:value=move || pass.get() on:input=move |ev| pass.set(event_target_value(&ev)) /></label>
                    <label>{tr!("Password again")}<input type="password" autocomplete="new-password" minlength="8" required
                        prop:value=move || pass2.get() on:input=move |ev| pass2.set(event_target_value(&ev)) /></label>
                    <div class="actions"><button class="primary" type="submit" disabled=move || busy.get()>{tr!("Save")}</button></div>
                    {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                    <p class="muted small"><A href="/forgot">{tr!("Ask for a new link")}</A></p>
                </form>
            }.into_any()
        }}
    }
}

/// Deleting the account, behind a confirmation (and the password when there is one).
/// `shown`: the page's account, emptied once deleted (its sections go with it).
#[component]
fn DeleteAccount(
    info: AccountView,
    page: Page,
    shown: RwSignal<Option<AccountView>>,
) -> impl IntoView {
    let c = state::ctx();
    let open = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let has = info.has_password;
    let media = !info.linked.is_empty();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        // Pressed twice: the second would hit an account already gone.
        if busy.get_untracked() {
            return;
        }
        let c = c.clone();
        let pw = password.get_untracked();
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let Some(k) = c.account_client() else {
                busy.try_set(false);
                error.try_set(Some(tr!("Signed out.")));
                return;
            };
            match k.account_delete(has.then_some(pw.as_str())).await {
                Ok(()) => {
                    page.error.set(None);
                    page.note
                        .set(Some(tr!("Your AlwyWatch account is deleted.")));
                    c.forget_account();
                    // This component goes with it: nothing of it is touched after.
                    shown.set(None);
                    return;
                }
                // The server's 403 for a wrong password reaches here as Unauthorized.
                Err(net::Error::Unauthorized) if has => {
                    error.try_set(Some(tr!("The password is wrong.")));
                }
                Err(e) => {
                    error.try_set(Some(account::explain(&e, false)));
                }
            }
            busy.try_set(false);
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
                        <div class="actions"><button class="danger" type="submit" disabled=move || busy.get()>{tr!("Delete my account")}</button>
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
            <p class="muted">{tr!("Your name and picture come with you, and you get Leaving soon and watch together. Settings stay in this browser.")}</p>
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
