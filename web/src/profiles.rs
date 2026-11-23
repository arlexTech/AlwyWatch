// SPDX-License-Identifier: AGPL-3.0-or-later
//! Profiles of this browser, as on the apps: several people on one computer, each with their
//! own accounts, AlwyWatch link and settings. A profile is a namespace in the browser's
//! storage (`aw.p.<id>.…`; the first profile keeps the plain `aw.…` keys), and switching
//! reloads the app into it. No PIN here: a browser's storage is readable by whoever uses it.

use i18n::tr;
use std::cell::RefCell;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

const LIST: &str = "aw.profiles";
const CURRENT: &str = "aw.profile";
pub const DEFAULT: &str = "default";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Added without a name: it takes the person's name at their first sign-in.
    #[serde(default)]
    pub auto: bool,
}

thread_local! {
    /// The profile this page runs as (fixed for the page's life: switching reloads).
    static ACTIVE: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn raw_list() -> Vec<Profile> {
    storage()
        .and_then(|s| s.get_item(LIST).ok().flatten())
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

fn save_list(list: &[Profile]) {
    if let Some(s) = storage() {
        let _ = s.set_item(LIST, &serde_json::to_string(list).unwrap_or_default());
    }
}

/// Every profile; one unnamed ("Main") until another is added.
pub fn list() -> Vec<Profile> {
    let mut l = raw_list();
    if !l.iter().any(|p| p.id == DEFAULT) {
        l.insert(
            0,
            Profile {
                id: DEFAULT.into(),
                name: tr!("Main"),
                auto: false,
            },
        );
    }
    l
}

/// The profile this page runs as.
pub fn current() -> String {
    ACTIVE.with(|a| {
        a.borrow_mut()
            .get_or_insert_with(|| {
                storage()
                    .and_then(|s| s.get_item(CURRENT).ok().flatten())
                    .filter(|id| list().iter().any(|p| &p.id == id))
                    .unwrap_or_else(|| DEFAULT.into())
            })
            .clone()
    })
}

/// Where `key` (`aw.…`) lives for a profile.
fn scoped_for(profile: &str, key: &str) -> String {
    if profile == DEFAULT || key == LIST || key == CURRENT {
        return key.to_owned();
    }
    format!("aw.p.{profile}.{}", key.strip_prefix("aw.").unwrap_or(key))
}

/// Where `key` lives for the current profile (used by every read and write in `state`).
pub fn scoped(key: &str) -> String {
    scoped_for(&current(), key)
}

fn clean(name: &str) -> String {
    let n: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(24)
        .collect();
    if n.is_empty() { "Profile".into() } else { n }
}

/// A new profile, named after the person once they sign in ("Add a profile").
pub fn add_new() -> String {
    let id = add(&tr!("New profile"));
    let mut l = list();
    if let Some(p) = l.iter_mut().find(|p| p.id == id) {
        p.auto = true;
    }
    save_list(&l);
    id
}

/// A new profile with Jellyfin/Plex logins only: never linked to the AlwyWatch server by
/// itself (its logins and settings stay in this browser).
pub fn add_offline() -> String {
    let id = add_new();
    if let Some(s) = storage() {
        let _ = s.set_item(&scoped_for(&id, crate::state::KEEP_OFF), "true");
    }
    id
}

/// The person of this profile is known now: a profile added without a name takes theirs.
pub fn adopt_name(name: &str) {
    let me = current();
    let mut l = list();
    if let Some(p) = l.iter_mut().find(|p| p.id == me && p.auto)
        && !name.trim().is_empty()
    {
        p.name = clean(name);
        p.auto = false;
        save_list(&l);
    }
}

fn add(name: &str) -> String {
    let mut b = [0u8; 4];
    if let Some(c) = web_sys::window().and_then(|w| w.crypto().ok()) {
        let _ = c.get_random_values_with_u8_array(&mut b);
    }
    let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let mut l = list();
    l.push(Profile {
        id: id.clone(),
        name: clean(name),
        auto: false,
    });
    save_list(&l);
    id
}

pub fn rename(id: &str, name: &str) {
    let mut l = list();
    if let Some(p) = l.iter_mut().find(|p| p.id == id) {
        p.name = clean(name);
        p.auto = false;
    }
    save_list(&l);
}

/// Removes a profile and everything it kept in this browser (not the one in use, nor the
/// first one, whose keys are the unprefixed ones).
pub fn remove(id: &str) {
    if id == DEFAULT || id == current() {
        return;
    }
    if let Some(s) = storage() {
        let prefix = format!("aw.p.{id}.");
        let keys: Vec<String> = (0..s.length().unwrap_or(0))
            .filter_map(|i| s.key(i).ok().flatten())
            .filter(|k| k.starts_with(&prefix))
            .collect();
        for k in keys {
            let _ = s.remove_item(&k);
        }
    }
    save_list(
        &list()
            .into_iter()
            .filter(|p| p.id != id)
            .collect::<Vec<_>>(),
    );
}

/// Switches profile: the app starts over as that person.
pub fn switch(id: &str) {
    switch_to(id, "/");
}

/// Switches profile and opens `path` in it.
pub fn switch_to(id: &str, path: &str) {
    let Some(w) = web_sys::window() else { return };
    if let Some(s) = storage() {
        let _ = s.set_item(CURRENT, id);
    }
    // This tab's room seat belongs to the profile being left.
    if let Ok(Some(t)) = w.session_storage() {
        let _ = t.clear();
    }
    let _ = w.location().set_href(path);
}

/// The menu's profile item: who is watching, one click to switch or add someone.
#[component]
pub fn ProfileMenu() -> impl IntoView {
    let all = list();
    let me = current();
    let name = all
        .iter()
        .find(|p| p.id == me)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    view! {
        <details class="profile-menu">
            <summary title=tr!("Switch profile")><span class="pdot">{initial(&name)}</span><span class="pname">{name}</span></summary>
            <div class="profile-list">
                {all.into_iter().filter(|p| p.id != me).map(|p| {
                    let id = p.id.clone();
                    view! {
                        <button on:click=move |_| switch(&id)><span class="pdot">{initial(&p.name)}</span>{p.name.clone()}</button>
                    }
                }).collect_view()}
                <a href="/settings/device?add=1">{tr!("Add a profile")}</a>
                <a href="/settings/device">{tr!("Manage profiles")}</a>
            </div>
        </details>
    }
}

fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

/// Settings → This device → Profiles.
#[component]
pub fn ProfilesSettings() -> impl IntoView {
    let version = RwSignal::new(0u32);
    let adding = RwSignal::new(
        leptos_router::hooks::use_query_map().with_untracked(|q| q.get("add").is_some()),
    );
    let renaming = RwSignal::new(None::<(String, String)>);
    let me = current();
    // A new profile starts empty: its own sign-in, then it takes the person's name.
    let sign_in = Callback::new(|()| switch_to(&add_new(), "/signin?step=signin"));
    let create = Callback::new(|()| switch_to(&add_new(), "/join"));
    let media = Callback::new(|()| switch_to(&add_offline(), "/signin?step=media"));
    view! {
        <section class="group">
            <h2>{tr!("Profiles")}</h2>
            <p class="muted small">{tr!("Several people on this browser: each profile has its own accounts, history and settings.")}</p>
            {move || {
                version.get();
                let me = me.clone();
                list().into_iter().map(|p| {
                    let here = p.id == me;
                    let (id, id2, id3, name) = (p.id.clone(), p.id.clone(), p.id.clone(), p.name.clone());
                    let editing = move || renaming.get().is_some_and(|(i, _)| i == id3);
                    let removable = !here && p.id != DEFAULT;
                    view! {
                        <div class="row">
                            {move || if editing() {
                                let id = id.clone();
                                view! {
                                    <form class="name-form" on:submit=move |ev| {
                                        ev.prevent_default();
                                        if let Some((_, n)) = renaming.get_untracked() {
                                            rename(&id, &n);
                                        }
                                        renaming.set(None);
                                        version.update(|v| *v += 1);
                                    }>
                                        <input maxlength="24" prop:value=move || renaming.get().map(|(_, n)| n).unwrap_or_default()
                                            on:input=move |ev| renaming.update(|r| if let Some(r) = r { r.1 = event_target_value(&ev) }) />
                                        <button class="small primary" type="submit">{tr!("Save")}</button>
                                    </form>
                                }.into_any()
                            } else {
                                view! { <div><strong>{name.clone()}</strong>{here.then(|| view! { <span class="muted">" · "{tr!("in use")}</span> })}</div> }.into_any()
                            }}
                            <div class="actions tight">
                                {(!here).then(|| { let id = p.id.clone(); view! { <button class="small primary" on:click=move |_| switch(&id)>{tr!("Switch")}</button> } })}
                                {
                                    let (id, name) = (p.id.clone(), p.name.clone());
                                    view! { <button class="small" on:click=move |_| renaming.set(Some((id.clone(), name.clone())))>{tr!("Rename")}</button> }
                                }
                                {removable.then(|| { let id = id2.clone(); view! { <button class="small subtle" on:click=move |_| {
                                    let sure = web_sys::window()
                                        .and_then(|w| w.confirm_with_message(&tr!("Remove this profile? Its accounts and settings in this browser are deleted.")).ok())

                                        .unwrap_or(false);
                                    if sure {
                                        remove(&id);
                                        version.update(|v| *v += 1);
                                    }
                                }>{tr!("Remove")}</button> } })}
                            </div>
                        </div>
                    }
                }).collect_view()
            }}
            {move || if adding.get() {
                view! {
                    <h3>{tr!("Add a profile")}</h3>
                    <crate::signin::StartCards on_sign_in=sign_in on_create=create on_media=media />
                    <div class="actions"><button class="small subtle" on:click=move |_| adding.set(false)>{tr!("Cancel")}</button></div>
                }.into_any()
            } else {
                view! { <div class="actions"><button class="small" on:click=move |_| adding.set(true)>{tr!("Add a profile")}</button></div> }.into_any()
            }}
        </section>
    }
}

/// Settings → Account for a profile without an AlwyWatch account: its name on this browser.
#[component]
pub fn LocalProfile() -> impl IntoView {
    let me = current();
    let name = RwSignal::new(
        list()
            .into_iter()
            .find(|p| p.id == me)
            .map(|p| p.name)
            .unwrap_or_default(),
    );
    let draft = RwSignal::new(None::<String>);
    view! {
        <div class="profile-head">
            <span class="pdot big" aria-hidden="true">{move || initial(&name.get())}</span>
            <strong class="profile-name">{move || name.get()}</strong>
        </div>
        {move || match draft.get() {
            Some(_) => {
                let me = me.clone();
                view! {
                    <form class="name-form" on:submit=move |ev| {
                        ev.prevent_default();
                        if let Some(n) = draft.get_untracked() {
                            rename(&me, &n);
                            name.set(clean(&n));
                        }
                        draft.set(None);
                    }>
                        <input aria-label=tr!("Name") maxlength="24" prop:value=move || draft.get().unwrap_or_default()
                            on:input=move |ev| draft.set(Some(event_target_value(&ev))) />
                        <button class="small primary" type="submit">{tr!("Save")}</button>
                        <button class="small subtle" type="button" on:click=move |_| draft.set(None)>{tr!("Cancel")}</button>
                    </form>
                }.into_any()
            }
            None => view! {
                <div class="actions tight"><button class="small" on:click=move |_| draft.set(Some(name.get_untracked()))>{tr!("Change name")}</button></div>
            }.into_any(),
        }}
        <p class="muted small">{tr!("Kept in this browser only.")}</p>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_profile_keeps_the_plain_keys() {
        assert_eq!(scoped_for(DEFAULT, "aw.accounts"), "aw.accounts");
        assert_eq!(scoped_for("ab12", "aw.accounts"), "aw.p.ab12.accounts");
        assert_eq!(scoped_for("ab12", LIST), LIST);
        assert_eq!(clean("  "), "Profile");
    }
}
