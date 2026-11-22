// SPDX-License-Identifier: AGPL-3.0-or-later
//! One AlwyWatch account for several sign-ins (docs/identity-apps.md): the one-time "Are these
//! both you?" dialog, Settings → AlwyWatch account → Linked sign-ins (list, Unlink, Link
//! another sign-in), and everything refreshed after a link or an unlink.
//!
//! The browser keeps one linked account (`Ctx.keep`, A) and several signed-in accounts
//! (`Ctx.accounts`). Each other account is asked for its own `Me` with its own token (read
//! only), to know whether it is on this AlwyWatch server and already the same person.

use keep_service::identity::{LinkedIdentity, MeWithIdentities};
use keep_service::{KeepClient, Me, ServerType};
use leptos::prelude::*;
use leptos::task::spawn_local;
use media_api::ServerKind;
use net::Error;
use serde::{Deserialize, Serialize};

use crate::profile::Avatar;
use crate::state::{self, Ctx};
use crate::ui::ErrorText;

/// "Not now" answers in this browser.
const NOT_NOW: &str = "aw.links.notnow";

// ---------------------------------------------------------------------------------------------
// Which pair to offer (pure)
// ---------------------------------------------------------------------------------------------

/// One account of this browser as the AlwyWatch server sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    /// `state::account_key`.
    pub account: String,
    pub server_id: String,
    pub server_name: String,
    pub kind: ServerKind,
    pub me: Me,
    /// Every sign-in of its AlwyWatch account (empty: an older server).
    pub identities: Vec<LinkedIdentity>,
}

impl Side {
    fn keys(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.me.user_key.as_str())
            .chain(self.identities.iter().map(|i| i.user_key.as_str()))
    }

    fn servers(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.server_id.as_str())
            .chain(self.identities.iter().map(|i| i.server_id.as_str()))
    }
}

/// A "Not now": the two accounts (forgotten when one signs out) and the two people.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NotNow {
    pub accounts: [String; 2],
    pub users: [String; 2],
}

/// Whether `b` can join `a`'s AlwyWatch account: not the same person already, and no server in
/// common (two users of one server are never one person: the server refuses).
pub fn linkable(a: &Side, b: &Side) -> bool {
    !a.keys().any(|k| b.keys().any(|l| l == k)) && !a.servers().any(|s| b.servers().any(|t| t == s))
}

fn dismissed(a: &Side, b: &Side, not_now: &[NotNow]) -> bool {
    not_now.iter().any(|n| {
        let (x, y) = (n.users[0].as_str(), n.users[1].as_str());
        (x == a.me.user_key && y == b.me.user_key) || (y == a.me.user_key && x == b.me.user_key)
    })
}

/// The account to offer in "Are these both you?" (index in `others`), if any.
pub fn to_offer(a: &Side, others: &[Side], not_now: &[NotNow]) -> Option<usize> {
    others
        .iter()
        .position(|b| linkable(a, b) && !dismissed(a, b, not_now))
}

pub fn remember(not_now: &mut Vec<NotNow>, a: &Side, b: &Side) {
    if !dismissed(a, b, not_now) {
        not_now.push(NotNow {
            accounts: [a.account.clone(), b.account.clone()],
            users: [a.me.user_key.clone(), b.me.user_key.clone()],
        });
    }
}

pub fn forget_account(not_now: &mut Vec<NotNow>, account: &str) {
    not_now.retain(|n| !n.accounts.iter().any(|a| a == account));
}

fn kind_name(k: ServerKind) -> &'static str {
    match k {
        ServerKind::Plex => "Plex",
        ServerKind::Jellyfin => "Jellyfin",
    }
}

fn type_name(k: ServerType) -> &'static str {
    match k {
        ServerType::Plex => "Plex",
        ServerType::Jellyfin => "Jellyfin",
    }
}

