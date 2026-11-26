// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together on the desktop (docs/together-apps.md): private rooms that start as a lobby,
//! people invited from a list (recent partners first), invitations announced anywhere in the
//! app, and the room a player follows (same protocol as Android, docs/keep-protocol.md).
//!
//! "Watch together" on a title creates the lobby and opens the Room page; in the player it
//! pauses and opens the room over the video, started from there. The room's `wait` loop runs
//! from then on (lobby and player alike: it is the seat's heartbeat) and opens the player at
//! each start. A start waits until every player has loaded the title, then plays at the same
//! instant everywhere (the server's clock, measured by the core). The host closing the player
//! takes everyone back to the waiting room; nobody leaves the room but with Leave. The player
//! (mpv, own thread) gets the room's play/pause/seek through a channel and sends back what the
//! viewer does. A second long poll, on my rooms and invitations, runs while an AlwyWatch server
//! is linked.

use std::sync::mpsc;

use super::*;
use crate::player::{FromPlayer, ToPlayer, TogetherLink};
use crate::{PersonRow, TogetherRow};

/// How a player starts with regard to watch together.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TogetherPlay {
    Alone,
    /// In the current room; `announce`: this title was chosen here (next episode), tell the room.
    Room {
        announce: bool,
    },
}

/// The room this app is in (one at a time).
pub(super) struct Room {
    code: String,
    member: String,
    seq: u64,
    state: Option<TogetherState>,
    to_player: Option<mpsc::Sender<ToPlayer>>,
    /// Our updates on their way: room answers in between are older than them.
    sending: u32,
    /// The room moved to this title (account, item): play it once the current player closes.
    switch_to: Option<(String, String)>,
    /// A player of the room is open.
    player_open: bool,
    /// The last start (`round`) the player opened for, or that the viewer closed: every later
    /// one opens it again.
    handled_round: u64,
    /// The start we said we loaded (`ready`).
    ready_round: u64,
    /// A `join` was already tried after a 404 (a dropped seat may come back).
    rejoined: bool,
    /// Bumped to stop this room's loop (a new room).
    generation: u64,
}

/// My rooms and invitations, and the people to invite.
#[derive(Default)]
pub(super) struct Lists {
    rooms: Option<TogetherRooms>,
    /// Bumped to stop the list's long poll.
    pub(super) generation: u64,
    room_gen: u64,
    /// Invitations already announced, and rooms left or declined here (never announced again).
    shown: HashSet<String>,
    gone: HashSet<String>,
    /// The invitation on the banner.
    banner: Option<String>,
    /// Posters from my servers, by title.
    posters: HashMap<String, Image>,
    posters_wanted: HashSet<String>,
    people: Vec<TogetherPerson>,
    picked: Vec<String>,
    filter: String,
}

/// Local clock in ms, only compared with itself (to move the room's position on).
pub(super) fn mono_ms() -> i64 {
    thread_local! {
        static START: std::time::Instant = std::time::Instant::now();
    }
    START.with(|s| s.elapsed().as_millis() as i64)
}

/// "Silo S2 · E7" or the movie title.
pub(super) fn media_title(m: &TogetherMedia) -> String {
    if m.title.trim().is_empty() {
        return tr!("Nothing chosen yet");
    }
    match (&m.series_title, m.season, m.episode) {
        (Some(s), Some(se), Some(ep)) => format!("{s} S{se} · E{ep}"),
        _ => m.title.clone(),
    }
}

/// Same title, whatever the room (poster cache key).
fn media_key(m: &TogetherMedia) -> String {
    format!(
        "{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}",
        m.kind, m.title, m.tmdb, m.imdb, m.tvdb, m.season, m.episode
    )
}

/// One person in the room: their seats grouped (a user on two devices appears twice).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Seat {
    pub user_key: String,
    pub name: String,
    pub avatar: Avatar,
    pub devices: usize,
    pub host: bool,
    /// One of the seats is this device's.
    pub me: bool,
}

/// Members grouped by user, in the room's order (by name), the host's user marked.
pub(super) fn group_members(s: &TogetherState, my_member: &str) -> Vec<Seat> {
    let mut out: Vec<Seat> = Vec::new();
    for m in &s.members {
        let host = m.id == s.host;
        let me = m.id == my_member;
        match out.iter_mut().find(|p| p.user_key == m.user_key) {
            Some(p) => {
                p.devices += 1;
                p.host |= host;
                p.me |= me;
            }
            None => out.push(Seat {
                user_key: m.user_key.clone(),
                name: m.name.clone(),
                avatar: m.avatar.clone(),
                devices: 1,
                host,
                me,
            }),
        }
    }
    out
}

/// "Host · you · 2 devices".
fn seat_detail(p: &Seat) -> String {
    let mut parts = Vec::new();
    if p.host {
        parts.push(tr!("Host"));
    }
    if p.me {
        parts.push(tr!("You"));
    }
    if p.devices > 1 {
        parts.push(trn!("{n} device", "{n} devices", p.devices));
    }
    parts.join(" · ")
}

/// A row of the people list.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Pick {
    pub user_key: String,
    pub name: String,
    pub detail: String,
    pub header: &'static str,
    pub avatar: Avatar,
}

/// The people to invite, in the server's order (recent partners, then everyone by name), without
/// `hide` (already in the room or invited) and filtered by name. A second line tells people
/// apart: their media-server name when it differs, their server when two names are equal.
pub(super) fn people_rows(
    people: &[TogetherPerson],
    hide: &HashSet<String>,
    filter: &str,
) -> Vec<Pick> {
    let filter = filter.trim().to_lowercase();
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    let mut out = Vec::new();
    let (mut recent, mut others) = (false, false);
    for p in people {
        if hide.contains(&p.user_key) {
            continue;
        }
        if !filter.is_empty()
            && !p.name.to_lowercase().contains(&filter)
            && !p.media_name.to_lowercase().contains(&filter)
        {
            continue;
        }
        let twin = people
            .iter()
            .any(|o| o.user_key != p.user_key && same(&o.name, &p.name));
        let mut detail = Vec::new();
        if !p.media_name.is_empty() && !same(&p.media_name, &p.name) {
            detail.push(p.media_name.clone());
        }
        if twin {
            if !p.server_name.is_empty() {
                detail.push(p.server_name.clone());
            } else if detail.is_empty() && !p.media_name.is_empty() {
                detail.push(p.media_name.clone());
            }
        }
        let header = if p.last_together_at.is_some() {
            if recent {
                ""
            } else {
                recent = true;
                "RECENT"
            }
        } else if others || !recent {
            others = true;
            ""
        } else {
            others = true;
            "EVERYONE"
        };
        out.push(Pick {
            user_key: p.user_key.clone(),
            name: p.name.clone(),
            detail: detail.join(" · "),
            header,
            avatar: p.avatar.clone(),
        });
    }
    out
}

/// "3 watching: Alex, Sam, Kim".
fn watching_note(s: &TogetherState) -> String {
    let people = group_members(s, "");
    let names: Vec<&str> = people.iter().map(|p| p.name.as_str()).collect();
    trn!(
        "{n} watching: {names}",
        "{n} watching: {names}",
        people.len(),
        names = names.join(", ")
    )
}

/// A failed room call in plain words.
pub(super) fn room_error(e: &CoreError) -> String {
    match e {
        CoreError::Server { status: 409 } => tr!("The room is full (20 people at most)."),
        CoreError::Server { status: 429 } => {
            tr!("Too many rooms or invitations at once: try again in a few minutes.")
        }
        CoreError::Server { status: 403 } => tr!("Only the room's host can do that."),
        CoreError::Server { status: 503 } => {
            tr!("Your AlwyWatch server has too many rooms open right now.")
        }
        CoreError::NotFound => tr!("This room is gone."),
        CoreError::Network { .. } => tr!("Can't reach your AlwyWatch server right now."),
        CoreError::Invalid { .. } => {
            tr!("Your AlwyWatch server answered something unexpected: it may need an update.")
        }
        e => e.to_string(),
    }
}

