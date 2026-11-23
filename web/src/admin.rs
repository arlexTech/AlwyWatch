// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings → Admin → Server settings (`/settings/admin/server`): the AlwyWatch server's
//! media servers and services, set up here as the *arr apps do (nothing in the compose file),
//! and Settings → Admin → Invitations: an email or a link that makes an AlwyWatch account, and
//! a Jellyfin login with it.

use i18n::{tr, trn};
use keep_service::KeepClient;
use keep_service::ServerType;
use keep_service::account::{self, InviteRequest};
use keep_service::settings::{
    AddJellyfin, AddPlex, KEPT, ServerSettings, SettingsServer, SettingsTest, Webhook,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use wasm_bindgen::JsCast;

use crate::state::{self, Ctx};
use crate::ui::{ErrorText, Loading};

/// Whoever may act as an admin here: the linked media sign-in, else the account session.
pub fn admin_client(c: &Ctx) -> Option<KeepClient> {
    c.keep_client().or_else(|| c.account_client())
}

/// After a change that starts the server again: true once it answers with its new settings.
pub async fn wait_for_restart(c: &Ctx) -> bool {
    // It starts again about half a second after answering.
    state::sleep(1500).await;
    for _ in 0..90 {
        if let Ok(s) = KeepClient::setup_state(&c.http, &Ctx::origin()).await
            && !s.needed
        {
            return true;
        }
        state::sleep(1000).await;
    }
    false
}

fn admin_error(e: &net::Error) -> String {
    match e {
        net::Error::Unauthorized => tr!("Only admins of this AlwyWatch server can see this."),
        net::Error::Invalid { message } => message.clone(),
        other => state::plain_keep(other),
    }
}

/// Copies to the clipboard (navigator.clipboard, when the browser allows it).
pub fn copy(text: &str) -> bool {
    let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
        return false;
    };
    let Ok(clip) = js_sys::Reflect::get(&nav, &"clipboard".into()) else {
        return false;
    };
    match js_sys::Reflect::get(&clip, &"writeText".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    {
        Some(f) => f.call1(&clip, &text.into()).is_ok(),
        None => false,
    }
}

/// The phone's share sheet (navigator.share), when there is one.
fn share_supported() -> bool {
    web_sys::window()
        .map(|w| js_sys::Reflect::has(&w.navigator(), &"share".into()).unwrap_or(false))
        .unwrap_or(false)
}

fn share(text: &str, url: &str) {
    let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
        return;
    };
    let data = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&data, &"text".into(), &text.into());
    let _ = js_sys::Reflect::set(&data, &"url".into(), &url.into());
    if let Some(f) = js_sys::Reflect::get(&nav, &"share".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    {
        let _ = f.call1(&nav, &data);
    }
}

/// A Copy button that says "Copied" for a moment.
#[component]
pub fn CopyButton(#[prop(into)] text: String) -> impl IntoView {
    let done = RwSignal::new(false);
    view! {
        <button type="button" class="small" on:click=move |_| {
            if copy(&text) {
                done.set(true);
                spawn_local(async move {
                    state::sleep(2000).await;
                    done.try_set(false);
                });
            }
        }>{move || if done.get() { tr!("Copied") } else { tr!("Copy") }}</button>
    }
}

/// Nothing typed: none.
fn some(v: String) -> Option<String> {
    (!v.trim().is_empty()).then_some(v)
}

// ----- Server settings --------------------------------------------------------------------

/// The page's state, shared by its sections.
#[derive(Clone, Copy)]
struct Form {
    /// As being edited.
    edit: RwSignal<ServerSettings>,
    /// As the server gave them.
    orig: RwSignal<ServerSettings>,
    /// Saving, adding a server, or waiting for the server to start again.
    busy: RwSignal<bool>,
    note: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
    /// Bumped to read the settings again.
    version: RwSignal<u32>,
}

impl Form {
    fn dirty(self) -> bool {
        self.edit.with(|e| self.orig.with(|o| e != o))
    }

    /// The server is starting again: says `msg`, waits for it, then reads the settings again.
    fn restarting(self, msg: String) {
        self.error.set(None);
        self.note.set(Some(msg));
        self.busy.set(true);
        spawn_local(async move {
            let ok = wait_for_restart(&state::ctx()).await;
            self.busy.try_set(false);
            if ok {
                self.note
                    .try_set(Some(tr!("Done: the server runs with the new settings.")));
                self.version.try_update(|v| *v += 1);
            } else {
                self.note.try_set(None);
                self.error.try_set(Some(tr!(
                    "The server hasn't come back yet. Reload this page in a moment."
                )));
            }
        });
    }
}

fn form() -> Form {
    expect_context::<Form>()
}

/// Getter and setter of a text field of an optional section (`edit.<section>.<field>`).
macro_rules! text_of {
    ($edit:ident, $sec:ident . $f:ident) => {
        (
            move || $edit.with(|s| s.$sec.as_ref().map(|x| x.$f.clone()).unwrap_or_default()),
            move |v: String| {
                $edit.update(|s| {
                    if let Some(x) = &mut s.$sec {
                        x.$f = v
                    }
                })
            },
        )
    };
}

/// The same for an optional text field (nothing typed: none).
macro_rules! opt_of {
    ($edit:ident, $sec:ident . $f:ident) => {
        (
            move || {
                $edit.with(|s| {
                    s.$sec
                        .as_ref()
                        .and_then(|x| x.$f.clone())
                        .unwrap_or_default()
                })
            },
            move |v: String| {
                $edit.update(|s| {
                    if let Some(x) = &mut s.$sec {
                        x.$f = some(v)
                    }
                })
            },
        )
    };
}

/// A secret of an optional section, as it is (`KEPT` when set and unchanged).
macro_rules! secret_of {
    ($edit:ident, $sec:ident . $f:ident) => {
        (
            move || $edit.with(|s| s.$sec.as_ref().and_then(|x| x.$f.clone())),
            move |v: Option<String>| {
                $edit.update(|s| {
                    if let Some(x) = &mut s.$sec {
                        x.$f = v
                    }
                })
            },
        )
    };
}