/// "Also signed in as alex (Home), …" (empty: nothing linked). `server_id`: the linked
/// account's server, for servers that don't mark the current sign-in.
pub fn also_signed_in(ids: &[LinkedIdentity], server_id: &str) -> String {
    if ids.len() < 2 {
        return String::new();
    }
    let marked = ids.iter().any(|i| i.current);
    let others: Vec<String> = ids
        .iter()
        .filter(|i| {
            if marked {
                !i.current
            } else {
                i.server_id != server_id
            }
        })
        .map(|i| format!("{} ({})", i.media_name, i.server_name))
        .collect();
    if others.is_empty() {
        String::new()
    } else {
        format!("Also signed in as {}", others.join(", "))
    }
}

pub fn unlink_question(i: &LinkedIdentity) -> String {
    format!(
        "{} on {} becomes a separate, new AlwyWatch account with no keep choices, name, avatar \
         or history. Continue?",
        i.media_name, i.server_name
    )
}

/// A failed link in plain words. `served`: whether this site still serves the other sign-in's
/// server (401 and 403 both arrive as `Unauthorized`).
pub fn link_error(e: &Error, server: &str, served: Option<bool>) -> String {
    match e {
        Error::Unauthorized if served == Some(false) => {
            format!("This site doesn't serve {server}, so this sign-in can't be linked.")
        }
        Error::Unauthorized => {
            format!(
                "Your {server} sign-in has expired: sign in again with that account, then link."
            )
        }
        Error::Status { status: 409 } => format!(
            "These are two different people on the same server: your AlwyWatch account already \
             has another sign-in on {server}."
        ),
        other => common_error(other),
    }
}

pub fn unlink_error(e: &Error) -> String {
    match e {
        Error::Status { status: 409 } => "The main sign-in can't be unlinked.".into(),
        Error::Unauthorized => {
            "The AlwyWatch server refused: sign in again with this account.".into()
        }
        other => common_error(other),
    }
}

fn common_error(e: &Error) -> String {
    match e {
        Error::Status { status: 429 } => {
            "Too many tries: wait a few minutes, then try again.".into()
        }
        Error::Status { status: 502 } => "The media server is unreachable right now.".into(),
        Error::NotFound | Error::Status { status: 405 } => {
            "Your AlwyWatch server doesn't support this yet: it needs an update.".into()
        }
        other => state::plain_keep(other),
    }
}

// ---------------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------------

/// Who the linked account and the other accounts are, and the dialog asked.
#[derive(Clone, Copy)]
pub struct Links {
    /// The linked account (`Ctx.keep`) with its sign-ins.
    pub me: RwSignal<Option<Side>>,
    /// The other accounts of this browser that are on this AlwyWatch server.
    pub others: RwSignal<Vec<Side>>,
    /// "Are these both you?" for this account; `true`: the one-time prompt ("Not now").
    pub dialog: RwSignal<Option<(String, bool)>>,
    /// Bumped after a link or an unlink: pages showing per-person data load again.
    pub revision: RwSignal<u32>,
    generation: StoredValue<u32>,
}

impl Links {
    pub fn new() -> Self {
        Self {
            me: RwSignal::new(None),
            others: RwSignal::new(Vec::new()),
            dialog: RwSignal::new(None),
            revision: RwSignal::new(0),
            generation: StoredValue::new(0),
        }
    }

    /// Signed out of the AlwyWatch server: nothing to list or offer.
    pub fn clear(self) {
        self.generation.update_value(|g| *g += 1);
        self.me.set(None);
        self.others.set(Vec::new());
        self.dialog.set(None);
    }
}

fn not_now() -> Vec<NotNow> {
    state::load(NOT_NOW).unwrap_or_default()
}

/// An account signed out of this browser: its "Not now" answers go, and it is no candidate.
pub fn account_removed(c: &Ctx, account: &str) {
    let mut list = not_now();
    forget_account(&mut list, account);
    state::save(NOT_NOW, &list);
    c.links
        .others
        .update(|o| o.retain(|s| s.account != account));
    if c.links
        .dialog
        .with_untracked(|d| d.as_ref().is_some_and(|(a, _)| a == account))
    {
        c.links.dialog.set(None);
    }
}

fn side(s: &media_api::Session, m: MeWithIdentities) -> Side {
    Side {
        account: state::account_key(s),
        server_id: s.server_id.clone(),
        server_name: s.server_name.clone(),
        kind: s.kind,
        me: m.me,
        identities: m.identities,
    }
}

