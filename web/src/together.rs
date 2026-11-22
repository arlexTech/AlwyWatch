// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together: private rooms on the AlwyWatch server. "Watch together" on a title opens a
//! lobby (never the player); its host invites people from a list (recent partners first), the
//! invited join directly (the invitation is the approval) and the host starts playback for
//! everyone. Nobody else can see a room; there is no code to type or show.
//!
//! The seat this tab holds in a room lives at the app level ([`Room`], in the app state): the
//! lobby and the player share it, and its long poll (the seat's heartbeat) keeps running while
//! moving between them. One more long poll ([`Invites`]) keeps my rooms and invitations
//! current while an account is linked, for the invitation banner.
//!
//! Same sync rules as the apps: each member plays their own server's copy (found by provider
//! ids), applies what others do, stays in step on the server's clock (measured from here, see
//! [`Room::server_now`]), and tells the room about their own play, pause, seek and title
//! changes (a title chosen here is announced; one chosen by the room is followed). Each start
//! waits until every player has loaded the title, then plays at the same instant everywhere.
//! The host leaving the player takes the room back to the waiting room: everyone stays in it,
//! and each start opens the player wherever the app is.

use keep_service::together::{
    ClockSync, TogetherCreate, TogetherInvite, TogetherJoined, TogetherMedia, TogetherMember,
    TogetherPerson, TogetherRoom, TogetherRooms, TogetherState, TogetherUpdate,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;
use leptos_router::components::A;
use leptos_router::hooks::{use_location, use_navigate, use_params_map};
use media_api::{AnyServer, ImageKind, Item, ItemKind, MediaServer, ProviderIds};
use net::Error;
use serde::{Deserialize, Serialize};

use crate::profile::Avatar;
use crate::state::{self, fmt_time};
use crate::ui::{ErrorText, Loading};

/// Further apart than this from the room, a member seeks.
pub const DRIFT_MS: u64 = 1500;

const NO_KEEP: &str =
    "Watch together needs an account on this AlwyWatch server: sign in with one in Settings.";

pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// How the room stands for a player now (see [`Room::phase`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// Waiting for players to load the title (`me`: this one too); the others still loading.
    Loading { me: bool, others: Vec<String> },
    /// Everyone loaded: it plays in this many ms, for everyone at once.
    StartsIn(i64),
    /// Playing or paused as usual.
    Live,
}

/// The phase of `s` for seat `me` at `server_now`.
pub fn phase_of(s: &TogetherState, me: &str, server_now: i64) -> Phase {
    if !s.loading.is_empty() {
        let mut others: Vec<String> = s
            .loading
            .iter()
            .filter(|id| *id != me)
            .filter_map(|id| s.members.iter().find(|m| &m.id == id))
            .map(|m| m.name.clone())
            .collect();
        others.sort();
        others.dedup();
        return Phase::Loading {
            me: s.is_loading(me),
            others,
        };
    }
    match s.starts_in(server_now) {
        Some(ms) => Phase::StartsIn(ms),
        None => Phase::Live,
    }
}

/// What a player shows while the room waits (None: it doesn't).
pub fn waiting_text(p: &Phase, ready_sent: bool) -> Option<String> {
    match p {
        Phase::Loading { me: true, .. } if !ready_sent => Some("Loading for everyone…".into()),
        Phase::Loading { others, .. } if !others.is_empty() => {
            Some(format!("Waiting for {}…", others.join(", ")))
        }
        Phase::Loading { .. } | Phase::StartsIn(_) => Some("Starting…".into()),
        Phase::Live => None,
    }
}

pub fn drifted(local_ms: u64, expected_ms: u64) -> bool {
    local_ms.abs_diff(expected_ms) > DRIFT_MS
}

/// Whether what the local player does now differs from the room (then it's the viewer's own
/// action, to send): the room's own changes, once applied, match it.
pub fn local_change(playing: bool, position_ms: u64, room_playing: bool, expected_ms: u64) -> bool {
    playing != room_playing || drifted(position_ms, expected_ms)
}

/// What to do with a room state from the server.
#[derive(Debug, PartialEq)]
pub enum Remote {
    /// Our own change (or ours are still on their way, so this one is older).
    Own,
    /// Someone picked another title: find it here and play it.
    NewTitle,
    /// Follow the room (play, pause, seek), with what happened to say.
    Follow(Option<String>),
}