impl App {
    // ----- my rooms and invitations ---------------------------------------------------------

    /// The Together page: the list as last seen (instantly), then fresh from the server.
    pub(super) fn load_together(self: &Rc<Self>) {
        if self.state.borrow().lists.rooms.is_none() {
            let cached = self
                .core
                .get_setting(ROOMS_KEY.into())
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str::<TogetherRooms>(&s).ok());
            self.state.borrow_mut().lists.rooms = cached;
        }
        self.render_rooms_list();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_rooms().await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(rooms) => t.set_rooms(rooms),
                Err(e) => {
                    if t.state.borrow().lists.rooms.is_none() {
                        t.ui().set_together_status(sstr(tr!(
                            "Can't load your rooms: {error}",
                            error = room_error(&e)
                        )));
                    }
                }
            }
        })
        .ok();
    }

    /// Runs the list's long poll while an AlwyWatch server is linked: invitations arrive
    /// through it.
    pub(super) fn start_rooms_poll(self: &Rc<Self>) {
        let generation = {
            let mut st = self.state.borrow_mut();
            st.lists.generation += 1;
            st.lists.generation
        };
        if !self.core.keep_active() {
            return;
        }
        // Play on the TV: my TVs, asked alongside.
        self.start_remote_poll(generation);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        let live = move |this: &std::rc::Weak<App>| {
            this.upgrade()
                .filter(|t| t.state.borrow().lists.generation == generation && t.core.keep_active())
        };
        slint::spawn_local(async move {
            let (mut version, mut failures) = (0u64, 0u32);
            loop {
                if live(&this).is_none() {
                    return;
                }
                let asked = mono_ms();
                let r = core.together_rooms_wait(version).await;
                let Some(t) = live(&this) else { return };
                match r {
                    Ok(list) => {
                        failures = 0;
                        let same = list.version == version;
                        version = list.version;
                        t.set_rooms(list);
                        drop(t);
                        // Never spin on a server that answers at once without news.
                        if same && mono_ms() - asked < 1000 {
                            async_sleep(Duration::from_secs(2)).await;
                        }
                    }
                    Err(_) => {
                        failures += 1;
                        drop(t);
                        let secs = (2u64 << failures.min(6)).min(120);
                        async_sleep(Duration::from_secs(secs)).await;
                    }
                }
            }
        })
        .ok();
    }

    /// Stops the list's long poll and forgets the rooms (AlwyWatch server unlinked).
    pub(super) fn stop_rooms_poll(&self) {
        {
            let mut st = self.state.borrow_mut();
            st.lists.generation += 1;
            st.lists.rooms = None;
            st.lists.banner = None;
            st.lists.people.clear();
        }
        self.stop_remote();
        let _ = self.core.set_setting(ROOMS_KEY.into(), String::new());
        self.render_banner();
    }

    /// A new list: remembered, shown, and new invitations announced.
    fn set_rooms(self: &Rc<Self>, list: TogetherRooms) {
        let fresh = {
            let mut st = self.state.borrow_mut();
            let lists = &mut st.lists;
            // A room gone from the list may invite me again later (the host left it to someone
            // who invites me back): that invitation is a new one, shown and announced.
            lists
                .shown
                .retain(|c| list.rooms.iter().any(|r| &r.code == c && r.invited));
            lists
                .gone
                .retain(|c| list.rooms.iter().any(|r| &r.code == c));
            let invited: Vec<&TogetherRoom> = list.rooms.iter().filter(|r| r.invited).collect();
            let fresh = invited
                .iter()
                .find(|r| !lists.shown.contains(&r.code) && !lists.gone.contains(&r.code))
                .map(|r| (*r).clone());
            lists.shown.extend(invited.iter().map(|r| r.code.clone()));
            if lists
                .banner
                .as_ref()
                .is_some_and(|b| !invited.iter().any(|r| &r.code == b))
            {
                lists.banner = None;
            }
            if let Some(r) = &fresh {
                lists.banner = Some(r.code.clone());
            }
            if let Ok(json) = serde_json::to_string(&list) {
                let _ = self.core.set_setting(ROOMS_KEY.into(), json);
            }
            lists.rooms = Some(list);
            fresh
        };
        if let Some(r) = fresh {
            self.notify_player(tr!(
                "{host} invites you to watch {title} · answer in AlwyWatch",
                host = r.host,
                title = media_title(&r.media)
            ));
        }
        self.render_banner();
        if self.ui().get_page() == Page::Together {
            self.render_rooms_list();
        }
    }

    pub(super) fn render_rooms_list(self: &Rc<Self>) {
        let ui = self.ui();
        let rooms: Vec<TogetherRoom> = {
            let st = self.state.borrow();
            let gone = &st.lists.gone;
            match &st.lists.rooms {
                Some(l) => l
                    .rooms
                    .iter()
                    .filter(|r| !(r.invited && gone.contains(&r.code)))
                    .cloned()
                    .collect(),
                None => vec![],
            }
        };
        ui.set_together_status(sstr(if rooms.is_empty() {
            tr!(
                "No rooms yet. Start one here, or open a movie or an episode and choose Watch together."
            )
        } else {
            String::new()
        }));
        let here = self
            .state
            .borrow()
            .together
            .as_ref()
            .map(|r| r.code.clone());
        let rows: Vec<TogetherRow> = rooms
            .iter()
            .map(|r| {
                let poster = self
                    .state
                    .borrow()
                    .lists
                    .posters
                    .get(&media_key(&r.media))
                    .cloned()
                    .unwrap_or_default();
                let others = r.members.len().saturating_sub(1);
                TogetherRow {
                    code: sstr(&r.code),
                    title: sstr(media_title(&r.media)),
                    people: sstr(if r.invited {
                        tr!("{host} invites you", host = r.host)
                    } else {
                        match others {
                            0 => r.host.clone(),
                            n => trn!(
                                "{host} and {n} other",
                                "{host} and {n} others",
                                n,
                                host = r.host
                            ),
                        }
                    }),
                    status: sstr({
                        let status = match (r.started, r.playing) {
                            (false, _) => tr!("Waiting room: not started yet"),
                            (true, true) => {
                                tr!("Watching · {time}", time = clock(room_position(r)))
                            }
                            (true, false) => {
                                tr!("Paused at {time}", time = clock(room_position(r)))
                            }
                        };
                        if !r.invited && here.as_deref() != Some(r.code.as_str()) {
                            tr!("{status} · open on another device", status = status)
                        } else {
                            status
                        }
                    }),

                    poster,
                    mine: r.mine,
                    invited: r.invited,
                    host_avatar: self.person_avatar(&r.host_user_key, &r.host, &r.host_avatar),
                }
            })
            .collect();
        ui.set_together_rooms(ModelRc::new(VecModel::from(rows)));
        for r in rooms {
            self.want_poster(&r.media);
        }
    }

    /// Looks the title up on my servers once, for its poster.
    fn want_poster(self: &Rc<Self>, media: &TogetherMedia) {
        let key = media_key(media);
        if !self
            .state
            .borrow_mut()
            .lists
            .posters_wanted
            .insert(key.clone())
        {
            return;
        }
        let accounts = self.home_accounts();
        let (core, this, media) = (self.core.clone(), Rc::downgrade(self), media.clone());
        slint::spawn_local(async move {
            let Ok(Some(found)) = core.together_find(accounts, media).await else {
                return;
            };
            let item = match core.cached_item(found.account_id.clone(), found.item_id.clone()) {
                Ok(Some(i)) => i.item,
                _ => match core
                    .refresh_item(found.account_id.clone(), found.item_id.clone(), false)
                    .await
                {
                    Ok(i) => i.item,
                    Err(_) => return,
                },
            };
            let Some(t) = this.upgrade() else { return };
            t.state
                .borrow_mut()
                .owners
                .insert(item.id.clone(), found.account_id);
            t.load_image(&item, ImageKind::SeriesPrimary, POSTER_PX, move |t, img| {
                t.state.borrow_mut().lists.posters.insert(key.clone(), img);
                match t.ui().get_page() {
                    Page::Together => t.render_rooms_list(),
                    Page::Room => t.render_room(),
                    _ => {}
                }
            });
        })
        .ok();
    }

    pub(super) fn render_banner(&self) {
        let ui = self.ui();
        let room = {
            let st = self.state.borrow();
            st.lists.banner.as_ref().and_then(|code| {
                st.lists
                    .rooms
                    .as_ref()?
                    .rooms
                    .iter()
                    .find(|r| &r.code == code)
                    .cloned()
            })
        };
        match room {
            Some(r) => {
                ui.set_invite_text(sstr(tr!(
                    "{host} invites you to watch {title}",
                    host = r.host,
                    title = media_title(&r.media)
                )));
                ui.set_invite_avatar(self.person_avatar(&r.host_user_key, &r.host, &r.host_avatar));
                ui.set_invite_code(sstr(&r.code));
            }
            None => ui.set_invite_code(SharedString::new()),
        }
    }

    pub(super) fn invite_join(self: &Rc<Self>) {
        let code = self.state.borrow_mut().lists.banner.take();
        self.render_banner();
        if let Some(code) = code {
            self.together_open(code);
        }
    }

    pub(super) fn invite_decline(self: &Rc<Self>) {
        let code = self.state.borrow_mut().lists.banner.take();
        self.render_banner();
        if let Some(code) = code {
            self.together_decline(code);
        }
    }

    /// The banner goes; the invitation stays on the Together page.
    pub(super) fn invite_dismiss(&self) {
        self.state.borrow_mut().lists.banner = None;
        self.render_banner();
    }

    /// "Not now" on an invitation.
    pub(super) fn together_decline(self: &Rc<Self>, code: String) {
        self.state.borrow_mut().lists.gone.insert(code.clone());
        if self.ui().get_page() == Page::Together {
            self.render_rooms_list();
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_decline(code).await;
            if let (Err(e), Some(t)) = (r, this.upgrade())
                && e != CoreError::NotFound
            {
                t.ui().set_error(sstr(tr!(
                    "Couldn't decline: {error}",
                    error = room_error(&e)
                )));
            }
        })
        .ok();
    }

    /// A room of the Together page (or the banner's): mine here → its page; otherwise join it
    /// (an invitation, or my room on another device).
    pub(super) fn together_open(self: &Rc<Self>, code: String) {
        let here = self
            .state
            .borrow()
            .together
            .as_ref()
            .map(|r| r.code.clone());
        match here {
            Some(c) if c == code => self.navigate(Route::Room),
            Some(_) => {
                self.navigate(Route::Room);
                self.ui().set_room_message(sstr(tr!(
                    "You're in this room. Leave it first to join another one."
                )));
            }
            None => self.join_room(code),
        }
    }

    fn join_room(self: &Rc<Self>, code: String) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_join(code.clone()).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(j) => {
                    t.state.borrow_mut().lists.gone.remove(&code);
                    t.enter_room(j);
                    t.navigate(Route::Room);
                    // A started room: straight to the player, in sync.
                    t.open_if_new_round();
                }
                Err(e) => t
                    .ui()
                    .set_error(sstr(tr!("Couldn't join: {error}", error = room_error(&e)))),
            }
        })
        .ok();
    }

    // ----- the room -------------------------------------------------------------------------

    /// "Watch together" on a title: a new lobby with it, then its page. In my own room, the
    /// host changes the title instead (everyone stays; a playing room loads it at once).
    pub(super) fn watch_together(self: &Rc<Self>, account: String, id: String) {
        let current = {
            let st = self.state.borrow();
            st.together.as_ref().map(|r| {
                let host = r
                    .state
                    .as_ref()
                    .is_some_and(|s| together_is_host(s.clone(), r.member.clone()));
                (r.code.clone(), r.member.clone(), host)
            })
        };
        let resume = self
            .state
            .borrow()
            .items
            .get(&id)
            .map_or(0, |i| i.user.position_ms);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        match current {
            Some((code, member, true)) => {
                slint::spawn_local(async move {
                    let r = core
                        .together_update(code, member, false, resume, Some(account), Some(id))
                        .await;
                    let Some(t) = this.upgrade() else { return };
                    t.navigate(Route::Room);
                    match r {
                        Ok(s) => {
                            t.accept_room(s);
                            t.render_room();
                        }
                        Err(e) => t.ui().set_room_message(sstr(room_error(&e))),
                    }
                })
                .ok();
            }
            Some(_) => {
                self.navigate(Route::Room);
                self.ui().set_room_message(sstr(tr!(
                    "You're already in a room. Leave it first to start another one."
                )));
            }
            None => {
                slint::spawn_local(async move {
                    let r = core.together_create(account, id).await;
                    let Some(t) = this.upgrade() else { return };
                    match r {
                        Ok(j) => {
                            t.enter_room(j);
                            t.navigate(Route::Room);
                        }
                        Err(e) => t.ui().set_error(sstr(tr!(
                            "Watch together unavailable: {error}",
                            error = room_error(&e)
                        ))),
                    }
                })
                .ok();
            }
        }
    }

    /// "New room" on the Together page: a lobby with no title yet. People join while the host
    /// browses; a title's "Watch in your room" then chooses what to watch.
    pub(super) fn together_new(self: &Rc<Self>) {
        if self.in_room() {
            self.navigate(Route::Room);
            self.ui().set_room_message(sstr(tr!(
                "You're already in a room. Leave it first to start another one."
            )));
            return;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_open().await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(j) => {
                    t.enter_room(j);
                    t.navigate(Route::Room);
                }
                Err(e) => t.ui().set_error(sstr(tr!(
                    "Watch together unavailable: {error}",
                    error = room_error(&e)
                ))),
            }
        })
        .ok();
    }

    fn enter_room(self: &Rc<Self>, j: TogetherJoined) {
        let generation = {
            let mut st = self.state.borrow_mut();
            st.lists.room_gen += 1;
            st.lists.room_gen
        };
        self.state.borrow_mut().together = Some(Room {
            code: j.state.code.clone(),
            member: j.member_id,
            seq: 0,
            state: None,
            to_player: None,
            sending: 0,
            switch_to: None,
            player_open: false,
            handled_round: 0,
            ready_round: 0,
            rejoined: false,
            generation,
        });
        self.ui().set_room_message(SharedString::new());
        self.ui().set_room_info(SharedString::new());
        self.accept_room(j.state);
        self.follow_room();
    }

    pub(super) fn in_room(&self) -> bool {
        self.state.borrow().together.is_some()
    }

    pub(super) fn render_room(self: &Rc<Self>) {
        let ui = self.ui();
        if ui.get_page() != Page::Room && !ui.get_player_room() {
            return;
        }
        let Some((s, member)) = self
            .state
            .borrow()
            .together
            .as_ref()
            .and_then(|r| Some((r.state.clone()?, r.member.clone())))
        else {
            return;
        };
        let host = together_is_host(s.clone(), member.clone());
        let host_name = s
            .members
            .iter()
            .find(|m| m.id == s.host)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| tr!("the host"));
        let now = self.core.together_now_ms();
        let untitled = s.media.title.trim().is_empty();
        ui.set_room_untitled(untitled);
        ui.set_room_title(sstr(media_title(&s.media)));
        ui.set_room_status(sstr(match (s.started, s.playing) {
            (false, _) if untitled && host => tr!(
                "Open a movie or an episode and choose Watch in your room. The people you invite can join in the meantime."
            ),
            (false, _) if untitled => {
                tr!("{host} is choosing what to watch.", host = host_name)
            }
            (false, _) if host && s.round > 0 => tr!(
                "Back in the waiting room: start again, or open another title and choose Watch together. Everyone stays here."
            ),
            (false, _) if host => {
                tr!("Waiting room: invite people, then start when everyone is here.")
            }
            (false, _) if s.round > 0 => {
                tr!("{host} is picking what to watch next.", host = host_name)
            }
            (false, _) => tr!("Waiting room: nothing plays until the host starts."),
            (true, _) if !s.loading.is_empty() => tr!("Starting: loading for everyone…"),
            (true, true) => tr!(
                "Watching · {time}",
                time = clock(together_position_at(s.clone(), now))
            ),
            (true, false) => tr!(
                "Paused at {time}",
                time = clock(together_position_at(s.clone(), now))
            ),
        }));
        ui.set_room_is_host(host);
        ui.set_room_started(s.started);
        ui.set_room_host_name(sstr(host_name));
        let members: Vec<PersonRow> = group_members(&s, &member)
            .iter()
            .map(|p| PersonRow {
                key: sstr(&p.user_key),
                name: sstr(&p.name),
                detail: sstr(seat_detail(p)),
                avatar: self.person_avatar(&p.user_key, &p.name, &p.avatar),
                header: SharedString::new(),
                selected: false,
            })
            .collect();
        let invited: Vec<PersonRow> = s
            .invited
            .iter()
            .map(|p| PersonRow {
                key: sstr(&p.user_key),
                name: sstr(&p.name),
                detail: sstr(tr!("Invited")),

                avatar: self.person_avatar(&p.user_key, &p.name, &p.avatar),
                header: SharedString::new(),
                selected: false,
            })
            .collect();
        ui.set_room_members(ModelRc::new(VecModel::from(members)));
        ui.set_room_invited(ModelRc::new(VecModel::from(invited)));
        let poster = self
            .state
            .borrow()
            .lists
            .posters
            .get(&media_key(&s.media))
            .cloned();
        ui.set_room_poster(poster.unwrap_or_default());
        self.want_poster(&s.media);
    }

    /// The host starts: playing from the beginning for everyone, the player opens here.
    pub(super) fn room_start(self: &Rc<Self>) {
        let Some((code, member)) = self.room_seat() else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        // Back in the waiting room after a title: from where the host stopped.
        let from = self
            .room_state()
            .filter(|s| s.round > 0)
            .map_or(0, |s| s.position_ms);
        slint::spawn_local(async move {
            let r = core.together_start(code, member, from).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(s) => {
                    t.accept_room(s);
                    t.render_room();
                    t.open_if_new_round();
                }
                Err(e) => t.ui().set_room_message(sstr(room_error(&e))),
            }
        })
        .ok();
    }

    fn room_seat(&self) -> Option<(String, String)> {
        let st = self.state.borrow();
        st.together
            .as_ref()
            .map(|r| (r.code.clone(), r.member.clone()))
    }

    /// A start this device hasn't opened the player for: open it.
    fn open_if_new_round(self: &Rc<Self>) {
        let open = {
            let mut st = self.state.borrow_mut();
            match st.together.as_mut() {
                Some(r) => {
                    let round = r.state.as_ref().filter(|s| s.started).map(|s| s.round);
                    match round {
                        Some(n) if n > r.handled_round && !r.player_open => {
                            r.handled_round = n;
                            true
                        }
                        _ => false,
                    }
                }
                None => false,
            }
        };
        if open {
            self.open_room_player();
        }
    }

    /// Plays the room's title from my servers, where the room is now.
    pub(super) fn open_room_player(self: &Rc<Self>) {
        let Some(s) = self.room_state() else { return };
        if self
            .state
            .borrow()
            .together
            .as_ref()
            .is_some_and(|r| r.player_open)
        {
            return;
        }
        let accounts = self.home_accounts();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let found = core.together_find(accounts, s.media.clone()).await;
            let Some(t) = this.upgrade() else { return };
            match found {
                Ok(Some(f)) => {
                    let now = t.core.together_now_ms();
                    let Some(start) = t.room_state().map(|s| together_position_at(s, now)) else {
                        return;
                    };
                    t.play_with(
                        f.account_id,
                        f.item_id,
                        Some(start),
                        TogetherPlay::Room { announce: false },
                    );
                }
                Ok(None) => t.ui().set_room_message(sstr(tr!(
                    "{title} isn't on your servers, so it can't play here.",
                    title = media_title(&s.media)
                ))),
                Err(e) => t.ui().set_room_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Leaves the room for good (the Leave button).
    pub(super) fn room_leave(self: &Rc<Self>) {
        if let Some(code) = self.room_seat().map(|(c, _)| c) {
            self.state.borrow_mut().lists.gone.insert(code);
        }
        self.to_player(ToPlayer::Quit);
        self.leave_room(None, true);
        self.drop_room_route();
        self.navigate(Route::Together);
    }

    /// The room page is no more: forget it in the history.
    fn drop_room_route(&self) {
        self.state
            .borrow_mut()
            .stack
            .retain(|r| !matches!(r, Route::Room));
    }

    /// The room is gone (closed on the server): said once, then back to where it makes sense.
    fn room_closed(self: &Rc<Self>) {
        self.leave_room(Some(&tr!("The watch together room closed")), false);
        if self.ui().get_page() == Page::Room {
            self.drop_room_route();
            self.navigate(Route::Together);
        }
        self.ui()
            .set_error(sstr(tr!("The watch together room closed.")));
    }

    /// The room's player is closed (another may follow at once: next episode, new title).
    /// The player's room button: "3 watching" while it plays in a room, else empty.
    pub(super) fn player_room_label(&self) -> String {
        let st = self.state.borrow();
        let Some(r) = st.together.as_ref().filter(|r| r.player_open) else {
            return String::new();
        };
        let Some(s) = r.state.as_ref() else {
            return String::new();
        };
        match group_members(s, "").len() {
            0 | 1 => tr!("Only you so far"),
            n => trn!("{n} watching", "{n} watching", n),
        }
    }

    pub(super) fn room_player_gone(&self) {
        if let Some(r) = self.state.borrow_mut().together.as_mut() {
            r.player_open = false;
            r.to_player = None;
        }
    }

    /// A message on the open player, whatever it plays.
    pub(super) fn notify_player(&self, text: String) {
        if let Some(tx) = self.state.borrow().player_notes.as_ref() {
            let _ = tx.send(text);
        }
    }

    /// A player of the room closed: back to the room's page (still in the room; Leave is
    /// there). An error from the player is said there. The host closing it takes everyone back
    /// to the waiting room (to pick something else).
    pub(super) fn room_player_closed(self: &Rc<Self>, error: Option<String>) {
        let lobby = {
            let mut st = self.state.borrow_mut();
            let Some(r) = st.together.as_mut() else {
                return;
            };
            r.player_open = false;
            r.to_player = None;
            let s = r.state.as_ref();
            if let Some(s) = s {
                r.handled_round = r.handled_round.max(s.round);
            }
            s.filter(|s| s.started && together_is_host((*s).clone(), r.member.clone()))
                .map(|s| (r.code.clone(), r.member.clone(), s.clone()))
        };
        if let (Some((code, member, s)), None) = (lobby, &error) {
            let at = together_position_at(s, self.core.together_now_ms());
            let (core, this) = (self.core.clone(), Rc::downgrade(self));
            slint::spawn_local(async move {
                let r = core.together_lobby(code, member, at).await;
                if let (Ok(s), Some(t)) = (r, this.upgrade()) {
                    t.accept_room(s);
                    t.render_room();
                }
            })
            .ok();
        }
        self.navigate(Route::Room);
        if let Some(e) = error {
            self.ui().set_room_message(sstr(e));
        }
    }

    // ----- inviting -------------------------------------------------------------------------

    pub(super) fn room_invite(self: &Rc<Self>) {
        {
            let mut st = self.state.borrow_mut();
            st.lists.picked.clear();
            st.lists.filter.clear();
        }
        let ui = self.ui();
        ui.set_picker_filter_text(SharedString::new());
        let empty = self.state.borrow().lists.people.is_empty();
        ui.set_picker_message(if empty {
            sstr(tr!("Loading…"))
        } else {
            SharedString::new()
        });
        ui.set_room_picker(true);
        self.render_picker();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_people().await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(people) => {
                    t.state.borrow_mut().lists.people = people;
                    t.ui().set_picker_message(SharedString::new());
                    t.render_picker();
                }
                Err(e) => t.ui().set_picker_message(sstr(tr!(
                    "Can't load the people: {error}",
                    error = room_error(&e)
                ))),
            }
        })
        .ok();
    }

    pub(super) fn render_picker(&self) {
        let ui = self.ui();
        if !ui.get_room_picker() {
            return;
        }
        let rows = {
            let st = self.state.borrow();
            let mut hide = HashSet::new();
            if let Some(s) = st.together.as_ref().and_then(|r| r.state.as_ref()) {
                hide.extend(s.members.iter().map(|m| m.user_key.clone()));
                hide.extend(s.invited.iter().map(|m| m.user_key.clone()));
            }
            let rows = people_rows(&st.lists.people, &hide, &st.lists.filter);
            let picked = &st.lists.picked;
            rows.into_iter()
                .map(|p| (picked.contains(&p.user_key), p))
                .collect::<Vec<_>>()
        };
        let loaded = !self.state.borrow().lists.people.is_empty();
        if loaded && rows.is_empty() {
            ui.set_picker_message(if self.state.borrow().lists.filter.is_empty() {
                sstr(tr!("Everyone is already in the room or invited."))
            } else {
                sstr(tr!("Nobody by that name."))
            });
        } else if loaded {
            ui.set_picker_message(SharedString::new());
        }
        let model: Vec<PersonRow> = rows
            .iter()
            .map(|(selected, p)| PersonRow {
                key: sstr(&p.user_key),
                name: sstr(&p.name),
                detail: sstr(&p.detail),
                avatar: self.person_avatar(&p.user_key, &p.name, &p.avatar),
                header: sstr(p.header),
                selected: *selected,
            })
            .collect();
        ui.set_picker_people(ModelRc::new(VecModel::from(model)));
        ui.set_picker_count(self.state.borrow().lists.picked.len() as i32);
    }

    pub(super) fn picker_toggle(&self, key: String) {
        {
            let mut st = self.state.borrow_mut();
            let picked = &mut st.lists.picked;
            match picked.iter().position(|k| *k == key) {
                Some(i) => {
                    picked.remove(i);
                }
                None => picked.push(key),
            }
        }
        self.render_picker();
    }

    pub(super) fn picker_filter(&self, text: String) {
        self.state.borrow_mut().lists.filter = text;
        self.render_picker();
    }

    pub(super) fn picker_send(self: &Rc<Self>) {
        let keys = self.state.borrow().lists.picked.clone();
        let ui = self.ui();
        if keys.is_empty() {
            ui.set_picker_message(sstr(tr!("Choose people first.")));
            return;
        }
        if keys.len() > 20 {
            ui.set_picker_message(sstr(tr!("20 people at most at once.")));
            return;
        }
        let Some((code, member)) = self.room_seat() else {
            return;
        };
        ui.set_picker_message(sstr(tr!("Sending…")));

        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_invite(code, member, keys).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(s) => {
                    t.accept_room(s);
                    t.ui().set_room_picker(false);
                    t.render_room();
                }
                Err(e) => t.ui().set_picker_message(sstr(room_error(&e))),
            }
        })
        .ok();
    }

    pub(super) fn picker_cancel(&self) {
        self.ui().set_room_picker(false);
    }

    pub(super) fn room_uninvite(self: &Rc<Self>, user_key: String) {
        let Some((code, member)) = self.room_seat() else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_uninvite(code, member, user_key).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(s) => {
                    t.accept_room(s);
                    t.render_room();
                }
                Err(e) => t.ui().set_room_message(sstr(room_error(&e))),
            }
        })
        .ok();
    }

    // ----- the room a player follows -------------------------------------------------------

    /// The channel a new player in the room listens to (replaces the previous player's).
    /// Every player's link to watch together; `in_room`: it plays the room's title now (else
    /// a room may start from it later, see [`App::player_watch_together`]).
    pub(super) fn player_link(self: &Rc<Self>, in_room: bool) -> TogetherLink {
        let (tx, rx) = mpsc::channel();
        let mut st = self.state.borrow_mut();
        st.player_tx = Some(tx.clone());
        if in_room && let Some(room) = st.together.as_mut() {
            room.to_player = Some(tx);
            room.player_open = true;
        }
        TogetherLink {
            rx,
            send: Box::new(|ev| {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                        t.from_player(ev);
                    }
                });
            }),
        }
    }

    /// Where the open player is (ms).
    fn player_position(&self) -> u64 {
        self.state
            .borrow()
            .player_mpv
            .as_ref()
            .and_then(|m| m.get_f64("time-pos"))
            .map_or(0, |s| (s.max(0.0) * 1000.0) as u64)
    }

    /// "Watch together" in the player while watching alone: pauses, opens a room for this
    /// title and shows it over the video (invite, then start from here: no reload).
    pub(super) fn player_watch_together(self: &Rc<Self>) {
        if self.in_room() {
            return;
        }
        let Some((account, id)) = self.state.borrow().player_item.clone() else {
            return;
        };
        if let Some(m) = self.state.borrow().player_mpv.clone() {
            let _ = m.command(&["set", "pause", "yes"]);
        }
        let ui = self.ui();
        ui.set_player_room_at(sstr(clock(self.player_position())));
        ui.set_room_title(SharedString::new());
        ui.set_room_members(ModelRc::default());
        ui.set_room_invited(ModelRc::default());
        ui.set_room_message(SharedString::new());
        ui.set_room_info(SharedString::new());
        ui.set_player_room(true);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_create(account, id).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(j) if t.ui().get_player_room() => {
                    t.enter_room(j);
                    let tx = t.state.borrow().player_tx.clone();
                    if let Some(r) = t.state.borrow_mut().together.as_mut() {
                        r.to_player = tx;
                        r.player_open = true;
                    }
                    t.render_room();
                }
                // Cancelled meanwhile: that room closes.
                Ok(j) => {
                    let _ = core.together_leave(j.state.code, j.member_id).await;
                }
                Err(e) => {
                    t.ui().set_player_room(false);
                    t.notify_player(tr!(
                        "Watch together unavailable: {error}",
                        error = room_error(&e)
                    ));
                }
            }
        })
        .ok();
    }

    /// The room dialog's "Start": everyone loads this title, then it plays for all at once.
    pub(super) fn player_room_start(self: &Rc<Self>) {
        let Some((code, member)) = self.room_seat() else {
            return;
        };
        let from = self.player_position();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_start(code, member, from).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(s) => {
                    t.accept_room(s.clone());
                    if let Some(r) = t.state.borrow_mut().together.as_mut() {
                        r.handled_round = r.handled_round.max(s.round);
                    }
                    let ui = t.ui();
                    ui.set_player_room(false);
                    ui.set_player_can_together(false);
                    t.sync_player(&s);
                    t.to_player(ToPlayer::Note(watching_note(&s)));
                }
                Err(e) => t.ui().set_room_message(sstr(room_error(&e))),
            }
        })
        .ok();
    }

    /// The room dialog's "Cancel": the room closes, playback goes on alone.
    pub(super) fn player_room_cancel(self: &Rc<Self>) {
        let ui = self.ui();
        ui.set_player_room(false);
        ui.set_room_picker(false);
        if let Some(code) = self.room_seat().map(|(c, _)| c) {
            self.state.borrow_mut().lists.gone.insert(code);
        }
        self.leave_room(None, true);
        if let Some(m) = self.state.borrow().player_mpv.clone() {
            let _ = m.command(&["set", "pause", "no"]);
        }
    }

    /// Right after a player started in the room: tell the room about our new title, or follow
    /// the room; then who is watching.
    pub(super) fn player_started(
        self: &Rc<Self>,
        account: String,
        id: String,
        play: TogetherPlay,
        start_ms: u64,
    ) {
        if play == (TogetherPlay::Room { announce: true }) {
            self.send_room(true, start_ms, Some((account, id)));
        } else if let Some(s) = self.room_state() {
            self.sync_player(&s);
        }
        if let Some(s) = self.room_state() {
            self.to_player(ToPlayer::Note(watching_note(&s)));
        }
    }

    /// The player could not start: the room stays, its page says why.
    pub(super) fn player_failed(self: &Rc<Self>, error: String) {
        if self.in_room() {
            self.room_player_closed(Some(error));
        } else {
            self.ui().set_error(sstr(error));
        }
    }

    fn to_player(&self, msg: ToPlayer) {
        if let Some(tx) = self
            .state
            .borrow()
            .together
            .as_ref()
            .and_then(|r| r.to_player.as_ref())
        {
            let _ = tx.send(msg);
        }
    }

    fn room_state(&self) -> Option<TogetherState> {
        self.state.borrow().together.as_ref()?.state.clone()
    }

    fn accept_room(&self, s: TogetherState) {
        let mut st = self.state.borrow_mut();
        let Some(room) = st.together.as_mut() else {
            return;
        };
        if s.code != room.code {
            return;
        }
        room.seq = room.seq.max(s.seq);
        let hosting = together_is_host(s.clone(), room.member.clone());
        room.state = Some(s);
        drop(st);
        // Title pages then offer "Watch in your room".
        self.ui().set_room_hosting(hosting);
    }

    /// Where the room is, for the player: on the server's clock, turned into an instant here
    /// (a start a moment ahead stays ahead); whether it waits for players to load the title.
    fn sync_player(&self, s: &TogetherState) {
        let Some(member) = self.room_seat().map(|(_, m)| m) else {
            return;
        };
        let now = self.core.together_now_ms();
        let starts_in = together_starts_in(s.clone(), now);
        let position_ms = if starts_in.is_some() {
            s.position_ms
        } else {
            together_position_at(s.clone(), now)
        };
        let at =
            std::time::Instant::now() + Duration::from_millis(starts_in.unwrap_or(0).max(0) as u64);
        let hold = !s.loading.is_empty();
        let load = together_is_loading(s.clone(), member.clone());
        let ready_round = self
            .state
            .borrow()
            .together
            .as_ref()
            .map_or(0, |r| r.ready_round);
        let others: Vec<String> = s
            .loading
            .iter()
            .filter(|id| **id != member)
            .filter_map(|id| s.members.iter().find(|m| &m.id == id))
            .map(|m| m.name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let waiting = if starts_in.is_some() || (hold && others.is_empty() && !load) {
            Some(tr!("Starting…"))
        } else if load && ready_round != s.round {
            Some(tr!("Loading for everyone…"))
        } else if hold {
            Some(tr!("Waiting for {names}…", names = others.join(", ")))
        } else {
            None
        };
        self.to_player(ToPlayer::Sync {
            playing: s.playing,
            position_ms,
            at,
            hold,
            load,
            waiting,
        });
    }

    /// The player loaded the title: tell the room (once per start).
    fn player_ready(self: &Rc<Self>) {
        let Some(((code, member), s)) = self.room_seat().zip(self.room_state()) else {
            return;
        };
        if !together_is_loading(s.clone(), member.clone()) {
            return;
        }
        let round = s.round;
        if let Some(r) = self.state.borrow_mut().together.as_mut() {
            if r.ready_round == round {
                return;
            }
            r.ready_round = round;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_ready(code, member, round).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(s) => {
                    t.accept_room(s.clone());
                    t.sync_player(&s);
                }
                Err(_) => {
                    if let Some(r) = t.state.borrow_mut().together.as_mut()
                        && r.ready_round == round
                    {
                        r.ready_round = 0;
                    }
                }
            }
        })
        .ok();
    }

    /// Long-polls the room (the seat's heartbeat, lobby and player alike) and applies what
    /// the others do.
    fn follow_room(self: &Rc<Self>) {
        let Some((code, generation)) = self
            .state
            .borrow()
            .together
            .as_ref()
            .map(|r| (r.code.clone(), r.generation))
        else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        let mine = move |t: &Rc<App>| {
            t.state
                .borrow()
                .together
                .as_ref()
                .is_some_and(|r| r.generation == generation)
        };
        slint::spawn_local(async move {
            let mut failures = 0u64;
            loop {
                let Some((member, seq)) = this.upgrade().filter(|t| mine(t)).and_then(|t| {
                    let st = t.state.borrow();
                    st.together.as_ref().map(|r| (r.member.clone(), r.seq))
                }) else {
                    return;
                };
                let r = core.together_wait(code.clone(), member, seq).await;
                let Some(t) = this.upgrade().filter(|t| mine(t)) else {
                    return;
                };
                match r {
                    Ok(s) => {
                        failures = 0;
                        t.on_room_state(s);
                    }
                    Err(CoreError::NotFound) => {
                        // A dropped seat may come back once; 404 again: the room is gone.
                        let tried = t
                            .state
                            .borrow_mut()
                            .together
                            .as_mut()
                            .is_none_or(|r| std::mem::replace(&mut r.rejoined, true));
                        if tried {
                            t.room_closed();
                            return;
                        }
                        drop(t);
                        let j = core.together_join(code.clone()).await;
                        let Some(t) = this.upgrade().filter(|t| mine(t)) else {
                            return;
                        };
                        match j {
                            Ok(j) => {
                                if let Some(r) = t.state.borrow_mut().together.as_mut() {
                                    r.member = j.member_id.clone();
                                    r.rejoined = false;
                                }
                                t.on_room_state(j.state);
                            }
                            Err(_) => {
                                t.room_closed();
                                return;
                            }
                        }
                    }
                    Err(_) => {
                        failures += 1;
                        drop(t);
                        async_sleep(Duration::from_millis((failures * 2000).min(10_000))).await;
                    }
                }
            }
        })
        .ok();
    }

    fn on_room_state(self: &Rc<Self>, s: TogetherState) {
        let (before, member, sending, player_open) = {
            let st = self.state.borrow();
            let Some(r) = st.together.as_ref() else {
                return;
            };
            (r.state.clone(), r.member.clone(), r.sending, r.player_open)
        };
        self.accept_room(s.clone());
        if s.started {
            // Playing again: the note about the waiting room is over.
            self.ui().set_room_info(SharedString::new());
        }
        self.render_room();
        self.render_picker();
        // Each start opens everyone's player (closed meanwhile: "Back to the video").
        if !player_open {
            self.open_if_new_round();
            return;
        }
        // The host went back to the waiting room (to pick something else): so do we, also on
        // the host's other devices (only the seat that sent it is already leaving).
        if !s.started {
            if before.as_ref().is_some_and(|b| b.started)
                && (s.by != member || !together_is_host(s.clone(), member.clone()))
            {
                let host = s
                    .members
                    .iter()
                    .find(|m| m.id == s.host)
                    .map_or_else(|| tr!("The host"), |m| m.name.clone());
                self.to_player(ToPlayer::Quit);
                self.ui().set_room_info(sstr(tr!(
                    "{host} went back to the waiting room.",
                    host = host
                )));
            }
            return;
        }
        let new_round = before.as_ref().is_some_and(|b| b.round != s.round);
        if let Some(r) = self.state.borrow_mut().together.as_mut() {
            r.handled_round = r.handled_round.max(s.round);
        }
        if s.by == member || sending > 0 {
            // Our own title: wait with the others.
            if !s.loading.is_empty() {
                self.sync_player(&s);
            }
            return;
        }
        if new_round || before.as_ref().is_some_and(|b| b.media != s.media) {
            self.follow_title(s);
            return;
        }
        if let Some(b) = &before
            && b.seq != s.seq
        {
            let name = |id: &str| {
                s.members
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.name.clone())
            };
            let note = if let Some(who) = name(&s.by) {
                Some(if b.playing && !s.playing {
                    tr!("{name} paused", name = who)
                } else if !b.playing && s.playing {
                    tr!("{name} resumed", name = who)
                } else {
                    tr!(
                        "{name} moved to {time}",
                        name = who,
                        time = clock(s.position_ms)
                    )
                })
            } else {
                let joined: Vec<&str> = s
                    .members
                    .iter()
                    .filter(|m| !b.members.iter().any(|o| o.id == m.id))
                    .map(|m| m.name.as_str())
                    .collect();
                let left: Vec<&str> = b
                    .members
                    .iter()
                    .filter(|m| !s.members.iter().any(|o| o.id == m.id))
                    .map(|m| m.name.as_str())
                    .collect();
                if !joined.is_empty() {
                    Some(tr!(
                        "{names} joined · {watching}",
                        names = joined.join(", "),
                        watching = watching_note(&s)
                    ))
                } else if !left.is_empty() {
                    Some(tr!(
                        "{names} left · {watching}",
                        names = left.join(", "),
                        watching = watching_note(&s)
                    ))
                } else {
                    None
                }
            };
            if let Some(n) = note {
                self.to_player(ToPlayer::Note(n));
            }
        }
        self.sync_player(&s);
    }

    /// The room moved to another title: find it here, close this player and play that one.
    fn follow_title(self: &Rc<Self>, s: TogetherState) {
        let accounts = self.home_accounts();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let found = core.together_find(accounts, s.media.clone()).await;
            let Some(t) = this.upgrade() else { return };
            match found {
                Ok(Some(f)) => {
                    if let Some(r) = t.state.borrow_mut().together.as_mut() {
                        r.switch_to = Some((f.account_id, f.item_id));
                    }
                    t.to_player(ToPlayer::Quit);
                }
                _ => {
                    t.to_player(ToPlayer::Note(tr!(
                        "{title} isn't on your servers",
                        title = media_title(&s.media)
                    )));
                    t.to_player(ToPlayer::Sync {
                        playing: false,
                        position_ms: 0,
                        at: std::time::Instant::now(),
                        hold: false,
                        load: false,
                        waiting: None,
                    });
                }
            }
        })
        .ok();
    }

    /// The title the room moved to, once the player closed for it.
    pub(super) fn take_switch(&self) -> Option<(String, String, u64)> {
        let mut st = self.state.borrow_mut();
        let r = st.together.as_mut()?;
        let (account, item) = r.switch_to.take()?;
        let s = r.state.clone();
        drop(st);
        let now = self.core.together_now_ms();
        let start = s.map_or(0, |s| together_position_at(s, now));
        Some((account, item, start))
    }

    fn from_player(self: &Rc<Self>, ev: FromPlayer) {
        match ev {
            FromPlayer::User {
                playing,
                position_ms,
            } => self.send_room(playing, position_ms, None),
            FromPlayer::Ready => self.player_ready(),
        }
    }

    /// Tells the room what we just did (with `title`, a new title for everyone).
    fn send_room(
        self: &Rc<Self>,
        playing: bool,
        position_ms: u64,
        title: Option<(String, String)>,
    ) {
        let (code, member) = {
            let mut st = self.state.borrow_mut();
            let Some(r) = st.together.as_mut() else {
                return;
            };
            r.sending += 1;
            (r.code.clone(), r.member.clone())
        };
        let new_title = title.is_some();
        let (account, item) = title.unzip();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .together_update(code, member, playing, position_ms, account, item)
                .await;
            let Some(t) = this.upgrade() else { return };
            let idle = t.state.borrow_mut().together.as_mut().is_some_and(|room| {
                room.sending = room.sending.saturating_sub(1);
                room.sending == 0
            });
            match r {
                Ok(s) => {
                    t.accept_room(s.clone());
                    // The player follows the room as it now is (our change, server-timed),
                    // unless a newer change of ours is still on its way.
                    if !new_title && idle {
                        t.sync_player(&s);
                    }
                }
                // The host doesn't allow it: say so and go back to what the room plays.
                Err(CoreError::Server { status: 423 }) => {
                    let Some(s) = t.room_state() else { return };
                    let note = if new_title {
                        tr!("Only the host can change what's playing in this room")
                    } else if s.playing != playing {
                        if playing {
                            tr!("Only the host can play in this room")
                        } else {
                            tr!("Only the host can pause in this room")
                        }
                    } else {
                        tr!("Only the host can move forward or back in this room")
                    };
                    t.to_player(ToPlayer::Note(note));

                    if new_title {
                        t.follow_title(s);
                    } else {
                        t.sync_player(&s);
                    }
                }
                Err(_) => {}
            }
        })
        .ok();
    }

    /// Leaves the room (Leave, or the room is gone).
    pub(super) fn leave_room(self: &Rc<Self>, note: Option<&str>, tell_server: bool) {
        let Some(room) = self.state.borrow_mut().together.take() else {
            return;
        };
        if let (Some(n), Some(tx)) = (note, room.to_player.as_ref()) {
            let _ = tx.send(ToPlayer::Note(n.to_owned()));
        }
        self.ui().set_room_picker(false);
        self.ui().set_room_hosting(false);
        if tell_server {
            let core = self.core.clone();
            core.runtime().spawn({
                let core = core.clone();
                async move {
                    let _ = core.together_leave(room.code, room.member).await;
                }
            });
        }
    }
}