/// A section's On/Off: off removes it, on brings back what was saved (or an empty one).
macro_rules! on_of {
    ($form:ident, $sec:ident) => {
        (
            move || $form.edit.with(|s| s.$sec.is_some()),
            move |on: bool| {
                let back = $form
                    .orig
                    .with_untracked(|o| o.$sec.clone())
                    .unwrap_or_default();
                $form
                    .edit
                    .update(|s| s.$sec = if on { Some(back) } else { None })
            },
        )
    };
}

/// A text field: what it is, one line on what it's for and where to find it, an example.
fn field(
    label: String,
    hint: String,
    placeholder: &'static str,
    (get, set): (
        impl Fn() -> String + Send + Sync + Copy + 'static,
        impl Fn(String) + Send + Sync + Copy + 'static,
    ),
) -> impl IntoView {
    view! {
        <label>{label}
            <input type="text" autocapitalize="off" spellcheck="false" autocomplete="off" placeholder=placeholder
                prop:value=get on:input=move |ev| set(event_target_value(&ev)) />
            {(!hint.is_empty()).then(|| view! { <span class="hint">{hint}</span> })}
        </label>
    }
}

/// A secret: never shown. Set ones read "••••" with Replace and Remove.
fn secret(
    label: String,
    hint: String,
    (get, set): (
        impl Fn() -> Option<String> + Send + Sync + Copy + 'static,
        impl Fn(Option<String>) + Send + Sync + Copy + 'static,
    ),
) -> impl IntoView {
    // Set when the page was read: "Cancel" puts it back.
    let was_set = untrack(|| get().as_deref() == Some(KEPT));
    // Only switching between "Set ••••" and the field redraws (not each key typed).
    let kept = Memo::new(move |_| get().as_deref() == Some(KEPT));
    view! {
        <div class="secret">
            <span class="label">{label}</span>
            {move || if kept.get() {
                view! {
                    <div class="row tight">
                        <span>{tr!("Set")}" ••••"</span>
                        <span class="actions tight">
                            <button type="button" class="small" on:click=move |_| set(Some(String::new()))>{tr!("Replace")}</button>
                            <button type="button" class="small subtle" on:click=move |_| set(None)>{tr!("Remove")}</button>
                        </span>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="row tight">
                        <input type="password" autocomplete="new-password" spellcheck="false" placeholder=tr!("Not set")
                            prop:value=move || get().unwrap_or_default()
                            on:input=move |ev| set(some(event_target_value(&ev))) />
                        {was_set.then(|| view! {
                            <button type="button" class="small subtle" on:click=move |_| set(Some(KEPT.to_owned()))>{tr!("Cancel")}</button>
                        })}
                    </div>
                }.into_any()
            }}
            {(!hint.is_empty()).then(|| view! { <span class="hint">{hint}</span> })}
        </div>
    }
}

/// A switch line: what it is and one line about it.
fn toggle(
    label: String,
    hint: String,
    (get, set): (
        impl Fn() -> bool + Send + Sync + Copy + 'static,
        impl Fn(bool) + Send + Sync + Copy + 'static,
    ),
) -> impl IntoView {
    view! {
        <label class="row toggle">
            <div>{label}{(!hint.is_empty()).then(|| view! { <div class="muted small">{hint}</div> })}</div>
            <input type="checkbox" prop:checked=get on:change=move |ev| set(event_target_checked(&ev)) />
        </label>
    }
}

/// "Advanced: address from this server", folded.
fn internal_address(
    (get, set): (
        impl Fn() -> String + Send + Sync + Copy + 'static,
        impl Fn(String) + Send + Sync + Copy + 'static,
    ),
    placeholder: &'static str,
) -> impl IntoView {
    view! {
        <details class="advanced" open=untrack(|| !get().is_empty())>
            <summary>{tr!("Advanced: address from this server")}</summary>
            {field(
                tr!("Address the AlwyWatch server uses"),
                tr!("When it can't use the public address, e.g. http://jellyfin:8096 when both run in Docker on the same network. Empty: the public address."),
                placeholder,
                (get, set),
            )}
        </details>
    }
}

/// A section's Test button, with its answer (green or red) beside it.
#[component]
fn Test(
    what: &'static str,
    #[prop(optional, into)] server_id: Option<String>,
    #[prop(optional)] email_to: Option<RwSignal<String>>,
) -> impl IntoView {
    let f = form();
    let result = RwSignal::new(None::<(bool, String)>);
    let testing = RwSignal::new(false);
    let run = move |_| {
        let server_id = server_id.clone();
        testing.set(true);
        result.set(None);
        spawn_local(async move {
            let Some(k) = admin_client(&state::ctx()) else {
                return;
            };
            let r = k
                .admin_settings_test(&SettingsTest {
                    what: what.to_owned(),
                    settings: f.edit.get_untracked(),
                    server_id,
                    email_to: email_to.and_then(|e| some(e.get_untracked().trim().to_owned())),
                })
                .await;
            testing.try_set(false);
            result.try_set(Some(match r {
                Ok(t) => (t.ok, t.message),
                Err(e) => (false, admin_error(&e)),
            }));
        });
    };
    view! {
        <div class="test-line">
            <button type="button" class="small" on:click=run disabled=move || testing.get()>
                {move || if testing.get() { tr!("Testing…") } else { tr!("Test") }}
            </button>
            {move || result.get().map(|(ok, m)| view! {
                <span class="test small" class:ok=ok class:bad=!ok role="status">{m}</span>
            })}
        </div>
    }
}

