// SPDX-License-Identifier: AGPL-3.0-or-later
//! One AlwyWatch account for several sign-ins (docs/identity-apps.md): the one-time "Are these
//! both you?" prompt, Settings → AlwyWatch account → Linked sign-ins (list, Unlink, Link another
//! sign-in), and everything refreshed after a link or an unlink.
//!
//! The synced account (A) talks to the AlwyWatch server through the core. Each other account of
//! this profile is asked for its own `Me` with its own token (read only), to know whether it is
//! on the AlwyWatch server and whether it already is the same person.

use keep_service::identity::MeWithIdentities;

use super::*;
use crate::{LinkRow, PersonRow};

/// "Not now" answers, kept per profile.
const NOT_NOW_KEY: &str = "keep.links.notnow";
/// The prompt waits for a moment without a video or another dialog.
const RETRY: Duration = Duration::from_secs(4);

/// One account of this profile as the AlwyWatch server sees it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Side {
    pub account_id: String,
    pub server_id: String,
    /// The media server's name (as signed in).
    pub server_name: String,
    pub kind: ServerKind,
    pub me: Me,
    /// Every sign-in of its AlwyWatch account (empty: an older server, or nothing linked).
    pub identities: Vec<LinkedIdentity>,
}

impl Side {
    /// Every user key this person is known by.
    fn keys(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.me.user_key.as_str())
            .chain(self.identities.iter().map(|i| i.user_key.as_str()))
    }

    /// Every server this person already has a sign-in on.
    fn servers(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.server_id.as_str())
            .chain(self.identities.iter().map(|i| i.server_id.as_str()))
    }
}

/// A "Not now": the two accounts (forgotten when one signs out) and the two people.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NotNow {
    pub accounts: [String; 2],
    pub users: [String; 2],
}

/// Kept as `[[account, account, user, user], …]`.
pub(super) fn encode_not_now(list: &[NotNow]) -> String {
    let rows: Vec<[&str; 4]> = list
        .iter()
        .map(|n| [&*n.accounts[0], &*n.accounts[1], &*n.users[0], &*n.users[1]])
        .collect();
    serde_json::to_string(&rows).unwrap_or_default()
}

pub(super) fn decode_not_now(s: &str) -> Vec<NotNow> {
    serde_json::from_str::<Vec<[String; 4]>>(s)
        .unwrap_or_default()
        .into_iter()
        .map(|[a, b, x, y]| NotNow {
            accounts: [a, b],
            users: [x, y],
        })
        .collect()
}

/// Whether `b` can join `a`'s AlwyWatch account: not the same person already, and no server
/// in common (two users of one server are never one person: the server refuses).
pub(super) fn linkable(a: &Side, b: &Side) -> bool {
    !a.keys().any(|k| b.keys().any(|l| l == k)) && !a.servers().any(|s| b.servers().any(|t| t == s))
}

fn dismissed(a: &Side, b: &Side, not_now: &[NotNow]) -> bool {
    not_now.iter().any(|n| {
        let (x, y) = (n.users[0].as_str(), n.users[1].as_str());
        (x == a.me.user_key && y == b.me.user_key) || (y == a.me.user_key && x == b.me.user_key)
    })
}

/// The account to offer in "Are these both you?" (index in `others`), if any.
pub(super) fn to_offer(a: &Side, others: &[Side], not_now: &[NotNow]) -> Option<usize> {
    others
        .iter()
        .position(|b| linkable(a, b) && !dismissed(a, b, not_now))
}

/// "Not now" for this pair (once).
pub(super) fn remember(not_now: &mut Vec<NotNow>, a: &Side, b: &Side) {
    if dismissed(a, b, not_now) {
        return;
    }
    not_now.push(NotNow {
        accounts: [a.account_id.clone(), b.account_id.clone()],
        users: [a.me.user_key.clone(), b.me.user_key.clone()],
    });
}