/// Asks the AlwyWatch server who each account of this browser is; `prompt`: then offers the
/// pair once.
pub async fn refresh(prompt: bool) {
    let c = state::ctx();
    let links = c.links;
    let Some(k) = c.keep.get_untracked() else {
        return links.clear();
    };
    let Some((mine, client)) = c
        .session(&k.account)
        .and_then(|s| c.keep_for(&s).map(|cl| (s, cl)))
    else {
        return;
    };
    links.generation.update_value(|g| *g += 1);
    let generation = links.generation.get_value();
    let others: Vec<(media_api::Session, KeepClient)> = c
        .accounts
        .get_untracked()
        .into_iter()
        .filter(|s| state::account_key(s) != k.account)
        .filter_map(|s| c.keep_for(&s).map(|cl| (s, cl)))
        .collect();
    let (me, found) =
        state::join2(
            client.me_with_identities(),
            state::join_all(others.iter().map(|(s, cl)| async move {
                cl.me_with_identities().await.ok().map(|m| side(s, m))
            })),
        )
        .await;
    if links.generation.try_get_value() != Some(generation) {
        return;
    }
    let Ok(me) = me else { return };
    let a = side(&mine, me);
    let found: Vec<Side> = found.into_iter().flatten().collect();
    let offer = to_offer(&a, &found, &not_now()).map(|i| found[i].account.clone());
    links.me.try_set(Some(a));
    links.others.try_set(found);
    if prompt
        && let Some(b) = offer
        && links.dialog.get_untracked().is_none()
    {
        links.dialog.try_set(Some((b, true)));
    }
}

/// After a link or an unlink my user key may differ: my profile (and its copy, and the stored
/// link's name), the copies of per-person lists, the rooms' long poll, and who each account
/// is, all read again.
async fn identity_changed() {
    let c = state::ctx();
    for key in [
        state::LEAVING,
        state::REQUESTS_MINE,
        state::REQUESTS_ALL,
        state::REQUESTS_EVERYONE,
        state::TOGETHER,
    ] {
        state::remove(key);
    }
    if let Some(k) = c.keep_client()
        && let Ok(m) = k.me_with_identities().await
    {
        // An unlinked current sign-in is a fresh account: its welcome step shows.
        c.welcome.set(!m.me.profile_set);
        c.set_me(m.me);
    }
    c.invites.stop();
    c.invites.run();
    c.links.revision.update(|r| *r += 1);
    refresh(false).await;
}

// ---------------------------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------------------------

fn person(s: &Side) -> impl IntoView + use<> {
    view! {
        <div class="person-row">
            <Avatar user_key=s.me.user_key.clone() name=s.me.name.clone() avatar=s.me.avatar.clone() size=44 />
            <div>
                <strong class="pname">{s.me.name.clone()}</strong>
                <div class="muted small">{format!("{} on {} ({})", s.me.media_name, s.server_name, kind_name(s.kind))}</div>
            </div>
        </div>
    }
}