/// Settings → Admin → Server settings.
#[component]
pub fn ServerSettingsPage() -> impl IntoView {
    let f = Form {
        edit: RwSignal::new(ServerSettings::default()),
        orig: RwSignal::new(ServerSettings::default()),
        busy: RwSignal::new(false),
        note: RwSignal::new(None),
        error: RwSignal::new(None),
        version: RwSignal::new(0),
    };
    provide_context(f);
    // Webhook addresses and Maintainerr's problem, with the settings once read.
    let loaded = RwSignal::new(None::<Result<(Vec<Webhook>, Option<String>), String>>);
    Effect::new(move |_| {
        f.version.get();
        spawn_local(async move {
            let Some(k) = admin_client(&state::ctx()) else {
                loaded.try_set(Some(Err(tr!(
                    "Sign in as an admin of this AlwyWatch server first."
                ))));
                return;
            };
            match k.admin_settings().await {
                Ok(v) => {
                    f.orig.try_set(v.settings.clone());
                    f.edit.try_set(v.settings);
                    loaded.try_set(Some(Ok((v.webhooks, v.maintainerr_problem))));
                }
                Err(e) => {
                    loaded.try_set(Some(Err(admin_error(&e))));
                }
            }
        });
    });
    // A link to one section (from Server status, Invitations) shows it once read.
    Effect::new(move |_| {
        if !loaded.with(|l| matches!(l, Some(Ok(_)))) {
            return;
        }
        spawn_local(async {
            state::sleep(30).await;
            let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
                return;
            };
            let hash = doc
                .location()
                .and_then(|l| l.hash().ok())
                .unwrap_or_default();
            let id = hash.trim_start_matches('#');
            if let Some(el) = (!id.is_empty())
                .then(|| doc.get_element_by_id(id))
                .flatten()
            {
                el.scroll_into_view();
            }
        });
    });
    let save = move |_| {
        f.error.set(None);
        f.note.set(None);
        f.busy.set(true);
        spawn_local(async move {
            let r = match admin_client(&state::ctx()) {
                Some(k) => k.put_admin_settings(&f.edit.get_untracked()).await,
                None => Err(net::Error::Unauthorized),
            };
            match r {
                Ok(()) => f.restarting(tr!(
                    "Saved. The server is starting again with the new settings…"
                )),
                Err(e) => {
                    f.busy.try_set(false);
                    f.error.try_set(Some(admin_error(&e)));
                }
            }
        });
    };
    view! {
        <p><A href="/settings/admin" attr:class="button small subtle">"‹ "{tr!("Admin")}</A></p>
        <h2>{tr!("Server settings")}</h2>
        <p class="muted small">{tr!("What this AlwyWatch server connects to. Changes apply when you save: the server starts again, which takes a few seconds.")}</p>
        {move || match loaded.get() {
            None => view! { <Loading /> }.into_any(),
            Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
            Some(Ok((webhooks, problem))) => view! {
                <Servers />
                <Maintainerr problem=problem />
                <Seerr />
                <Tautulli />
                <Email />
                <SignUp />
                <Webhooks list=webhooks />
                <TestMode />
            }.into_any(),
        }}
        <div class="save-bar">
            <button class="primary" on:click=save disabled=move || f.busy.get() || !f.dirty()>{tr!("Save")}</button>
            <button class="subtle" on:click=move |_| f.edit.set(f.orig.get_untracked())
                disabled=move || f.busy.get() || !f.dirty()>{tr!("Undo changes")}</button>
            {move || f.note.get().map(|n| view! { <span class="small" role="status">{n}</span> })}
            {move || f.error.get().map(|e| view! { <span class="small error" role="alert">{e}</span> })}
        </div>
    }
}

fn kind_name(k: Option<ServerType>) -> &'static str {
    match k {
        Some(ServerType::Plex) => "Plex",
        _ => "Jellyfin",
    }
}