/// An account signed out: its pairs may be asked again once it is back.
pub(super) fn forget_account(not_now: &mut Vec<NotNow>, account_id: &str) {
    not_now.retain(|n| !n.accounts.iter().any(|a| a == account_id));
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

/// "Home · Plex".
pub(super) fn identity_detail(i: &LinkedIdentity) -> String {
    format!("{} · {}", i.server_name, type_name(i.kind))
}

/// "alex (Home)": a sign-in in a sentence.
pub(super) fn identity_short(i: &LinkedIdentity) -> String {
    format!("{} ({})", i.media_name, i.server_name)
}

/// "Also signed in as …" under my name: the other sign-ins of my account (empty: none).
/// `server_id`: the synced account's server, for servers that don't mark the current one.
pub(super) fn also_signed_in(ids: &[LinkedIdentity], server_id: &str) -> String {
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
        .map(identity_short)
        .collect();
    if others.is_empty() {
        String::new()
    } else {
        format!("Also signed in as {}", others.join(", "))
    }
}

/// The unlink confirmation.
pub(super) fn unlink_question(i: &LinkedIdentity) -> String {
    format!(
        "{} on {} becomes a separate, new AlwyWatch account with no keep choices, name, avatar \
         or history. Continue?",
        i.media_name, i.server_name
    )
}

/// A failed link in plain words. `server`: the other sign-in's server; `served`: whether the
/// AlwyWatch server still serves it (the core reports 401 and 403 alike).
pub(super) fn link_error(e: &CoreError, server: &str, served: Option<bool>) -> String {
    match e {
        CoreError::Unauthorized if served == Some(false) => {
            format!(
                "Your AlwyWatch server doesn't serve {server}, so this sign-in can't be linked."
            )
        }
        CoreError::Unauthorized => {
            format!(
                "Your {server} sign-in has expired: sign in again with that account, then link."
            )
        }
        CoreError::NotSignedIn { .. } => {
            format!("Sign in again with your {server} account, then link.")
        }
        CoreError::Server { status: 409 } => format!(
            "These are two different people on the same server: your AlwyWatch account already \
             has another sign-in on {server}."
        ),
        other => common_error(other),
    }
}

pub(super) fn unlink_error(e: &CoreError) -> String {
    match e {
        CoreError::Server { status: 409 } => "The main sign-in can't be unlinked.".into(),
        CoreError::Unauthorized => {
            "Your AlwyWatch server refused: sign in again with this account.".into()
        }
        other => common_error(other),
    }
}

fn common_error(e: &CoreError) -> String {
    match e {
        CoreError::Server { status: 429 } => {
            "Too many tries: wait a few minutes, then try again.".into()
        }
        CoreError::Server { status: 502 } => "The media server is unreachable right now.".into(),
        CoreError::Server { status } if *status >= 500 => {
            "Your AlwyWatch server had a problem: try again in a moment.".into()
        }
        CoreError::NotFound | CoreError::Server { status: 405 } => {
            "Your AlwyWatch server doesn't support this yet: it needs an update.".into()
        }
        CoreError::Network { .. } => "Can't reach your AlwyWatch server right now.".into(),
        e => e.to_string(),
    }
}

/// What the dialog is about.
#[derive(Clone, Debug, PartialEq)]
enum Asking {
    /// Link this account (id); `prompted`: the one-time prompt (else picked in Settings).
    Link {
        account: String,
        prompted: bool,
    },
    Unlink(LinkedIdentity),
}

#[derive(Default)]
pub(super) struct LinksState {
    /// The synced account, and the other accounts of this profile on the AlwyWatch server.
    me: Option<Side>,
    others: Vec<Side>,
    /// My sign-ins as `links()` last returned them.
    links: Vec<LinkedIdentity>,
    asking: Option<Asking>,
    /// The prompt waits (a video, another dialog): bumped to stop waiting.
    retry_gen: u64,
    /// Bumped by each check: older answers are dropped.
    check_gen: u64,
}

impl App {
    fn not_now(&self) -> Vec<NotNow> {
        self.core
            .get_setting(NOT_NOW_KEY.into())
            .ok()
            .flatten()
            .map(|s| decode_not_now(&s))
            .unwrap_or_default()
    }

    fn save_not_now(&self, list: &[NotNow]) {
        let _ = self
            .core
            .set_setting(NOT_NOW_KEY.into(), encode_not_now(list));
    }

    /// An account signed out of this profile: its "Not now" answers go.
    pub(super) fn links_account_removed(&self, account_id: &str) {
        let mut list = self.not_now();
        forget_account(&mut list, account_id);
        self.save_not_now(&list);
        let mut st = self.state.borrow_mut();
        st.links.others.retain(|s| s.account_id != account_id);
    }

    /// Asks the AlwyWatch server who each account of this profile is, then renders Linked
    /// sign-ins and (`prompt`) offers the pair once.
    pub(super) fn check_links(self: &Rc<Self>, prompt: bool) {
        let Some(k) = self.core.keep_connection() else {
            self.links_reset();
            return;
        };
        if !self.core.keep_active() {
            return;
        }
        let accounts = self.core.accounts().unwrap_or_default();
        let Some(mine) = accounts.iter().find(|a| a.id == k.account_id).cloned() else {
            return;
        };
        let others: Vec<(Account, String)> = accounts
            .into_iter()
            .filter(|a| a.id != k.account_id)
            .filter_map(|a| self.keep_token(&a.id).map(|t| (a, t)))
            .collect();
        let generation = {
            let mut st = self.state.borrow_mut();
            st.links.check_gen += 1;
            st.links.check_gen
        };
        let (core, this, base) = (self.core.clone(), Rc::downgrade(self), k.base_url);
        slint::spawn_local(async move {
            let me = core.keep_me().await;
            let links = core.keep_links().await;
            // The other accounts with their own tokens (never shown, never logged).
            let job = core.runtime().spawn(async move {
                let Some(http) = account_http() else {
                    return vec![];
                };
                let mut found = vec![];
                for (a, token) in others {
                    let client =
                        keep_service::KeepClient::new(http.clone(), &base, &a.server_id, &token);
                    if let Ok(m) = client.me_with_identities().await {
                        found.push((a, m));
                    }
                }
                found
            });
            let found = job.await.unwrap_or_default();
            let Some(t) = this.upgrade() else { return };
            if t.state.borrow().links.check_gen != generation {
                return;
            }
            let Ok(me) = me else { return };
            let links = links.unwrap_or_default();
            let side = |a: &Account, me: Me, identities: Vec<LinkedIdentity>| Side {
                account_id: a.id.clone(),
                server_id: a.server_id.clone(),
                server_name: a.server_name.clone(),
                kind: a.kind,
                me,
                identities,
            };
            {
                let mut st = t.state.borrow_mut();
                st.links.me = Some(side(&mine, me, links.clone()));
                st.links.others = found
                    .into_iter()
                    .map(|(a, m): (Account, MeWithIdentities)| side(&a, m.me, m.identities))
                    .collect();
                st.links.links = links;
            }
            t.render_links();
            if prompt {
                t.offer_link();
            }
        })
        .ok();
    }

    /// No AlwyWatch server any more: nothing to list or offer.
    pub(super) fn links_reset(&self) {
        {
            let mut st = self.state.borrow_mut();
            st.links = LinksState {
                check_gen: st.links.check_gen + 1,
                retry_gen: st.links.retry_gen + 1,
                ..Default::default()
            };
        }
        self.render_links();
        self.ui().set_link_dialog(SharedString::new());
    }

    /// Settings → Linked sign-ins, and "Also signed in as …".
    pub(super) fn render_links(&self) {
        let ui = self.ui();
        let st = self.state.borrow();
        let l = &st.links;
        let server_id = l.me.as_ref().map(|m| m.server_id.as_str()).unwrap_or("");
        ui.set_me_also(sstr(also_signed_in(&l.links, server_id)));
        let rows: Vec<LinkRow> = if l.links.len() > 1 {
            l.links
                .iter()
                .map(|i| LinkRow {
                    key: sstr(&i.user_key),
                    title: sstr(&i.media_name),
                    detail: sstr(identity_detail(i)),
                    server: match i.kind {
                        ServerType::Plex => "plex".into(),
                        ServerType::Jellyfin => "jellyfin".into(),
                    },
                    main: i.canonical,
                    here: i.current,
                })
                .collect()
        } else {
            vec![]
        };
        ui.set_links(ModelRc::new(VecModel::from(rows)));
        let candidates: Vec<NamedId> = match &l.me {
            Some(a) => l
                .others
                .iter()
                .filter(|b| linkable(a, b))
                .map(|b| NamedId {
                    id: sstr(&b.account_id),
                    title: sstr(format!(
                        "{} · {} ({})",
                        b.me.media_name,
                        b.server_name,
                        kind_name(b.kind)
                    )),
                    selected: false,
                })
                .collect(),
            None => vec![],
        };
        ui.set_link_candidates(ModelRc::new(VecModel::from(candidates)));
    }

    /// The one-time prompt, once nothing else is on screen.
    fn offer_link(self: &Rc<Self>) {
        let target = {
            let st = self.state.borrow();
            let Some(a) = st.links.me.as_ref() else {
                return;
            };
            to_offer(a, &st.links.others, &self.not_now())
                .map(|i| st.links.others[i].account_id.clone())
        };
        let Some(account) = target else { return };
        if self.busy_screen() {
            let generation = {
                let mut st = self.state.borrow_mut();
                st.links.retry_gen += 1;
                st.links.retry_gen
            };
            let w = Rc::downgrade(self);
            slint::Timer::single_shot(RETRY, move || {
                if let Some(t) = w.upgrade()
                    && t.state.borrow().links.retry_gen == generation
                {
                    t.offer_link();
                }
            });
            return;
        }
        self.open_link_dialog(account, true);
    }

    /// A video, a dialog or "Who's watching?" is up: the prompt never covers them.
    fn busy_screen(&self) -> bool {
        let ui = self.ui();
        self.state.borrow().player_notes.is_some()
            // The welcome step comes first.
            || self.me_profile_set() == Some(false)
            || !ui.get_account_dialog().is_empty()
            || !ui.get_link_dialog().is_empty()
            || !ui.get_profile_dialog().is_empty()
            || !ui.get_mark_confirm().is_empty()
            || ui.get_profile_gate()
            || ui.get_crop_open()
            || ui.get_room_picker()
    }

    fn person_of(&self, s: &Side) -> PersonRow {
        PersonRow {
            key: sstr(&s.me.user_key),
            name: sstr(&s.me.name),
            detail: sstr(format!(
                "{} on {} ({})",
                s.me.media_name,
                s.server_name,
                kind_name(s.kind)
            )),
            avatar: self.person_avatar(&s.me.user_key, &s.me.name, &s.me.avatar),
            header: SharedString::new(),
            selected: false,
        }
    }

    fn open_link_dialog(self: &Rc<Self>, account: String, prompted: bool) {
        let (a, b) = {
            let st = self.state.borrow();
            let b = st.links.others.iter().find(|s| s.account_id == account);
            match (st.links.me.clone(), b.cloned()) {
                (Some(a), Some(b)) => (a, b),
                _ => return,
            }
        };
        let ui = self.ui();
        ui.set_link_a(self.person_of(&a));
        ui.set_link_b(self.person_of(&b));
        ui.set_link_prompted(prompted);
        ui.set_link_busy(false);
        ui.set_link_message(SharedString::new());
        self.state.borrow_mut().links.asking = Some(Asking::Link { account, prompted });
        ui.set_link_dialog("ask".into());
    }

    /// "Link another sign-in" → that account.
    pub(super) fn link_ask(self: &Rc<Self>, account: String) {
        self.open_link_dialog(account, false);
    }

    /// "Not now" (remembered for the pair) or Cancel (picked in Settings); Escape too.
    pub(super) fn link_dialog_close(self: &Rc<Self>) {
        let ui = self.ui();
        if ui.get_link_busy() {
            return;
        }
        let asking = self.state.borrow_mut().links.asking.take();
        if let Some(Asking::Link {
            account,
            prompted: true,
        }) = asking
        {
            let pair = {
                let st = self.state.borrow();
                st.links.me.clone().zip(
                    st.links
                        .others
                        .iter()
                        .find(|s| s.account_id == account)
                        .cloned(),
                )
            };
            if let Some((a, b)) = pair {
                let mut list = self.not_now();
                remember(&mut list, &a, &b);
                self.save_not_now(&list);
            }
        }
        ui.set_link_dialog(SharedString::new());
        // Another pair may be waiting (three accounts).
        self.offer_link_later();
    }

    fn offer_link_later(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        slint::Timer::single_shot(Duration::from_millis(300), move || {
            if let Some(t) = w.upgrade() {
                t.offer_link();
            }
        });
    }

    /// Link: the other account's token proves it is the same person (sent as the synced one).
    pub(super) fn link_confirm(self: &Rc<Self>) {
        let ui = self.ui();
        if ui.get_link_busy() {
            return;
        }
        let Some(Asking::Link { account, .. }) = self.state.borrow().links.asking.clone() else {
            return;
        };
        let Some(b) = self
            .state
            .borrow()
            .links
            .others
            .iter()
            .find(|s| s.account_id == account)
            .cloned()
        else {
            return;
        };
        let Some(token) = self.keep_token(&account) else {
            ui.set_link_message(sstr(link_error(
                &CoreError::NotSignedIn {
                    account_id: String::new(),
                },
                &b.server_name,
                None,
            )));
            return;
        };
        ui.set_link_busy(true);
        ui.set_link_message(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_link(account, token).await;
            let message = match &r {
                Err(e @ CoreError::Unauthorized) => {
                    // 401 and 403 look alike here: is that server still served?
                    let served = match core.keep_connection() {
                        Some(k) => core
                            .keep_servers(k.base_url)
                            .await
                            .ok()
                            .map(|l| l.iter().any(|s| s.id == b.server_id)),
                        None => None,
                    };
                    Some(link_error(e, &b.server_name, served))
                }
                Err(e) => Some(link_error(e, &b.server_name, None)),
                Ok(_) => None,
            };
            let Some(t) = this.upgrade() else { return };
            t.ui().set_link_busy(false);
            match (r, message) {
                (Ok(list), _) => {
                    t.state.borrow_mut().links.links = list;
                    t.state.borrow_mut().links.asking = None;
                    t.ui().set_link_dialog(SharedString::new());
                    t.render_links();
                    t.identity_changed();
                }
                (Err(_), Some(m)) => t.ui().set_link_message(sstr(m)),
                (Err(e), None) => t.ui().set_link_message(sstr(common_error(&e))),
            }
        })
        .ok();
    }

    /// Unlink (never offered for the main sign-in): asked first.
    pub(super) fn unlink_ask(self: &Rc<Self>, user_key: String) {
        let Some(i) = self
            .state
            .borrow()
            .links
            .links
            .iter()
            .find(|i| i.user_key == user_key)
            .cloned()
        else {
            return;
        };
        let ui = self.ui();
        ui.set_unlink_question(sstr(unlink_question(&i)));
        ui.set_link_busy(false);
        ui.set_link_message(SharedString::new());
        self.state.borrow_mut().links.asking = Some(Asking::Unlink(i));
        ui.set_link_dialog("unlink".into());
    }

    pub(super) fn unlink_confirm(self: &Rc<Self>) {
        let ui = self.ui();
        if ui.get_link_busy() {
            return;
        }
        let Some(Asking::Unlink(i)) = self.state.borrow().links.asking.clone() else {
            return;
        };
        ui.set_link_busy(true);
        ui.set_link_message(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_unlink(i.user_key.clone()).await;
            let Some(t) = this.upgrade() else { return };
            t.ui().set_link_busy(false);
            match r {
                Ok(list) => {
                    t.state.borrow_mut().links.links = list;
                    t.state.borrow_mut().links.asking = None;
                    t.ui().set_link_dialog(SharedString::new());
                    t.render_links();
                    t.identity_changed();
                }
                Err(e) => t.ui().set_link_message(sstr(unlink_error(&e))),
            }
        })
        .ok();
    }

    /// After a link or an unlink my user key may differ: the connection's name, my profile
    /// (and its cache), the leaving-soon list, the rooms' long poll and people list, Seerr
    /// through the server, and who each account is, all read again.
    fn identity_changed(self: &Rc<Self>) {
        let Some(k) = self.core.keep_connection() else {
            return;
        };
        let token = self.keep_token(&k.account_id);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            if let Some(token) = token {
                // Same account again: refreshes the stored name and admin flag, drops the cached
                // list, and moves Seerr (through the server) to the account as it is now.
                let _ = core.keep_connect(k.base_url, k.account_id, token).await;
            }
            let Some(t) = this.upgrade() else { return };
            t.refresh_keep_settings();
            t.refresh_settings();
            t.forget_me();
            t.load_me();
            // An unlinked current sign-in is a fresh account: its welcome step shows.
            t.check_welcome();
            t.stop_rooms_poll();
            t.start_rooms_poll();
            t.refresh_seerr_settings();
            match t.ui().get_page() {
                Page::Leaving => t.load_leaving(),
                Page::Triage => t.load_triage(),
                _ => {}
            }
            t.check_links(false);
        })
        .ok();
    }
}

