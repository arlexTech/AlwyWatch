// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings → Admin: inviting someone (by email or with a link, with a Jellyfin login if
//! wanted), the invitations made, the web app's Server settings; and, on the "Create an
//! AlwyWatch account" step, what an invitation code gives.

use super::*;
use crate::InviteRow;

/// A Jellyfin server an invitation can make a login on.
struct Target {
    id: String,
    name: String,
    /// (id, name) of the users whose access a new login can copy.
    users: Vec<(String, String)>,
    last_like: Option<String>,
}

/// An invitation in the list.
struct Made {
    id: i64,
    created_by: String,
    expires: i64,
    uses_left: i64,
    email: Option<String>,
    jellyfin: Option<String>,
    used_by: Vec<String>,
}

#[derive(Default)]
pub(super) struct InviteState {
    mail: bool,
    targets: Vec<Target>,
    target: usize,
    /// Whose access the new Jellyfin login copies: `None` = not chosen (the last one used,
    /// else the first user), `Some("")` = Jellyfin's defaults.
    like: Option<String>,
    made: Vec<Made>,
    busy: bool,
    /// Bumped on every keystroke in the sign-up invitation field (debounce).
    check_gen: u64,
}

impl InviteState {
    /// The user whose access is copied ("" = Jellyfin's defaults).
    fn like_shown(&self) -> String {
        let Some(t) = self.targets.get(self.target) else {
            return String::new();
        };
        if let Some(l) = &self.like {
            return l.clone();
        }
        t.last_like
            .clone()
            .filter(|l| t.users.iter().any(|(id, _)| id == l))
            .or_else(|| t.users.first().map(|(id, _)| id.clone()))
            .unwrap_or_default()
    }
}

/// The code in what was typed or pasted: the code itself or the invitation link
/// (`https://watch.example.org/join/ABCD`).
pub(super) fn invite_code(typed: &str) -> String {
    let t = typed.trim();
    match t.rfind("/join/") {
        Some(i) => t[i + 6..]
            .split(['?', '#', '/'])
            .next()
            .unwrap_or_default()
            .to_owned(),
        None => t.to_owned(),
    }
}

/// "Ends in 6 days · with a login on HomeJelly · by alex · used by sam".
fn detail(m: &Made, now: i64) -> String {
    let active = m.uses_left > 0 && m.expires > now;
    let mut parts = vec![];
    if active {
        parts.push(tr!("Ends in {time}", time = elapsed(m.expires - now)));
    } else if m.used_by.is_empty() {
        parts.push(tr!("Ended"));
    }
    if let Some(j) = &m.jellyfin {
        parts.push(tr!("with a login on {server}", server = j));
    }
    parts.push(tr!("by {name}", name = m.created_by));
    if !m.used_by.is_empty() {
        parts.push(tr!("used by {names}", names = m.used_by.join(", ")));
    }
    parts.join(" · ")
}

impl App {
    /// The AlwyWatch server's web app (this profile's connection, else its account's).
    fn alwywatch_base(&self) -> Option<String> {
        self.core
            .keep_connection()
            .map(|k| k.base_url)
            .or_else(|| self.core.account().map(|a| a.base_url))
            .map(|u| u.trim_end_matches('/').to_owned())
    }

    /// Settings → Admin → Server settings: set up in the web app.
    pub(super) fn open_server_settings(&self) {
        if let Some(base) = self.alwywatch_base() {
            platform::open_url(&format!("{base}/settings/admin/server"));
        }
    }