/// Test hooks (headless screenshots, `ALWYWATCH_TEST_PAGE`): pages with made-up rooms and
/// people, nothing sent anywhere.
impl App {
    fn test_media(series: bool) -> TogetherMedia {
        TogetherMedia {
            kind: if series { "episode" } else { "movie" }.into(),
            title: if series { "The Signal" } else { "Heat" }.into(),
            year: Some(1995),
            tmdb: None,
            imdb: None,
            tvdb: None,
            series_title: series.then(|| "Silo".into()),
            series_tmdb: None,
            series_tvdb: None,
            season: series.then_some(2),
            episode: series.then_some(7),
        }
    }

    /// `room` (the host's lobby), `room-guest` (waiting for the host), `room-picker`.
    pub fn test_room(self: &Rc<Self>, step: &str) {
        let preset = |p: &str| Avatar {
            preset: Some(p.into()),
            custom_version: None,
        };
        let member = |id: &str, name: &str, user: &str, avatar: Avatar| TogetherMember {
            id: id.into(),
            name: name.into(),
            user_key: user.into(),
            avatar,
        };
        let me = if step == "room-guest" { "s1" } else { "a1" };
        let state = TogetherState {
            code: "000000".into(),
            seq: 3,
            media: Self::test_media(true),
            started: false,
            playing: false,
            position_ms: 0,
            at_ms: 0,
            now_ms: 0,
            round: 0,
            loading: vec![],
            ready_by_ms: 0,
            by: String::new(),
            members: vec![
                member("a1", "Alex", "t:alex", preset("p05")),
                member("a2", "Alex", "t:alex", preset("p05")),
                member("s1", "Sam", "t:sam", Avatar::default()),
            ],
            host: "a1".into(),
            invited: vec![TogetherInvitee {
                user_key: "t:kim".into(),
                name: "Kim".into(),
                avatar: preset("p08"),
            }],
            rights: Default::default(),
        };
        self.state.borrow_mut().together = Some(Room {
            code: state.code.clone(),
            member: me.into(),
            seq: state.seq,
            state: Some(state),
            to_player: None,
            sending: 0,
            switch_to: None,
            player_open: false,
            handled_round: 0,
            ready_round: 0,
            rejoined: false,
            generation: u64::MAX,
        });
        self.navigate(Route::Room);
        if step == "player-room" {
            let ui = self.ui();
            ui.set_player_title("Monsters, Inc.".into());
            ui.set_player_dur(5_520_000);
            ui.set_player_pos(1_834_000);
            ui.set_player_paused(true);
            ui.set_player_on(true);
            ui.set_player_room_at(sstr(clock(1_834_000)));
            ui.set_player_room(true);
            self.render_room();
        }
        if step == "room-picker" {
            let person =
                |key: &str, name: &str, media: &str, server: &str, at, avatar| TogetherPerson {
                    user_key: key.into(),
                    name: name.into(),
                    media_name: media.into(),
                    avatar,
                    server_name: server.into(),
                    last_together_at: at,
                };
            self.state.borrow_mut().lists.people = vec![
                person("t:jo", "Jo", "jo", "Plex", Some(2), preset("p12")),
                person("t:sam", "Sam", "sam", "Plex", Some(1), Avatar::default()),
                person("t:max", "Max", "maxime", "Plex", None, preset("p02")),
                person("t:sam2", "Sam", "samuel", "Jellyfin", None, preset("p09")),
                person("t:zoe", "Zoe", "zoe", "Jellyfin", None, Avatar::default()),
            ];
            self.state.borrow_mut().lists.picked = vec!["t:max".into()];
            let ui = self.ui();
            ui.set_picker_filter_text(SharedString::new());
            ui.set_room_picker(true);
            self.render_picker();
        }
    }