#[component]
fn Servers() -> impl IntoView {
    let f = form();
    // "jellyfin" or "plex" while adding one; the address to start from (renewing a key).
    let adding = RwSignal::new(None::<(&'static str, String)>);
    let remove = move |s: SettingsServer| {
        if f.edit.with_untracked(|e| e.servers.len()) <= 1 {
            f.error.set(Some(tr!(
                "Keep at least one media server: without one, nobody can sign in."
            )));
            return;
        }
        let maintainerr = f.edit.with_untracked(|e| {
            e.maintainerr
                .as_ref()
                .is_some_and(|m| !m.url.is_empty() && m.server == s.id)
        });
        let question = if maintainerr {
            tr!(
                "Remove {name}? Maintainerr is set up for it: Leaving soon stops until Maintainerr manages another of these servers.",
                name = s.name.clone()
            )
        } else {
            tr!(
                "Remove {name}? Its people can no longer sign in to AlwyWatch with it.",
                name = s.name.clone()
            )
        };
        let sure = web_sys::window()
            .and_then(|w| w.confirm_with_message(&question).ok())
            .unwrap_or(false);
        if sure {
            f.edit.update(|e| e.servers.retain(|x| x.id != s.id));
        }
    };
    // Redrawn when the servers change, not at each key typed elsewhere (Test answers stay).
    let servers = Memo::new(move |_| f.edit.with(|e| e.servers.clone()));
    view! {
        <section class="group" id="servers">
            <h2>{tr!("Media servers")}</h2>
            <p class="muted small">{tr!("People sign in to AlwyWatch with their login on these servers.")}</p>
            {move || servers.get().into_iter().map(|s| {
                let jellyfin = s.kind != Some(ServerType::Plex);
                let has_key = s.api_key.as_deref().is_some_and(|k| !k.is_empty());
                let (s2, url) = (s.clone(), s.public_url.clone());
                view! {
                    <div class="server-row">
                        <div class="row wrap">
                            <div>
                                <strong>{s.name.clone()}</strong>
                                <span class="muted small">" · "{kind_name(s.kind)}" · "{s.public_url.clone()}</span>
                                {s.internal_url.clone().map(|i| view! { <div class="muted small">{tr!("From this server: {address}", address = i)}</div> })}
                                {jellyfin.then(|| view! {
                                    <div class="small" class:muted=has_key class:accent=!has_key>{if has_key {
                                        tr!("API key: set")
                                    } else {
                                        tr!("API key: missing. “Until watched” and invitations with a Jellyfin login need it: renew it.")
                                    }}</div>
                                })}
                            </div>
                            <span class="actions tight">
                                {jellyfin.then(|| view! {
                                    <button type="button" class="small" on:click=move |_| adding.set(Some(("jellyfin", url.clone())))>{tr!("Renew key")}</button>
                                })}
                                <button type="button" class="small subtle" on:click=move |_| remove(s2.clone())>{tr!("Remove")}</button>
                            </span>
                        </div>
                        <Test what="server" server_id=s.id.clone() />
                    </div>
                }
            }).collect_view()}
            <div class="actions">
                <button type="button" on:click=move |_| adding.set(Some(("jellyfin", String::new())))>{tr!("Add a Jellyfin server")}</button>
                <button type="button" on:click=move |_| adding.set(Some(("plex", String::new())))>{tr!("Add a Plex server")}</button>
            </div>
            {move || adding.get().map(|(kind, url)| {
                let done = Callback::new(move |()| adding.set(None));
                if kind == "plex" {
                    view! { <AddPlexForm done=done /> }.into_any()
                } else {
                    view! { <AddJellyfinForm url=url done=done /> }.into_any()
                }
            })}
        </section>
    }
}

/// The line under the add forms while other changes wait to be saved.
fn save_first(f: Form) -> impl IntoView {
    move || {
        f.dirty().then(|| view! {
            <p class="muted small">{tr!("Save or undo your other changes first: adding a server starts the server again.")}</p>
        })
    }
}

#[component]
fn AddJellyfinForm(url: String, done: Callback<()>) -> impl IntoView {
    let f = form();
    let url = RwSignal::new(url);
    let internal = RwSignal::new(String::new());
    let user = RwSignal::new(String::new());
    let pass = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        error.set(None);
        f.busy.set(true);
        spawn_local(async move {
            let r = match admin_client(&state::ctx()) {
                Some(k) => {
                    k.admin_add_jellyfin(&AddJellyfin {
                        url: url.get_untracked().trim().to_owned(),
                        internal_url: some(internal.get_untracked().trim().to_owned()),
                        username: user.get_untracked().trim().to_owned(),
                        password: pass.get_untracked(),
                    })
                    .await
                }
                None => Err(net::Error::Unauthorized),
            };
            match r {
                Ok(a) => {
                    done.run(());
                    f.restarting(tr!(
                        "{name} is set. The server is starting again with it…",
                        name = a.name
                    ));
                }
                Err(e) => {
                    f.busy.try_set(false);
                    error.try_set(Some(admin_error(&e)));
                }
            }
        });
    };
    view! {
        <form class="form" on:submit=submit>
            <h3>{tr!("Add a Jellyfin server")}</h3>
            {field(tr!("Address"), tr!("The address you open Jellyfin at."), "https://jellyfin.example.org",
                (move || url.get(), move |v| url.set(v)))}
            {internal_address((move || internal.get(), move |v| internal.set(v)), "http://jellyfin:8096")}
            <label>{tr!("Administrator's username")}<input autocomplete="off" autocapitalize="off" spellcheck="false"
                prop:value=move || user.get() on:input=move |ev| user.set(event_target_value(&ev)) /></label>
            <label>{tr!("Password")}<input type="password" autocomplete="off" prop:value=move || pass.get()
                on:input=move |ev| pass.set(event_target_value(&ev)) />
                <span class="hint">{tr!("Used once to make AlwyWatch's own API key on this server, not kept.")}</span></label>
            {save_first(f)}
            <div class="actions">
                <button class="primary" type="submit" disabled=move || f.busy.get() || f.dirty()>{tr!("Add")}</button>
                <button type="button" on:click=move |_| done.run(())>{tr!("Cancel")}</button>
            </div>
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        </form>
    }
}

/// The plex.tv sign-ins this browser already has (from Plex logins).
fn plex_tokens(c: &Ctx) -> Vec<String> {
    c.accounts
        .get_untracked()
        .iter()
        .filter(|s| s.kind == media_api::ServerKind::Plex)
        .filter_map(|s| c.plex_tv_token(&state::account_key(s)))
        .collect()
}

#[component]
fn AddPlexForm(done: Callback<()>) -> impl IntoView {
    let f = form();
    let url = RwSignal::new(String::new());
    let internal = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let has_token = !plex_tokens(&state::ctx()).is_empty();
    // `fresh`: sign in to Plex again (another account) instead of using the one here.
    let add = move |fresh: bool| {
        error.set(None);
        f.busy.set(true);
        let win = fresh.then(crate::signin::open_plex_tab).flatten();
        spawn_local(async move {
            let c = state::ctx();
            let token = if fresh {
                match crate::signin::plex_tv_token(&c, win.as_ref(), || {
                    f.busy.try_get_untracked().is_some()
                })
                .await
                {
                    Ok(Some(t)) => t,
                    Ok(None) => return,
                    Err(e) => {
                        f.busy.try_set(false);
                        error.try_set(Some(e));
                        return;
                    }
                }
            } else {
                plex_tokens(&c).into_iter().next().unwrap_or_default()
            };
            let r = match admin_client(&c) {
                Some(k) => {
                    k.admin_add_plex(&AddPlex {
                        url: url.get_untracked().trim().to_owned(),
                        internal_url: some(internal.get_untracked().trim().to_owned()),
                        token,
                    })
                    .await
                }
                None => Err(net::Error::Unauthorized),
            };
            match r {
                Ok(a) => {
                    done.run(());
                    f.restarting(tr!(
                        "{name} is set. The server is starting again with it…",
                        name = a.name
                    ));
                }
                Err(e) => {
                    f.busy.try_set(false);
                    error.try_set(Some(admin_error(&e)));
                }
            }
        });
    };
    view! {
        <div class="form">
            <h3>{tr!("Add a Plex server")}</h3>
            {field(tr!("Address"), tr!("The address you open Plex at."), "https://plex.example.org",
                (move || url.get(), move |v| url.set(v)))}
            {internal_address((move || internal.get(), move |v| internal.set(v)), "http://plex:32400")}
            <p class="muted small">{tr!("Only the server's owner can add it: AlwyWatch checks it with the owner's Plex sign-in.")}</p>
            {save_first(f)}
            <div class="actions">
                {has_token.then(|| view! {
                    <button class="primary" type="button" on:click=move |_| add(false) disabled=move || f.busy.get() || f.dirty()>{tr!("Add with my Plex sign-in")}</button>
                })}
                <button type="button" class:primary=!has_token on:click=move |_| add(true) disabled=move || f.busy.get() || f.dirty()>
                    {if has_token { tr!("Sign in with another Plex account") } else { tr!("Sign in with Plex and add") }}
                </button>
                <button type="button" on:click=move |_| done.run(())>{tr!("Cancel")}</button>
            </div>
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        </div>
    }
}

#[component]
fn Maintainerr(problem: Option<String>) -> impl IntoView {
    let f = form();
    let edit = f.edit;
    let on = on_of!(f, maintainerr);
    let minutes = (
        move || {
            edit.with(|s| s.sync_minutes)
                .map(|m| m.to_string())
                .unwrap_or_default()
        },
        move |v: String| edit.update(|s| s.sync_minutes = v.trim().parse().ok()),
    );
    let servers = Memo::new(move |_| edit.with(|e| e.servers.clone()));
    view! {
        <section class="group" id="maintainerr">
            <h2>"Maintainerr"</h2>
            <p class="muted small">{tr!("Leaving soon: what Maintainerr will delete, and people's keep requests.")}</p>
            {toggle(tr!("Use Maintainerr"), String::new(), on)}
            {problem.map(|p| view! { <p class="small accent" role="status">{p}</p> })}
            <Show when=on.0>
                <div class="form">
                    {field(tr!("Address"), tr!("How the AlwyWatch server reaches Maintainerr (it doesn't need to be public)."),
                        "http://maintainerr:6246", text_of!(edit, maintainerr.url))}
                    {secret(tr!("API key (optional)"), tr!("Only when Maintainerr asks for one: Maintainerr → Settings → API key."),
                        secret_of!(edit, maintainerr.api_key))}
                    <label>{tr!("Media server it manages")}
                        <select prop:value=move || edit.with(|s| s.maintainerr.as_ref().map(|m| m.server.clone()).unwrap_or_default())
                            on:change=move |ev| { let v = event_target_value(&ev); edit.update(|s| if let Some(m) = &mut s.maintainerr { m.server = v }) }>
                            <option value="">{tr!("Choose…")}</option>
                            {move || servers.get().into_iter().map(|s| {
                                let selected = edit.with_untracked(|e| e.maintainerr.as_ref().is_some_and(|m| m.server == s.id));
                                view! { <option value=s.id.clone() selected=selected>{format!("{} ({})", s.name, kind_name(s.kind))}</option> }
                            }).collect_view()}
                        </select>
                        <span class="hint">{tr!("Maintainerr works with one: the one chosen in Maintainerr → Settings.")}</span>
                    </label>
                    {field(tr!("Read Maintainerr every (minutes)"), tr!("Empty: every 10 minutes."), "10", minutes)}
                    <Test what="maintainerr" />
                </div>
            </Show>
        </section>
    }
}

#[component]
fn Seerr() -> impl IntoView {
    let f = form();
    let edit = f.edit;
    let on = on_of!(f, seerr);
    let create = (
        move || edit.with(|s| s.seerr.as_ref().is_some_and(|x| x.create_users)),
        move |v: bool| {
            edit.update(|s| {
                if let Some(x) = &mut s.seerr {
                    x.create_users = v
                }
            })
        },
    );
    view! {
        <section class="group" id="seerr">
            <h2>"Seerr"</h2>
            <p class="muted small">{tr!("Requests in every app, each person as their own Seerr user.")}</p>
            {toggle(tr!("Requests through AlwyWatch"), String::new(), on)}
            <Show when=on.0>
                <div class="form">
                    {field(tr!("Address"), tr!("How the AlwyWatch server reaches Seerr."), "http://seerr:5055", text_of!(edit, seerr.url))}
                    {field(tr!("Public address (optional)"), tr!("Seerr's own site, for links to it."), "https://seerr.example.org",
                        opt_of!(edit, seerr.public_url))}
                    {secret(tr!("API key"), tr!("Seerr → Settings → General → API Key."), secret_of!(edit, seerr.api_key))}
                    {toggle(tr!("Create Seerr users automatically"), tr!("Someone without a Seerr user gets one at their first request."), create)}
                    <Test what="seerr" />
                </div>
            </Show>
        </section>
    }
}

#[component]
fn Tautulli() -> impl IntoView {
    let f = form();
    let edit = f.edit;
    let on = on_of!(f, tautulli);
    view! {
        <section class="group" id="tautulli">
            <h2>"Tautulli"</h2>
            <p class="muted small">{tr!("Plex: notices when a title kept “until watched” has been watched.")}</p>
            {toggle(tr!("Use Tautulli"), String::new(), on)}
            <Show when=on.0>
                <div class="form">
                    {field(tr!("Address"), tr!("How the AlwyWatch server reaches Tautulli."), "http://tautulli:8181", text_of!(edit, tautulli.url))}
                    {secret(tr!("API key"), tr!("Tautulli → Settings → Web Interface → API Key."), secret_of!(edit, tautulli.api_key))}
                    <Test what="tautulli" />
                </div>
            </Show>
        </section>
    }
}

#[component]
fn Email() -> impl IntoView {
    let f = form();
    let edit = f.edit;
    let on = on_of!(f, smtp);
    let to = RwSignal::new(String::new());
    let port = (
        move || {
            edit.with(|s| s.smtp.as_ref().and_then(|m| m.port))
                .map(|p| p.to_string())
                .unwrap_or_default()
        },
        move |v: String| {
            edit.update(|s| {
                if let Some(m) = &mut s.smtp {
                    m.port = v.trim().parse().ok()
                }
            })
        },
    );
    let security = move || {
        edit.with(|s| {
            s.smtp
                .as_ref()
                .map(|m| m.security.clone())
                .unwrap_or_default()
        })
    };
    view! {
        <section class="group" id="email">
            <h2>{tr!("Email")}</h2>
            <p class="muted small">{tr!("To send invitations by email. Any email provider works: use its SMTP settings.")}</p>
            {toggle(tr!("Send emails"), String::new(), on)}
            <Show when=on.0>
                <div class="form">
                    {field(tr!("SMTP server"), tr!("In your email provider's help, under SMTP."), "smtp.example.org", text_of!(edit, smtp.host))}
                    <label>{tr!("Security")}
                        <select prop:value=security on:change=move |ev| { let v = event_target_value(&ev); edit.update(|s| if let Some(m) = &mut s.smtp { m.security = v }) }>
                            {[("", tr!("Automatic")), ("force_tls", tr!("SSL/TLS (port 465)")), ("starttls", tr!("STARTTLS (port 587)")), ("off", tr!("None (not encrypted)"))]
                                .into_iter().map(|(v, name)| view! { <option value=v selected=move || security() == v>{name}</option> }).collect_view()}
                        </select>
                    </label>
                    {field(tr!("Port (optional)"), tr!("Empty: the usual one for the security chosen."), "587", port)}
                    {field(tr!("Username"), tr!("Often the email address itself."), "alwywatch@example.org", opt_of!(edit, smtp.username))}
                    {secret(tr!("Password"), tr!("Some providers want an app password here."), secret_of!(edit, smtp.password))}
                    {field(tr!("Sent from"), tr!("The address emails come from."), "alwywatch@example.org", text_of!(edit, smtp.from))}
                    {field(tr!("Sender name (optional)"), tr!("Empty: AlwyWatch."), "AlwyWatch", opt_of!(edit, smtp.from_name))}
                    <label>{tr!("Send a test email to (optional)")}
                        <input type="email" autocomplete="email" placeholder="you@example.org" prop:value=move || to.get()
                            on:input=move |ev| to.set(event_target_value(&ev)) /></label>
                    <Test what="email" email_to=to />
                </div>
            </Show>
        </section>
    }
}

#[component]
fn SignUp() -> impl IntoView {
    let f = form();
    let edit = f.edit;
    let site = (
        move || edit.with(|s| s.public_url.clone().unwrap_or_default()),
        move |v: String| edit.update(|s| s.public_url = some(v)),
    );
    let here = Ctx::origin();
    let here2 = here.clone();
    let registration = move || edit.with(|s| s.registration.clone());
    let pocket = on_of!(f, pocketid);
    let callback = move || {
        edit.with(|s| {
            s.public_url.as_deref().map(|u| {
                format!(
                    "{}/v1/account/oidc/callback",
                    u.trim().trim_end_matches('/')
                )
            })
        })
    };
    view! {
        <section class="group" id="signup">
            <h2>{tr!("Sign-up and sign-in")}</h2>
            <div class="form">
                {field(tr!("This site's address"), tr!("Needed for links in emails, passkeys and Pocket ID."), "https://watch.example.org", site)}
                {move || (site.0() != here).then(|| {
                    let here = here2.clone();
                    view! {
                        <p><button type="button" class="small" on:click=move |_| edit.update(|s| s.public_url = Some(here.clone()))>
                            {tr!("Use this page's address ({address})", address = here2.clone())}</button></p>
                    }
                })}
                <label>{tr!("Who can create an AlwyWatch account")}
                    <select prop:value=move || match registration().as_str() { "" => "invite".to_owned(), r => r.to_owned() }
                        on:change=move |ev| { let v = event_target_value(&ev); edit.update(|s| s.registration = v) }>
                        {[("invite", tr!("People with an invitation")), ("open", tr!("Anyone")), ("closed", tr!("Nobody"))]
                            .into_iter().map(|(v, name)| view! {
                                <option value=v selected=move || registration() == v || (v == "invite" && registration().is_empty())>{name}</option>
                            }).collect_view()}
                    </select>
                    <span class="hint">{tr!("People with a login on one of the media servers can always sign in.")}</span>
                </label>
            </div>
            <h3 id="pocketid">"Pocket ID"</h3>
            {toggle(tr!("Sign in with Pocket ID"), tr!("Single sign-on, for accounts without a password."), pocket)}
            <Show when=pocket.0>
                <div class="form">
                    {field(tr!("Pocket ID address"), String::new(), "https://id.example.org", text_of!(edit, pocketid.issuer))}
                    <div class="secret">
                        <span class="label">{tr!("Callback URL")}</span>
                        {move || match callback() {
                            Some(u) => view! { <div class="row tight"><code class="wrap">{u.clone()}</code><CopyButton text=u /></div> }.into_any(),
                            None => view! { <span class="hint">{tr!("Set this site's address above first.")}</span> }.into_any(),
                        }}
                        <span class="hint">{tr!("In Pocket ID → OIDC Clients → Add, paste this callback URL, then copy the client ID and secret here.")}</span>
                    </div>
                    {field(tr!("Client ID"), String::new(), "", text_of!(edit, pocketid.client_id))}
                    {secret(tr!("Client secret"), String::new(), secret_of!(edit, pocketid.client_secret))}
                    <Test what="pocketid" />
                </div>
            </Show>
        </section>
    }
}

#[component]
fn Webhooks(list: Vec<Webhook>) -> impl IntoView {
    let empty = list.is_empty();
    view! {
        <section class="group" id="webhooks">
            <h2>{tr!("Webhooks (optional)")}</h2>
            <p class="muted small">{tr!("Without them, watched titles are noticed within 15 minutes. With them, at once.")}</p>
            {empty.then(|| view! { <p class="muted small">{tr!("Set this site's address and save to get them.")}</p> })}
            {list.into_iter().map(|w| {
                let (name, where_) = match w.kind.as_str() {
                    "plex" => ("Plex", tr!("Needs Plex Pass: Plex → Settings → Webhooks → Add webhook.")),
                    "tautulli" => ("Tautulli", tr!("Tautulli → Settings → Notification Agents → Add → Webhook, triggered on “Watched”.")),
                    _ => ("Jellyfin", tr!("Install Jellyfin's Webhook plugin, then add a “Generic” destination with this address.")),
                };
                view! {
                    <div class="webhook">
                        <strong>{name}</strong>
                        <div class="row tight"><code class="wrap">{w.url.clone()}</code><CopyButton text=w.url /></div>
                        <div class="muted small">{where_}</div>
                    </div>
                }
            }).collect_view()}
        </section>
    }
}

#[component]
fn TestMode() -> impl IntoView {
    let edit = form().edit;
    let on = (
        move || edit.with(|s| s.dry_run),
        move |v: bool| edit.update(|s| s.dry_run = v),
    );
    view! {
        <section class="group" id="testmode">
            <h2>{tr!("Test mode")}</h2>
            {toggle(tr!("Test mode"), tr!("AlwyWatch reads everything but changes nothing in Maintainerr or Seerr: keep choices and requests are not applied."), on)}
            <Show when=on.0>
                <p class="small accent">{tr!("Turn it off once everything works, or keep choices will never protect anything.")}</p>
            </Show>
        </section>
    }
}

// ----- Invitations ------------------------------------------------------------------------

fn day(t: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t as f64 * 1000.0));
    d.to_locale_date_string(i18n::language(), &wasm_bindgen::JsValue::UNDEFINED)
        .as_string()
        .unwrap_or_default()
}