pub fn on_remote(
    before: Option<&TogetherState>,
    s: &TogetherState,
    me: &str,
    sending: bool,
    known: Option<&TogetherMedia>,
) -> Remote {
    if s.by == me || sending {
        return Remote::Own;
    }
    let restarted = before.is_some_and(|b| b.started && s.started && b.round != s.round);
    if known.is_some_and(|k| *k != s.media) || restarted {
        return Remote::NewTitle;
    }
    let Some(b) = before.filter(|b| b.seq != s.seq) else {
        return Remote::Follow(None);
    };
    let names = |of: &[TogetherMember], without: &[TogetherMember]| {
        of.iter()
            .filter(|m| without.iter().all(|w| w.id != m.id))
            .map(|m| m.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let note = if let Some(who) = s.members.iter().find(|m| m.id == s.by) {
        let who = &who.name;
        Some(if b.started && !s.started {
            format!("{who} went back to the waiting room")
        } else if !b.started && s.started {
            format!("{who} started")
        } else if b.playing && !s.playing {
            format!("{who} paused")
        } else if !b.playing && s.playing {
            format!("{who} resumed")
        } else {
            format!("{who} moved to {}", fmt_time(s.position_ms))
        })
    } else if s.members.len() > b.members.len() {
        Some(format!("{} joined", names(&s.members, &b.members)))
    } else if s.members.len() < b.members.len() {
        Some(format!("{} left", names(&b.members, &s.members)))
    } else {
        None
    };
    Remote::Follow(note)
}

pub fn media_label(m: &TogetherMedia) -> String {
    match &m.series_title {
        Some(show) => format!(
            "{show} · S{} · E{}",
            m.season.unwrap_or(1),
            m.episode.unwrap_or(1)
        ),
        None => m.title.clone(),
    }
}

pub fn same_title(a: &str, b: &str) -> bool {
    let n = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    !a.is_empty() && n(a) == n(b)
}

fn ids_match(
    p: &ProviderIds,
    tmdb: &Option<String>,
    imdb: &Option<String>,
    tvdb: &Option<String>,
) -> bool {
    let eq = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
    eq(&p.tmdb, tmdb) || eq(&p.imdb, imdb) || eq(&p.tvdb, tvdb)
}

/// A title as every server can find it (an episode carries its show's ids).
pub async fn media_of(srv: &AnyServer, item: &Item) -> TogetherMedia {
    let episode = item.kind == ItemKind::Episode;
    let series = match (&item.series_id, episode) {
        (Some(id), true) => srv.item(id).await.ok(),
        _ => None,
    };
    let p = &item.provider_ids;
    TogetherMedia {
        kind: if episode { "episode" } else { "movie" }.into(),
        title: item.title.clone(),
        year: item.year,
        tmdb: p.tmdb.clone(),
        imdb: p.imdb.clone(),
        tvdb: p.tvdb.clone(),
        series_title: item.series_title.clone(),
        series_tmdb: series.as_ref().and_then(|s| s.provider_ids.tmdb.clone()),
        series_tvdb: series.as_ref().and_then(|s| s.provider_ids.tvdb.clone()),
        season: item.season_number,
        episode: item.episode_number,
    }
}

/// The room's title on one server: by provider ids, else by title (and year).
async fn find_on(srv: &AnyServer, m: &TogetherMedia) -> Option<String> {
    if m.kind == "episode" {
        let show_name = m.series_title.clone().unwrap_or_default();
        let shows = srv.search(&show_name, 20).await.ok()?;
        let series = || shows.iter().filter(|i| i.kind == ItemKind::Series);
        let show = series()
            .find(|i| ids_match(&i.provider_ids, &m.series_tmdb, &None, &m.series_tvdb))
            .or_else(|| series().find(|i| same_title(&i.title, &show_name)));
        // Search can answer with episodes standing for their show.
        let show_id = show.map(|s| s.id.clone()).or_else(|| {
            shows
                .iter()
                .find(|i| {
                    i.kind == ItemKind::Episode
                        && same_title(i.series_title.as_deref().unwrap_or(""), &show_name)
                })
                .and_then(|i| i.series_id.clone())
        })?;
        let seasons = srv.seasons(&show_id).await.ok()?;
        let season = seasons.iter().find(|s| s.season_number == m.season)?;
        let episodes = srv.episodes(&show_id, &season.id).await.ok()?;
        return episodes
            .iter()
            .find(|e| e.episode_number == m.episode)
            .map(|e| e.id.clone());
    }
    let found = srv.search(&m.title, 20).await.ok()?;
    let movies = || found.iter().filter(|i| i.kind == ItemKind::Movie);
    movies()
        .find(|i| ids_match(&i.provider_ids, &m.tmdb, &m.imdb, &m.tvdb))
        .or_else(|| {
            movies().find(|i| {
                same_title(&i.title, &m.title)
                    && (m.year.is_none() || i.year.is_none() || i.year == m.year)
            })
        })
        .map(|i| i.id.clone())
}

/// The room's title on the user's servers (the AlwyWatch account's first): account, item.
pub async fn find(m: &TogetherMedia) -> Option<(String, String)> {
    let c = state::ctx();
    let linked = c.keep.get_untracked().map(|k| k.account);
    let mut servers = c.servers();
    servers.sort_by_key(|(acc, _)| Some(acc) != linked.as_ref());
    for (acc, srv) in servers {
        if let Some(id) = find_on(&srv, m).await {
            return Some((acc, id));
        }
    }
    None
}

/// The URL of the player in a room (the seat itself is the app's: see [`Room`]).
pub fn play_href(acc: &str, item: &str, start_ms: u64, code: &str) -> String {
    format!(
        "/play/{acc}/{item}?t={start_ms}&room={}",
        net::percent_encode(code)
    )
}

pub fn room_href(code: &str) -> String {
    format!("/together/{}", net::percent_encode(code))
}

/// Errors of room calls in plain words.
pub fn plain_together(e: &Error) -> String {
    match e {
        Error::NotFound => "This room has closed, or the invitation was withdrawn.".into(),
        Error::Status { status: 409 } => "This room is full.".into(),
        Error::Status { status: 429 } => {
            "Too many rooms or invitations in a short time. Try again in a few minutes.".into()
        }
        Error::Status { status: 503 } => {
            "The AlwyWatch server has too many rooms open right now. Try again later.".into()
        }
        other => state::plain_keep(other),
    }
}

/// What a guest is told when the host's rights refuse their change.
pub fn refused_note(u: &TogetherUpdate, room: Option<&TogetherState>) -> String {
    let what = match room {
        _ if u.media.is_some() => "change what's playing",
        Some(r) if r.playing != u.playing => {
            if u.playing {
                "play"
            } else {
                "pause"
            }
        }
        _ => "move forward or back",
    };
    format!("Only the host can {what} in this room")
}

/// One person in a room: their first seat, how many devices they're on, whether they host.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub member: TogetherMember,
    pub devices: usize,
    pub host: bool,
}

/// The members as people: a user on two devices is shown once, with a device count.
pub fn group_members(s: &TogetherState) -> Vec<Person> {
    let who = |m: &TogetherMember| {
        if m.user_key.is_empty() {
            m.id.clone()
        } else {
            m.user_key.clone()
        }
    };
    let mut out: Vec<(String, Person)> = Vec::new();
    for m in &s.members {
        let key = who(m);
        let host = m.id == s.host;
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, p)) => {
                p.devices += 1;
                p.host |= host;
            }
            None => out.push((
                key,
                Person {
                    member: m.clone(),
                    devices: 1,
                    host,
                },
            )),
        }
    }
    out.into_iter().map(|(_, p)| p).collect()
}

/// Invitations in `after` that weren't in `before` (compared by code), minus the rooms just
/// declined or left.
pub fn new_invitations(
    before: &[TogetherRoom],
    after: &[TogetherRoom],
    suppressed: &[String],
) -> Vec<TogetherRoom> {
    after
        .iter()
        .filter(|r| r.invited)
        .filter(|r| !before.iter().any(|b| b.code == r.code && b.invited))
        .filter(|r| !suppressed.contains(&r.code))
        .cloned()
        .collect()
}

/// Two people (or more) with this name: show their server too.
pub fn name_taken(name: &str, all: &[TogetherPerson]) -> bool {
    let n = name.to_lowercase();
    all.iter().filter(|p| p.name.to_lowercase() == n).count() > 1
}

/// Who can still be invited (not in the room nor invited), matching `filter` (name, media
/// name or server): recent partners, then everyone else, in the server's order.
pub fn people_to_invite<'a>(
    all: &'a [TogetherPerson],
    s: &TogetherState,
    filter: &str,
) -> (Vec<&'a TogetherPerson>, Vec<&'a TogetherPerson>) {
    let f = filter.trim().to_lowercase();
    let shown = all.iter().filter(|p| {
        !s.members.iter().any(|m| m.user_key == p.user_key)
            && !s.invited.iter().any(|i| i.user_key == p.user_key)
            && (f.is_empty()
                || [&p.name, &p.media_name, &p.server_name]
                    .iter()
                    .any(|t| t.to_lowercase().contains(&f)))
    });
    shown.partition(|p| p.last_together_at.is_some())
}

/// Where this tab's seat is kept, so a reload keeps it.
const SEAT: &str = "aw.room";

#[derive(Serialize, Deserialize)]
struct Seat {
    code: String,
    member: String,
}

/// The room this tab has a seat in. One per app: the lobby and the player share it, so moving
/// between them keeps the seat (its long poll is the seat's heartbeat).
#[derive(Clone, Copy)]
pub struct Room {
    pub code: RwSignal<Option<String>>,
    pub member: RwSignal<Option<String>>,
    pub state: RwSignal<Option<TogetherState>>,
    /// What just happened ("Ann paused"), shown for a few seconds.
    pub note: RwSignal<Option<String>>,
    /// A room that closed under us (its code), for its page to say so.
    pub closed: RwSignal<Option<String>>,
    /// Others' changes for the player to apply: (new title, state).
    pub incoming: RwSignal<Option<(bool, TogetherState)>>,
    /// The title chosen here (next episode…): tell the room once it starts.
    pub announce: StoredValue<bool>,
    /// The room waits before playing ("Waiting for Sam…"), for the player to show.
    pub waiting: RwSignal<Option<String>>,
    /// The last start (`round`) the player opened for (or that this tab chose to skip): every
    /// later one opens it again.
    pub handled_round: StoredValue<u64>,
    /// The start we said we loaded.
    ready_round: StoredValue<u64>,
    /// The server's clock seen from here, and when it was measured (local ms).
    clock: StoredValue<(ClockSync, f64)>,
    seq: StoredValue<u64>,
    media: StoredValue<Option<TogetherMedia>>,
    /// Our updates on their way: room answers in between are older than them.
    sending: StoredValue<u32>,
    /// Bumped on every join/leave, so an old long-poll loop stops.
    generation: StoredValue<u32>,
}

impl Room {
    pub fn new() -> Self {
        Self {
            code: RwSignal::new(None),
            member: RwSignal::new(None),
            state: RwSignal::new(None),
            note: RwSignal::new(None),
            closed: RwSignal::new(None),
            incoming: RwSignal::new(None),
            announce: StoredValue::new(false),
            waiting: RwSignal::new(None),
            handled_round: StoredValue::new(0),
            ready_round: StoredValue::new(0),
            clock: StoredValue::new((ClockSync::default(), f64::NEG_INFINITY)),
            seq: StoredValue::new(0),
            media: StoredValue::new(None),
            sending: StoredValue::new(0),
            generation: StoredValue::new(0),
        }
    }