/// "Are these both you?": both people (names, servers, avatars), Link focused first; Not now
/// (the one-time prompt, remembered for the pair) or Cancel (picked in Settings); Escape too.
#[component]
pub fn LinkDialog(account: String, prompted: bool) -> impl IntoView {
    let c = state::ctx();
    let links = c.links;
    let a = links.me.get_untracked();
    let b = links
        .others
        .with_untracked(|o| o.iter().find(|s| s.account == account).cloned());
    let (Some(a), Some(b)) = (a, b) else {
        links.dialog.set(None);
        return ().into_any();
    };
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let first = NodeRef::<leptos::html::Button>::new();
    Effect::new(move |_| {
        if let Some(b) = first.get() {
            let _ = b.focus();
        }
    });
    let (a2, b2) = (a.clone(), b.clone());
    let close = move || {
        if busy.get_untracked() {
            return;
        }
        if prompted {
            let mut list = not_now();
            remember(&mut list, &a2, &b2);
            state::save(NOT_NOW, &list);
        }
        links.dialog.set(None);
    };
    let close2 = close.clone();
    let close3 = close.clone();
    let b3 = b.clone();
    let link = move || {
        if busy.get_untracked() {
            return;
        }
        let c = state::ctx();
        let b = b3.clone();
        let session = c.session(&b.account);
        let (Some(k), Some(token)) = (
            c.keep_client(),
            session.as_ref().and_then(|s| c.keep_token(s)),
        ) else {
            error.set(Some(format!(
                "Sign in again with your {} account, then link.",
                b.server_name
            )));
            return;
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match k.link(&b.server_id, &token).await {
                Ok(list) => {
                    links.me.update(|m| {
                        if let Some(m) = m {
                            m.identities = list;
                        }
                    });
                    busy.try_set(false);
                    links.dialog.set(None);
                    identity_changed().await;
                }
                Err(e) => {
                    let served = if e == Error::Unauthorized {
                        KeepClient::servers(&c.http, &Ctx::origin())
                            .await
                            .ok()
                            .map(|l| l.iter().any(|s| s.id == b.server_id))
                    } else {
                        None
                    };
                    error.try_set(Some(link_error(&e, &b.server_name, served)));
                    busy.try_set(false);
                }
            }
        });
    };
    view! {
        // A click beside the one-time prompt is no answer ("Not now" is remembered).
        <div class="dialog-back" on:click=move |_| if !prompted { close() }
            on:keydown=move |ev| if ev.key() == "Escape" { close2() }>
            <div class="dialog link-dialog" role="alertdialog" aria-modal="true" aria-labelledby="link-title"
                aria-describedby="link-what" on:click=|ev| ev.stop_propagation()>
                <h2 id="link-title">"Are these both you?"</h2>
                <div class="link-pair">{person(&a)}{person(&b)}</div>
                <p id="link-what">"Use one AlwyWatch account for both: the same name, picture, keep choices and watch-together rooms whichever you sign in with. Only link sign-ins that are really yours."</p>
                <p class="muted small">"This can't be undone as such: unlinking later gives that sign-in a fresh, empty account."</p>
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                <div class="actions">
                    <button disabled=move || busy.get() on:click=move |_| close3()>
                        {if prompted { "Not now" } else { "Cancel" }}
                    </button>
                    <button class="primary" node_ref=first disabled=move || busy.get() on:click=move |_| link()>
                        {move || if busy.get() { "Linking…" } else { "Link" }}
                    </button>
                </div>
            </div>
        </div>
    }
    .into_any()
}

/// "Also signed in as …" under "Signed in as" (nothing when nothing is linked).
#[component]
pub fn AlsoSignedIn() -> impl IntoView {
    let links = state::ctx().links;
    move || {
        links
            .me
            .with(|m| {
                m.as_ref()
                    .map(|m| also_signed_in(&m.identities, &m.server_id))
            })
            .filter(|t| !t.is_empty())
            .map(|t| view! { <p class="muted small">{t}</p> })
    }
}

