// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together: the rooms open on the AlwyWatch server, joining one (its host lets you in),
//! and the room a player follows. Same protocol and rules as the apps: each member plays
//! their own server's copy (found by provider ids), long-polls the room, applies what others
//! do, seeks when more than 1.5 s apart, and tells the room about their own play, pause, seek
//! and title changes (a title chosen here is announced; one chosen by the room is followed).

use keep_service::together::{
    KnockStatus, TogetherAnswer, TogetherCreate, TogetherMedia, TogetherMember, TogetherRoom,
    TogetherState, TogetherUpdate,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use media_api::{AnyServer, Item, ItemKind, MediaServer, ProviderIds};
use net::Error;

use crate::state::{self, fmt_time};
use crate::ui::Loading;

/// Further apart than this from the room, a member seeks.
pub const DRIFT_MS: u64 = 1500;

pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// Where playback should be now, from a room state received at `received_at_ms` (local
/// clock): the server's position moved on by the time since, while playing.
pub fn expected_position(s: &TogetherState, received_at_ms: f64, now_ms: f64) -> u64 {
    let since = (now_ms - received_at_ms).max(0.0) as i64;
    s.position_at(s.now_ms + since)
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
    if known.is_some_and(|k| *k != s.media) {
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
        Some(if b.playing && !s.playing {
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

/// The URL of the player in a room.
pub fn play_href(acc: &str, item: &str, start_ms: u64, code: &str, member: &str) -> String {
    format!(
        "/play/{acc}/{item}?t={start_ms}&room={}&member={}",
        net::percent_encode(code),
        net::percent_encode(member)
    )
}

/// The room a player follows (none while `code` is empty). Lives as long as the player page,
/// across the titles it plays.
#[derive(Clone, Copy)]
pub struct Room {
    pub code: RwSignal<Option<String>>,
    pub member: RwSignal<Option<String>>,
    pub state: RwSignal<Option<TogetherState>>,
    /// What just happened ("Ann paused"), shown for a few seconds.
    pub note: RwSignal<Option<String>>,
    /// Join requests answered here, until the server's next state drops them.
    answered: RwSignal<Vec<String>>,
    /// Others' changes for the player to apply: (new title, state).
    pub incoming: RwSignal<Option<(bool, TogetherState)>>,
    /// The title chosen here (next episode…): tell the room once it starts.
    pub announce: StoredValue<bool>,
    seq: StoredValue<u64>,
    received_at: StoredValue<f64>,
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
            answered: RwSignal::new(Vec::new()),
            incoming: RwSignal::new(None),
            announce: StoredValue::new(false),
            seq: StoredValue::new(0),
            received_at: StoredValue::new(0.0),
            media: StoredValue::new(None),
            sending: StoredValue::new(0),
            generation: StoredValue::new(0),
        }
    }

    /// In a room (safe after the page is gone).
    pub fn active(&self) -> bool {
        self.code.try_get_untracked().flatten().is_some()
    }

    /// Our own update is on its way (a title announcement, say).
    pub fn busy(&self) -> bool {
        self.sending.try_get_value().unwrap_or(0) > 0
    }

    fn ids(&self) -> Option<(String, String)> {
        Some((
            self.code.try_get_untracked().flatten()?,
            self.member.try_get_untracked().flatten()?,
        ))
    }

    fn accept(&self, s: TogetherState) {
        self.received_at.set_value(now_ms());
        if s.seq > self.seq.get_value() {
            self.seq.set_value(s.seq);
        }
        self.state.set(Some(s));
    }

    /// Where the room's playback is now, and whether it plays.
    pub fn expected(&self) -> Option<(u64, bool)> {
        let at = self.received_at.get_value();
        self.state.with_untracked(|s| {
            s.as_ref()
                .map(|s| (expected_position(s, at, now_ms()), s.playing))
        })
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

    /// The next join request for the host to answer.
    pub fn request(&self) -> Option<TogetherMember> {
        let me = self.member.get()?;
        let answered = self.answered.get();
        self.state.with(|s| {
            s.as_ref().filter(|s| s.host == me).and_then(|s| {
                s.requests
                    .iter()
                    .find(|r| !answered.contains(&r.id))
                    .cloned()
            })
        })
    }

    /// Joins (or rejoins) a room and starts following it.
    pub fn open(self, code: String, member: String, s: Option<TogetherState>) {
        self.code.set(Some(code));
        self.member.set(Some(member));
        if let Some(s) = s {
            self.media.set_value(Some(s.media.clone()));
            self.accept(s);
        }
        self.follow();
    }

    /// Long-polls the room and hands others' changes to the player.
    fn follow(self) {
        self.generation.update_value(|g| *g += 1);
        let mine = self.generation.get_value();
        let c = state::ctx();
        spawn_local(async move {
            let mut failures = 0u32;
            loop {
                if self.generation.try_get_value() != Some(mine) {
                    break;
                }
                let Some((code, me)) = self.ids() else { break };
                let Some(k) = c.keep_client() else { break };
                let after = self.seq.try_get_value().unwrap_or(0);
                match k.together_wait(&code, &me, after).await {
                    Ok(s) => {
                        failures = 0;
                        if self.generation.try_get_value() != Some(mine) {
                            break;
                        }
                        self.on_state(s);
                    }
                    Err(Error::NotFound) => {
                        if self.generation.try_get_value() == Some(mine) {
                            self.leave("The watch together room closed", false);
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

    fn on_state(&self, s: TogetherState) {
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

    /// Opens a room with what plays now: people on the same AlwyWatch server see it.
    pub fn create(self, srv: AnyServer, item: Item, position_ms: u64, playing: bool) {
        if self.active() {
            return;
        }
        let c = state::ctx();
        self.note.set(Some("Opening a room…".into()));
        spawn_local(async move {
            let Some(k) = c.keep_client() else {
                self.note.try_set(Some(
                    "Watch together needs an account on this AlwyWatch server.".into(),
                ));
                return;
            };
            let media = media_of(&srv, &item).await;
            let res = k
                .together_create(&TogetherCreate {
                    media,
                    position_ms,
                    playing,
                })
                .await;
            if self.note.try_get_untracked().is_none() {
                // The player is gone meanwhile: don't leave a room open.
                if let Ok(j) = res {
                    let _ = k.together_leave(&j.state.code, &j.member_id).await;
                }
                return;
            }
            match res {
                Ok(j) => {
                    self.open(j.state.code.clone(), j.member_id, Some(j.state));
                    self.note.set(Some(
                        "Watch together is open: friends can join from Together".into(),
                    ));
                }
                Err(e) => self
                    .note
                    .set(Some(format!("Watch together unavailable: {e}"))),
            }
        });
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
                };
                if let Ok(s) = k.together_update(&code, &update).await
                    && self.ids().is_some_and(|(c, _)| c == code)
                {
                    self.media.set_value(Some(s.media.clone()));
                    self.accept(s);
                }
            }
            let _ = self.sending.try_update_value(|n| *n = n.saturating_sub(1));
        });
    }

    /// The host lets someone in, or not.
    pub fn answer(self, request_id: String, accept: bool) {
        let Some((code, me)) = self.ids() else { return };
        self.answered.update(|a| a.push(request_id.clone()));
        let c = state::ctx();
        spawn_local(async move {
            let Some(k) = c.keep_client() else { return };
            let before = self.state.get_untracked();
            let answer = TogetherAnswer {
                member_id: me,
                accept,
            };
            // Gone meanwhile (they stopped asking): nothing to do.
            if let Ok(after) = k.together_answer(&code, &request_id, &answer).await {
                let joined = after
                    .members
                    .iter()
                    .filter(|m| {
                        before
                            .as_ref()
                            .is_some_and(|b| b.members.iter().all(|x| x.id != m.id))
                    })
                    .map(|m| m.name.clone())
                    .collect::<Vec<_>>();
                if !joined.is_empty() {
                    self.note.set(Some(format!("{} joined", joined.join(", "))));
                }
                self.accept(after);
            }
        });
    }

    /// Leaves the room (telling the server, unless it already closed it).
    pub fn leave(self, note: &str, tell_server: bool) {
        let Some((code, me)) = self.ids() else { return };
        let _ = self.generation.try_update_value(|g| *g += 1);
        self.code.try_set(None);
        self.member.try_set(None);
        self.state.try_set(None);
        let _ = self.media.try_update_value(|m| *m = None);
        if !note.is_empty() {
            self.note.try_set(Some(note.to_owned()));
        }
        if tell_server {
            let c = state::ctx();
            spawn_local(async move {
                if let Some(k) = c.keep_client() {
                    let _ = k.together_leave(&code, &me).await;
                }
            });
        }
    }
}

/// A room to join: from the list (title known) or by its code.
#[derive(Clone)]
struct Join {
    code: String,
    host: String,
    media: Option<TogetherMedia>,
}

/// The rooms open on the AlwyWatch server, and joining one.
#[component]
pub fn Together() -> impl IntoView {
    let rooms = RwSignal::new(None::<Vec<TogetherRoom>>);
    let unreachable = RwSignal::new(false);
    let joining = RwSignal::new(None::<Join>);
    let code = RwSignal::new(String::new());
    let c = state::ctx();
    spawn_local(async move {
        loop {
            let Some(k) = c.keep_client() else {
                rooms.try_set(Some(Vec::new()));
                break;
            };
            match k.together_rooms().await {
                Ok(l) => {
                    unreachable.try_set(false);
                    if rooms.try_set(Some(l)).is_some() {
                        break;
                    }
                }
                Err(_) => {
                    unreachable.try_set(true);
                    if rooms
                        .try_update(|r| {
                            r.get_or_insert_with(Vec::new);
                        })
                        .is_none()
                    {
                        break;
                    }
                }
            }
            state::sleep(8_000).await;
            if rooms.try_get_untracked().is_none() {
                break;
            }
        }
    });
    let join_code = move || {
        let text = code.get_untracked().trim().to_owned();
        if !text.is_empty() {
            joining.set(Some(Join {
                code: text,
                host: "the host".into(),
                media: None,
            }));
        }
    };
    view! {
        <h1>"Watch together"</h1>
        <p class="muted">"What friends on your AlwyWatch server are watching together. Join one: its host lets you in."</p>
        <form class="join-code" on:submit=move |ev| { ev.prevent_default(); join_code(); }>
            <input class="search small" placeholder="Room code" aria-label="Room code"
                prop:value=move || code.get() on:input=move |ev| code.set(event_target_value(&ev)) />
            <button class="primary" type="submit">"Join"</button>
        </form>
        {move || match rooms.get() {
            None => view! { <Loading /> }.into_any(),
            Some(l) if l.is_empty() => view! {
                <p class="muted">{if unreachable.get() {
                    "Can't reach your AlwyWatch server right now. Trying again…"
                } else {
                    "Nobody is watching together right now. To start, open a movie or an episode and choose Watch together: friends will see it here."
                }}</p>
            }.into_any(),
            Some(l) => view! {
                <div class="leaving">
                    {l.into_iter().map(|r| {
                        let others = r.members.len().saturating_sub(1);
                        let who = match others {
                            0 => r.host.clone(),
                            1 => format!("{} and 1 other", r.host),
                            n => format!("{} and {n} others", r.host),
                        };
                        let at = r.position_ms + if r.playing { (r.now_ms - r.at_ms).max(0) as u64 } else { 0 };
                        let status = [
                            Some(if r.playing { "Playing".to_string() } else { "Paused".to_string() }),
                            Some(fmt_time(at)),
                            r.mine.then(|| "you're in it on another device".to_string()),
                        ].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                        let join = Join { code: r.code.clone(), host: r.host.clone(), media: Some(r.media.clone()) };
                        view! {
                            <div class="leaving-row room-row">
                                <div class="leaving-info">
                                    <strong>{media_label(&r.media)}</strong>
                                    <span>{who}</span>
                                    <span class="muted small">{status}</span>
                                </div>
                                <button class="primary" on:click=move |_| joining.set(Some(join.clone()))>"Join"</button>
                            </div>
                        }
                    }).collect_view()}
                </div>
            }.into_any(),
        }}
        {move || joining.get().map(|j| view! { <JoinDialog join=j on_close=move || joining.set(None) /> })}
    }
}

/// Asks the host and waits for their answer; accepted, the player opens in sync with the room.
/// Closing the dialog while waiting withdraws the request.
#[component]
fn JoinDialog(join: Join, on_close: impl Fn() + Clone + Send + Sync + 'static) -> impl IntoView {
    let message = RwSignal::new(None::<String>);
    let request = StoredValue::new(None::<String>);
    let title = join
        .media
        .as_ref()
        .map(media_label)
        .unwrap_or_else(|| format!("Room {}", join.code));
    let navigate = use_navigate();
    let c = state::ctx();
    let code = join.code.clone();
    let host = join.host.clone();
    on_cleanup(move || {
        if let Some(id) = request.try_get_value().flatten() {
            let c = state::ctx();
            spawn_local(async move {
                if let Some(k) = c.keep_client() {
                    let _ = k.together_knock_cancel(&code, &id).await;
                }
            });
        }
    });
    spawn_local(async move {
        let end = |m: String| {
            let _ = request.try_update_value(|r| *r = None);
            message.try_set(Some(m));
        };
        let gone = || message.try_get_untracked().is_none();
        let Some(k) = c.keep_client() else {
            end("Watch together needs an account on this AlwyWatch server.".into());
            return;
        };
        let mut found = None;
        if let Some(m) = &join.media {
            found = find(m).await;
            if found.is_none() {
                end(format!(
                    "{} isn't on your servers, so you can't watch it with them.",
                    media_label(m)
                ));
                return;
            }
        }
        if gone() {
            return;
        }
        let mut ks = match k.together_knock(&join.code).await {
            Ok(x) => x,
            Err(Error::NotFound) => return end("This session has ended.".into()),
            Err(e) => return end(format!("Couldn't ask to join: {e}")),
        };
        let _ = request.try_update_value(|r| *r = Some(ks.request_id.clone()));
        let mut failures = 0u32;
        while ks.status == KnockStatus::Waiting {
            if gone() {
                return;
            }
            match k.together_knock_wait(&join.code, &ks.request_id).await {
                Ok(x) => {
                    failures = 0;
                    ks = x;
                }
                Err(Error::NotFound) => return end("This session has ended.".into()),
                Err(e) => {
                    failures += 1;
                    if failures > 5 {
                        return end(format!("Couldn't ask to join: {e}"));
                    }
                    state::sleep(2000 * failures as i32).await;
                }
            }
        }
        let _ = request.try_update_value(|r| *r = None);
        let joined = match (ks.status, ks.joined) {
            (KnockStatus::Accepted, Some(j)) => j,
            _ => return end(format!("{} can't let you in right now.", join.host)),
        };
        let received = now_ms();
        let found = match found {
            Some(f) => Some(f),
            None => find(&joined.state.media).await,
        };
        let Some((acc, item)) = found else {
            let _ = k
                .together_leave(&joined.state.code, &joined.member_id)
                .await;
            return end(format!(
                "{} isn't on your servers, so you can't watch it with them.",
                media_label(&joined.state.media)
            ));
        };
        if gone() {
            let _ = k
                .together_leave(&joined.state.code, &joined.member_id)
                .await;
            return;
        }
        let start = expected_position(&joined.state, received, now_ms());
        navigate(
            &play_href(&acc, &item, start, &joined.state.code, &joined.member_id),
            Default::default(),
        );
    });
    let close = on_close.clone();
    view! {
        <div class="dialog-back" on:click=move |_| close()>
            <div class="dialog" role="dialog" aria-modal="true" on:click=|ev| ev.stop_propagation()>
                <h2>{title}</h2>
                {move || match message.get() {
                    None => view! { <p>{format!("Waiting for {host} to let you in…")}</p> }.into_any(),
                    Some(m) => view! { <p>{m}</p> }.into_any(),
                }}
                <div class="actions">
                    {
                        let on_close = on_close.clone();
                        move || {
                            let on_close = on_close.clone();
                            let waiting = message.with(Option::is_none);
                            view! {
                                <button class:primary=!waiting on:click=move |_| on_close()>
                                    {if waiting { "Cancel" } else { "OK" }}
                                </button>
                            }
                        }
                    }
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    fn state(seq: u64, playing: bool, by: &str) -> TogetherState {
        TogetherState {
            code: "R".into(),
            seq,
            media: media("X"),
            playing,
            position_ms: 60_000,
            at_ms: 1_000,
            now_ms: 1_000,
            by: by.into(),
            members: vec![member("a", "Ann"), member("b", "Bob")],
            host: "a".into(),
            requests: vec![],
        }
    }

    #[test]
    fn position_moves_with_local_time_while_playing() {
        let s = state(1, true, "a");
        assert_eq!(expected_position(&s, 500.0, 2_500.0), 62_000);
        // A local clock going backwards never rewinds.
        assert_eq!(expected_position(&s, 500.0, 100.0), 60_000);
        let paused = state(1, false, "a");
        assert_eq!(expected_position(&paused, 500.0, 9_500.0), 60_000);
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
            play_href("jf.s.u", "42", 1000, "AB 1", "m"),
            "/play/jf.s.u/42?t=1000&room=AB%201&member=m"
        );
    }
}