/// The HTTP client the other accounts' `Me` is read with (the avatars' one).
fn account_http() -> Option<net::reqwest::Client> {
    super::account::http_client()
}

/// Test hooks (headless screenshots, `ALWYWATCH_TEST_PAGE`): made-up accounts, nothing sent.
impl App {
    fn test_sides() -> (Side, Side) {
        let rights = Rights {
            can_keep_forever: true,
            max_forever: None,
            max_keep_days: None,
        };
        let me = |key: &str, name: &str, media: &str, preset: Option<&str>| Me {
            user_key: key.into(),
            name: name.into(),
            media_name: media.into(),
            name_is_custom: name != media,
            avatar: Avatar {
                preset: preset.map(Into::into),
                custom_version: None,
            },
            profile_set: true,
            server_id: String::new(),
            is_admin: false,
            rights: rights.clone(),
            forever_count: 0,
        };
        (
            Side {
                account_id: "t-a".into(),
                server_id: "px".into(),
                server_name: "Home Plex".into(),
                kind: ServerKind::Plex,
                me: me("plex:px:1", "Sam", "sam.plex", Some("p08")),
                identities: vec![],
            },
            Side {
                account_id: "t-b".into(),
                server_id: "jf".into(),
                server_name: "Home Jellyfin".into(),
                kind: ServerKind::Jellyfin,
                me: me("jellyfin:jf:7", "sam", "sam", None),
                identities: vec![],
            },
        )
    }