    /// In a room.
    pub fn active(&self) -> bool {
        self.code.try_get_untracked().flatten().is_some()
    }

    /// In this room.
    pub fn is(&self, code: &str) -> bool {
        self.code
            .try_get_untracked()
            .flatten()
            .is_some_and(|c| c == code)
    }

    /// Our own update is on its way (a title announcement, say).
    pub fn busy(&self) -> bool {
        self.sending.try_get_value().unwrap_or(0) > 0
    }

    /// This tab's seat: (room code, member id).
    pub fn ids(&self) -> Option<(String, String)> {
        Some((
            self.code.try_get_untracked().flatten()?,
            self.member.try_get_untracked().flatten()?,
        ))
    }

    /// This seat has the host's rights (not tracked).
    pub fn hosting_untracked(&self) -> bool {
        let me = self.member.get_untracked();
        self.state
            .with_untracked(|s| s.as_ref().zip(me).is_some_and(|(s, m)| s.is_host(&m)))
    }

    /// This seat has the host's rights (reactive).
    pub fn hosting(&self) -> bool {
        let me = self.member.get();
        self.state
            .with(|s| s.as_ref().zip(me).is_some_and(|(s, m)| s.is_host(&m)))
    }

    fn accept(&self, s: TogetherState) {
        if s.seq > self.seq.get_value() {
            self.seq.set_value(s.seq);
        }
        self.state.set(Some(s));
    }

    /// The server's clock now (Unix ms): measured from here, else this device's.
    pub fn server_now(&self) -> i64 {
        let local = now_ms() as i64;
        self.clock
            .try_with_value(|(c, _)| c.server_now(local))
            .flatten()
            .unwrap_or(local)
    }

    /// Measures the server clock when it never was or a couple of minutes ago.
    async fn sync_clock(&self, k: &keep_service::KeepClient) {
        let fresh = self
            .clock
            .try_with_value(|(_, at)| now_ms() - at < 120_000.0)
            .unwrap_or(true);
        if fresh {
            return;
        }
        let mut sync = ClockSync::default();
        for _ in 0..5 {
            let sent = now_ms() as i64;
            if let Ok(c) = k.together_clock().await {
                sync.add(sent, c.now_ms, now_ms() as i64);
            }
        }
        if sync.offset().is_some() {
            let _ = self.clock.try_set_value((sync, now_ms()));
        }
    }

    /// Where the room's playback is now, and whether it plays (not while it waits for
    /// players to load, nor before a start everyone heard).
    pub fn expected(&self) -> Option<(u64, bool)> {
        let now = self.server_now();
        self.state.try_with_untracked(|s| {
            s.as_ref().map(|s| {
                (
                    s.position_at(now),
                    s.playing && s.loading.is_empty() && s.starts_in(now).is_none(),
                )
            })
        })?
    }

    /// The room's phase for this seat, where it stands (or will start), and its start.
    pub fn phase(&self) -> Option<(Phase, u64, u64)> {
        let me = self.member.try_get_untracked().flatten()?;
        let now = self.server_now();
        self.state.try_with_untracked(|s| {
            s.as_ref()
                .map(|s| (phase_of(s, &me, now), s.position_at(now), s.round))
        })?
    }

    /// Whether we already said we loaded the current start.
    pub fn ready_sent(&self) -> bool {
        let round = self
            .state
            .try_with_untracked(|s| s.as_ref().map(|s| s.round))
            .flatten();
        round.is_some() && round == self.ready_round.try_get_value()
    }

    /// This player has loaded the title: tell the room (once per start).
    pub fn ready(self) {
        let Some((code, me)) = self.ids() else { return };
        let Some(round) = self
            .state
            .with_untracked(|s| s.as_ref().filter(|s| s.is_loading(&me)).map(|s| s.round))
        else {
            return;
        };
        if self.ready_round.get_value() == round {
            return;
        }
        self.ready_round.set_value(round);
        let Some(k) = state::ctx().keep_client() else {
            return;
        };
        spawn_local(async move {
            let r = k
                .together_ready(
                    &code,
                    &keep_service::together::TogetherReady {
                        member_id: me,
                        round,
                    },
                )
                .await;
            match r {
                Ok(s) => self.on_state(s),
                Err(_) => {
                    let _ = self.ready_round.try_update_value(|r| {
                        if *r == round {
                            *r = 0;
                        }
                    });
                }
            }
        });
    }

    /// The host leaves the player: back to the waiting room for everyone (they stay).
    pub fn lobby(self, position_ms: u64) {
        let Some((code, me)) = self.ids() else { return };
        let Some(k) = state::ctx().keep_client() else {
            return;
        };
        spawn_local(async move {
            if let Ok(s) = k.together_lobby(&code, &me, position_ms).await {
                self.on_state(s);
            }
        });
    }

    /// Whether the room's title is this one (episodes by show, season and number).
    pub fn plays(&self, item: &Item) -> bool {
        self.media.with_value(|m| match m {
            None => true,
            Some(m) if m.kind == "episode" => {
                item.season_number == m.season
                    && item.episode_number == m.episode
                    && item.series_title.as_deref().is_some_and(|t| {
                        m.series_title.as_deref().is_some_and(|x| same_title(t, x))
                    })
            }
            Some(m) => {
                ids_match(&item.provider_ids, &m.tmdb, &m.imdb, &m.tvdb)
                    || same_title(&item.title, &m.title)
            }
        })
    }

    /// At start: the seat this tab had before a reload (its long poll tells whether it's
    /// still there).
    pub fn restore(self) {
        if let Some(seat) = state::tab_load::<Seat>(SEAT)
            && state::ctx().keep.get_untracked().is_some()
        {
            self.open(seat.code, seat.member, None);
        }
    }

    /// Holds a seat (just created or joined) and follows the room.
    pub fn open(self, code: String, member: String, s: Option<TogetherState>) {
        if !self.is(&code) {
            self.state.set(None);
            self.seq.set_value(0);
            self.media.set_value(None);
            self.handled_round.set_value(0);
            self.ready_round.set_value(0);
        }
        self.closed.set(None);
        self.code.set(Some(code.clone()));
        self.member.set(Some(member.clone()));
        state::tab_save(SEAT, Some(&Seat { code, member }));
        if let Some(s) = s {
            self.media.set_value(Some(s.media.clone()));
            self.accept(s);
        }
        self.follow();
    }

    /// Long-polls the room (the seat's heartbeat) and hands others' changes to the player.
    fn follow(self) {
        self.generation.update_value(|g| *g += 1);
        let mine = self.generation.get_value();
        let current = move || self.generation.try_get_value() == Some(mine);
        spawn_local(async move {
            let mut failures = 0u32;
            let mut rejoined = false;
            loop {
                if !current() {
                    break;
                }
                let Some((code, me)) = self.ids() else { break };
                let Some(k) = state::ctx().keep_client() else {
                    break;
                };
                self.sync_clock(&k).await;
                let after = self.seq.get_value();
                match k.together_wait(&code, &me, after).await {
                    Ok(s) => {
                        if !current() {
                            break;
                        }
                        failures = 0;
                        rejoined = false;
                        self.on_state(s);
                    }
                    // The seat was dropped (a minute without polling, say): it may come back
                    // once; else the room is gone.
                    Err(Error::NotFound) => {
                        if !current() {
                            break;
                        }
                        if !rejoined {
                            rejoined = true;
                            if let Ok(j) = k.together_join(&code).await {
                                if !current() {
                                    if !self.is(&code) {
                                        let _ = k.together_leave(&code, &j.member_id).await;
                                    }
                                    break;
                                }
                                self.member.set(Some(j.member_id.clone()));
                                state::tab_save(
                                    SEAT,
                                    Some(&Seat {
                                        code: code.clone(),
                                        member: j.member_id,
                                    }),
                                );
                                self.on_state(j.state);
                                continue;
                            }
                        }
                        if current() {
                            self.gone(&code);
                        }
                        break;
                    }
                    Err(_) => {
                        failures += 1;
                        state::sleep((failures * 2000).min(10_000) as i32).await;
                    }
                }
            }
        });
    }