    /// Settings → Admin → Invitations: what an invitation can do here, and the list.
    pub(super) fn invites_load(self: &Rc<Self>) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.account_invitations().await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(v) => {
                    {
                        let mut st = t.state.borrow_mut();
                        let s = &mut st.invites;
                        // The chosen server stays chosen while it is still there.
                        let chosen = s.targets.get(s.target).map(|x| x.id.clone());
                        s.mail = v.mail;
                        s.targets = v
                            .jellyfin
                            .into_iter()
                            .map(|j| Target {
                                id: j.server_id,
                                name: j.name,
                                users: j.users.into_iter().map(|u| (u.id, u.name)).collect(),
                                last_like: j.last_like,
                            })
                            .collect();
                        s.target = chosen
                            .and_then(|c| s.targets.iter().position(|x| x.id == c))
                            .unwrap_or(0);
                        s.made = v
                            .invites
                            .into_iter()
                            .map(|i| Made {
                                id: i.id,
                                created_by: i.created_by,
                                expires: i.expires,
                                uses_left: i.uses_left,
                                email: i.email,
                                jellyfin: i.jellyfin,
                                used_by: i.used_by,
                            })
                            .collect();
                    }
                    t.ui().set_invites_message(SharedString::new());
                    t.render_invites();
                }
                Err(e) => t.ui().set_invites_message(sstr(e.to_string())),
            }
            t.ui().set_invites_loaded(true);
        })
        .ok();
    }

    fn render_invites(&self) {
        let ui = self.ui();
        let st = self.state.borrow();
        let s = &st.invites;
        ui.set_invite_mail(s.mail);
        let target = s.targets.get(s.target);
        ui.set_invite_server(sstr(target.map(|t| t.name.as_str()).unwrap_or_default()));
        ui.set_invite_server_many(s.targets.len() > 1);
        let like = s.like_shown();
        ui.set_invite_like(sstr(
            target
                .and_then(|t| t.users.iter().find(|(id, _)| *id == like))
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| tr!("Jellyfin's defaults")),
        ));
        let now = now_secs();
        let rows: Vec<InviteRow> = s
            .made
            .iter()
            .map(|m| InviteRow {
                id: sstr(m.id.to_string()),
                title: sstr(m.email.clone().unwrap_or_else(|| tr!("Link"))),
                detail: sstr(detail(m, now)),
                active: m.uses_left > 0 && m.expires > now,
            })
            .collect();
        ui.set_invites(ModelRc::new(VecModel::from(rows)));
    }

    /// "Jellyfin server": the next one.
    pub(super) fn invite_next_server(&self) {
        {
            let mut st = self.state.borrow_mut();
            let s = &mut st.invites;
            if !s.targets.is_empty() {
                s.target = (s.target + 1) % s.targets.len();
                s.like = None;
            }
        }
        self.render_invites();
    }

    /// "With the same access as": the next user, then Jellyfin's defaults.
    pub(super) fn invite_next_like(&self) {
        {
            let mut st = self.state.borrow_mut();
            let s = &mut st.invites;
            let Some(t) = s.targets.get(s.target) else {
                return;
            };
            let mut ids: Vec<String> = t.users.iter().map(|(id, _)| id.clone()).collect();
            ids.push(String::new());
            let now = s.like_shown();
            let i = ids.iter().position(|x| *x == now).map_or(0, |i| i + 1);
            s.like = Some(ids[i % ids.len()].clone());
        }
        self.render_invites();
    }

    /// "Send invitation" (with an email) or "Create link".
    pub(super) fn admin_invite(self: &Rc<Self>) {
        let ui = self.ui();
        let (jellyfin, like, mail) = {
            let st = self.state.borrow();
            let s = &st.invites;
            if s.busy {
                return;
            }
            let target = s.targets.get(s.target).filter(|_| ui.get_invite_jellyfin());
            (
                target.map(|t| t.id.clone()),
                target.map(|_| s.like_shown()).filter(|l| !l.is_empty()),
                s.mail,
            )
        };
        let email = Some(ui.get_invite_email().trim().to_owned()).filter(|e| mail && !e.is_empty());
        let note = Some(ui.get_invite_note().to_string()).filter(|_| email.is_some());
        let days = i64::from(ui.get_invite_days().max(1));
        let uses = if email.is_some() {
            1
        } else {
            i64::from(ui.get_invite_uses().max(1))
        };
        self.state.borrow_mut().invites.busy = true;
        ui.set_invite_busy(true);
        ui.set_invite_made_message(SharedString::new());
        ui.set_invite_sent_to(SharedString::new());
        ui.set_invite_made_code(SharedString::new());
        ui.set_invite_made_link(SharedString::new());
        ui.set_invite_copied(false);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .account_invite_send(email, jellyfin, like, note, days, uses)
                .await;
            let Some(t) = this.upgrade() else { return };
            t.state.borrow_mut().invites.busy = false;
            let ui = t.ui();
            ui.set_invite_busy(false);
            match r {
                Ok(i) => {
                    if let Some(to) = i.sent_to {
                        ui.set_invite_sent_to(sstr(to));
                    } else {
                        ui.set_invite_made_code(sstr(&i.code));
                        ui.set_invite_made_link(sstr(i.url.unwrap_or_default()));
                        ui.set_invite_made_hint(sstr(trn!(
                            "Valid {n} day, for {count}. In AlwyWatch: Add a profile → Create an AlwyWatch account.",
                            "Valid {n} days, for {count}. In AlwyWatch: Add a profile → Create an AlwyWatch account.",
                            i.expires_in_days,
                            count = trn!("{n} person", "{n} people", i.uses),
                        )));
                    }
                    ui.set_invite_email(SharedString::new());
                    ui.set_invite_note(SharedString::new());
                    t.invites_load();
                }
                Err(e) => ui.set_invite_made_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// "End": the invitation stops working.
    pub(super) fn invite_end(self: &Rc<Self>, id: String) {
        let Ok(id) = id.parse::<i64>() else { return };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.account_revoke_invite(id).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(()) => t.invites_load(),
                Err(e) => t.ui().set_invites_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// "Create an AlwyWatch account": clears what an earlier invitation said.
    pub(super) fn create_invite_reset(&self) {
        self.state.borrow_mut().invites.check_gen += 1;
        let ui = self.ui();
        ui.set_create_invite_from(SharedString::new());
        ui.set_create_invite_note(SharedString::new());
        ui.set_create_invite_bad(false);
    }

    /// The invitation field changed: once typing pauses, who invited and what it gives.
    pub(super) fn create_invite_edited(self: &Rc<Self>, url: String, typed: String) {
        self.create_invite_reset();
        let code = invite_code(&typed);
        if code.len() < 4 {
            return;
        }
        let generation = self.state.borrow().invites.check_gen;
        let this = Rc::downgrade(self);
        slint::Timer::single_shot(Duration::from_millis(500), move || {
            let Some(t) = this.upgrade() else { return };
            if t.state.borrow().invites.check_gen != generation {
                return;
            }
            let (core, this) = (t.core.clone(), Rc::downgrade(&t));
            slint::spawn_local(async move {
                let r = match normalize_server_url(url) {
                    Ok(url) => core.account_invite_info(url, code).await,
                    Err(e) => Err(e),
                };
                let Some(t) = this.upgrade() else { return };
                if t.state.borrow().invites.check_gen != generation {
                    return;
                }
                let ui = t.ui();
                match r {
                    Ok(i) => {
                        ui.set_create_invite_from(sstr(tr!(
                            "{inviter} invited you",
                            inviter = i.inviter
                        )));
                        ui.set_create_invite_note(sstr(
                            i.jellyfin
                                .map(|server| {
                                    tr!(
                                        "This username and password will also be your {server} (Jellyfin) login.",
                                        server = server
                                    )
                                })
                                .unwrap_or_default(),
                        ));
                        if let Some(name) = i.suggested_username
                            && ui.get_create_username().trim().is_empty()
                        {
                            ui.set_create_username(sstr(name));
                        }
                    }
                    Err(e) => {
                        ui.set_create_invite_note(sstr(e.to_string()));
                        ui.set_create_invite_bad(true);
                    }
                }
            })
            .ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_code_from_link_or_code() {
        assert_eq!(invite_code(" ABCD1234 "), "ABCD1234");
        assert_eq!(
            invite_code("https://watch.example.org/join/ABCD1234"),
            "ABCD1234"
        );
        assert_eq!(
            invite_code("https://watch.example.org/join/ABCD1234/?x=1"),
            "ABCD1234"
        );
    }
}