    /// The Together page with an invitation (and its banner) and one of my rooms.
    pub fn test_invitation(self: &Rc<Self>) {
        let room =
            |code: &str, host: &str, invited: bool, started: bool, series: bool| TogetherRoom {
                code: code.into(),
                host: host.into(),
                host_avatar: Avatar {
                    preset: invited.then(|| "p03".into()),
                    custom_version: None,
                },
                host_user_key: format!("t:{host}"),
                media: Self::test_media(series),
                members: vec![host.into(), "Kim".into()],
                started,
                playing: started,
                position_ms: 754_000,
                at_ms: 0,
                now_ms: 0,
                mine: !invited,
                hosting: false,
                invited,
            };
        let list = TogetherRooms {
            version: 1,
            rooms: vec![
                room("111111", "Sam", true, false, false),
                room("222222", "Jo", false, true, true),
            ],
        };
        {
            let mut st = self.state.borrow_mut();
            st.lists.rooms = Some(list);
            st.lists.banner = Some("111111".into());
        }
        self.navigate(Route::Together);
        self.render_banner();
    }
}

/// Where the rooms list is cached (rendered instantly on the Together page).
pub(super) const ROOMS_KEY: &str = "together.rooms";

/// Where a listed room is now (its position moved on while playing).
fn room_position(r: &TogetherRoom) -> u64 {
    if r.playing {
        r.position_ms + (r.now_ms - r.at_ms).max(0) as u64
    } else {
        r.position_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, name: &str, user: &str) -> TogetherMember {
        TogetherMember {
            id: id.into(),
            name: name.into(),
            user_key: user.into(),
            avatar: Avatar::default(),
        }
    }

    fn state(members: Vec<TogetherMember>, host: &str) -> TogetherState {
        TogetherState {
            code: "123456".into(),
            seq: 1,
            media: TogetherMedia {
                kind: "movie".into(),
                title: "Heat".into(),
                year: Some(1995),
                tmdb: Some("949".into()),
                imdb: None,
                tvdb: None,
                series_title: None,
                series_tmdb: None,
                series_tvdb: None,
                season: None,
                episode: None,
            },
            started: false,
            playing: false,
            position_ms: 0,
            at_ms: 0,
            now_ms: 0,
            round: 0,
            loading: vec![],
            ready_by_ms: 0,
            by: String::new(),
            members,
            host: host.into(),
            invited: vec![],
            rights: Default::default(),
        }
    }

    #[test]
    fn members_are_grouped_by_user() {
        let s = state(
            vec![
                member("a1", "Alex", "u:alex"),
                member("a2", "Alex", "u:alex"),
                member("s1", "Sam", "u:sam"),
            ],
            "a2",
        );
        let g = group_members(&s, "s1");
        assert_eq!(g.len(), 2);
        assert_eq!(
            (g[0].name.as_str(), g[0].devices, g[0].host, g[0].me),
            ("Alex", 2, true, false)
        );
        assert_eq!(
            (g[1].name.as_str(), g[1].devices, g[1].host, g[1].me),
            ("Sam", 1, false, true)
        );
        assert_eq!(seat_detail(&g[0]), "Host · 2 devices");
        assert_eq!(seat_detail(&g[1]), "You");
        assert_eq!(watching_note(&s), "2 watching: Alex, Sam");
    }

    fn person(key: &str, name: &str, media: &str, server: &str, at: Option<i64>) -> TogetherPerson {
        TogetherPerson {
            user_key: key.into(),
            name: name.into(),
            media_name: media.into(),
            avatar: Avatar::default(),
            server_name: server.into(),
            last_together_at: at,
        }
    }

    #[test]
    fn people_keep_the_order_with_headers_and_labels() {
        let people = vec![
            person("k1", "Kim", "kim", "Plex", Some(200)),
            person("s1", "Sam", "sam", "Plex", Some(100)),
            person("a1", "Alex", "alexandre", "Plex", None),
            person("s2", "Sam", "samuel", "Jellyfin", None),
            person("z1", "Zoe", "Zoe", "Plex", None),
        ];
        let rows = people_rows(&people, &HashSet::new(), "");
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Kim", "Sam", "Alex", "Sam", "Zoe"]);
        let headers: Vec<&str> = rows.iter().map(|r| r.header).collect();
        assert_eq!(headers, ["RECENT", "", "EVERYONE", "", ""]);
        // Same display name twice: the server tells them apart.
        assert_eq!(rows[1].detail, "Plex");
        assert_eq!(rows[3].detail, "samuel · Jellyfin");
        // Media-server name shown when it differs (case aside).
        assert_eq!(rows[2].detail, "alexandre");
        assert_eq!(rows[0].detail, "");
        assert_eq!(rows[4].detail, "");
    }

    #[test]
    fn people_hide_room_members_and_filter() {
        let people = vec![
            person("k1", "Kim", "", "Plex", Some(200)),
            person("a1", "Alex", "", "Plex", None),
            person("b1", "Bob", "alexis", "Plex", None),
        ];
        let hide: HashSet<String> = ["k1".to_owned()].into();
        let rows = people_rows(&people, &hide, "");
        // No recent partner left: no headers at all.
        assert_eq!(rows.iter().map(|r| r.header).collect::<Vec<_>>(), ["", ""]);
        let rows = people_rows(&people, &HashSet::new(), " ALEX");
        let keys: Vec<&str> = rows.iter().map(|r| r.user_key.as_str()).collect();
        assert_eq!(keys, ["a1", "b1"], "by display or media-server name");
    }

    #[test]
    fn plain_errors() {
        assert!(room_error(&CoreError::Server { status: 409 }).contains("full"));
        assert!(room_error(&CoreError::Server { status: 429 }).contains("few minutes"));
        assert_eq!(room_error(&CoreError::NotFound), "This room is gone.");
    }
}