    /// A state from the server (long poll, or the answer to our own call).
    pub fn on_state(&self, s: TogetherState) {
        if !self.is(&s.code) {
            return;
        }
        let before = self.state.get_untracked();
        let me = self.member.get_untracked().unwrap_or_default();
        let sending = self.sending.get_value() > 0;
        let known = self.media.get_value();
        self.accept(s.clone());
        self.media.set_value(Some(s.media.clone()));
        match on_remote(before.as_ref(), &s, &me, sending, known.as_ref()) {
            Remote::Own => {}
            Remote::NewTitle => self.incoming.set(Some((true, s))),
            Remote::Follow(note) => {
                if note.is_some() {
                    self.note.set(note);
                }
                self.incoming.set(Some((false, s)));
            }
        }
    }

    /// Tells the room what we just did (play, pause, seek; with `title`, the title too).
    pub fn send(self, title: Option<(AnyServer, Item)>, playing: bool, position_ms: u64) {
        let Some((code, me)) = self.ids() else { return };
        let c = state::ctx();
        self.sending.update_value(|n| *n += 1);
        spawn_local(async move {
            let media = match title {
                Some((srv, item)) => Some(media_of(&srv, &item).await),
                None => None,
            };
            if let Some(k) = c.keep_client() {
                let update = TogetherUpdate {
                    member_id: me,
                    playing,
                    position_ms,
                    media,
                    started: false,
                    lobby: false,
                };
                match k.together_update(&code, &update).await {
                    Ok(s) if self.is(&code) => {
                        self.media.set_value(Some(s.media.clone()));
                        self.accept(s);
                    }
                    // The host doesn't allow it: say so and go back in step with the room.
                    Err(Error::Status { status: 423 }) if self.is(&code) => {
                        self.note.try_set(Some(refused_note(
                            &update,
                            self.state.get_untracked().as_ref(),
                        )));
                        if let Some(s) = self.state.get_untracked() {
                            self.incoming.try_set(Some((false, s)));
                        }
                    }
                    _ => {}
                }
            }
            let _ = self.sending.try_update_value(|n| *n = n.saturating_sub(1));
        });
    }

    fn clear(&self) -> Option<(String, String)> {
        let ids = self.ids()?;
        let _ = self.generation.try_update_value(|g| *g += 1);
        self.code.try_set(None);
        self.member.try_set(None);
        self.state.try_set(None);
        let _ = self.media.try_update_value(|m| *m = None);
        let _ = self.seq.try_update_value(|s| *s = 0);
        state::tab_save::<Seat>(SEAT, None);
        state::ctx().invites.suppress(&ids.0);
        Some(ids)
    }

    /// Leaves the room for good (a new invitation is needed to come back).
    pub fn leave(self, note: &str, tell_server: bool) {
        // The client first: leaving can come with signing out.
        let k = tell_server.then(|| state::ctx().keep_client()).flatten();
        let Some((code, me)) = self.clear() else {
            return;
        };
        if !note.is_empty() {
            self.note.try_set(Some(note.to_owned()));
        }
        if let Some(k) = k {
            spawn_local(async move {
                let _ = k.together_leave(&code, &me).await;
            });
        }
    }

    /// The room closed under us.
    fn gone(self, code: &str) {
        self.clear();
        self.closed.try_set(Some(code.to_owned()));
        self.note
            .try_set(Some("The watch together room closed".into()));
    }
}

/// Opens a lobby with this title (the caller is its host): its code, for the Room page.
pub async fn create_room(srv: AnyServer, item: Item) -> Result<String, String> {
    let c = state::ctx();
    let k = c.keep_client().ok_or(NO_KEEP)?;
    let media = media_of(&srv, &item).await;
    let j = k
        .together_create(&TogetherCreate {
            media,
            rights: crate::prefs::load().together,
        })
        .await
        .map_err(|e| plain_together(&e))?;
    if c.room.active() {
        c.room.leave("", true);
    }
    let code = j.state.code.clone();
    c.room.open(code.clone(), j.member_id, Some(j.state));
    Ok(code)
}

/// Takes a seat in a room I'm invited to (or in on another device).
pub async fn join(code: &str) -> Result<TogetherJoined, String> {
    let k = state::ctx().keep_client().ok_or(NO_KEEP)?;
    k.together_join(code).await.map_err(|e| plain_together(&e))
}

/// After joining: the seat is this tab's (another room's is left), then the Room page while a
/// lobby, the player once started.
pub async fn enter(j: TogetherJoined, navigate: impl Fn(&str, NavigateOptions) + Clone) {
    let room = state::ctx().room;
    let code = j.state.code.clone();
    if room.active() && !room.is(&code) {
        room.leave("", true);
    }
    let started = j.state.started;
    let round = j.state.round;
    room.open(code.clone(), j.member_id, Some(j.state));
    if started {
        room.handled_round.set_value(round);
        go_play(room, navigate, || true).await;
    } else {
        navigate(&room_href(&code), Default::default());
    }
}

/// Opens the player in sync with the started room, playing this user's copy of its title.
/// `still`: the page asking is still there (finding the title takes a moment).
pub async fn go_play(
    room: Room,
    navigate: impl Fn(&str, NavigateOptions),
    still: impl Fn() -> bool,
) {
    let Some(s) = room.state.try_get_untracked().flatten() else {
        return;
    };
    match find(&s.media).await {
        None => {
            room.note.try_set(Some(format!(
                "{} isn't on your servers, so you can't play it here",
                media_label(&s.media)
            )));
        }
        Some((acc, item)) => {
            if !still() || !room.is(&s.code) {
                return;
            }
            let at = room.expected().map_or(0, |(p, _)| p);
            navigate(&play_href(&acc, &item, at, &s.code), Default::default());
        }
    }
}

/// My rooms and invitations, kept current by one long poll while an account is linked to the
/// AlwyWatch server; new invitations go to the banner.
#[derive(Clone, Copy)]
pub struct Invites {
    pub rooms: RwSignal<Option<TogetherRooms>>,
    /// New invitations not answered yet (the banner shows the first).
    pub banner: RwSignal<Vec<TogetherRoom>>,
    pub unreachable: RwSignal<bool>,
    /// Rooms just declined or left: no banner for them.
    suppressed: StoredValue<Vec<String>>,
    generation: StoredValue<u32>,
}

impl Invites {
    pub fn new(cached: Option<TogetherRooms>) -> Self {
        Self {
            rooms: RwSignal::new(cached),
            banner: RwSignal::new(Vec::new()),
            unreachable: RwSignal::new(false),
            suppressed: StoredValue::new(Vec::new()),
            generation: StoredValue::new(0),
        }
    }

    /// Starts the long poll (stopping any earlier one).
    pub fn run(self) {
        self.generation.update_value(|g| *g += 1);
        let mine = self.generation.get_value();
        let current = move || self.generation.try_get_value() == Some(mine);
        spawn_local(async move {
            let mut version = 0u64;
            let mut failures = 0u32;
            loop {
                if !current() {
                    break;
                }
                let Some(k) = state::ctx().keep_client() else {
                    break;
                };
                let res = k.together_rooms_wait(version).await;
                if !current() {
                    break;
                }
                match res {
                    Ok(r) => {
                        failures = 0;
                        self.unreachable.set(false);
                        version = r.version;
                        self.apply(r);
                    }
                    Err(e) => {
                        failures += 1;
                        self.unreachable.set(true);
                        let ms = if e == Error::Unauthorized {
                            60_000
                        } else {
                            (failures * 3000).min(30_000)
                        };
                        state::sleep(ms as i32).await;
                    }
                }
            }
        });
    }

    /// Signed out of the AlwyWatch server: no more polling, nothing to show.
    pub fn stop(self) {
        self.generation.update_value(|g| *g += 1);
        self.banner.set(Vec::new());
        self.rooms.set(None);
        self.unreachable.set(false);
    }