    fn test_identity(
        key: &str,
        server: &str,
        kind: ServerType,
        media: &str,
        canonical: bool,
        current: bool,
    ) -> LinkedIdentity {
        LinkedIdentity {
            user_key: key.into(),
            server_id: server.to_lowercase(),
            server_name: server.into(),
            kind,
            media_name: media.into(),
            canonical,
            current,
        }
    }

    /// `links` (Settings with two linked sign-ins and one to link), `links-ask` (the prompt),
    /// `links-unlink` (the confirmation).
    pub fn test_links(self: &Rc<Self>, step: &str) {
        self.test_account("");
        let (a, b) = Self::test_sides();
        {
            let mut st = self.state.borrow_mut();
            if step == "links-ask" {
                st.links.links = vec![];
            } else {
                st.links.links = vec![
                    Self::test_identity(
                        "plex:px:1",
                        "Home Plex",
                        ServerType::Plex,
                        "sam.plex",
                        true,
                        true,
                    ),
                    Self::test_identity(
                        "jellyfin:old:3",
                        "Old Jellyfin",
                        ServerType::Jellyfin,
                        "samuel",
                        false,
                        false,
                    ),
                ];
            }
            let mut a = a;
            a.identities = st.links.links.clone();
            st.links.me = Some(a);
            st.links.others = vec![b];
        }
        self.render_links();
        match step {
            "links-ask" => self.open_link_dialog("t-b".into(), true),
            "links-unlink" => self.unlink_ask("jellyfin:old:3".into()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(account: &str, server: &str, key: &str) -> Side {
        Side {
            account_id: account.into(),
            server_id: server.into(),
            server_name: server.to_uppercase(),
            kind: ServerKind::Jellyfin,
            me: Me {
                user_key: key.into(),
                name: "N".into(),
                media_name: "n".into(),
                name_is_custom: false,
                avatar: Avatar {
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
        // Same server: never one person.
        let same_server = side("c", "px", "plex:px:9");
        assert_eq!(to_offer(&a, &[same_server], &[]), None);
        // Already one account: B answers with A's key, or A lists B.
        let mut merged = b.clone();
        merged.me.user_key = "plex:px:1".into();
        assert_eq!(to_offer(&a, &[merged], &[]), None);
        let mut a2 = a.clone();
        a2.identities = vec![
            id("plex:px:1", "px", true, true),
            id("jellyfin:jf:2", "jf", false, false),
        ];
        assert_eq!(to_offer(&a2, std::slice::from_ref(&b), &[]), None);
        // A's account already has a sign-in on B's server (someone else): the server says 409.
        let mut a3 = a.clone();
        a3.identities = vec![
            id("plex:px:1", "px", true, true),
            id("jellyfin:jf:5", "jf", false, false),
        ];
        assert_eq!(to_offer(&a3, std::slice::from_ref(&b), &[]), None);
        // The first linkable one of several.
        let c = side("c", "jf2", "jellyfin:jf2:3");
        let px2 = side("d", "px", "plex:px:4");
        assert_eq!(to_offer(&a, &[px2, c], &[]), Some(1));
    }

    #[test]
    fn not_now_is_remembered_per_pair_until_a_sign_out() {
        let a = side("a", "px", "plex:px:1");
        let b = side("b", "jf", "jellyfin:jf:2");
        let c = side("c", "jf2", "jellyfin:jf2:3");
        let mut list = vec![];
        remember(&mut list, &a, &b);
        remember(&mut list, &a, &b);
        assert_eq!(list.len(), 1);
        assert_eq!(to_offer(&a, &[b.clone(), c.clone()], &list), Some(1));
        // Either order of the pair.
        assert!(dismissed(&b, &a, &list));
        // Stays when another account signs out, goes when one of the pair does.
        forget_account(&mut list, "c");
        assert_eq!(list.len(), 1);
        forget_account(&mut list, "b");
        assert!(list.is_empty());
        assert_eq!(to_offer(&a, &[b], &list), Some(0));
        // Survives a restart (kept as JSON).
        let mut l2 = vec![];
        remember(&mut l2, &a, &c);
        let back = decode_not_now(&encode_not_now(&l2));
        assert_eq!(back, l2);
    }

    #[test]
    fn labels_read_plainly() {
        let ids = vec![
            id("plex:px:1", "px", true, true),
            id("jellyfin:jf:2", "jf", false, false),
        ];
        assert_eq!(
            also_signed_in(&ids, "px"),
            "Also signed in as m-jellyfin:jf:2 (JF)"
        );
        // An older server marks no current sign-in: the synced server's is left out.
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
        assert_eq!(identity_detail(&ids[0]), "PX · Plex");
        assert!(
            unlink_question(&ids[1]).starts_with("m-jellyfin:jf:2 on JF becomes a separate, new")
        );
    }

    #[test]
    fn errors_read_plainly() {
        let e = |s| CoreError::Server { status: s };
        assert!(
            link_error(&e(409), "Fin", None)
                .starts_with("These are two different people on the same server")
        );
        assert!(
            link_error(&CoreError::Unauthorized, "Fin", Some(true))
                .contains("sign in again with that account")
        );
        assert!(link_error(&CoreError::Unauthorized, "Fin", None).contains("sign in again"));
        assert!(
            link_error(&CoreError::Unauthorized, "Fin", Some(false)).contains("doesn't serve Fin")
        );
        assert!(link_error(&e(429), "Fin", None).contains("Too many tries"));
        assert!(
            link_error(&CoreError::Network { detail: "x".into() }, "Fin", None)
                .contains("Can't reach")
        );
        assert!(link_error(&e(502), "Fin", None).contains("unreachable"));
        assert_eq!(unlink_error(&e(409)), "The main sign-in can't be unlinked.");
        assert!(unlink_error(&e(429)).contains("Too many tries"));
    }
}