/// Settings → Admin → Invitations: invite someone by email or with a link, with a Jellyfin
/// login made at the same time; the invitations, which can be ended.
#[component]
pub fn Invitations() -> impl IntoView {
    let list = RwSignal::new(None::<Result<account::Invitations, String>>);
    let version = RwSignal::new(0u32);
    let email = RwSignal::new(String::new());
    let with_jellyfin = RwSignal::new(true);
    // The Jellyfin server and the user whose access is copied ("" = Jellyfin's defaults).
    let server = RwSignal::new(String::new());
    let like = RwSignal::new(String::new());
    let note = RwSignal::new(String::new());
    let days = RwSignal::new(7i64);
    let uses = RwSignal::new(1i64);
    let made = RwSignal::new(None::<account::Invite>);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    Effect::new(move |_| {
        version.get();
        spawn_local(async move {
            let Some(k) = admin_client(&state::ctx()) else {
                return;
            };
            let r = k.account_invites().await;
            if let Ok(inv) = &r
                && server.get_untracked().is_empty()
                && let Some(t) = inv.jellyfin.first()
            {
                server.try_set(t.server_id.clone());
                like.try_set(
                    t.last_like
                        .clone()
                        .filter(|l| t.users.iter().any(|u| &u.id == l))
                        .or_else(|| t.users.first().map(|u| u.id.clone()))
                        .unwrap_or_default(),
                );
            }
            list.try_set(Some(r.map_err(|e| admin_error(&e))));
        });
    });
    let info = move || list.with(|l| l.as_ref().and_then(|r| r.as_ref().ok()).cloned());
    let mail = move || info().is_some_and(|i| i.mail);
    let targets = move || info().map(|i| i.jellyfin).unwrap_or_default();
    let by_email = move || mail() && !email.get().trim().is_empty();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        error.set(None);
        made.set(None);
        let to = by_email().then(|| email.get_untracked().trim().to_owned());
        let jellyfin = (with_jellyfin.get_untracked() && !targets().is_empty())
            .then(|| server.get_untracked())
            .filter(|s| !s.is_empty());
        let has_url = info().is_some_and(|i| i.public_url);
        let req = InviteRequest {
            days: days.get_untracked(),
            uses: if to.is_some() {
                1
            } else {
                uses.get_untracked()
            },
            like: jellyfin.as_ref().and_then(|_| some(like.get_untracked())),
            jellyfin,
            note: to
                .as_ref()
                .and_then(|_| some(note.get_untracked().trim().chars().take(500).collect())),
            email: to,
            lang: Some(i18n::language().to_owned()),
        };
        busy.set(true);
        spawn_local(async move {
            let r = match admin_client(&state::ctx()) {
                Some(k) => k.account_invite_with(&req).await,
                None => Err(net::Error::Unauthorized),
            };
            busy.try_set(false);
            match r {
                Ok(i) => {
                    if i.sent_to.is_some() {
                        email.try_set(String::new());
                        note.try_set(String::new());
                    }
                    made.try_set(Some(i));
                    version.try_update(|v| *v += 1);
                }
                Err(e) => {
                    let text = match e {
                        net::Error::Status { status: 501 } if !has_url => tr!(
                            "Set this site's address in Server settings first: invitations need it for their link."
                        ),
                        net::Error::Status { status: 501 } => {
                            tr!("Set up email in Server settings first.")
                        }
                        net::Error::Status { status: 502 } => tr!(
                            "The email could not be sent. Check Server settings → Email (its Test button tells why)."
                        ),
                        e => admin_error(&e),
                    };
                    error.try_set(Some(text));
                }
            }
        });
    };
    let end = move |id: i64| {
        spawn_local(async move {
            let Some(k) = admin_client(&state::ctx()) else {
                return;
            };
            match k.account_revoke_invite(id).await {
                Ok(()) => {
                    version.try_update(|v| *v += 1);
                }
                Err(e) => {
                    error.try_set(Some(admin_error(&e)));
                }
            }
        });
    };
    view! {
        <section class="group" id="invitations">
            <h2>{tr!("Invitations")}</h2>
            <p class="muted small">{tr!("Invite someone: with one link they create an AlwyWatch account, and a Jellyfin login too if you want.")}</p>
            {move || match list.get() {
                None => view! { <Loading /> }.into_any(),
                Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
                Some(Ok(inv)) => {
                    let mail_on = inv.mail;
                    let public = inv.public_url;
                    let tg = inv.jellyfin.clone();
                    view! {
                        <form class="form" on:submit=submit>
                            <label>{tr!("Email (optional)")}
                                <input type="email" autocomplete="off" placeholder="friend@example.org" disabled=!mail_on
                                    prop:value=move || email.get() on:input=move |ev| email.set(event_target_value(&ev)) />
                                {if mail_on {
                                    view! { <span class="hint">{tr!("Empty: just make a link to send yourself.")}</span> }.into_any()
                                } else {
                                    view! { <span class="hint"><A href="/settings/admin/server#email">{tr!("Set up email in Server settings to send invitations by email")}</A></span> }.into_any()
                                }}
                            </label>
                            {(!tg.is_empty()).then(|| {
                                let tg2 = tg.clone();
                                let one = tg.len() == 1;
                                let first_name = tg[0].name.clone();
                                view! {
                                    <label class="row toggle">
                                        <div>{if one {
                                            tr!("Also make a Jellyfin login on {server}", server = first_name)
                                        } else {
                                            tr!("Also make a Jellyfin login")
                                        }}<div class="muted small">{tr!("Same username and password as their AlwyWatch account.")}</div></div>
                                        <input type="checkbox" prop:checked=move || with_jellyfin.get() on:change=move |ev| with_jellyfin.set(event_target_checked(&ev)) />
                                    </label>
                                    <Show when=move || with_jellyfin.get()>
                                        {(!one).then(|| view! {
                                            <label>{tr!("On")}
                                                <select prop:value=move || server.get() on:change=move |ev| { server.set(event_target_value(&ev)); like.set(String::new()) }>
                                                    {tg.iter().map(|t| { let id = t.server_id.clone(); view! {
                                                        <option value=t.server_id.clone() selected=move || server.get() == id>{t.name.clone()}</option>
                                                    }}).collect_view()}
                                                </select>
                                            </label>
                                        })}
                                        {
                                            let tg = tg2.clone();
                                            move || {
                                                let users = tg.iter().find(|t| t.server_id == server.get()).map(|t| t.users.clone()).unwrap_or_default();
                                                view! {
                                                    <label>{tr!("With the same access as")}
                                                        <select prop:value=move || like.get() on:change=move |ev| like.set(event_target_value(&ev))>
                                                            {users.into_iter().map(|u| { let id = u.id.clone(); view! {
                                                                <option value=u.id.clone() selected=move || like.get() == id>{u.name}</option>
                                                            }}).collect_view()}
                                                            <option value="" selected=move || like.get().is_empty()>{tr!("Jellyfin's defaults")}</option>
                                                        </select>
                                                        <span class="hint">{tr!("Libraries and parental ratings are copied from this person.")}</span>
                                                    </label>
                                                }
                                            }
                                        }
                                    </Show>
                                }
                            })}
                            <Show when=by_email>
                                <label>{tr!("Personal note (optional)")}
                                    <textarea maxlength="500" rows="3" prop:value=move || note.get() on:input=move |ev| note.set(event_target_value(&ev))></textarea>
                                    <span class="hint">{tr!("A few words in the email.")}</span>
                                </label>
                            </Show>
                            <label>{tr!("Valid for")}
                                <select on:change=move |ev| days.set(event_target_value(&ev).parse().unwrap_or(7))>
                                    {[1i64, 7, 30].into_iter().map(|d| view! {
                                        <option value=d.to_string() selected=move || days.get() == d>{trn!("{n} day", "{n} days", d)}</option>
                                    }).collect_view()}
                                </select>
                            </label>
                            <Show when=move || !by_email()>
                                <label>{tr!("How many people can use it")}
                                    <select on:change=move |ev| uses.set(event_target_value(&ev).parse().unwrap_or(1))>
                                        {[1i64, 5, 10].into_iter().map(|n| view! {
                                            <option value=n.to_string() selected=move || uses.get() == n>{n.to_string()}</option>
                                        }).collect_view()}
                                    </select>
                                </label>
                            </Show>
                            {(!public).then(|| view! {
                                <p class="muted small"><A href="/settings/admin/server#signup">{tr!("Set this site's address in Server settings so invitations come with a link.")}</A></p>
                            })}
                            <div class="actions">
                                <button class="primary" type="submit" disabled=move || busy.get()>
                                    {move || if by_email() { tr!("Send invitation") } else { tr!("Create link") }}
                                </button>
                            </div>
                        </form>
                    }.into_any()
                }
            }}
            {move || made.get().map(|i| match i.sent_to.clone() {
                Some(to) => view! { <p class="test ok" role="status">{tr!("Sent to {email}", email = to)}</p> }.into_any(),
                None => {
                    let url = i.url.clone().unwrap_or_default();
                    let has_url = !url.is_empty();
                    let (u1, u2) = (url.clone(), url.clone());
                    view! {
                        <div class="made">
                            {has_url.then(|| view! {
                                <div class="row tight"><code class="wrap">{u1.clone()}</code><CopyButton text=u1.clone() />
                                    {share_supported().then(|| view! {
                                        <button type="button" class="small" on:click=move |_| share(&tr!("Join me on AlwyWatch"), &u2)>{tr!("Share")}</button>
                                    })}
                                </div>
                            })}
                            <p class="small">{
                                let (before, after) = crate::ui::around(&tr!("Invitation code: {code}"), "{code}");
                                view! { {before}<strong class="code">{i.code.clone()}</strong>{after} }
                            }</p>
                            <p class="muted small">{tr!("They open the link (or {site}/join and type the code).", site = Ctx::origin())}</p>
                        </div>
                    }.into_any()
                }
            })}
            {move || error.get().map(|e| view! { <ErrorText text=e /> })}
            {move || info().filter(|i| !i.invites.is_empty()).map(|i| {
                let now = (js_sys::Date::now() / 1000.0) as i64;
                view! {
                    <h3>{tr!("Sent and waiting")}</h3>
                    {i.invites.into_iter().map(|x| {
                        let active = x.uses_left > 0 && x.expires > now;
                        let id = x.id;
                        let what = x.email.clone().unwrap_or_else(|| tr!("Link"));
                        let mut details = vec![tr!("by {name}", name = x.created_by.clone())];
                        if let Some(j) = &x.jellyfin {
                            details.push(tr!("with a login on {server}", server = j.clone()));
                        }
                        if !x.used_by.is_empty() {
                            details.push(tr!("used by {names}", names = x.used_by.join(", ")));
                        }
                        details.push(if active {
                            tr!("until {date}", date = day(x.expires))
                        } else if x.uses_left <= 0 {
                            tr!("used up")
                        } else {
                            tr!("ended")
                        });
                        view! {
                            <div class="row">
                                <div><strong>{what}</strong><div class="muted small">{details.join(" · ")}</div></div>
                                {active.then(|| view! { <button type="button" class="small" on:click=move |_| end(id)>{tr!("End")}</button> })}
                            </div>
                        }
                    }).collect_view()}
                }
            })}
        </section>
    }
}