/// Settings → AlwyWatch account → Linked sign-ins: shown with two sign-ins or more, or when
/// another account of this browser can be linked.
#[component]
pub fn LinkedSignins() -> impl IntoView {
    let c = state::ctx();
    let links = c.links;
    spawn_local(refresh(false));
    let unlinking = RwSignal::new(None::<LinkedIdentity>);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let ids = move || {
        links
            .me
            .with(|m| m.as_ref().map(|m| m.identities.clone()))
            .unwrap_or_default()
    };
    let candidates = move || {
        links.me.with(|a| match a {
            Some(a) => links.others.with(|o| {
                o.iter()
                    .filter(|b| linkable(a, b))
                    .cloned()
                    .collect::<Vec<_>>()
            }),
            None => Vec::new(),
        })
    };
    let unlink = move || {
        let Some(i) = unlinking.get_untracked() else {
            return;
        };
        let Some(k) = state::ctx().keep_client() else {
            return;
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match k.unlink(&i.user_key).await {
                Ok(list) => {
                    links.me.update(|m| {
                        if let Some(m) = m {
                            m.identities = list;
                        }
                    });
                    busy.try_set(false);
                    unlinking.try_set(None);
                    identity_changed().await;
                }
                Err(e) => {
                    error.try_set(Some(unlink_error(&e)));
                    busy.try_set(false);
                }
            }
        });
    };
    let cancel = NodeRef::<leptos::html::Button>::new();
    Effect::new(move |_| {
        if let Some(b) = cancel.get() {
            let _ = b.focus();
        }
    });
    let shown = move || ids().len() > 1 || !candidates().is_empty();
    view! {
        <Show when=shown>
            <div class="links">
                <h3>"Linked sign-ins"</h3>
                <p class="muted small">{move || if ids().len() > 1 {
                    "These sign-ins are one AlwyWatch account: same name, avatar, keep choices and watch-together rooms."
                } else {
                    "Another account in this browser may be you too. Linking makes them one AlwyWatch account."
                }}</p>
                {move || (ids().len() > 1).then(|| ids().into_iter().map(|i| {
                    let detail = format!("{} · {}", i.server_name, type_name(i.kind));
                    let (main, here) = (i.canonical, i.current);
                    let name = i.media_name.clone();
                    view! {
                        <div class="row link-row-item">
                            <div>
                                <strong>{name}</strong>" "
                                {main.then(|| view! { <span class="tag main">"Main"</span> })}
                                {here.then(|| view! { <span class="tag">"This browser"</span> })}
                                <div class="muted small">{detail}</div>
                            </div>
                            {(!main).then(|| {
                                let i = i.clone();
                                view! {
                                    <button class="small" on:click=move |_| { error.set(None); unlinking.set(Some(i.clone())) }>"Unlink"</button>
                                }
                            })}
                        </div>
                    }
                }).collect_view())}
                {move || {
                    let list = candidates();
                    (!list.is_empty()).then(|| view! {
                        <h3 class="small-head">"Link another sign-in"</h3>
                        {list.into_iter().map(|b| {
                            let acc = b.account.clone();
                            view! {
                                <div class="row">
                                    <div><strong>{b.me.media_name.clone()}</strong>" on "{b.server_name.clone()}
                                        <span class="muted">{format!(" · {}", kind_name(b.kind))}</span></div>
                                    <button class="small" on:click=move |_| links.dialog.set(Some((acc.clone(), false)))>"Link…"</button>
                                </div>
                            }
                        }).collect_view()}
                    })
                }}
                <p class="muted small">"Another sign-in of yours isn't listed? Add it with “Add an account” above, then link it here."</p>
            </div>
        </Show>
        {move || unlinking.get().map(|i| {
            let question = unlink_question(&i);
            let close = move || if !busy.get_untracked() { unlinking.set(None) };
            view! {
                <div class="dialog-back" on:click=move |_| close()
                    on:keydown=move |ev| if ev.key() == "Escape" { close() }>
                    <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="unlink-title" on:click=|ev| ev.stop_propagation()>
                        <h2 id="unlink-title">"Unlink this sign-in?"</h2>
                        <p>{question}</p>
                        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                        <div class="actions">
                            <button node_ref=cancel disabled=move || busy.get() on:click=move |_| close()>"Cancel"</button>
                            <button class="danger" disabled=move || busy.get() on:click=move |_| unlink()>
                                {move || if busy.get() { "Unlinking…" } else { "Unlink" }}
                            </button>
                        </div>
                    </div>
                </div>
            }
        })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keep_service::{Avatar as AvatarSpec, Rights};

    fn side(account: &str, server: &str, key: &str) -> Side {
        Side {
            account: account.into(),
            server_id: server.into(),
            server_name: server.to_uppercase(),
            kind: ServerKind::Jellyfin,
            me: Me {
                user_key: key.into(),
                name: "N".into(),
                media_name: "n".into(),
                name_is_custom: false,
                avatar: AvatarSpec {
                    preset: None,
                    custom_version: None,
                },
                profile_set: true,
                server_id: server.into(),
                is_admin: false,
                rights: Rights {
                    can_keep_forever: true,
                    max_forever: None,
                    max_keep_days: None,
                },
                forever_count: 0,
            },
            identities: vec![],
        }
    }

    fn id(key: &str, server: &str, canonical: bool, current: bool) -> LinkedIdentity {
        LinkedIdentity {
            user_key: key.into(),
            server_id: server.into(),
            server_name: server.to_uppercase(),
            kind: ServerType::Plex,
            media_name: format!("m-{key}"),
            canonical,
            current,
        }
    }

    #[test]
    fn offers_two_people_on_two_servers_only() {
        let a = side("a", "px", "plex:px:1");
        let b = side("b", "jf", "jellyfin:jf:2");
        assert_eq!(to_offer(&a, std::slice::from_ref(&b), &[]), Some(0));
        assert_eq!(to_offer(&a, &[side("c", "px", "plex:px:9")], &[]), None);
        let mut merged = b.clone();
        merged.me.user_key = "plex:px:1".into();
        assert_eq!(to_offer(&a, &[merged], &[]), None);
        let mut a2 = a.clone();
        a2.identities = vec![
            id("plex:px:1", "px", true, true),
            id("jellyfin:jf:2", "jf", false, false),
        ];
        assert_eq!(to_offer(&a2, std::slice::from_ref(&b), &[]), None);
        let mut a3 = a.clone();
        a3.identities = vec![
            id("plex:px:1", "px", true, true),
            id("jellyfin:jf:5", "jf", false, false),
        ];
        assert_eq!(to_offer(&a3, std::slice::from_ref(&b), &[]), None);
        let c = side("c", "jf2", "jellyfin:jf2:3");
        assert_eq!(
            to_offer(&a, &[side("d", "px", "plex:px:4"), c], &[]),
            Some(1)
        );
    }

    #[test]
    fn not_now_is_remembered_per_pair_until_a_sign_out() {
        let a = side("a", "px", "plex:px:1");
        let b = side("b", "jf", "jellyfin:jf:2");
        let c = side("c", "jf2", "jellyfin:jf2:3");
        let mut list = vec![];
        remember(&mut list, &a, &b);
        remember(&mut list, &b, &a);
        assert_eq!(list.len(), 1);
        assert_eq!(to_offer(&a, &[b.clone(), c.clone()], &list), Some(1));
        forget_account(&mut list, "c");
        assert_eq!(list.len(), 1);
        forget_account(&mut list, "b");
        assert!(list.is_empty());
        assert_eq!(to_offer(&a, &[b], &list), Some(0));
        let mut l2 = vec![];
        remember(&mut l2, &a, &c);
        let back: Vec<NotNow> = serde_json::from_str(&serde_json::to_string(&l2).unwrap()).unwrap();
        assert_eq!(back, l2);
    }

    #[test]
    fn labels_and_errors_read_plainly() {
        let ids = vec![
            id("plex:px:1", "px", true, true),
            id("jellyfin:jf:2", "jf", false, false),
        ];
        assert_eq!(
            also_signed_in(&ids, "px"),
            "Also signed in as m-jellyfin:jf:2 (JF)"
        );
        let unmarked: Vec<_> = ids
            .iter()
            .cloned()
            .map(|mut i| {
                i.current = false;
                i
            })
            .collect();
        assert_eq!(
            also_signed_in(&unmarked, "jf"),
            "Also signed in as m-plex:px:1 (PX)"
        );
        assert_eq!(also_signed_in(&ids[..1], "px"), "");
        assert!(
            unlink_question(&ids[1]).starts_with("m-jellyfin:jf:2 on JF becomes a separate, new")
        );
        let st = |s| Error::Status { status: s };
        assert!(
            link_error(&st(409), "Fin", None)
                .starts_with("These are two different people on the same server")
        );
        assert!(
            link_error(&Error::Unauthorized, "Fin", Some(true))
                .contains("sign in again with that account")
        );
        assert!(link_error(&Error::Unauthorized, "Fin", Some(false)).contains("doesn't serve Fin"));
        assert!(link_error(&st(429), "Fin", None).contains("Too many tries"));
        assert!(link_error(&st(502), "Fin", None).contains("unreachable"));
        assert!(
            link_error(
                &Error::Network {
                    message: "x".into()
                },
                "Fin",
                None
            )
            .contains("Can't reach")
        );
        assert_eq!(
            unlink_error(&st(409)),
            "The main sign-in can't be unlinked."
        );
    }
}
