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

pub fn add(name: &str) -> String {
    let mut b = [0u8; 4];
    if let Some(c) = web_sys::window().and_then(|w| w.crypto().ok()) {
        let _ = c.get_random_values_with_u8_array(&mut b);
    }
    let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let mut l = list();
    l.push(Profile {
        id: id.clone(),
        name: clean(name),
    });
    save_list(&l);
    id
}

pub fn rename(id: &str, name: &str) {
    let mut l = list();
    if let Some(p) = l.iter_mut().find(|p| p.id == id) {
        p.name = clean(name);
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
    let Some(w) = web_sys::window() else { return };
    if let Some(s) = storage() {
        let _ = s.set_item(CURRENT, id);
    }
    // This tab's room seat belongs to the profile being left.
    if let Ok(Some(t)) = w.session_storage() {
        let _ = t.clear();
    }
    let _ = w.location().set_href("/");
}

/// The menu's profile item (several profiles): who is watching, one click to switch.
#[component]
pub fn ProfileMenu() -> impl IntoView {
    let all = list();
    if all.len() < 2 {
        return ().into_any();
    }
    let me = current();
    let name = all
        .iter()
        .find(|p| p.id == me)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    view! {
        <details class="profile-menu">
            <summary title=tr!("Switch profile")><span class="pdot">{initial(&name)}</span>{name}</summary>
            <div class="profile-list">
                {all.into_iter().filter(|p| p.id != me).map(|p| {
                    let id = p.id.clone();
                    view! {
                        <button on:click=move |_| switch(&id)><span class="pdot">{initial(&p.name)}</span>{p.name.clone()}</button>
                    }
                }).collect_view()}
                <a href="/settings/profiles">{tr!("Manage profiles")}</a>
            </div>
        </details>
    }
    .into_any()
}

fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

/// Settings → Profiles.
#[component]
pub fn ProfilesSettings() -> impl IntoView {
    let version = RwSignal::new(0u32);
    let new_name = RwSignal::new(String::new());
    let renaming = RwSignal::new(None::<(String, String)>);
    let me = current();
    let add_one = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let n = new_name.get_untracked();
        if !n.trim().is_empty() {
            // A new profile starts empty: its own sign-in.
            switch(&add(&n));
        }
    };
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
            <form class="name-form" on:submit=add_one>
                <input placeholder=tr!("New profile's name") maxlength="24" prop:value=move || new_name.get()
                    on:input=move |ev| new_name.set(event_target_value(&ev)) />
                <button class="small" type="submit">{tr!("Add and switch")}</button>
            </form>
        </section>
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
