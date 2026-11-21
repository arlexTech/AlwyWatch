// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together on the desktop: the rooms open on the AlwyWatch server, asking a host to join,
//! and the room a player follows (same protocol as Android, see docs/keep-protocol.md).
//!
//! The player (mpv, own thread) gets the room's play/pause/seek through a channel and sends
//! back what the viewer does; the host answers join requests on the video with Y or N.

use std::sync::mpsc;

use super::*;
use crate::TogetherRow;
use crate::player::{FromPlayer, ToPlayer, TogetherLink};

/// How a player starts with regard to watch together.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TogetherPlay {
    Alone,
    /// Open a room with this title ("Watch together" on its page).
    Create,
    /// In the current room; `announce`: this title was chosen here (next episode), tell the room.
    Room {
        announce: bool,
    },
}

/// The room this app follows (one at a time).
pub(super) struct Room {
    code: String,
    member: String,
    seq: u64,
    state: Option<TogetherState>,
    received_ms: i64,
    to_player: Option<mpsc::Sender<ToPlayer>>,
    /// Join requests answered here (until the server's next state drops them).
    answered: HashSet<String>,
    /// Our updates on their way: room answers in between are older than them.
    sending: u32,
    /// The room moved to this title (account, item): play it once the current player closes.
    switch_to: Option<(String, String)>,
}

/// Local clock in ms, only compared with itself (to move the room's position on).
fn mono_ms() -> i64 {
    thread_local! {
        static START: std::time::Instant = std::time::Instant::now();
    }
    START.with(|s| s.elapsed().as_millis() as i64)
}

/// "Silo S2 · E7" or the movie title.
fn media_title(m: &TogetherMedia) -> String {
    match (&m.series_title, m.season, m.episode) {
        (Some(s), Some(se), Some(ep)) => format!("{s} S{se} · E{ep}"),
        _ => m.title.clone(),
    }
}

impl App {
    // ----- the rooms list ----------------------------------------------------------------