    fn apply(self, r: TogetherRooms) {
        let before = self
            .rooms
            .with_untracked(|x| x.as_ref().map(|x| x.rooms.clone()))
            .unwrap_or_default();
        // A room gone from the list may invite again later.
        self.suppressed
            .update_value(|s| s.retain(|c| r.rooms.iter().any(|x| x.code == *c)));
        let fresh = self
            .suppressed
            .with_value(|s| new_invitations(&before, &r.rooms, s));
        self.banner.update(|b| {
            b.retain(|x| r.rooms.iter().any(|y| y.code == x.code && y.invited));
            for f in fresh {
                if !b.iter().any(|x| x.code == f.code) {
                    b.push(f);
                }
            }
        });
        state::save(state::TOGETHER, &r);
        self.rooms.set(Some(r));
    }

    /// A room declined or left: off the banner and the list at once.
    pub fn suppress(self, code: &str) {
        let _ = self.suppressed.try_update_value(|s| {
            if !s.iter().any(|c| c == code) {
                s.push(code.to_owned());
            }
        });
        self.banner.try_update(|b| b.retain(|r| r.code != code));
        self.rooms.try_update(|r| {
            if let Some(r) = r {
                r.rooms.retain(|x| x.code != code);
            }
        });
    }

    /// "Not now": the invitation goes away.
    pub fn decline(self, code: String) {
        self.suppress(&code);
        if let Some(k) = state::ctx().keep_client() {
            spawn_local(async move {
                let _ = k.together_decline(&code).await;
            });
        }
    }

    /// Closes the banner without answering (the invitation stays on the Together page).
    pub fn dismiss(self, code: &str) {
        self.banner.update(|b| b.retain(|r| r.code != code));
    }
}

fn room_status(started: bool) -> &'static str {
    if started { "Watching" } else { "Lobby" }
}

/// Hosting a room: this title becomes the room's (everyone stays; a playing room loads it).
pub async fn retitle_room(srv: AnyServer, item: Item) -> Result<String, String> {
    let c = state::ctx();
    let k = c.keep_client().ok_or(NO_KEEP)?;
    let (code, me) = c.room.ids().ok_or("You're not in a room.")?;
    let media = media_of(&srv, &item).await;
    let s = k
        .together_update(
            &code,
            &TogetherUpdate {
                member_id: me,
                playing: false,
                position_ms: item.user.position_ms,
                media: Some(media),
                started: false,
                lobby: false,
            },
        )
        .await
        .map_err(|e| plain_together(&e))?;
    c.room.on_state(s);
    Ok(code)
}

/// "Watch together" on a title page: opens a lobby, then its Room page. Hosting a room
/// already: the title becomes that room's.
#[component]
pub fn TogetherButton(srv: AnyServer, item: Item) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let navigate = use_navigate();
    let alive = StoredValue::new(());
    let click = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        let (srv, item, navigate) = (srv.clone(), item.clone(), navigate.clone());
        let hosting = state::ctx().room.active() && state::ctx().room.hosting_untracked();
        spawn_local(async move {
            let res = if hosting {
                retitle_room(srv, item).await
            } else {
                create_room(srv, item).await
            };
            if alive.try_get_value().is_none() {
                return;
            }
            busy.set(false);
            match res {
                Ok(code) => navigate(&room_href(&code), Default::default()),
                Err(e) => error.set(Some(e)),
            }
        });
    };
    view! {
        <button on:click=click disabled=move || busy.get()>
            {move || if busy.get() {
                "Opening a room…"
            } else if state::ctx().room.code.get().is_some() && state::ctx().room.hosting() {
                "Watch in your room"
            } else {
                "Watch together"
            }}
        </button>
        {move || error.get().map(|e| view! { <span class="error small">{e}</span> })}
    }
}

/// Joins a room from the Together page or the banner (the invitation is the approval).
fn join_and_enter(
    code: String,
    busy: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
    navigate: impl Fn(&str, NavigateOptions) + Clone + 'static,
) {
    if busy.get_untracked().is_some() {
        return;
    }
    busy.set(Some(code.clone()));
    error.set(None);
    spawn_local(async move {
        let res = join(&code).await;
        busy.try_set(None);
        match res {
            Ok(j) => {
                let c = state::ctx();
                c.invites.dismiss(&code);
                enter(j, navigate).await;
            }
            Err(e) => {
                error.try_set(Some(e));
            }
        }
    });
}