    /// Lists the open rooms; refreshed every 15 s while the page is shown.
    pub(super) fn load_together(self: &Rc<Self>) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.together_rooms().await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(rooms) => t.render_rooms(rooms),
                Err(_) => {
                    t.ui()
                        .set_together_status("Can't reach your AlwyWatch server right now.".into());
                }
            }
        })
        .ok();
        let w = Rc::downgrade(self);
        self.together_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(15),
            move || {
                if let Some(t) = w.upgrade() {
                    if t.ui().get_page() == Page::Together {
                        t.load_together();
                    } else {
                        t.together_timer.stop();
                    }
                }
            },
        );
    }

    fn render_rooms(self: &Rc<Self>, rooms: Vec<TogetherRoom>) {
        let ui = self.ui();
        ui.set_together_status(sstr(if rooms.is_empty() {
            "Nobody is watching together right now. To start, open a movie or an episode and \
             choose Watch together: people on your AlwyWatch server will see it here."
        } else {
            ""
        }));
        let model = Rc::new(VecModel::from(
            rooms
                .iter()
                .map(|r| {
                    let others = r.members.len().saturating_sub(1);
                    let at = clock(together_position_of_room(r));
                    TogetherRow {
                        code: sstr(&r.code),
                        title: sstr(media_title(&r.media)),
                        people: sstr(match others {
                            0 => r.host.clone(),
                            1 => format!("{} and 1 other", r.host),
                            n => format!("{} and {n} others", r.host),
                        }),
                        status: sstr(format!(
                            "{} · {at}{}",
                            if r.playing { "Playing" } else { "Paused" },
                            if r.mine {
                                " · you're in it on another device"
                            } else {
                                ""
                            }
                        )),
                        poster: Image::default(),
                        mine: r.mine,
                    }
                })
                .collect::<Vec<_>>(),
        ));
        ui.set_together_rooms(ModelRc::from(model.clone()));
        self.state.borrow_mut().rooms_seen = rooms.clone();
        // Posters from the viewer's own servers.
        let accounts = self.home_accounts();
        for (row, r) in rooms.into_iter().enumerate() {
            let (core, this, m, accounts) = (
                self.core.clone(),
                Rc::downgrade(self),
                Rc::downgrade(&model),
                accounts.clone(),
            );
            slint::spawn_local(async move {
                let Ok(Some(found)) = core.together_find(accounts, r.media).await else {
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
                t.load_image(&item, ImageKind::Primary, POSTER_PX, move |_, img| {
                    if let Some(m) = m.upgrade()
                        && let Some(mut row_data) = m.row_data(row)
                    {
                        row_data.poster = img;
                        m.set_row_data(row, row_data);
                    }
                });
            })
            .ok();
        }
    }

    // ----- joining -------------------------------------------------------------------------

    /// Finds the room's title on our servers, asks its host, and plays once let in.
    pub(super) fn together_join(self: &Rc<Self>, code: String) {
        let Some(room) = self
            .state
            .borrow()
            .rooms_seen
            .iter()
            .find(|r| r.code == code)
            .cloned()
        else {
            return;
        };
        let join = {
            let mut st = self.state.borrow_mut();
            st.join_gen += 1;
            st.join_gen
        };
        let ui = self.ui();
        ui.set_join_title(sstr(media_title(&room.media)));
        ui.set_join_message("Finding it on your servers…".into());
        ui.set_join_done(false);
        let accounts = self.home_accounts();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let live = |this: &std::rc::Weak<App>| {
                this.upgrade().filter(|t| t.state.borrow().join_gen == join)
            };
            let say = |t: &Rc<App>, msg: String, done: bool| {
                t.ui().set_join_message(sstr(msg));
                t.ui().set_join_done(done);
            };
            let found = core.together_find(accounts, room.media.clone()).await;
            let Some(t) = live(&this) else { return };
            let found = match found {
                Ok(Some(f)) => f,
                Ok(None) => {
                    return say(
                        &t,
                        format!("{} isn't on your servers.", media_title(&room.media)),
                        true,
                    );
                }
                Err(e) => return say(&t, e.to_string(), true),
            };
            say(&t, format!("Asking {} to let you in…", room.host), false);
            drop(t);
            let mut knock = match core.together_knock(code.clone()).await {
                Ok(k) => k,
                Err(e) => {
                    if let Some(t) = live(&this) {
                        say(&t, e.to_string(), true);
                    }
                    return;
                }
            };
            if let Some(t) = live(&this) {
                t.state.borrow_mut().knock = Some((code.clone(), knock.request_id.clone()));
            }
            let mut failures = 0;
            let joined = loop {
                if live(&this).is_none() {
                    return;
                }
                match knock.status {
                    KnockStatus::Accepted => break knock.joined.take(),
                    KnockStatus::Declined => {
                        if let Some(t) = live(&this) {
                            t.state.borrow_mut().knock = None;
                            say(
                                &t,
                                format!("{} can't let you in right now.", room.host),
                                true,
                            );
                        }
                        return;
                    }
                    KnockStatus::Waiting => {}
                }
                match core
                    .together_knock_wait(code.clone(), knock.request_id.clone())
                    .await
                {
                    Ok(k) => {
                        failures = 0;
                        knock = k;
                    }
                    Err(e) => {
                        failures += 1;
                        if failures > 3 || e == CoreError::NotFound {
                            if let Some(t) = live(&this) {
                                t.state.borrow_mut().knock = None;
                                say(&t, "The room closed meanwhile.".into(), true);
                            }
                            return;
                        }
                        async_sleep(Duration::from_secs(2)).await;
                    }
                }
            };
            let Some(t) = live(&this) else { return };
            t.state.borrow_mut().knock = None;
            let Some(j) = joined else {
                return say(&t, "The room closed meanwhile.".into(), true);
            };
            t.ui().set_join_message(SharedString::new());
            let start = together_position(j.state.clone(), mono_ms(), mono_ms());
            t.enter_room(j);
            t.play_with(
                found.account_id,
                found.item_id,
                Some(start),
                TogetherPlay::Room { announce: false },
            );
        })
        .ok();
    }

    /// Closes the join dialog; a request still waiting is withdrawn.
    pub(super) fn together_cancel(self: &Rc<Self>) {
        let knock = {
            let mut st = self.state.borrow_mut();
            st.join_gen += 1;
            st.knock.take()
        };
        self.ui().set_join_message(SharedString::new());
        if let Some((code, request)) = knock {
            let core = self.core.clone();
            core.runtime().spawn({
                let core = core.clone();
                async move {
                    let _ = core.together_knock_cancel(code, request).await;
                }
            });
        }
    }

    // ----- the room a player follows -------------------------------------------------------

    fn enter_room(self: &Rc<Self>, j: TogetherJoined) {
        self.state.borrow_mut().together = Some(Room {
            code: j.state.code.clone(),
            member: j.member_id,
            seq: 0,
            state: None,
            received_ms: 0,
            to_player: None,
            answered: HashSet::new(),
            sending: 0,
            switch_to: None,
        });
        self.accept_room(j.state);
        self.follow_room();
    }

    pub(super) fn in_room(&self) -> bool {
        self.state.borrow().together.is_some()
    }

    /// Opens a room for a title about to play here.
    pub(super) async fn create_room(
        this: &std::rc::Weak<App>,
        core: &Arc<AlwyWatch>,
        account: String,
        id: String,
        start_ms: u64,
    ) {
        let r = core.together_create(account, id, start_ms, true).await;
        let Some(t) = this.upgrade() else { return };
        match r {
            Ok(j) => t.enter_room(j),
            Err(e) => t
                .ui()
                .set_error(sstr(format!("Watch together unavailable: {e}"))),
        }
    }

    /// The channel a new player in the room listens to (replaces the previous player's).
    pub(super) fn player_link(self: &Rc<Self>) -> Option<TogetherLink> {
        let (tx, rx) = mpsc::channel();
        let mut st = self.state.borrow_mut();
        let room = st.together.as_mut()?;
        room.to_player = Some(tx);
        Some(TogetherLink {
            rx,
            send: Box::new(|ev| {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                        t.from_player(ev);
                    }
                });
            }),
        })
    }

    /// Right after a player started in the room: say the room is open, tell the room about our
    /// new title, or follow the room.
    pub(super) fn player_started(
        self: &Rc<Self>,
        account: String,
        id: String,
        play: TogetherPlay,
        start_ms: u64,
    ) {
        match play {
            TogetherPlay::Create => self.to_player(ToPlayer::Note(
                "Watch together is open: friends can join from Together".into(),
            )),
            TogetherPlay::Room { announce: true } => {
                self.send_room(true, start_ms, Some((account, id)))
            }
            _ => {
                if let Some(s) = self.room_state() {
                    self.sync_player(&s);
                }
            }
        }
        self.show_request();
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
        room.state = Some(s);
        room.received_ms = mono_ms();
    }

    /// The host sees the first join request not answered yet; others see none.
    fn show_request(&self) {
        let req = {
            let st = self.state.borrow();
            let Some(room) = st.together.as_ref() else {
                return;
            };
            room.state.as_ref().and_then(|s| {
                (s.host == room.member)
                    .then(|| {
                        s.requests
                            .iter()
                            .find(|r| !room.answered.contains(&r.id))
                            .map(|r| (r.id.clone(), r.name.clone()))
                    })
                    .flatten()
            })
        };
        self.to_player(ToPlayer::Request(req));
    }

    fn sync_player(&self, s: &TogetherState) {
        let received = self
            .state
            .borrow()
            .together
            .as_ref()
            .map_or(0, |r| r.received_ms);
        self.to_player(ToPlayer::Sync {
            playing: s.playing,
            position_ms: together_position(s.clone(), received, mono_ms()),
        });
    }

    /// Long-polls the room and applies what the others do.
    fn follow_room(self: &Rc<Self>) {
        let Some(code) = self
            .state
            .borrow()
            .together
            .as_ref()
            .map(|r| r.code.clone())
        else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let mut failures = 0u64;
            loop {
                let Some((member, seq)) = this.upgrade().and_then(|t| {
                    let st = t.state.borrow();
                    st.together
                        .as_ref()
                        .filter(|r| r.code == code)
                        .map(|r| (r.member.clone(), r.seq))
                }) else {
                    return;
                };
                let r = core.together_wait(code.clone(), member, seq).await;
                let Some(t) = this.upgrade() else { return };
                if t.state.borrow().together.as_ref().map(|r| &r.code) != Some(&code) {
                    return;
                }
                match r {
                    Ok(s) => {
                        failures = 0;
                        t.on_room_state(s);
                    }
                    Err(CoreError::NotFound) => {
                        t.leave_room(Some("The watch together room closed"), false);
                        return;
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
        let (before, member, sending) = {
            let st = self.state.borrow();
            let Some(r) = st.together.as_ref() else {
                return;
            };
            (r.state.clone(), r.member.clone(), r.sending)
        };
        self.accept_room(s.clone());
        self.show_request();
        if s.by == member || sending > 0 {
            return;
        }
        if let Some(b) = &before
            && b.media != s.media
        {
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
                    format!("{who} paused")
                } else if !b.playing && s.playing {
                    format!("{who} resumed")
                } else {
                    format!("{who} moved to {}", clock(s.position_ms))
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
                    Some(format!("{} joined", joined.join(", ")))
                } else if !left.is_empty() {
                    Some(format!("{} left", left.join(", ")))
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
                    t.to_player(ToPlayer::Note(format!(
                        "{} isn't on your servers",
                        media_title(&s.media)
                    )));
                    t.to_player(ToPlayer::Sync {
                        playing: false,
                        position_ms: 0,
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
        let start = r.state.as_ref().map_or(0, |s| {
            together_position(s.clone(), r.received_ms, mono_ms())
        });
        Some((account, item, start))
    }

    fn from_player(self: &Rc<Self>, ev: FromPlayer) {
        match ev {
            FromPlayer::User {
                playing,
                position_ms,
            } => self.send_room(playing, position_ms, None),
            FromPlayer::Answer { request_id, accept } => {
                let (code, member, name) = {
                    let mut st = self.state.borrow_mut();
                    let Some(r) = st.together.as_mut() else {
                        return;
                    };
                    r.answered.insert(request_id.clone());
                    let name = r
                        .state
                        .as_ref()
                        .and_then(|s| s.requests.iter().find(|q| q.id == request_id))
                        .map(|q| q.name.clone());
                    (r.code.clone(), r.member.clone(), name)
                };
                self.show_request();
                let (core, this) = (self.core.clone(), Rc::downgrade(self));
                slint::spawn_local(async move {
                    // Gone meanwhile (they cancelled): nothing to do.
                    if let Ok(s) = core.together_answer(code, member, request_id, accept).await
                        && let Some(t) = this.upgrade()
                    {
                        // The answer already holds the new member: no later change says so.
                        if let (true, Some(n)) = (accept, name) {
                            t.to_player(ToPlayer::Note(format!("{n} joined")));
                        }
                        t.accept_room(s);
                        t.show_request();
                    }
                })
                .ok();
            }
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
        let (account, item) = title.unzip();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .together_update(code, member, playing, position_ms, account, item)
                .await;
            let Some(t) = this.upgrade() else { return };
            if let Some(room) = t.state.borrow_mut().together.as_mut() {
                room.sending = room.sending.saturating_sub(1);
            }
            if let Ok(s) = r {
                t.accept_room(s);
            }
        })
        .ok();
    }

    /// Leaves the room (the player closed, or the room is gone).
    pub(super) fn leave_room(self: &Rc<Self>, note: Option<&str>, tell_server: bool) {
        let Some(room) = self.state.borrow_mut().together.take() else {
            return;
        };
        if let (Some(n), Some(tx)) = (note, room.to_player.as_ref()) {
            let _ = tx.send(ToPlayer::Note(n.to_owned()));
        }
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

/// Where a listed room is now (its position moved on while playing).
fn together_position_of_room(r: &TogetherRoom) -> u64 {
    if r.playing {
        r.position_ms + (r.now_ms - r.at_ms).max(0) as u64
    } else {
        r.position_ms
    }
}