/// My rooms and the rooms I'm invited to. No code to type: rooms are private.
#[component]
pub fn Together() -> impl IntoView {
    let c = state::ctx();
    let inv = c.invites;
    let room = c.room;
    let navigate = use_navigate();
    let busy = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let linked = move || state::ctx().keep.get().is_some();
    view! {
        <h1>"Watch together"</h1>
        <p class="muted">"Private rooms: only the people the host invites can see them."</p>
        {move || (!linked()).then(|| view! { <ErrorText text=NO_KEEP.to_string() /> })}
        {move || error.get().map(|e| view! { <ErrorText text=e /> })}
        {move || (inv.unreachable.get() && linked()).then(|| view! {
            <p class="muted small">"Can't reach your AlwyWatch server right now. Trying again…"</p>
        })}
        {move || match inv.rooms.get() {
            None if linked() => view! { <Loading /> }.into_any(),
            None => ().into_any(),
            Some(r) if r.rooms.is_empty() => view! {
                <p class="muted empty">"No rooms yet. Open a movie or an episode and choose Watch together, then invite people."</p>
            }.into_any(),
            Some(r) => {
                let (invites, mine): (Vec<_>, Vec<_>) = r.rooms.into_iter().partition(|r| r.invited);
                let navigate = navigate.clone();
                view! {
                    {(!invites.is_empty()).then(|| view! { <h2>"Invitations"</h2> })}
                    <div class="leaving">
                        {invites.into_iter().map(|r| {
                            let (code, code2, code3) = (r.code.clone(), r.code.clone(), r.code.clone());
                            let navigate = navigate.clone();
                            view! {
                                <div class="leaving-row room-row">
                                    <Avatar user_key=r.host_user_key.clone() name=r.host.clone() avatar=r.host_avatar.clone() size=40 />
                                    <div class="leaving-info">
                                        <span>{format!("{} invites you", r.host)}</span>
                                        <strong>{media_label(&r.media)}</strong>
                                        <span class="muted small">{room_status(r.started)}</span>
                                    </div>
                                    <div class="actions tight">
                                        <button class="primary small" disabled=move || busy.get().is_some()
                                            on:click=move |_| join_and_enter(code.clone(), busy, error, navigate.clone())>
                                            {move || if busy.get().as_deref() == Some(code3.as_str()) { "Joining…" } else { "Join" }}
                                        </button>
                                        <button class="small" on:click=move |_| inv.decline(code2.clone())>"Not now"</button>
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                    {(!mine.is_empty()).then(|| view! { <h2>"My rooms"</h2> })}
                    <div class="leaving">
                        {mine.into_iter().map(|r| {
                            let code = r.code.clone();
                            let navigate = navigate.clone();
                            let who = if r.hosting { "You host".to_string() } else { format!("Hosted by {}", r.host) };
                            let members = r.members.join(", ");
                            let open = move |_| {
                                if room.is(&code) {
                                    navigate(&room_href(&code), Default::default());
                                } else {
                                    // Joined on another device: a seat here too.
                                    join_and_enter(code.clone(), busy, error, navigate.clone());
                                }
                            };
                            view! {
                                <div class="leaving-row room-row">
                                    <Avatar user_key=r.host_user_key.clone() name=r.host.clone() avatar=r.host_avatar.clone() size=40 />
                                    <div class="leaving-info">
                                        <strong>{media_label(&r.media)}</strong>
                                        <span>{who}</span>
                                        <span class="muted small">{format!("{} · {members}", room_status(r.started))}</span>
                                    </div>
                                    <button class="small" disabled=move || busy.get().is_some() on:click=open>"Open"</button>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }
        }}
    }
}

/// The Room page (`/together/:code`): the lobby before the start, who's in the room after.
#[component]
pub fn RoomPage() -> impl IntoView {
    let params = use_params_map();
    let code = Memo::new(move |_| params.read().get("code").unwrap_or_default());
    let c = state::ctx();
    let room = c.room;
    let navigate = use_navigate();
    let busy = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let here = move || room.code.get().is_some_and(|x| x == code.get());
    view! {
        {move || {
            if here() {
                return view! { <Lobby room=room /> }.into_any();
            }
            if state::ctx().keep.get().is_none() {
                return view! { <h1>"Watch together"</h1><ErrorText text=NO_KEEP.to_string() /> }.into_any();
            }
            let code = code.get();
            if room.closed.get().as_deref() == Some(code.as_str()) {
                return view! {
                    <h1>"Watch together"</h1>
                    <p class="muted">"This room has closed."</p>
                    <A href="/together" attr:class="button">"Back to Together"</A>
                }.into_any();
            }
            let navigate = navigate.clone();
            view! {
                <h1>"Watch together"</h1>
                <p class="muted">"You're not in this room. If you were invited, join it."</p>
                <div class="actions">
                    <button class="primary" disabled=move || busy.get().is_some()
                        on:click=move |_| join_and_enter(code.clone(), busy, error, navigate.clone())>"Join"</button>
                    <A href="/together" attr:class="button">"Back to Together"</A>
                </div>
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
            }.into_any()
        }}
    }
}

#[component]
fn Lobby(room: Room) -> impl IntoView {
    let navigate = use_navigate();
    let picking = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let alive = StoredValue::new(());
    let alive = move || alive.try_get_value().is_some();
    // Each start opens the player by itself, wherever the app is (see `SeatBanner`).
    // The title on my servers (a poster; or a word that it can't be played here).
    let media = Memo::new(move |_| room.state.with(|s| s.as_ref().map(|s| s.media.clone())));
    let found = LocalResource::new(move || {
        let m = media.get();
        async move {
            let m = m?;
            let (acc, id) = find(&m).await?;
            let srv = state::ctx().server(&acc)?;
            let item = srv.item(&id).await.ok();
            Some(item.and_then(|i| {
                state::image(
                    &srv,
                    &i,
                    &[ImageKind::Primary, ImageKind::SeriesPrimary],
                    140,
                )
            }))
        }
    });
    let me = move || state::ctx().me.get().map(|m| m.user_key);
    let call = move |f: Box<dyn FnOnce(keep_service::KeepClient, String, String) -> CallFut>| {
        let Some((code, member)) = room.ids() else {
            return;
        };
        let Some(k) = state::ctx().keep_client() else {
            return error.set(Some(NO_KEEP.into()));
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let res = f(k, code, member).await;
            busy.try_set(false);
            match res {
                Ok(s) => room.on_state(s),
                Err(e) => {
                    error.try_set(Some(plain_together(&e)));
                }
            }
        });
    };
    // Back in the waiting room after a title: again from where the host stopped.
    let start = move |_| {
        let from = room
            .state
            .with_untracked(|s| s.as_ref().filter(|s| s.round > 0).map(|s| s.position_ms))
            .unwrap_or(0);
        call(Box::new(move |k, code, member| {
            Box::pin(async move { k.together_start(&code, &member, from).await })
        }))
    };
    let nav2 = navigate.clone();
    let leave = move |_| {
        room.leave("", true);
        nav2("/together", Default::default());
    };
    let nav3 = navigate.clone();
    let watch = move |_| {
        let nav = nav3.clone();
        spawn_local(async move { go_play(room, nav, alive).await });
    };
    view! {
        {move || room.state.get().map(|s| {
            let host = s.members.iter().find(|m| m.id == s.host).map(|m| m.name.clone()).unwrap_or_else(|| "the host".into());
            let hosting = room.hosting();
            let people = group_members(&s);
            let watch = watch.clone();
            let leave = leave.clone();
            view! {
                <div class="room-head">
                    {move || found.get().flatten().flatten().map(|p| view! { <img class="room-poster" src=p alt="" /> })}
                    <div>
                        <p class="muted">{if s.started { "Watch together · watching now" } else { "Watch together · lobby" }}</p>
                        <h1>{media_label(&s.media)}</h1>
                        {move || matches!(found.get(), Some(None)).then(|| view! {
                            <p class="muted small">"This title isn't on your servers: you can be in the room but can't play it here."</p>
                        })}
                    </div>
                </div>
                <div class="actions">
                    {match (s.started, hosting) {
                        (true, _) => view! { <button class="primary" on:click=watch>"Open the player"</button> }.into_any(),
                        (false, true) => view! {
                            <button class="primary" disabled=move || busy.get() on:click=start>"Start watching"</button>
                        }.into_any(),
                        (false, false) if s.round > 0 => view! { <p class="accent waiting">{format!("{host} is picking what to watch next")}</p> }.into_any(),
                        (false, false) => view! { <p class="accent waiting">{format!("Waiting for {host} to start")}</p> }.into_any(),
                    }}
                    {hosting.then(|| view! { <button on:click=move |_| picking.set(true)>"Invite"</button> })}
                    <button class="subtle" on:click=leave>"Leave"</button>
                </div>
                {move || room.note.get().map(|n| view! { <p class="muted small" role="status">{n}</p> })}
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                <section class="group">
                    <h2>{format!("In the room · {}", people.len())}</h2>
                    {people.into_iter().map(|p| {
                        let mine = me().is_some_and(|k| k == p.member.user_key);
                        let extra = [
                            p.host.then(|| "host".to_string()),
                            (p.devices > 1).then(|| format!("{} devices", p.devices)),
                            mine.then(|| "you".to_string()),
                        ].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                        view! {
                            <div class="person-row">
                                <Avatar user_key=p.member.user_key.clone() name=p.member.name.clone() avatar=p.member.avatar.clone() size=36 />
                                <span class="pname">{p.member.name.clone()}</span>
                                <span class="muted small">{extra}</span>
                            </div>
                        }
                    }).collect_view()}
                </section>
                {(!s.invited.is_empty()).then(|| view! {
                    <section class="group">
                        <h2>{format!("Invited · {}", s.invited.len())}</h2>
                        {s.invited.iter().map(|i| {
                            let key = i.user_key.clone();
                            view! {
                                <div class="person-row">
                                    <Avatar user_key=i.user_key.clone() name=i.name.clone() avatar=i.avatar.clone() size=36 />
                                    <span class="pname">{i.name.clone()}</span>
                                    {hosting.then(|| view! {
                                        <button class="small subtle" disabled=move || busy.get() on:click=move |_| {
                                            let key = key.clone();
                                            call(Box::new(move |k, code, member| {
                                                Box::pin(async move { k.together_uninvite(&code, &member, &key).await })
                                            }))
                                        }>"Remove"</button>
                                    })}
                                </div>
                            }
                        }).collect_view()}
                    </section>
                })}
                {(hosting && !s.started && s.round > 0).then(|| view! {
                    <p class="muted">"Back in the waiting room: start this again, or open another title and choose Watch in your room. Everyone stays here."</p>
                })}
                {(hosting && !s.started && s.round == 0 && s.invited.is_empty() && s.members.len() < 2).then(|| view! {
                    <p class="muted">"Invite people: they join right away, then start when everyone's here."</p>
                })}
            }
        })}
        {move || picking.get().then(|| view! { <PeoplePicker room=room on_close=move || picking.set(false) /> })}
    }
}

type CallFut = std::pin::Pin<Box<dyn std::future::Future<Output = Result<TogetherState, Error>>>>;

/// The host picks people to invite: recent partners first, a filter, several at once.
#[component]
pub fn PeoplePicker(
    room: Room,
    on_close: impl Fn() + Clone + Send + Sync + 'static,
) -> impl IntoView {
    /// The server takes at most this many at once.
    const MAX: usize = 20;
    let people = LocalResource::new(|| async {
        let k = state::ctx().keep_client().ok_or(NO_KEEP.to_string())?;
        k.together_people().await.map_err(|e| plain_together(&e))
    });
    let filter = RwSignal::new(String::new());
    let picked = RwSignal::new(Vec::<String>::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let close = on_close.clone();
    let send = move |_| {
        let keys = picked.get_untracked();
        if keys.is_empty() || busy.get_untracked() {
            return;
        }
        let (Some(code), Some(member)) = (room.code.get_untracked(), room.member.get_untracked())
        else {
            return;
        };
        let Some(k) = state::ctx().keep_client() else {
            return error.set(Some(NO_KEEP.into()));
        };
        busy.set(true);
        error.set(None);
        let close = close.clone();
        spawn_local(async move {
            let invite = TogetherInvite {
                member_id: member,
                user_keys: keys,
            };
            let res = k.together_invite(&code, &invite).await;
            busy.try_set(false);
            match res {
                Ok(s) => {
                    room.on_state(s);
                    close();
                }
                Err(e) => {
                    error.try_set(Some(plain_together(&e)));
                }
            }
        });
    };
    let row = move |p: &TogetherPerson, all: &[TogetherPerson]| {
        let key = p.user_key.clone();
        let (k1, k2, k3) = (key.clone(), key.clone(), key);
        let server = if !p.server_name.is_empty() {
            p.server_name.clone()
        } else {
            p.media_name.clone()
        };
        let detail = name_taken(&p.name, all).then_some(server);
        let selected = move || picked.with(|v| v.contains(&k1));
        view! {
            <button class="pick-row" class:selected=selected.clone() aria-pressed=move || selected().to_string()
                on:click=move |_| picked.update(|v| {
                    if let Some(i) = v.iter().position(|x| *x == k2) {
                        v.remove(i);
                    } else if v.len() < MAX {
                        v.push(k2.clone());
                    }
                })>
                <span class="check" aria-hidden="true">{move || if picked.with(|v| v.contains(&k3)) { "✓" } else { "" }}</span>
                <Avatar user_key=p.user_key.clone() name=p.name.clone() avatar=p.avatar.clone() size=32 />
                <span class="pname">{p.name.clone()}</span>
                {detail.map(|d| view! { <span class="muted small">{d}</span> })}
            </button>
        }
    };
    let input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if let Some(i) = input.get() {
            let _ = i.focus();
        }
    });
    let (close1, close2, close3) = (on_close.clone(), on_close.clone(), on_close);
    view! {
        <div class="dialog-back" on:click=move |_| close1()
            on:keydown=move |ev| if ev.key() == "Escape" { close2() }>
            <div class="dialog people" role="dialog" aria-modal="true" aria-labelledby="invite-title" on:click=|ev| ev.stop_propagation()>
                <h2 id="invite-title">"Invite people"</h2>
                <input class="search small" type="search" placeholder="Filter by name" aria-label="Filter by name" node_ref=input
                    prop:value=move || filter.get() on:input=move |ev| filter.set(event_target_value(&ev)) />
                <div class="pick-list">
                    {move || match people.get() {
                        None => view! { <Loading /> }.into_any(),
                        Some(Err(e)) => view! { <ErrorText text=e /> }.into_any(),
                        Some(Ok(all)) => {
                            let Some(s) = room.state.get() else { return ().into_any() };
                            let f = filter.get();
                            let (recent, others) = people_to_invite(&all, &s, &f);
                            if recent.is_empty() && others.is_empty() {
                                let text = if !f.trim().is_empty() {
                                    "Nobody matches."
                                } else if all.is_empty() {
                                    "Nobody else has signed in to this AlwyWatch server yet."
                                } else {
                                    "Everyone is already in the room or invited."
                                };
                                return view! { <p class="muted">{text}</p> }.into_any();
                            }
                            view! {
                                {(!recent.is_empty()).then(|| view! { <h3 class="pick-head">"Recent"</h3> })}
                                {recent.iter().map(|p| row(p, &all)).collect_view()}
                                {(!recent.is_empty() && !others.is_empty()).then(|| view! { <h3 class="pick-head">"Everyone"</h3> })}
                                {others.iter().map(|p| row(p, &all)).collect_view()}
                            }.into_any()
                        }
                    }}
                </div>
                {move || error.get().map(|e| view! { <ErrorText text=e /> })}
                <div class="actions">
                    <button on:click=move |_| close3()>"Cancel"</button>
                    <button class="primary" disabled=move || busy.get() || picked.with(Vec::is_empty) on:click=send>
                        {move || match picked.with(Vec::len) {
                            0 => "Invite".to_string(),
                            1 => "Invite 1 person".to_string(),
                            n => format!("Invite {n} people"),
                        }}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// A new invitation, anywhere in the app: Join / Not now. In the player (`compact`), a small
/// line in a corner rather than a banner over the video.
#[component]
pub fn InviteBanner(#[prop(optional)] compact: bool) -> impl IntoView {
    let inv = state::ctx().invites;
    let navigate = use_navigate();
    let busy = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    move || {
        let r = inv.banner.with(|b| b.first().cloned())?;
        let more = inv.banner.with(Vec::len) - 1;
        let (c1, c2, c3) = (r.code.clone(), r.code.clone(), r.code.clone());
        let navigate = navigate.clone();
        let text = format!("{} invites you to watch {}", r.host, media_label(&r.media));
        let join = move |_| join_and_enter(c1.clone(), busy, error, navigate.clone());
        Some(view! {
            <div class=if compact { "invite-note" } else { "banner invite" } role="status">
                {(!compact).then(|| view! { <Avatar user_key=r.host_user_key.clone() name=r.host.clone() avatar=r.host_avatar.clone() size=32 /> })}
                <span>{text}{(more > 0).then(|| format!(" (+{more} more)"))}</span>
                <button class="small primary" disabled=move || busy.get().is_some() on:click=join>
                    {move || if busy.get().is_some() { "Joining…" } else { "Join" }}
                </button>
                <button class="small" on:click=move |_| inv.decline(c2.clone())>"Not now"</button>
                <button class="small subtle" aria-label="Close" on:click=move |_| inv.dismiss(&c3)>"✕"</button>
                {move || error.get().map(|e| view! { <span class="error small">{e}</span> })}
            </div>
        })
    }
}

/// In a room but on another page: a way back to it.
#[component]
pub fn SeatBanner() -> impl IntoView {
    let room = state::ctx().room;
    let location = use_location();
    // Each start of the room opens the player, wherever the app is (the host may have picked
    // the title from any page). A player of the room already open follows by itself.
    let navigate = use_navigate();
    Effect::new(move |_| {
        let Some((started, round)) = room
            .state
            .with(|s| s.as_ref().map(|s| (s.started, s.round)))
        else {
            return;
        };
        if !started || round <= room.handled_round.get_value() {
            return;
        }
        room.handled_round.set_value(round);
        let in_player = location.pathname.get_untracked().starts_with("/play/")
            && location.search.get_untracked().contains("room=");
        if !in_player {
            let nav = navigate.clone();
            spawn_local(async move { go_play(room, nav, || true).await });
        }
    });
    move || {
        let code = room.code.get()?;
        let href = room_href(&code);
        if location.pathname.get() == href {
            return None;
        }
        let (label, started) = room
            .state
            .with(|s| s.as_ref().map(|s| (media_label(&s.media), s.started)))
            .unwrap_or_default();
        Some(view! {
            <div class="banner seat" role="status">
                <span>{format!("You're in a watch together room{}{}", if label.is_empty() { "" } else { ": " }, label)}
                    {started.then_some(" · watching now")}</span>
                <A href=href attr:class="button small">"Open the room"</A>
            </div>
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keep_service::together::TogetherInvitee;

    fn media(title: &str) -> TogetherMedia {
        TogetherMedia {
            kind: "movie".into(),
            title: title.into(),
            year: None,
            tmdb: None,
            imdb: None,
            tvdb: None,
            series_title: None,
            series_tmdb: None,
            series_tvdb: None,
            season: None,
            episode: None,
        }
    }

    fn member(id: &str, name: &str) -> TogetherMember {
        TogetherMember {
            id: id.into(),
            name: name.into(),
            user_key: format!("jf:{}", name.to_lowercase()),
            avatar: Default::default(),
        }
    }

    fn state(seq: u64, playing: bool, by: &str) -> TogetherState {
        TogetherState {
            code: "R".into(),
            seq,
            media: media("X"),
            started: true,
            playing,
            position_ms: 60_000,
            at_ms: 1_000,
            now_ms: 1_000,
            round: 0,
            loading: vec![],
            ready_by_ms: 0,
            by: by.into(),
            members: vec![member("a", "Ann"), member("b", "Bob")],
            host: "a".into(),
            invited: vec![],
            rights: Default::default(),
        }
    }

    fn room(code: &str, invited: bool) -> TogetherRoom {
        TogetherRoom {
            code: code.into(),
            host: "Ann".into(),
            host_avatar: Default::default(),
            host_user_key: "jf:ann".into(),
            media: media("X"),
            members: vec!["Ann".into()],
            started: false,
            playing: false,
            position_ms: 0,
            at_ms: 0,
            now_ms: 0,
            mine: !invited,
            hosting: false,
            invited,
        }
    }

    fn person(key: &str, name: &str, server: &str, recent: Option<i64>) -> TogetherPerson {
        TogetherPerson {
            user_key: key.into(),
            name: name.into(),
            media_name: name.to_lowercase(),
            avatar: Default::default(),
            server_name: server.into(),
            last_together_at: recent,
        }
    }

    #[test]
    fn phases_of_a_start() {
        let mut s = state(3, false, "");
        s.members = vec![member("a", "Ana"), member("b", "Bo")];
        s.loading = vec!["a".into(), "b".into()];
        assert_eq!(
            phase_of(&s, "a", 0),
            Phase::Loading {
                me: true,
                others: vec!["Bo".into()]
            }
        );
        assert_eq!(
            waiting_text(&phase_of(&s, "a", 0), true),
            Some("Waiting for Bo…".into())
        );
        s.loading.clear();
        s.playing = true;
        s.at_ms = 5_000;
        assert_eq!(phase_of(&s, "a", 4_000), Phase::StartsIn(1_000));
        assert_eq!(phase_of(&s, "a", 5_000), Phase::Live);
    }

    #[test]
    fn small_drift_is_tolerated() {
        assert!(!drifted(10_000, 11_500));
        assert!(drifted(10_000, 11_501));
        assert!(drifted(12_000, 10_000));
        assert!(!local_change(true, 10_000, true, 10_800));
        assert!(local_change(false, 10_000, true, 10_000));
        assert!(local_change(true, 40_000, true, 10_000));
    }

    #[test]
    fn remote_states_are_told_apart() {
        let before = state(1, true, "a");
        // Ours, or while ours are on their way.
        assert_eq!(
            on_remote(Some(&before), &state(2, false, "b"), "b", false, None),
            Remote::Own
        );
        assert_eq!(
            on_remote(Some(&before), &state(2, false, "a"), "b", true, None),
            Remote::Own
        );
        // Someone else paused.
        assert_eq!(
            on_remote(
                Some(&before),
                &state(2, false, "a"),
                "b",
                false,
                Some(&media("X"))
            ),
            Remote::Follow(Some("Ann paused".into()))
        );
        // Another title.
        assert_eq!(
            on_remote(
                Some(&before),
                &state(2, true, "a"),
                "b",
                false,
                Some(&media("Y"))
            ),
            Remote::NewTitle
        );
        // Same state again (the long poll's heartbeat): follow quietly, correcting drift.
        assert_eq!(
            on_remote(Some(&before), &state(1, true, "a"), "b", false, None),
            Remote::Follow(None)
        );
        // Someone joined (a server change).
        let mut joined = state(2, true, "");
        joined.members.push(member("c", "Cid"));
        assert_eq!(
            on_remote(Some(&before), &joined, "b", false, None),
            Remote::Follow(Some("Cid joined".into()))
        );
        // The host started the lobby.
        let mut lobby = state(1, false, "");
        lobby.started = false;
        assert_eq!(
            on_remote(Some(&lobby), &state(2, true, "a"), "b", false, None),
            Remote::Follow(Some("Ann started".into()))
        );
        // An invitation (server change, same members): nothing to say.
        let mut invited = state(2, true, "");
        invited.invited.push(TogetherInvitee {
            user_key: "jf:cid".into(),
            name: "Cid".into(),
            avatar: Default::default(),
        });
        assert_eq!(
            on_remote(Some(&before), &invited, "b", false, None),
            Remote::Follow(None)
        );
    }

    #[test]
    fn titles_match_loosely() {
        assert!(same_title("Monsters, Inc.", "monsters inc"));
        assert!(!same_title("", ""));
        let mut m = media("Show ep");
        m.series_title = Some("The Show".into());
        m.season = Some(2);
        m.episode = Some(3);
        assert_eq!(media_label(&m), "The Show · S2 · E3");
        assert_eq!(
            play_href("jf.s.u", "42", 1000, "AB 1"),
            "/play/jf.s.u/42?t=1000&room=AB%201"
        );
        assert_eq!(room_href("123456"), "/together/123456");
    }

    #[test]
    fn members_are_grouped_by_user() {
        let mut s = state(1, true, "a");
        // Ann on a second device (host's user), and a seat without a user key.
        s.members = vec![
            member("a", "Ann"),
            member("b", "Bob"),
            member("a2", "Ann"),
            TogetherMember {
                id: "z".into(),
                name: "Old".into(),
                user_key: String::new(),
                avatar: Default::default(),
            },
        ];
        s.host = "a2".into();
        let g = group_members(&s);
        assert_eq!(g.len(), 3);
        assert_eq!(
            (g[0].member.id.as_str(), g[0].devices, g[0].host),
            ("a", 2, true)
        );
        assert_eq!(
            (g[1].member.id.as_str(), g[1].devices, g[1].host),
            ("b", 1, false)
        );
        assert_eq!(g[2].devices, 1);
    }

    #[test]
    fn only_new_invitations_reach_the_banner() {
        let before = vec![room("1", true), room("2", false)];
        let after = vec![
            room("1", true),
            room("3", true),
            room("4", true),
            room("2", false),
        ];
        let fresh = new_invitations(&before, &after, &["4".to_string()]);
        // 1 was already there, 4 was just declined, 2 is mine.
        assert_eq!(
            fresh.iter().map(|r| r.code.as_str()).collect::<Vec<_>>(),
            ["3"]
        );
        // A room of mine that becomes an invitation (I left on all devices, invited again).
        let fresh = new_invitations(&before, &[room("2", true)], &[]);
        assert_eq!(fresh.len(), 1);
        assert!(new_invitations(&after, &after, &[]).is_empty());
    }

    #[test]
    fn people_are_filtered_and_split() {
        let all = vec![
            person("jf:ann", "Ann", "Home", Some(10)),
            person("jf:bob", "Bob", "Home", None),
            person("px:alex", "alex", "Plex", None),
            person("jf:alex", "Alex", "Home", None),
            person("jf:cid", "Cid", "Home", Some(5)),
        ];
        let mut s = state(1, false, "");
        s.members = vec![member("a", "Ann")];
        s.invited = vec![TogetherInvitee {
            user_key: "jf:bob".into(),
            name: "Bob".into(),
            avatar: Default::default(),
        }];
        let (recent, others) = people_to_invite(&all, &s, "");
        let keys = |v: &[&TogetherPerson]| v.iter().map(|p| p.user_key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&recent), ["jf:cid"]);
        assert_eq!(keys(&others), ["px:alex", "jf:alex"]);
        // By name or server, ignoring case and spaces around.
        let (recent, others) = people_to_invite(&all, &s, " plex ");
        assert!(recent.is_empty());
        assert_eq!(keys(&others), ["px:alex"]);
        assert!(name_taken("ALEX", &all));
        assert!(!name_taken("Cid", &all));
    }
}
