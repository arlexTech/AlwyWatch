// SPDX-License-Identifier: AGPL-3.0-or-later
//! Glue between the Slint UI and the core. Every page renders what the core has cached in the
//! same frame it is opened, then refreshes in the background (same model as Android).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use alwywatch_core::*;
use i18n::{tr, trn};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
    Weak,
};

use crate::platform::{self, Tokens};
use crate::profiles::ProfileStore;
mod account;
mod history;
mod invites;
mod links;
mod playback;
mod profiles;
mod remote;
mod together;
use crate::player::{self, ExternalSub, PlayRequest, TrackSetup};
use crate::{
    AccountRow, AdminRow, AppWindow, Card, CardMenu, Detail, DownloadRow, ForeverRow, KeepPanel,
    LeavingRow, NamedId, Page, Rail, SeerrPage, SeerrPanel, ServerChoice, StatusRow,
};
use together::TogetherPlay;

#[path = "dashboard.rs"]
mod dashboard;

/// Seerr turned off here on purpose: not connected through the server again (as Android).
const SEERR_OFF: &str = "seerr.off";
const POSTER_PX: u32 = 300;
const EPISODE_PX: u32 = 320;
const BACKDROP_PX: u32 = 1280;
const PAGE: u32 = 60;

#[derive(Clone, PartialEq)]
enum Route {
    Home,
    Downloads,
    Requests,
    Leaving,
    Triage,
    KeepAdmin,
    Dashboard,
    Library(String, String),
    /// A library narrowed to a genre or a collection: library, title, genre, collection.
    Filtered(String, String, Option<String>, Option<String>),
    Detail(String),
    /// A Seerr title by key ("Movie:603", "Tv:1399").
    SeerrTitle(String),
    /// A saga (TMDB collection): id, name.
    SeerrSaga(i64, String),
    Search,
    MyList,
    Settings,
    Add,
    Together,
    /// The watch together room I'm in.
    Room,
    /// Everything in Continue watching (a row's "See all").
    Continue,
    /// A library's Browse tab, newest first ("New in …" → "See all"): library, title.
    Recent(String, String),
    Profiles,
    HistoryCopy,
}

/// What a poster grid shows: a library (sorted), or one of its genres or collections.
#[derive(Clone, PartialEq)]
struct GridKey {
    library: String,
    sort: SortBy,
    genre: Option<String>,
    collection: Option<String>,
}

impl GridKey {
    fn library(library: String, sort: SortBy) -> Self {
        GridKey {
            library,
            sort,
            genre: None,
            collection: None,
        }
    }
}

struct GridState {
    key: GridKey,
    loaded: u32,
    total: u32,
    loading: bool,
}

struct State {
    account: Option<String>,
    stack: Vec<Route>,
    /// Scroll positions (home, grid, title page) of the pages under the top of `stack`.
    scroll: Vec<(f32, f32, f32)>,
    grid: Option<GridState>,
    grid_model: Rc<VecModel<Card>>,
    episodes_model: Rc<VecModel<Card>>,
    rail_models: Vec<Rc<VecModel<Card>>>,
    /// Item metadata for everything currently on screen, by id (for play/prefetch/next episode).
    items: HashMap<String, Item>,
    /// Account each item/library id comes from (several servers on one home). Ids not listed
    /// belong to the current account.
    owners: HashMap<String, String>,
    jellyfin_url: String,
    plex_token: String,
    plex_servers: Vec<PlexServerResource>,
    /// Servers listed by an AlwyWatch server on the add screen, and that server's address.
    preset_servers: Vec<ServerEntry>,
    preset_url: String,
    /// Plex server picked from that list: selected automatically after the plex.tv sign-in.
    preset_plex: Option<String>,
    series: Option<String>,
    season: Option<String>,
    search_gen: u64,
    /// The last search's results (text, titles, server badges): Back from a result shows
    /// them again instead of an empty page under the same text.
    search_results: Option<(String, Vec<Item>, bool)>,
    seerr_url: String,
    seerr_selected: Option<SeerrResult>,
    /// A request is on its way: a second click would send it twice (Seerr refuses the
    /// second, whose error then replaced "Requested").
    seerr_requesting: bool,
    seerr_seasons: Vec<(SeerrSeason, bool)>,
    triage: Vec<LeavingItem>,
    /// A swipe card's page is open: Back shows the same card instead of reloading.
    triage_back: bool,
    /// Leaving soon posters, loaded around the visible rows only: (url, requested).
    leaving_posters: Vec<(Option<String>, bool)>,
    leaving_model: Option<Rc<VecModel<LeavingRow>>>,
    /// Watch together: the room I'm in (lobby or playing), my rooms and invitations.
    together: Option<together::Room>,
    lists: together::Lists,
    /// My AlwyWatch profile, the welcome step's draft and the cropper.
    me: account::MeState,
    /// Who each account is on the AlwyWatch server, and linking them (one person).
    links: links::LinksState,
    /// Kept forever (admin), redrawn when avatars arrive.
    forever: Option<keep_service::ForeverStats>,
    /// "Mark unwatched" waiting for its confirmation: (season or series id, its label).
    mark_pending: Option<String>,
    /// Short messages for the open player (an invitation arrived).
    player_notes: Option<std::sync::mpsc::Sender<String>>,
    /// The playing mpv, for the in-window controls.
    player_mpv: Option<Arc<crate::mpv::Mpv>>,
    /// What the player plays (account, item) and its link for watch together (every player
    /// has one, so a room can start from it without reloading).
    player_item: Option<(String, String)>,
    /// A title is being started (asking the server): another Play press waits for it.
    player_starting: bool,
    /// The size picked in the player's quality menu (None: the original file), kept for
    /// the next titles, and the menu's choices for the title playing.
    quality: Option<Quality>,
    quality_choices: Vec<Option<Quality>>,
    /// A quality was picked: the player closes and starts again where it was.
    quality_restart: bool,
    player_tx: Option<std::sync::mpsc::Sender<player::ToPlayer>>,
    /// The home's Continue watching, for its "See all" page.
    continue_items: Vec<Item>,
    admin_users: Vec<AdminUser>,
    /// Seerr's users, for "Requests as" (empty when the AlwyWatch server takes no requests).
    admin_seerr: Vec<keep_service::KeepSeerrUser>,
    armed: Option<(String, i32)>,
    poll_gen: u64,
    /// Bumped to stop waiting for a device transfer.
    transfer_gen: u64,
    /// The title page's first trailer: (title id, account, trailer).
    trailer: Option<(String, String, Trailer)>,
    /// The title page whose saga was looked up (once per page).
    saga_for: Option<String>,
    /// Copy watch history: kept while away from its page.
    history: history::HistoryJob,
    /// Settings → Admin → Invitations, and the sign-up step's invitation check.
    invites: invites::InviteState,
    /// "Recently added" show cards: what is new and what opening them shows, by card id.
    notes: HashMap<String, RailNote>,
    /// The season a show's page opens on (a card's note): (show, season).
    pending_season: Option<(String, String)>,
    /// The card whose actions are open, and the row it is in.
    card_menu: Option<(String, String)>,
    /// Play on the TV: my TVs and the mini remote.
    remote: remote::RemoteUi,
}

thread_local! {
    /// The UI-thread App, reachable from `invoke_from_event_loop` closures (which must be `Send`).
    static APP: RefCell<std::rc::Weak<App>> = const { RefCell::new(std::rc::Weak::new()) };
}

pub struct App {
    downloads_timer: slint::Timer,
    runner_timer: slint::Timer,
    history_timer: slint::Timer,
    /// Sends progress saved while offline (downloads watched on the train) once back online.
    progress_timer: slint::Timer,
    progress_ticks: std::cell::Cell<u32>,
    core: Arc<AlwyWatch>,
    ui: Weak<AppWindow>,
    tokens: Tokens,
    profiles: ProfileStore,
    state: RefCell<State>,
    images: RefCell<HashMap<String, Image>>,
    /// Custom avatars by (user, version); None while loading or after a failure.
    faces: RefCell<HashMap<(String, u32), Option<Image>>>,
    /// The admin dashboard's choice and last readings.
    dash: RefCell<dashboard::Dash>,
}

/// Clears `State::player_starting` when a start ends, however it ends.
struct Starting(std::rc::Weak<App>);

impl Drop for Starting {
    fn drop(&mut self) {
        if let Some(t) = self.0.upgrade() {
            t.state.borrow_mut().player_starting = false;
        }
    }
}

fn sstr(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

/// Tabs of a library page, the first by default.
const LIBRARY_TABS: [&str; 4] = ["recommended", "browse", "collections", "categories"];

/// "plex" or "jellyfin": the server behind an account id, for the card marks.
fn server_kind(account: &str) -> &'static str {
    if account.starts_with("plex:") {
        "plex"
    } else if account.is_empty() {
        ""
    } else {
        "jellyfin"
    }
}

fn item_subtitle(i: &Item) -> String {
    match i.kind {
        ItemKind::Episode => [
            i.series_title.clone(),
            i.season_number
                .zip(i.episode_number)
                .map(|(s, e)| format!("S{s}E{e}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
        _ => [
            i.year.map(|y| y.to_string()),
            i.runtime_ms
                .filter(|&r| r >= 60_000)
                .map(|r| tr!("{minutes} min", minutes = r / 60_000)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
    }
}

fn image_ref(i: &Item, kind: ImageKind) -> Option<ImageRef> {
    let found = i.images.iter().find(|r| r.kind == kind).cloned();
    // A poster for anything: an episode's show poster, else the item's own.
    if kind == ImageKind::SeriesPrimary {
        return found.or_else(|| image_ref(i, ImageKind::Primary));
    }
    found
}

impl App {
    /// `gate`: ask "Who's watching?" (or the profile's PIN) before showing the app.
    pub fn new(
        core: Arc<AlwyWatch>,
        ui: &AppWindow,
        tokens: Tokens,
        profiles: ProfileStore,
        gate: bool,
    ) -> Rc<Self> {
        let account = core
            .get_setting("current_account".into())
            .ok()
            .flatten()
            .filter(|a| core.is_active(a.clone()))
            .or_else(|| {
                core.accounts()
                    .ok()?
                    .into_iter()
                    .find(|a| core.is_active(a.id.clone()))
                    .map(|a| a.id)
            });
        let app = Rc::new(Self {
            downloads_timer: slint::Timer::default(),
            runner_timer: slint::Timer::default(),
            history_timer: slint::Timer::default(),
            progress_timer: slint::Timer::default(),
            progress_ticks: std::cell::Cell::new(0),
            core,
            ui: ui.as_weak(),
            tokens,
            profiles,
            state: RefCell::new(State {
                account,
                stack: vec![],
                seerr_requesting: false,
                player_starting: false,
                scroll: vec![],
                grid: None,
                grid_model: Rc::new(VecModel::default()),
                episodes_model: Rc::new(VecModel::default()),
                rail_models: vec![],
                items: HashMap::new(),
                owners: HashMap::new(),
                jellyfin_url: String::new(),
                plex_token: String::new(),
                plex_servers: vec![],
                preset_servers: vec![],
                preset_url: String::new(),
                preset_plex: None,
                series: None,
                season: None,
                search_gen: 0,
                search_results: None,
                seerr_url: String::new(),
                seerr_selected: None,
                seerr_seasons: vec![],
                triage: vec![],
                triage_back: false,
                leaving_posters: vec![],
                leaving_model: None,
                together: None,
                lists: Default::default(),
                me: Default::default(),
                links: Default::default(),
                forever: None,
                player_notes: None,
                player_mpv: None,
                player_item: None,
                quality: None,
                quality_choices: vec![],
                quality_restart: false,
                player_tx: None,
                continue_items: vec![],
                mark_pending: None,
                admin_users: vec![],
                admin_seerr: vec![],
                armed: None,
                poll_gen: 0,
                transfer_gen: 0,
                trailer: None,
                saga_for: None,
                history: Default::default(),
                invites: Default::default(),
                notes: HashMap::new(),
                pending_season: None,
                card_menu: None,
                remote: Default::default(),
            }),
            images: RefCell::new(HashMap::new()),
            faces: RefCell::new(HashMap::new()),
            dash: Default::default(),
        });
        APP.with(|a| *a.borrow_mut() = Rc::downgrade(&app));
        ui.set_grid(ModelRc::from(app.state.borrow().grid_model.clone()));
        ui.set_episodes(ModelRc::from(app.state.borrow().episodes_model.clone()));
        app.load_player_settings();
        app.load_language_label();
        // Settings changed on the viewer's other devices (kept with their Jellyfin user).
        {
            let (core, this) = (app.core.clone(), Rc::downgrade(&app));
            slint::spawn_local(async move {
                if core.sync_settings().await.unwrap_or(0) > 0
                    && let Some(a) = this.upgrade()
                {
                    a.load_player_settings();
                }
            })
            .ok();
        }
        ui.set_spdif(
            app.core
                .get_setting("player.spdif".into())
                .ok()
                .flatten()
                .as_deref()
                == Some("1"),
        );
        app.wire(ui);
        app.refresh_seerr_settings();
        app.refresh_keep_settings();
        app.check_services();
        app.start_download_runner();
        let first = if app.state.borrow().account.is_some() {
            Route::Home
        } else {
            Route::Add
        };
        app.navigate(first);
        // After the first page: a page change clears the banner.
        app.show_reminders();
        app.render_profiles();
        if gate {
            app.show_gate();
        }
        // Invitations arrive through the rooms' long poll; the welcome step once per account.
        app.start_rooms_poll();
        app.check_welcome();
        // "Are these both you?" once for two accounts of one person (after the welcome step).
        app.check_links(true);
        app
    }

    // ----- downloads (in-process) --------------------------------------------------------------

    /// Runs the queue in the background now and every minute while something is pending, so
    /// downloads left waiting by a network loss continue once it is back.
    fn start_download_runner(self: &Rc<Self>) {
        self.kick_downloads();
        let w = Rc::downgrade(self);
        self.runner_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(60),
            move || {
                if let Some(a) = w.upgrade()
                    && a.core.has_pending_downloads()
                {
                    a.kick_downloads();
                }
            },
        );
        let w = Rc::downgrade(self);
        self.downloads_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(1),
            move || {
                if let Some(a) = w.upgrade()
                    && a.ui().get_page() == Page::Downloads
                {
                    a.refresh_downloads();
                }
            },
        );
    }

    pub(super) fn kick_downloads(&self) {
        let core = self.core.clone();
        self.core.runtime().spawn(async move {
            let _ = core.run_downloads().await;
        });
    }

    fn refresh_downloads(&self) {
        let gb = |b: u64| tr!("{size} GB", size = format!("{:.1}", b as f64 / 1e9));
        let rows: Vec<DownloadRow> = self
            .core
            .downloads()
            .unwrap_or_default()
            .into_iter()
            .map(|d| {
                let size = d
                    .total
                    .map(|t| tr!("{done} / {total}", done = gb(d.downloaded), total = gb(t)))
                    .unwrap_or_else(|| gb(d.downloaded));
                let status = match d.state {
                    DownloadState::Queued => tr!("Queued · {size}", size = size),
                    DownloadState::Running => tr!("Downloading · {size}", size = size),
                    DownloadState::Paused => tr!("Paused · {size}", size = size),
                    DownloadState::PausedNoSpace => {
                        tr!("Paused: disk full. Free some space, then resume.")
                    }
                    DownloadState::Waiting => {
                        tr!("Waiting for the network · {size}", size = size)
                    }
                    DownloadState::Failed => tr!(
                        "Failed: {error}",
                        error = d.error.clone().unwrap_or_default()
                    ),
                    DownloadState::Completed => {
                        tr!("Downloaded · {path}", path = d.file_path)
                    }
                };
                DownloadRow {
                    id: d.id as i32,
                    account: sstr(&d.account_id),
                    item: sstr(&d.item_id),
                    title: sstr(&d.title),
                    status: sstr(status),
                    progress: d
                        .total
                        .filter(|t| *t > 0)
                        .map(|t| d.downloaded as f32 / t as f32)
                        .unwrap_or(0.0),
                    completed: d.state == DownloadState::Completed,
                    active: matches!(
                        d.state,
                        DownloadState::Queued | DownloadState::Running | DownloadState::Waiting
                    ),
                }
            })
            .collect();
        // Refreshed every second while shown: rows change in place, so the buttons under the
        // pointer stay (a new list each time rebuilt them, losing clicks mid-press).
        let ui = self.ui();
        let model = ui.get_downloads();
        if let Some(vm) = model.as_any().downcast_ref::<VecModel<DownloadRow>>() {
            if vm.row_count() == rows.len() {
                for (i, r) in rows.into_iter().enumerate() {
                    if vm.row_data(i).as_ref() != Some(&r) {
                        vm.set_row_data(i, r);
                    }
                }
            } else {
                vm.set_vec(rows);
            }
            return;
        }
        ui.set_downloads(ModelRc::new(VecModel::from(rows)));
    }

    fn download(self: &Rc<Self>, id: String) {
        let Some(account) = self.account_of(&id) else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.download_item(account, id, None).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(_) => {
                    t.kick_downloads();
                    t.navigate(Route::Downloads);
                }
                Err(e) => t.set_error(Some(e)),
            }
        })
        .ok();
    }

    // ----- keep service ---------------------------------------------------------------------

    fn leaving_row(i: &LeavingItem) -> LeavingRow {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let when = match i.protection {
            Protection::Forever => tr!("Kept forever"),
            Protection::UntilWatched => tr!("Kept until watched"),
            p => match i.effective_deletion_date {
                None => tr!("No date"),
                // Date passed but the owner has not turned deletion on: nothing happens yet.
                Some(d) if d < now && !i.collection_active && p == Protection::None => {
                    tr!("Deletion paused")
                }
                Some(d) => {
                    let days = (d - now).div_euclid(86_400);
                    let s = match days {
                        d if d < 0 => tr!("Any day now"),
                        0 => tr!("Leaves today"),
                        1 => tr!("Leaves tomorrow"),
                        d => trn!("Leaves in {n} day", "Leaves in {n} days", d),
                    };
                    if p == Protection::Extended {
                        tr!("{when} (extended)", when = s)
                    } else {
                        s
                    }
                }
            },
        };
        let vote = i
            .my_vote
            .as_ref()
            .map(|v| match v.kind {
                VoteKind::DontCare => tr!("You're fine with it going"),
                VoteKind::KeepDays => tr!("You keep it a while"),
                VoteKind::KeepUntilWatched => tr!("You keep it until you watch it"),
                VoteKind::KeepForever => match &v.reason {
                    Some(r) => tr!("You keep it forever ({reason})", reason = r),
                    None => tr!("You keep it forever"),
                },
            })
            .unwrap_or_default();
        let others = i.counts.forever + i.counts.until_watched + i.counts.days;
        LeavingRow {
            key: sstr(&i.key),
            // "Title · 2013": a bare year ran into titles ending in a number.
            title: sstr(match i.year {
                Some(y) => format!("{} · {y}", i.title),
                None => i.title.clone(),
            }),
            when: sstr(when),
            vote: sstr(vote),
            others: sstr(if others > 0 {
                trn!("{n} keep request", "{n} keep requests", others)
            } else {
                String::new()
            }),
            poster: Image::default(),
            urgent: i.protection == Protection::None,
            voted: i.my_vote.is_some(),
        }
    }

    fn render_leaving(self: &Rc<Self>, list: &[LeavingItem]) {
        let model = Rc::new(VecModel::from(
            list.iter().map(Self::leaving_row).collect::<Vec<_>>(),
        ));
        self.ui().set_leaving(ModelRc::from(model.clone()));
        {
            let mut st = self.state.borrow_mut();
            st.leaving_posters = list.iter().map(|i| (i.poster_url.clone(), false)).collect();
            st.leaving_model = Some(model);
        }
        self.leaving_scrolled(0);
        let mut cols: Vec<NamedId> = vec![];
        for i in list {
            if let (Some(id), Some(t)) = (i.collection_id, &i.collection_title)
                && !cols.iter().any(|c| c.id.as_str() == id.to_string())
            {
                cols.push(NamedId {
                    id: sstr(id.to_string()),
                    title: sstr(t),
                    selected: false,
                });
            }
        }
        self.ui()
            .set_admin_collections(ModelRc::new(VecModel::from(cols)));
    }

    /// Loads the posters of the rows on screen (and a few past them), each once.
    fn leaving_scrolled(self: &Rc<Self>, first: i32) {
        let first = first.max(0) as usize;
        let (model, todo) = {
            let mut st = self.state.borrow_mut();
            let Some(model) = st.leaving_model.clone() else {
                return;
            };
            let end = (first + 16).min(st.leaving_posters.len());
            let todo: Vec<(usize, String)> = (first.min(end)..end)
                .filter_map(|row| {
                    let p = &mut st.leaving_posters[row];
                    if p.1 {
                        return None;
                    }
                    p.1 = true;
                    p.0.clone().map(|u| (row, u))
                })
                .collect();
            (model, todo)
        };
        for (row, url) in todo {
            let m = Rc::downgrade(&model);
            self.load_url_image(url, move |_, img| {
                if let Some(m) = m.upgrade()
                    && let Some(mut r) = m.row_data(row)
                {
                    r.poster = img;
                    m.set_row_data(row, r);
                }
            });
        }
    }

    fn load_leaving(self: &Rc<Self>) {
        if let Ok(Some(list)) = self.core.cached_leaving_soon() {
            self.render_leaving(&list);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_leaving_soon(false).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(list) => {
                    if t.ui().get_page() == Page::Leaving || t.ui().get_page() == Page::Keepadmin {
                        t.render_leaving(&list);
                    }
                }
                Err(e) => t.set_error(Some(e)),
            }
        })
        .ok();
    }

    fn vote_request(choice: &str) -> Option<VoteRequest> {
        let r = |kind, days, reminder_days, reason: Option<&str>| VoteRequest {
            kind,
            days,
            reminder_days,
            reason: reason.map(str::to_owned),
        };
        Some(match choice {
            "days7" => r(VoteKind::KeepDays, Some(7), None, None),
            "days30" => r(VoteKind::KeepDays, Some(30), None, None),
            "watched" => r(VoteKind::KeepUntilWatched, None, Some(7), None),
            "love" => r(VoteKind::KeepForever, None, None, None),
            "dontcare" => r(VoteKind::DontCare, None, None, None),
            _ => return None,
        })
    }

    fn keep_vote(self: &Rc<Self>, key: String, choice: String) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = match Self::vote_request(&choice) {
                Some(v) => core.keep_vote(key, v).await,
                None => core.keep_unvote(key).await,
            };
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(_) => {
                    t.ui().set_leaving_open(SharedString::new());
                    if t.ui().get_page() == Page::Detail {
                        t.render_keep_panel_of_page();
                    } else if let Ok(Some(list)) = t.core.cached_leaving_soon() {
                        t.render_leaving(&list);
                    }
                }
                // The title page has no keep message line: its error banner says it.
                Err(e) if t.ui().get_page() == Page::Detail => t.set_error(Some(e)),
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    fn show_triage_card(self: &Rc<Self>) {
        let first = self.state.borrow().triage.first().cloned();
        let ui = self.ui();
        ui.set_triage_left(self.state.borrow().triage.len() as i32);
        let Some(i) = first else { return };
        ui.set_triage_card(Self::leaving_row(&i));
        if let Some(url) = i.poster_url.clone() {
            let key = i.key.clone();
            self.load_url_image(url, move |t, img| {
                let ui = t.ui();
                let mut c = ui.get_triage_card();
                if c.key.as_str() == key {
                    c.poster = img;
                    ui.set_triage_card(c);
                }
            });
        }
    }

    fn load_triage(self: &Rc<Self>) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_triage(50).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(list) => {
                    t.state.borrow_mut().triage = list;
                    t.show_triage_card();
                }
                Err(e) => t.set_error(Some(e)),
            }
        })
        .ok();
    }

    /// The swipe card's title page (a click on its poster, or Enter); Back returns to it.
    fn triage_open(self: &Rc<Self>) {
        let Some(item) = self.state.borrow().triage.first().cloned() else {
            return;
        };
        let (core, this, prefer) = (self.core.clone(), Rc::downgrade(self), self.account());
        slint::spawn_local(async move {
            let kind = if item.kind == "show" {
                SeerrMediaType::Tv
            } else {
                SeerrMediaType::Movie
            };
            let found = match item.tmdb_id.as_deref().and_then(|t| t.parse().ok()) {
                Some(tmdb) => core
                    .library_title(kind, tmdb, prefer.clone())
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let at = found
                .map(|f| (f.account_id, f.item_id))
                .or_else(|| prefer.zip(item.server_item_id.clone()));
            let Some(t) = this.upgrade() else { return };
            if let Some((account, id)) = at {
                t.state.borrow_mut().triage_back = true;
                t.own_ids(std::iter::once(&id), &account);
                t.navigate(Route::Detail(id));
            }
        })
        .ok();
    }

    fn triage_vote(self: &Rc<Self>, choice: String) {
        let item = {
            let mut st = self.state.borrow_mut();
            if st.triage.is_empty() {
                return;
            }
            st.triage.remove(0)
        };
        self.show_triage_card();
        if let Some(v) = Self::vote_request(&choice) {
            let (core, this) = (self.core.clone(), Rc::downgrade(self));
            slint::spawn_local(async move {
                if let (Err(e), Some(t)) = (core.keep_vote(item.key, v).await, this.upgrade()) {
                    t.ui().set_keep_message(sstr(e.to_string()));
                }
            })
            .ok();
        }
    }

    fn render_admin(&self) {
        let users = self.state.borrow().admin_users.clone();
        let seerr = self.state.borrow().admin_seerr.clone();
        let requests_as = |id: Option<i64>| {
            if seerr.is_empty() {
                return String::new();
            }
            match id.and_then(|id| seerr.iter().find(|s| s.id == id)) {
                Some(s) => tr!("Requests as: {name}", name = s.name),
                None => tr!("Requests as: automatic"),
            }
        };
        let rows: Vec<AdminRow> = users
            .iter()
            .map(|u| AdminRow {
                key: sstr(&u.user_key),
                name: sstr(person_label(
                    &u.name,
                    &u.media_name,
                    users
                        .iter()
                        .any(|o| o.user_key != u.user_key && o.name.eq_ignore_ascii_case(&u.name)),
                )),
                avatar: self.person_avatar(&u.user_key, &u.name, &u.avatar),
                detail: sstr(if u.is_admin {
                    trn!(
                        "admin · {n} kept forever",
                        "admin · {n} kept forever",
                        u.forever_count
                    )
                } else {
                    trn!("{n} kept forever", "{n} kept forever", u.forever_count)
                }),
                forever: sstr(if u.rights.can_keep_forever {
                    tr!("Forever: allowed")
                } else {
                    tr!("Forever: blocked")
                }),
                max_forever: sstr(tr!(
                    "Max forever: {count}",
                    count = u
                        .rights
                        .max_forever
                        .map(|v| v.to_string())
                        .unwrap_or("∞".into())
                )),
                max_days: sstr(tr!(
                    "Max days: {days}",
                    days = u
                        .rights
                        .max_keep_days
                        .map(|v| v.to_string())
                        .unwrap_or("∞".into())
                )),
                seerr: sstr(requests_as(u.seerr_user_id)),
                seerr_id: sstr(u.seerr_user_id.map(|i| i.to_string()).unwrap_or_default()),
            })
            .collect();
        let choices: Vec<NamedId> = std::iter::once(NamedId {
            id: SharedString::new(),
            title: sstr(tr!("Automatic (their own Seerr user)")),
            selected: false,
        })
        .chain(seerr.iter().map(|s| NamedId {
            id: sstr(s.id.to_string()),
            title: sstr(&s.name),
            selected: false,
        }))
        .collect();
        let ui = self.ui();
        ui.set_admin_seerr_users(ModelRc::new(VecModel::from(choices)));
        ui.set_admin_users(ModelRc::new(VecModel::from(rows)));
    }

    /// Admin → People: whose Seerr user a person's requests go to (None: automatic again).
    fn admin_seerr(self: &Rc<Self>, user: String, seerr_id: String) {
        let id = seerr_id.parse::<i64>().ok();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_admin_link_seerr(user, id).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(u) => {
                    for x in t.state.borrow_mut().admin_users.iter_mut() {
                        if x.user_key == u.user_key {
                            *x = u.clone();
                        }
                    }
                    t.render_admin();
                }
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    fn load_admin(self: &Rc<Self>) {
        self.load_leaving();
        self.load_forever();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_admin_users().await;
            // Empty when the AlwyWatch server takes no requests: no "Requests as" then.
            let seerr = core.keep_admin_seerr_users().await.unwrap_or_default();
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(u) => {
                    let mut st = t.state.borrow_mut();
                    st.admin_users = u;
                    st.admin_seerr = seerr;
                    drop(st);
                    t.render_admin();
                }
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Who keeps what forever, and how much of it someone else keeps too.
    fn load_forever(self: &Rc<Self>) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_admin_forever().await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(s) => {
                    t.state.borrow_mut().forever = Some(s);
                    t.render_forever();
                }
                Err(e) => t.ui().set_forever_summary(sstr(e.to_string())),
            }
        })
        .ok();
    }

    fn render_forever(&self) {
        let Some(s) = self.state.borrow().forever.clone() else {
            return;
        };
        let ui = self.ui();
        if s.people.is_empty() {
            ui.set_forever_summary(sstr(tr!("Nobody keeps anything forever yet.")));
        } else {
            ui.set_forever_summary(sstr(tr!(
                "{titles} titles · {shared} kept by several people · {alone} by one person",
                titles = s.titles,
                shared = s.shared,
                alone = s.titles.saturating_sub(s.shared)
            )));
        }
        let names: HashMap<&str, &str> = s
            .people
            .iter()
            .map(|p| (p.user_key.as_str(), p.name.as_str()))
            .collect();
        let people: Vec<ForeverRow> = s
            .people
            .iter()
            .map(|p| {
                let overlaps = p
                    .overlaps
                    .iter()
                    .take(3)
                    .map(|o| format!("{} ({})", o.name, o.both))
                    .collect::<Vec<_>>()
                    .join(", ");
                let twin = s
                    .people
                    .iter()
                    .any(|o| o.user_key != p.user_key && o.name.eq_ignore_ascii_case(&p.name));
                ForeverRow {
                    title: sstr(format!(
                        "{}  ·  {}",
                        person_label(&p.name, &p.media_name, twin),
                        p.count
                    )),
                    detail: sstr(if overlaps.is_empty() {
                        tr!(
                            "{shared} shared · {alone} only them",
                            shared = p.shared,
                            alone = p.alone
                        )
                    } else {
                        tr!(
                            "{shared} shared · {alone} only them · most with {people}",
                            shared = p.shared,
                            alone = p.alone,
                            people = overlaps
                        )
                    }),
                    shared: p.shared as f32 / p.count.max(1) as f32,
                    avatar: self.person_avatar(&p.user_key, &p.name, &p.avatar),
                }
            })
            .collect();
        let titles: Vec<ForeverRow> = s
            .items
            .iter()
            .map(|i| ForeverRow {
                title: sstr(match i.year {
                    Some(y) => format!("{} ({y})", i.title),
                    None => i.title.clone(),
                }),
                detail: sstr(
                    i.keepers
                        .iter()
                        .map(|k| names.get(k.as_str()).copied().unwrap_or(k))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                shared: -1.0,
                avatar: Default::default(),
            })
            .collect();
        ui.set_forever_people(ModelRc::new(VecModel::from(people)));
        ui.set_forever_titles(ModelRc::new(VecModel::from(titles)));
    }

    fn admin_rights(self: &Rc<Self>, user: String, field: String) {
        let Some(u) = self
            .state
            .borrow()
            .admin_users
            .iter()
            .find(|u| u.user_key == user)
            .cloned()
        else {
            return;
        };
        let mut r = u.rights.clone();
        let cycle = |cur: Option<u32>, steps: &[Option<u32>]| {
            let i = steps
                .iter()
                .position(|s| *s == cur)
                .map(|i| (i + 1) % steps.len())
                .unwrap_or(0);
            steps[i]
        };
        match field.as_str() {
            "forever" => r.can_keep_forever = !r.can_keep_forever,
            "max-forever" => {
                r.max_forever = cycle(r.max_forever, &[None, Some(5), Some(10), Some(25)])
            }
            _ => r.max_keep_days = cycle(r.max_keep_days, &[None, Some(7), Some(30), Some(90)]),
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let res = core.keep_admin_set_rights(user, r).await;
            let Some(t) = this.upgrade() else { return };
            match res {
                Ok(nu) => {
                    for x in t.state.borrow_mut().admin_users.iter_mut() {
                        if x.user_key == nu.user_key {
                            *x = nu.clone();
                        }
                    }
                    t.render_admin();
                }
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Two clicks on the same button confirm (it rewrites dates in Maintainerr).
    fn admin_countdown(self: &Rc<Self>, collection: String, stagger: i32) {
        let confirmed = self.state.borrow().armed.as_ref() == Some(&(collection.clone(), stagger));
        if !confirmed {
            self.state.borrow_mut().armed = Some((collection, stagger));
            self.ui().set_keep_message(sstr(tr!(
                "Click the same button again to confirm the countdown."
            )));
            return;
        }
        self.state.borrow_mut().armed = None;
        let Ok(id) = collection.parse::<i64>() else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_admin_countdown(id, stagger.max(0) as u32).await;
            if let Some(t) = this.upgrade() {
                t.ui().set_keep_message(sstr(match r {
                    Ok(r) => tr!(
                        "Countdown restarted: {items} items, {skipped} skipped.",
                        items = r.reset,
                        skipped = r.skipped
                    ),
                    Err(e) => e.to_string(),
                }));
                t.load_leaving();
            }
        })
        .ok();
    }

    fn keep_token(&self, account_id: &str) -> Option<String> {
        let account = self
            .core
            .accounts()
            .ok()?
            .into_iter()
            .find(|a| a.id == account_id)?;
        match account.kind {
            ServerKind::Plex => self.tokens.get(&format!("{account_id}:plextv")),
            ServerKind::Jellyfin => self.tokens.get(account_id),
        }
    }

    /// Hides Leaving soon (Maintainerr not answering the AlwyWatch server) and Requests (Seerr
    /// through a server without it): the last answer, then asked again.
    fn apply_services(&self) {
        let ui = self.ui();
        let s = self.core.keep_services();
        ui.set_leaving_available(s.leaving_soon);
        let via_keep = self.core.seerr_connection().is_some_and(|c| c.via_keep);
        ui.set_requests_available(!via_keep || s.requests);
        ui.set_seerr_via_keep(via_keep);
        ui.set_seerr_bridge_offered(s.requests);
    }

    fn check_services(self: &Rc<Self>) {
        self.apply_services();
        if !self.core.keep_active() {
            return;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            if let Ok(services) = core.refresh_keep_services().await
                && let Some(this) = this.upgrade()
            {
                this.apply_services();
                // Requests through the server, offered since the sign-in (Seerr added or
                // switched later): connected now, unless turned off here on purpose.
                if services.requests
                    && core.seerr_connection().is_none()
                    && core
                        .get_setting(SEERR_OFF.to_owned())
                        .ok()
                        .flatten()
                        .as_deref()
                        != Some("1")
                {
                    this.seerr_bridge(true);
                }
            }
        })
        .ok();
    }

    fn refresh_keep_settings(&self) {
        self.refresh_account_settings();
        let ui = self.ui();
        self.apply_services();
        ui.set_keep_active(self.core.keep_active());
        let address = self
            .core
            .keep_connection()
            .map(|k| k.base_url)
            .or_else(|| self.core.account().map(|a| a.base_url))
            .unwrap_or_default();
        ui.set_alwy_address(sstr(address.trim_start_matches("https://")));
        if address.is_empty() {
            ui.set_alwy_state(SharedString::new());
        }
        let Some(k) = self.core.keep_connection() else {
            ui.set_keep_admin(false);
            return;
        };
        ui.set_keep_admin(k.is_admin);
        // The login AlwyWatch uses (it follows the one showing now).
        let server = self
            .core
            .accounts()
            .ok()
            .and_then(|l| l.into_iter().find(|a| a.id == k.account_id))
            .map(|a| a.server_name)
            .unwrap_or_default();
        ui.set_keep_label(sstr(if k.is_admin {
            tr!(
                "Through your login {name} on {server} · admin",
                name = k.user_name,
                server = server
            )
        } else {
            tr!(
                "Through your login {name} on {server}",
                name = k.user_name,
                server = server
            )
        }));
    }

    /// Connects an AlwyWatch server: lists the media servers it serves, then syncs the only
    /// matching account directly or lets the user choose among several.
    fn keep_connect(self: &Rc<Self>, url: String) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let listed = match normalize_server_url(url) {
                Ok(u) => core.keep_servers(u.clone()).await.map(|l| (u, l)),
                Err(e) => Err(e),
            };
            let Some(t) = this.upgrade() else { return };
            match listed {
                Ok((url, list)) => t.keep_offer(url, &list),
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Accounts on a server the AlwyWatch server serves: the one showing now is used, else the
    /// first one (no choice to make: AlwyWatch follows the login showing now).
    fn keep_offer(self: &Rc<Self>, url: String, list: &[ServerEntry]) {
        let usable: Vec<Account> = self
            .core
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| list.iter().any(|e| e.id == a.server_id))
            .collect();
        let shown = self.account();
        let Some(pick) = usable
            .iter()
            .find(|a| Some(&a.id) == shown.as_ref())
            .or(usable.first())
        else {
            let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
            self.ui().set_keep_message(sstr(tr!(
                "This needs a login on one of its servers ({servers}). Add one first: Settings → Servers → Add a server.",
                servers = names.join(", ")
            )));
            return;
        };
        self.keep_sync_as(url, pick.id.clone());
    }

    /// The login showing now changed: AlwyWatch follows it when its server is on the AlwyWatch
    /// server (otherwise the login used for AlwyWatch stays).
    fn keep_follow_shown(self: &Rc<Self>) {
        let (Some(k), Some(shown)) = (self.core.keep_connection(), self.account()) else {
            return;
        };
        if k.account_id == shown {
            return;
        }
        let Some(server) = self
            .core
            .accounts()
            .ok()
            .and_then(|l| l.into_iter().find(|a| a.id == shown))
            .map(|a| a.server_id)
        else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let Ok(list) = core.keep_servers(k.base_url.clone()).await else {
                return;
            };
            let Some(t) = this.upgrade() else { return };
            // Still the login showing now, and served there.
            if t.account().as_deref() == Some(shown.as_str()) && list.iter().any(|e| e.id == server)
            {
                t.keep_sync_as(k.base_url, shown);
            }
        })
        .ok();
    }

    fn keep_sync_as(self: &Rc<Self>, url: String, account_id: String) {
        let Some(account) = self
            .core
            .accounts()
            .ok()
            .and_then(|l| l.into_iter().find(|a| a.id == account_id))
        else {
            return;
        };
        let Some(token) = self.keep_token(&account.id) else {
            self.ui().set_keep_message(sstr(tr!(
                "Sign in to {server} again first.",
                server = account.server_name
            )));
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_connect(url, account_id, token).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(k) => {
                    t.profiles.set_last_address(&k.base_url);
                    t.ui().set_keep_message(SharedString::new());
                    t.refresh_keep_settings();
                    t.refresh_settings();
                    // Another account may mean another person: their profile, their rooms.
                    t.forget_me();
                    t.load_me();
                    t.check_welcome();
                    t.stop_rooms_poll();
                    t.start_rooms_poll();
                    t.check_links(true);
                    // Seerr through the same server when not set up yet (optional there).
                    if t.core.seerr_connection().is_none() {
                        t.seerr_bridge(true);
                    }
                    t.refresh_seerr_settings();
                }
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// On launch: "you wanted to watch X before it leaves".
    fn show_reminders(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let due = self.core.keep_due_reminders(now);
        if !due.is_empty() {
            let titles: Vec<String> = due.iter().map(|i| i.title.clone()).collect();
            self.ui().set_keep_message(sstr(tr!(
                "Reminder: you wanted to watch {titles} before it leaves.",
                titles = titles.join(", ")
            )));
        }
    }

    // ----- Seerr ----------------------------------------------------------------------------

    fn load_url_image(self: &Rc<Self>, url: String, apply: impl Fn(&Rc<Self>, Image) + 'static) {
        let key = format!("url|{url}");
        if let Some(img) = self.images.borrow().get(&key) {
            apply(self, img.clone());
            return;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let path = match core.cached_remote_image(url.clone()) {
                Some(p) => p,
                None => match core.remote_image(url).await {
                    Ok(p) => p,
                    Err(_) => return,
                },
            };
            let decoded = core
                .runtime()
                .spawn_blocking(move || {
                    let img = image::ImageReader::open(path)
                        .ok()?
                        .with_guessed_format()
                        .ok()?
                        .decode()
                        .ok()?
                        .into_rgba8();
                    let (w, h) = img.dimensions();
                    Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        img.as_raw(),
                        w,
                        h,
                    ))
                })
                .await
                .ok()
                .flatten();
            let (Some(this), Some(buf)) = (this.upgrade(), decoded) else {
                return;
            };
            let img = Image::from_rgba8(buf);
            this.images.borrow_mut().insert(key, img.clone());
            apply(&this, img);
        })
        .ok();
    }

    fn seerr_cards(
        self: &Rc<Self>,
        cards: Vec<(String, String, String, Option<String>)>,
        set: fn(&AppWindow, ModelRc<Card>),
    ) {
        let model = Rc::new(VecModel::from(
            cards
                .iter()
                .map(|(id, title, sub, _)| Card {
                    id: sstr(id),
                    title: sstr(title),
                    subtitle: sstr(sub),
                    image: Image::default(),
                    progress: 0.0,
                    played: false,
                    server: SharedString::default(),
                })
                .collect::<Vec<_>>(),
        ));
        set(&self.ui(), ModelRc::from(model.clone()));
        for (row, (id, _, _, url)) in cards.into_iter().enumerate() {
            let Some(url) = url else { continue };
            let m = Rc::downgrade(&model);
            self.load_url_image(url, move |_, img| {
                if let Some(m) = m.upgrade()
                    && let Some(mut c) = m.row_data(row)
                    && c.id.as_str() == id
                {
                    c.image = img;
                    m.set_row_data(row, c);
                }
            });
        }
    }

    fn availability_label(a: Availability) -> String {
        match a {
            Availability::Available => tr!("Available"),
            Availability::PartiallyAvailable => tr!("Partly available"),
            Availability::Processing => tr!("Downloading"),
            Availability::Pending => tr!("Requested"),
            Availability::Blocklisted => tr!("Blocked"),
            _ => String::new(),
        }
    }

    fn load_seerr_requests(self: &Rc<Self>) {
        let render = |this: &Rc<Self>, list: Vec<SeerrRequest>| {
            let cards = list
                .into_iter()
                .map(|r| {
                    let status = match r.status {
                        RequestStatus::Pending => tr!("Waiting for approval"),
                        RequestStatus::Approved => tr!("Approved"),
                        RequestStatus::Declined => tr!("Declined"),
                        RequestStatus::Failed => tr!("Failed"),
                        RequestStatus::Completed => tr!("Available"),
                    };
                    (
                        format!("{:?}:{}", r.media_type, r.tmdb_id),
                        r.title.unwrap_or_else(|| format!("TMDB {}", r.tmdb_id)),
                        status,
                        r.poster_url,
                    )
                })
                .collect();
            this.seerr_cards(cards, AppWindow::set_seerr_mine);
            this.ui().set_seerr_mine_loaded(true);
        };
        if let Ok(Some(list)) = self.core.cached_seerr_requests(false) {
            render(self, list);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_seerr_requests(false, false).await;
            // Lists the servers' titles ahead, so opening a request is instant.
            if let Some(first) = r.as_ref().ok().and_then(|l| l.first()) {
                let (kind, tmdb) = (first.media_type, first.tmdb_id);
                let core = core.clone();
                slint::spawn_local(async move {
                    let _ = core.library_title(kind, tmdb, None).await;
                })
                .ok();
            }
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(list) => render(&t, list),
                Err(e) => t.ui().set_seerr_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    fn seerr_search(self: &Rc<Self>, text: String) {
        let generation = {
            let mut st = self.state.borrow_mut();
            st.search_gen += 1;
            st.search_gen
        };
        if text.chars().count() < 2 {
            self.ui().set_seerr_results(ModelRc::default());
            return;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            async_sleep(Duration::from_millis(350)).await;
            if this
                .upgrade()
                .is_none_or(|t| t.state.borrow().search_gen != generation)
            {
                return;
            }
            let r = core.seerr_search(text).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(results) => {
                    let cards = Self::result_cards(&results);
                    t.seerr_cards(cards, AppWindow::set_seerr_results);
                }
                Err(e) => t.ui().set_seerr_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// A request or result: its library page when a server has it (to play it), else Seerr's.
    fn seerr_open(self: &Rc<Self>, key: String) {
        let Some((kind, tmdb)) = Self::seerr_key(&key) else {
            return self.navigate(Route::SeerrTitle(key));
        };
        let (core, this, prefer) = (self.core.clone(), Rc::downgrade(self), self.account());
        slint::spawn_local(async move {
            let found = core.library_title(kind, tmdb, prefer).await.ok().flatten();
            let Some(t) = this.upgrade() else { return };
            match found {
                Some(at) => {
                    t.own_ids(std::iter::once(&at.item_id), &at.account_id);
                    t.navigate(Route::Detail(at.item_id));
                }
                None => t.navigate(Route::SeerrTitle(key)),
            }
        })
        .ok();
    }

    /// "Movie:603" → (Movie, 603).
    fn seerr_key(key: &str) -> Option<(SeerrMediaType, i64)> {
        let (kind, id) = key.split_once(':')?;
        let kind = match kind {
            "Movie" => SeerrMediaType::Movie,
            "Tv" => SeerrMediaType::Tv,
            _ => return None,
        };
        Some((kind, id.parse().ok()?))
    }

    /// A Seerr title page (like Seerr's own): artwork, facts, request, saga, cast.
    fn load_seerr_title(self: &Rc<Self>, key: String) {
        let Some((kind, tmdb)) = Self::seerr_key(&key) else {
            return;
        };
        let ui = self.ui();
        // Keep what is shown when reloading the same title (after a request).
        if ui.get_seerr_title().key != key.as_str() {
            ui.set_seerr_title(SeerrPage {
                key: sstr(&key),
                ..Default::default()
            });
            ui.set_seerr_cast(ModelRc::default());
            ui.set_seerr_seasons(ModelRc::default());
            ui.set_seerr_panel(SeerrPanel::default());
            self.state.borrow_mut().seerr_seasons.clear();
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.seerr_title(kind, tmdb).await;
            let Some(this) = this.upgrade() else { return };
            let ui = this.ui();
            if ui.get_seerr_title().key != key.as_str() {
                return;
            }
            let t = match r {
                Ok(t) => t,
                Err(e) => return this.set_error(Some(e)),
            };
            let is_tv = t.media_type == SeerrMediaType::Tv;
            let mut meta: Vec<String> = t.year.iter().cloned().collect();
            if is_tv {
                meta.push(match t.seasons {
                    Some(n) => trn!("{n} season", "{n} seasons", n),
                    None => tr!("Show"),
                });
            }
            meta.extend(t.runtime_min.map(|m| {
                if m >= 60 {
                    tr!("{hours} h {minutes} min", hours = m / 60, minutes = m % 60)
                } else {
                    tr!("{minutes} min", minutes = m)
                }
            }));
            meta.extend(t.rating.map(|r| format!("★ {r:.1}")));
            let prev = ui.get_seerr_title();
            ui.set_seerr_title(SeerrPage {
                key: sstr(&key),
                title: sstr(&t.title),
                meta: sstr(meta.join("  ·  ")),
                tagline: sstr(t.tagline.clone().unwrap_or_default()),
                overview: sstr(t.overview.clone().unwrap_or_default()),
                genres: sstr(
                    t.genres
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("  ·  "),
                ),
                status: sstr(Self::availability_label(t.availability)),
                available: t.availability == Availability::Available,
                saga_id: sstr(
                    t.collection
                        .as_ref()
                        .map(|c| c.id.to_string())
                        .unwrap_or_default(),
                ),
                saga_name: sstr(
                    t.collection
                        .as_ref()
                        .map(|c| c.name.clone())
                        .unwrap_or_default(),
                ),
                backdrop: prev.backdrop,
                poster: prev.poster,
            });
            let can_request = matches!(
                t.availability,
                Availability::Unknown | Availability::Deleted
            ) || (is_tv && t.availability == Availability::PartiallyAvailable);
            ui.set_seerr_panel(SeerrPanel {
                open: false,
                key: sstr(&key),
                title: sstr(&t.title),
                overview: SharedString::new(),
                status: SharedString::new(),
                is_tv,
                can_request,
            });
            this.state.borrow_mut().seerr_selected = Some(SeerrResult {
                tmdb_id: t.tmdb_id,
                media_type: t.media_type,
                title: t.title.clone(),
                year: t.year.clone(),
                overview: t.overview.clone(),
                poster_url: t.poster_url.clone(),
                availability: t.availability,
                download_progress: t.download_progress,
            });
            for (url, poster) in [
                (t.backdrop_url.clone(), false),
                (t.poster_url.clone(), true),
            ] {
                let Some(url) = url else { continue };
                let key = key.clone();
                this.load_url_image(url, move |this, img| {
                    let ui = this.ui();
                    let mut cur = ui.get_seerr_title();
                    if cur.key == key.as_str() {
                        if poster {
                            cur.poster = img;
                        } else {
                            cur.backdrop = img;
                        }
                        ui.set_seerr_title(cur);
                    }
                });
            }
            let cast = t
                .cast
                .iter()
                .enumerate()
                .map(|(n, c)| {
                    (
                        format!("cast:{n}"),
                        c.name.clone(),
                        c.character.clone().unwrap_or_default(),
                        c.photo_url.clone(),
                    )
                })
                .collect();
            this.seerr_cards(cast, AppWindow::set_seerr_cast);
            if is_tv && can_request {
                let seasons = core.seerr_seasons(tmdb).await.unwrap_or_default();
                this.state.borrow_mut().seerr_seasons =
                    seasons.into_iter().map(|s| (s, false)).collect();
                this.render_seerr_seasons();
            }
        })
        .ok();
    }

    /// A saga: its films with their state; each opens its page.
    fn load_seerr_saga(self: &Rc<Self>, id: i64, name: String) {
        let ui = self.ui();
        ui.set_seerr_saga(SeerrPage {
            key: sstr(id.to_string()),
            title: sstr(&name),
            ..Default::default()
        });
        ui.set_seerr_parts(ModelRc::default());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.seerr_collection(id).await;
            let Some(this) = this.upgrade() else { return };
            let ui = this.ui();
            if ui.get_seerr_saga().key != id.to_string().as_str() {
                return;
            }
            let c = match r {
                Ok(c) => c,
                Err(e) => return this.set_error(Some(e)),
            };
            ui.set_seerr_saga(SeerrPage {
                key: sstr(id.to_string()),
                title: sstr(&c.name),
                meta: sstr(trn!("{n} film", "{n} films", c.parts.len())),
                overview: sstr(c.overview.clone().unwrap_or_default()),
                ..Default::default()
            });
            this.seerr_cards(Self::result_cards(&c.parts), AppWindow::set_seerr_parts);
        })
        .ok();
    }

    /// Seerr results as cards: key, title, "year · state", poster.
    fn result_cards(results: &[SeerrResult]) -> Vec<(String, String, String, Option<String>)> {
        results
            .iter()
            .map(|r| {
                let sub = [
                    r.year.clone().unwrap_or_default(),
                    Self::availability_label(r.availability),
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
                (
                    format!("{:?}:{}", r.media_type, r.tmdb_id),
                    r.title.clone(),
                    sub,
                    r.poster_url.clone(),
                )
            })
            .collect()
    }

    fn render_seerr_seasons(&self) {
        let rows: Vec<NamedId> = self
            .state
            .borrow()
            .seerr_seasons
            .iter()
            .map(|(s, sel)| {
                let status = Self::availability_label(s.availability);
                NamedId {
                    id: sstr(s.number.to_string()),
                    title: sstr(if status.is_empty() {
                        s.name.clone()
                    } else {
                        format!("{} · {status}", s.name)
                    }),
                    selected: *sel,
                }
            })
            .collect();
        self.ui()
            .set_seerr_seasons(ModelRc::new(VecModel::from(rows)));
    }

    fn seerr_request(self: &Rc<Self>, all: bool) {
        let (r, seasons) = {
            let st = self.state.borrow();
            let Some(r) = st.seerr_selected.clone() else {
                return;
            };
            let chosen: Vec<u32> = st
                .seerr_seasons
                .iter()
                .filter(|(_, sel)| *sel)
                .map(|(s, _)| s.number)
                .collect();
            (r, if all { vec![] } else { chosen })
        };
        if !all && r.media_type == SeerrMediaType::Tv && seasons.is_empty() {
            self.ui()
                .set_seerr_message(sstr(tr!("Pick at least one season.")));
            return;
        }
        if std::mem::replace(&mut self.state.borrow_mut().seerr_requesting, true) {
            return;
        }
        self.ui().set_seerr_message(sstr(tr!("Requesting…")));
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let res = core.seerr_request(r.media_type, r.tmdb_id, seasons).await;
            let Some(t) = this.upgrade() else { return };
            t.state.borrow_mut().seerr_requesting = false;
            t.ui().set_seerr_message(sstr(match res {
                Ok(_) => tr!("Requested: {title}", title = r.title),
                Err(e) => tr!(
                    "Could not request {title}: {error}",
                    title = r.title,
                    error = e
                ),
            }));
            let _ = core.refresh_seerr_requests(true, false).await;
            let key = t.ui().get_seerr_title().key.to_string();
            if t.ui().get_page() == Page::Seerrtitle {
                t.load_seerr_title(key);
            } else {
                t.load_seerr_requests();
            }
        })
        .ok();
    }

    fn refresh_seerr_settings(&self) {
        let ui = self.ui();
        self.apply_services();
        ui.set_seerr_active(self.core.seerr_active());
        if let Some(c) = self.core.seerr_connection() {
            ui.set_seerr_label(sstr(if c.via_keep {
                c.user_name
            } else {
                format!("{} @ {}", c.user_name, c.base_url)
            }));
        }
    }

    /// Seerr through the AlwyWatch server (any media server type). `quiet`: the server may
    /// simply not offer it, which is not worth a message after connecting it.
    fn seerr_bridge(self: &Rc<Self>, quiet: bool) {
        if !quiet {
            let _ = self.core.set_setting(SEERR_OFF.to_owned(), "0".to_owned());
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.seerr_connect_bridge().await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(_) => t.ui().set_seerr_message(SharedString::new()),
                Err(e) if !quiet => t.ui().set_seerr_message(sstr(e.to_string())),
                Err(_) => {}
            }
            t.refresh_seerr_settings();
        })
        .ok();
    }

    fn seerr_signed_in(self: &Rc<Self>, r: Result<SeerrSignedIn, CoreError>) {
        match r {
            Ok(s) => {
                if let Err(e) = self.tokens.put("seerr", &s.cookie) {
                    self.ui().set_seerr_message(sstr(tr!(
                        "could not store the session: {error}",
                        error = e
                    )));
                    return;
                }
                let _ = self.core.set_setting(SEERR_OFF.to_owned(), "0".to_owned());
                self.ui().set_seerr_message(SharedString::new());
                self.ui().set_seerr_step("url".into());
                self.refresh_seerr_settings();
            }
            Err(e) => self.ui().set_seerr_message(sstr(e.to_string())),
        }
    }

    fn seerr_url(self: &Rc<Self>, url: String) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = match normalize_server_url(url) {
                Ok(u) => core.seerr_info(u).await,
                Err(e) => Err(e),
            };
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(info) => {
                    t.state.borrow_mut().seerr_url = info.base_url.clone();
                    let rows: Vec<NamedId> = info
                        .logins
                        .iter()
                        .filter_map(|l| {
                            let (id, title) = match l {
                                SeerrLogin::Plex => ("plex", tr!("Sign in with Plex")),
                                SeerrLogin::Jellyfin => ("jellyfin", tr!("Sign in with Jellyfin")),
                                SeerrLogin::Emby => ("emby", tr!("Sign in with Emby")),
                                SeerrLogin::Local => ("local", tr!("Seerr account")),

                                SeerrLogin::NotConfigured => return None,
                            };
                            Some(NamedId {
                                id: id.into(),
                                title: title.into(),
                                selected: false,
                            })
                        })
                        .collect();
                    t.ui().set_seerr_logins(ModelRc::new(VecModel::from(rows)));
                    t.ui().set_seerr_step("choose".into());
                    t.ui().set_seerr_message(SharedString::new());
                }
                Err(e) => t.ui().set_seerr_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    fn seerr_login(self: &Rc<Self>, method: String) {
        let url = self.state.borrow().seerr_url.clone();
        let generation = {
            let mut st = self.state.borrow_mut();
            st.poll_gen += 1;
            st.poll_gen
        };
        let ui = self.ui();
        ui.set_seerr_code(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        match method.as_str() {
            "plex" => {
                ui.set_seerr_step("plex".into());
                slint::spawn_local(async move {
                    let pin = match core.plex_create_pin(true).await {
                        Ok(p) => p,
                        Err(e) => {
                            if let Some(t) = this.upgrade() {
                                t.seerr_signed_in(Err(e));
                            }
                            return;
                        }
                    };
                    platform::open_url(&pin.auth_url);
                    loop {
                        async_sleep(Duration::from_secs(2)).await;
                        if this
                            .upgrade()
                            .is_none_or(|t| t.state.borrow().poll_gen != generation)
                        {
                            return;
                        }
                        match core.plex_check_pin(pin.id).await {
                            Ok(token) => {
                                let r = core.seerr_sign_in_plex(url.clone(), token).await;
                                if let Some(t) = this.upgrade() {
                                    t.seerr_signed_in(r);
                                }
                                return;
                            }
                            Err(CoreError::Pending) => continue,
                            Err(e) => {
                                if let Some(t) = this.upgrade() {
                                    t.seerr_signed_in(Err(e));
                                }
                                return;
                            }
                        }
                    }
                })
                .ok();
            }
            "jellyfin" | "emby" => {
                ui.set_seerr_step("jellyfin".into());
                if method == "jellyfin" {
                    slint::spawn_local(async move {
                        let Ok(qc) = core.seerr_quick_connect_start(url.clone()).await else {
                            return;
                        };
                        if let Some(t) = this.upgrade() {
                            t.ui().set_seerr_code(sstr(&qc.code));
                        }
                        loop {
                            async_sleep(Duration::from_secs(3)).await;
                            if this
                                .upgrade()
                                .is_none_or(|t| t.state.borrow().poll_gen != generation)
                            {
                                return;
                            }
                            match core
                                .seerr_quick_connect_poll(url.clone(), qc.secret.clone())
                                .await
                            {
                                Err(CoreError::Pending) => continue,
                                r => {
                                    if let Some(t) = this.upgrade() {
                                        t.seerr_signed_in(r);
                                    }
                                    return;
                                }
                            }
                        }
                    })
                    .ok();
                }
            }
            _ => ui.set_seerr_step("local".into()),
        }
    }

    fn seerr_password(self: &Rc<Self>, user: String, pass: String) {
        let url = self.state.borrow().seerr_url.clone();
        let local = self.ui().get_seerr_step() == "local";
        self.state.borrow_mut().poll_gen += 1;
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = if local {
                core.seerr_sign_in_local(url, user, pass).await
            } else {
                core.seerr_sign_in_jellyfin(url, user, pass).await
            };
            if let Some(t) = this.upgrade() {
                t.seerr_signed_in(r);
            }
        })
        .ok();
    }

    fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window alive")
    }

    fn account(&self) -> Option<String> {
        self.state.borrow().account.clone()
    }

    /// The account an item (or library) belongs to: its server on a combined home, else the
    /// current account.
    fn account_of(&self, id: &str) -> Option<String> {
        let owner = self.state.borrow().owners.get(id).cloned();
        owner.or_else(|| self.account())
    }

    /// Items loaded through `account` (episodes, seasons, library pages) belong to it.
    fn own(&self, items: &[Item], account: &str) {
        let mut st = self.state.borrow_mut();
        for i in items {
            st.owners.insert(i.id.clone(), account.to_owned());
        }
    }

    fn own_ids<'a>(&self, ids: impl Iterator<Item = &'a String>, account: &str) {
        let mut st = self.state.borrow_mut();
        for id in ids {
            st.owners.insert(id.clone(), account.to_owned());
        }
    }

    /// Remembers "recently added" notes (what is new for a show card, what it opens).
    fn keep_notes<'a>(&self, notes: impl IntoIterator<Item = &'a RailNote>) {
        let mut st = self.state.borrow_mut();
        for n in notes {
            st.notes.insert(n.item_id.clone(), n.clone());
        }
    }

    /// Cards with a note say what is new ("S1 · E6 · Edenia") instead of year and seasons.
    fn label_noted(&self, model: &Rc<VecModel<Card>>) {
        for row in 0..model.row_count() {
            let Some(mut c) = model.row_data(row) else {
                continue;
            };
            let label = self
                .state
                .borrow()
                .notes
                .get(c.id.as_str())
                .map(|n| n.label.clone());
            if let Some(label) = label {
                c.subtitle = sstr(&label);
                model.set_row_data(row, c);
            }
        }
    }

    fn set_error(&self, e: Option<CoreError>) {
        self.ui()
            .set_error(sstr(e.map(|e| e.to_string()).unwrap_or_default()));
    }

    // ----- navigation -----------------------------------------------------------------------

    fn navigate(self: &Rc<Self>, route: Route) {
        let ui = self.ui();
        {
            let mut st = self.state.borrow_mut();
            if st.stack.last() != Some(&route) {
                // Where the page left was scrolled to, for Back; the new one starts at the top.
                if !st.stack.is_empty() {
                    let at = (ui.get_home_y(), ui.get_grid_y(), ui.get_detail_y());
                    let depth = st.stack.len() - 1;
                    st.scroll.truncate(depth);
                    st.scroll.push(at);
                }
                st.stack.push(route.clone());
                ui.set_home_y(0.0);
                ui.set_grid_y(0.0);
                ui.set_detail_y(0.0);
            }
        }
        self.show(route);
    }

    fn back(self: &Rc<Self>) {
        let route = {
            let mut st = self.state.borrow_mut();
            if st.stack.len() <= 1 {
                return;
            }
            st.stack.pop();
            let depth = st.stack.len() - 1;
            let (home, grid, detail) = st.scroll.get(depth).copied().unwrap_or_default();
            st.scroll.truncate(depth);
            let ui = self.ui();
            ui.set_home_y(home);
            ui.set_grid_y(grid);
            ui.set_detail_y(detail);
            st.stack.last().cloned()
        };
        if let Some(r) = route {
            self.show(r);
        }
    }

    fn reset(self: &Rc<Self>, route: Route) {
        {
            let mut st = self.state.borrow_mut();
            st.stack.clear();
            st.scroll.clear();
        }
        self.navigate(route);
    }

    fn show(self: &Rc<Self>, route: Route) {
        let ui = self.ui();
        ui.set_error(SharedString::new());
        // Its notes (a vote that failed, a reminder) are about the page left: one stayed on
        // every page until the app closed.
        ui.set_keep_message(SharedString::new());
        ui.set_can_go_back(self.state.borrow().stack.len() > 1);
        match route {
            Route::Home => {
                ui.set_page(Page::Home);
                self.load_home();
            }
            Route::Library(id, title) => {
                ui.set_page(Page::Library);
                ui.set_library_id(sstr(&id));
                ui.set_grid_title(sstr(&title));
                let tab = self
                    .account_of(&id)
                    .and_then(|a| self.core.get_setting(Self::tab_key(&a, &id)).ok().flatten())
                    .filter(|t| LIBRARY_TABS.contains(&t.as_str()))
                    .unwrap_or_else(|| LIBRARY_TABS[0].to_owned());
                self.show_library_tab(&id, &tab);
            }
            Route::Filtered(library, title, genre, collection) => {
                ui.set_page(Page::Library);
                ui.set_library_id(sstr(&library));
                ui.set_grid_title(sstr(&title));
                ui.set_library_tab(SharedString::new());
                self.open_grid(GridKey {
                    library,
                    sort: SortBy::Title,
                    genre,
                    collection,
                });
            }
            Route::Detail(id) => {
                ui.set_page(Page::Detail);
                self.load_detail(id);
            }
            Route::Search => {
                ui.set_page(Page::Search);
                let model = self.state.borrow().grid_model.clone();
                model.set_vec(vec![]);
                let text = ui.get_search_text().to_string();
                let kept = self.state.borrow().search_results.clone();
                match kept {
                    Some((t, items, badges)) if t == text => {
                        self.fill(&model, &items, ImageKind::Primary, POSTER_PX, 0, badges);
                    }
                    _ if !text.is_empty() => self.search(text),
                    _ => {}
                }
            }
            Route::MyList => {
                ui.set_page(Page::Mylist);
                ui.set_list_title(sstr(tr!("My list")));
                self.load_watchlist();
            }
            Route::Continue => {
                ui.set_page(Page::Mylist);
                ui.set_list_title(sstr(tr!("Continue watching")));
                ui.set_status(SharedString::new());
                let items = self.state.borrow().continue_items.clone();
                let model = self.state.borrow().grid_model.clone();
                model.set_vec(vec![]);
                let badges = self.home_accounts().len() > 1;
                self.fill(
                    &model,
                    &items,
                    ImageKind::SeriesPrimary,
                    POSTER_PX,
                    0,
                    badges,
                );
            }
            Route::Recent(library, title) => {
                ui.set_page(Page::Library);
                ui.set_library_id(sstr(&library));
                ui.set_grid_title(sstr(&title));
                ui.set_library_tab("browse".into());
                self.state.borrow_mut().grid = None;
                self.state.borrow().grid_model.set_vec(vec![]);
                self.open_grid(GridKey::library(library, SortBy::DateAdded));
            }
            Route::Leaving => {
                ui.set_page(Page::Leaving);
                self.load_leaving();
            }
            Route::Together => {
                ui.set_page(Page::Together);
                self.load_together();
            }
            Route::Room => {
                if self.in_room() {
                    ui.set_page(Page::Room);
                    self.render_room();
                } else {
                    ui.set_page(Page::Together);
                    self.load_together();
                }
            }
            Route::Triage => {
                ui.set_page(Page::Triage);
                let back = std::mem::take(&mut self.state.borrow_mut().triage_back);
                if back {
                    self.show_triage_card();
                } else {
                    self.load_triage();
                }
            }
            Route::KeepAdmin => {
                ui.set_page(Page::Keepadmin);
                self.load_admin();
            }
            Route::Dashboard => {
                ui.set_page(Page::Dashboard);
                self.dash_open();
            }
            Route::SeerrTitle(key) => {
                ui.set_page(Page::Seerrtitle);
                ui.set_seerr_message(SharedString::new());
                self.load_seerr_title(key);
            }
            Route::SeerrSaga(id, name) => {
                ui.set_page(Page::Seerrsaga);
                self.load_seerr_saga(id, name);
            }
            Route::Requests => {
                ui.set_page(Page::Requests);
                self.load_seerr_requests();
            }
            Route::Downloads => {
                ui.set_page(Page::Downloads);
                self.refresh_downloads();
            }
            Route::Settings => {
                ui.set_page(Page::Settings);
                self.refresh_seerr_settings();
                self.refresh_keep_settings();
                self.refresh_settings();
                self.render_profiles();
                self.load_me();
                self.render_links();
                self.check_links(false);
                if ui.get_keep_admin() && ui.get_settings_section() == "admin" {
                    self.admin_open();
                }
            }
            Route::Profiles => {
                ui.set_page(Page::Profiles);
                self.render_profiles();
            }
            Route::HistoryCopy => {
                ui.set_page(Page::Historycopy);
                self.render_history();
            }
            Route::Add => {
                ui.set_page(Page::Add);
                ui.set_status(SharedString::new());
                // Connected to an AlwyWatch server: offer its servers not added yet, then
                // Jellyfin or Plex directly.
                ui.set_add_step("choose".into());
                ui.set_add_create(false);
                if let Some(k) = self.core.keep_connection() {
                    ui.set_add_from_keep(true);
                    ui.set_preset_servers(ModelRc::default());
                    self.add_keep_servers(k.base_url);
                } else {
                    ui.set_add_from_keep(false);
                    // Opened from Settings (Add a server): straight to Jellyfin or Plex.
                    if !self.core.accounts().unwrap_or_default().is_empty() {
                        ui.set_add_step("direct".into());
                    }
                }
            }
        }
    }

    // ----- images ---------------------------------------------------------------------------

    /// Fills a card's image asynchronously: memory cache, then core disk cache, then network.
    fn load_image(
        self: &Rc<Self>,
        item: &Item,
        kind: ImageKind,
        px: u32,
        apply: impl Fn(&Rc<Self>, Image) + 'static,
    ) {
        let Some(account) = self.account_of(&item.id) else {
            return;
        };
        let Some(r) = image_ref(item, kind).or_else(|| image_ref(item, ImageKind::Primary)) else {
            return;
        };
        let key = format!("{account}|{}|{px}", r.path);
        if let Some(img) = self.images.borrow().get(&key) {
            apply(self, img.clone());
            return;
        }
        let (core, this, id) = (self.core.clone(), Rc::downgrade(self), item.id.clone());
        slint::spawn_local(async move {
            let path = match core.cached_image(account.clone(), id.clone(), r.clone(), px) {
                Some(p) => p,
                None => match core.image(account, id, r, px).await {
                    Ok(p) => p,
                    Err(_) => return,
                },
            };
            // Decode off the UI thread (image crate), build the Slint image on the UI thread.
            let decoded = core
                .runtime()
                .spawn_blocking(move || {
                    let img = image::ImageReader::open(path)
                        .ok()?
                        .with_guessed_format()
                        .ok()?
                        .decode()
                        .ok()?
                        .into_rgba8();
                    let (w, h) = img.dimensions();
                    Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        img.as_raw(),
                        w,
                        h,
                    ))
                })
                .await
                .ok()
                .flatten();
            let (Some(this), Some(buf)) = (this.upgrade(), decoded) else {
                return;
            };
            let img = Image::from_rgba8(buf);
            {
                let mut cache = this.images.borrow_mut();
                if cache.len() > 400 {
                    cache.clear();
                }
                cache.insert(key, img.clone());
            }
            apply(&this, img);
        })
        .ok();
    }

    fn card(&self, i: &Item) -> Card {
        let progress = match (i.user.position_ms, i.runtime_ms) {
            (p, Some(r)) if p > 0 && r > 0 => (p as f32 / r as f32).clamp(0.0, 1.0),
            _ => 0.0,
        };
        Card {
            id: sstr(&i.id),
            title: sstr(if i.kind == ItemKind::Episode {
                i.series_title.as_deref().unwrap_or(&i.title)
            } else {
                &i.title
            }),
            subtitle: sstr(item_subtitle(i)),
            image: Image::default(),
            progress,
            played: i.user.played,
            server: SharedString::default(),
        }
    }

    /// Fills `model` with cards for `items` and loads their images.
    fn fill(
        self: &Rc<Self>,
        model: &Rc<VecModel<Card>>,
        items: &[Item],
        kind: ImageKind,
        px: u32,
        offset: usize,
        badges: bool,
    ) {
        {
            let mut st = self.state.borrow_mut();
            for i in items {
                st.items.insert(i.id.clone(), i.clone());
            }
        }
        for (n, i) in items.iter().enumerate() {
            let row = offset + n;
            let mut card = self.card(i);
            // Screens mixing servers mark each card with its server.
            if badges {
                card.server = sstr(server_kind(&self.account_of(&i.id).unwrap_or_default()));
            }
            let key = image_ref(i, kind).map(|r| {
                format!(
                    "{}|{}|{px}",
                    self.account_of(&i.id).unwrap_or_default(),
                    r.path
                )
            });
            if let Some(img) = key.and_then(|k| self.images.borrow().get(&k).cloned()) {
                card.image = img;
            }
            if row < model.row_count() {
                model.set_row_data(row, card);
            } else {
                model.push(card);
            }
            let m = Rc::downgrade(model);
            let id = i.id.clone();
            self.load_image(i, kind, px, move |_, img| {
                if let Some(m) = m.upgrade()
                    && let Some(mut c) = m.row_data(row)
                    && c.id.as_str() == id
                {
                    c.image = img;
                    m.set_row_data(row, c);
                }
            });
        }
    }

    // ----- home -----------------------------------------------------------------------------

    fn render_home(self: &Rc<Self>, home: &Home, badges: bool) {
        let ui = self.ui();
        // One row like Plex: in progress and next episodes, most recent first (older caches
        // without the merged list fall back to building it here).
        let cw = if home.continue_watching.is_empty() {
            alwywatch_core::continue_watching(&home.resume, &home.next_up)
        } else {
            home.continue_watching.clone()
        };
        self.state.borrow_mut().continue_items = cw.clone();
        let cw_title = tr!("Continue watching");
        let new_in = |library: &str| tr!("New in {library}", library = library);
        let mut rails: Vec<(String, Vec<Item>)> = vec![(cw_title.clone(), cw)];
        rails.extend(
            home.latest
                .iter()
                .map(|r| (new_in(&r.library.title), r.items.clone())),
        );
        // "See all" keys, by title.
        let mut keys: HashMap<String, String> = home
            .latest
            .iter()
            .map(|r| (new_in(&r.library.title), format!("lib:{}", r.library.id)))
            .collect();
        keys.insert(cw_title.clone(), "continue".into());
        self.keep_notes(home.latest.iter().flat_map(|r| &r.notes));
        rails.retain(|(_, items)| !items.is_empty());
        let models: Vec<Rc<VecModel<Card>>> =
            rails.iter().map(|_| Rc::new(VecModel::default())).collect();
        // Posters everywhere, like Plex: an episode shows its show's poster.
        for ((t, items), m) in rails.iter().zip(&models) {
            if *t == cw_title {
                self.fill(m, items, ImageKind::SeriesPrimary, POSTER_PX, 0, badges);
            } else {
                self.fill(m, items, ImageKind::Primary, POSTER_PX, 0, badges);
                self.label_noted(m);
            }
        }
        let mut rail_vec: Vec<Rail> = rails
            .iter()
            .zip(&models)
            .map(|((t, _), m)| Rail {
                title: sstr(t),
                items: ModelRc::from(m.clone()),
                landscape: false,
                see_all: sstr(keys.get(t).cloned().unwrap_or_default()),
            })
            .collect();
        // Leaving soon (from the keep service cache), after what the user is watching.
        if self.core.keep_active()
            && self.core.keep_services().leaving_soon
            && let Ok(Some(list)) = self.core.cached_leaving_soon()
        {
            let leaving: Vec<&LeavingItem> = list
                .iter()
                .filter(|i| matches!(i.protection, Protection::None | Protection::Extended))
                .take(15)
                .collect();
            if !leaving.is_empty() {
                let model = Rc::new(VecModel::from(
                    leaving
                        .iter()
                        .map(|i| Card {
                            id: sstr(format!("leaving:{}", i.key)),
                            title: sstr(&i.title),
                            subtitle: Self::leaving_row(i).when,
                            image: Image::default(),
                            progress: 0.0,
                            played: false,
                            server: SharedString::default(),
                        })
                        .collect::<Vec<_>>(),
                ));
                for (row, i) in leaving.iter().enumerate() {
                    let Some(url) = i.poster_url.clone() else {
                        continue;
                    };
                    let m = Rc::downgrade(&model);
                    self.load_url_image(url, move |_, img| {
                        if let Some(m) = m.upgrade()
                            && let Some(mut c) = m.row_data(row)
                        {
                            c.image = img;
                            m.set_row_data(row, c);
                        }
                    });
                }
                // After what the user is watching.
                let at = rail_vec
                    .iter()
                    .take_while(|r| r.title == cw_title.as_str())
                    .count();
                rail_vec.insert(
                    at,
                    Rail {
                        title: sstr(tr!("Leaving soon")),
                        items: ModelRc::from(model),
                        landscape: false,
                        see_all: "leaving".into(),
                    },
                );
            }
        }
        ui.set_rails(ModelRc::new(VecModel::from(rail_vec)));
        ui.set_libraries(ModelRc::new(VecModel::from(
            home.libraries
                .iter()
                .filter(|l| l.kind != LibraryKind::Other)
                .map(|l| NamedId {
                    id: sstr(&l.id),
                    title: sstr(&l.title),
                    selected: false,
                })
                .collect::<Vec<_>>(),
        )));
        self.state.borrow_mut().rail_models = models;
        ui.set_status(if rails.is_empty() {
            sstr(tr!("Nothing here yet."))
        } else {
            SharedString::new()
        });
    }

    /// Accounts shown on the home: every signed-in one ("all servers together", the default,
    /// same setting as Android), the preferred one first; else only the current account.
    fn home_accounts(&self) -> Vec<String> {
        let Some(current) = self.account() else {
            return vec![];
        };
        let setting = |k: &str| self.core.get_setting(k.to_owned()).ok().flatten();
        if setting("ui.combined").as_deref() == Some("0") {
            return vec![current];
        }
        let preferred = setting("ui.preferred_account").unwrap_or(current);
        let mut all: Vec<String> = self
            .core
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.id)
            .collect();
        all.sort_by_key(|a| *a != preferred);
        all
    }

    fn render_merged(self: &Rc<Self>, homes: Vec<AccountHome>) {
        let badges = homes.len() > 1;
        let merged = merge_homes(homes);
        self.state.borrow_mut().owners.extend(merged.owners);
        self.render_home(&merged.home, badges);
    }

    fn load_home(self: &Rc<Self>) {
        let accounts = self.home_accounts();
        if accounts.is_empty() {
            return;
        }
        let cached: Vec<AccountHome> = accounts
            .iter()
            .filter_map(|a| {
                let home = self.core.cached_home(a.clone()).ok().flatten()?;
                Some(AccountHome {
                    account_id: a.clone(),
                    home,
                })
            })
            .collect();
        if cached.is_empty() {
            self.ui().set_status(sstr(tr!("Loading…")));
        } else {
            self.render_merged(cached.clone());
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let mut fresh = Vec::new();
            let mut error = None;
            for a in accounts {
                match core.refresh_home(a.clone(), false).await {
                    Ok(home) => fresh.push(AccountHome {
                        account_id: a,
                        home,
                    }),
                    // A server that does not answer keeps its cached rails.
                    Err(e) => {
                        if let Some(c) = cached.iter().find(|c| c.account_id == a) {
                            fresh.push(c.clone());
                        }
                        error = Some(e);
                    }
                }
            }
            let Some(this) = this.upgrade() else { return };
            if this.ui().get_page() != Page::Home {
                return;
            }
            if fresh.is_empty() {
                this.set_error(error);
            } else {
                this.render_merged(fresh);
            }
        })
        .ok();
    }

    // ----- library grid ---------------------------------------------------------------------

    fn tab_key(account: &str, library: &str) -> String {
        format!("ui.libtab.{account}.{library}")
    }

    /// A library like Plex: Recommended (the home's rails, this library's kind only), Browse
    /// (the whole library, sortable), Collections and Categories (genres). The tab is remembered.
    fn show_library_tab(self: &Rc<Self>, library: &str, tab: &str) {
        let Some(account) = self.account_of(library) else {
            return;
        };
        let ui = self.ui();
        ui.set_library_tab(sstr(tab));
        let _ = self
            .core
            .set_setting(Self::tab_key(&account, library), tab.to_owned());
        // Only Browse pages through the grid.
        self.state.borrow_mut().grid = None;
        self.state.borrow().grid_model.set_vec(vec![]);
        match tab {
            "browse" => self.open_grid(GridKey::library(library.to_owned(), SortBy::Title)),
            "collections" => self.load_collections(account, library.to_owned()),
            "categories" => self.load_genres(account, library.to_owned()),
            _ => self.load_recommended(account, library.to_owned()),
        }
    }

    /// The home's rails of the library's server, keeping this library's kind only.
    fn load_recommended(self: &Rc<Self>, account: String, library: String) {
        let (acct, lib) = (account.clone(), library.clone());
        let render = move |this: &Rc<Self>, home: &Home| {
            let (account, library) = (&acct, &lib);
            let kind = home
                .libraries
                .iter()
                .find(|l| &l.id == library)
                .map(|l| l.kind);
            let keep = |i: &Item| match kind {
                Some(LibraryKind::Movies) => i.kind == ItemKind::Movie,
                Some(LibraryKind::Shows) => {
                    matches!(i.kind, ItemKind::Episode | ItemKind::Series)
                }
                _ => true,
            };
            let cw: Vec<Item> = home
                .continue_watching
                .iter()
                .filter(|i| keep(i))
                .cloned()
                .collect();
            let rail = home.latest.iter().find(|r| &r.library.id == library);
            let added = rail.map(|r| r.items.clone()).unwrap_or_default();
            if let Some(r) = rail {
                this.keep_notes(&r.notes);
                // What a note opens is on this account too.
                for n in &r.notes {
                    let ids = std::iter::once(&n.open_id).chain(&n.open_season_id);
                    this.own_ids(ids, account);
                }
            }
            this.own(&cw, account);
            this.own(&added, account);
            let mut rails = vec![];
            for (recent, items, landscape) in [(false, cw, false), (true, added, false)] {
                if items.is_empty() {
                    continue;
                }
                let m = Rc::new(VecModel::default());
                // Posters, an episode showing its show's.
                this.fill(&m, &items, ImageKind::SeriesPrimary, POSTER_PX, 0, false);
                if recent {
                    this.label_noted(&m);
                }
                rails.push(Rail {
                    title: sstr(if recent {
                        tr!("Recently added")
                    } else {
                        tr!("Continue watching")
                    }),
                    items: ModelRc::from(m),
                    landscape,
                    see_all: if recent {
                        sstr(format!("lib:{library}"))
                    } else {
                        "continue".into()
                    },
                });
            }
            this.ui().set_lib_rails(ModelRc::new(VecModel::from(rails)));
        };
        self.ui().set_lib_rails(ModelRc::default());
        if let Ok(Some(h)) = self.core.cached_home(account.clone()) {
            render(self, &h);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_home(account.clone(), false).await;
            let Some(this) = this.upgrade() else { return };
            if this.ui().get_library_tab() != "recommended" {
                return;
            }
            match r {
                Ok(h) => render(&this, &h),
                Err(e) => this.set_error(Some(e)),
            }
        })
        .ok();
    }

    /// A library's collections as posters; one opens its titles.
    fn load_collections(self: &Rc<Self>, account: String, library: String) {
        let acct = account.clone();
        let render = move |this: &Rc<Self>, cols: &[Item]| {
            this.own(cols, &acct);
            let model = this.state.borrow().grid_model.clone();
            model.set_vec(vec![]);
            this.fill(&model, cols, ImageKind::Primary, POSTER_PX, 0, false);
            this.ui().set_status(sstr(if cols.is_empty() {
                tr!("No collections in this library.")
            } else {
                String::new()
            }));
        };
        if let Some(c) = self
            .core
            .cached_collections(account.clone(), library.clone())
        {
            render(self, &c);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .refresh_collections(account.clone(), library.clone(), false)
                .await;
            let Some(this) = this.upgrade() else { return };
            if this.ui().get_library_tab() != "collections" {
                return;
            }
            match r {
                Ok(c) => render(&this, &c),
                Err(e) => this.set_error(Some(e)),
            }
        })
        .ok();
    }

    /// A library's genres; one opens the library narrowed to it.
    fn load_genres(self: &Rc<Self>, account: String, library: String) {
        let render = |this: &Rc<Self>, genres: &[Genre]| {
            let rows: Vec<NamedId> = genres
                .iter()
                .map(|g| NamedId {
                    id: sstr(&g.id),
                    title: sstr(&g.name),
                    selected: false,
                })
                .collect();
            this.ui().set_genres(ModelRc::new(VecModel::from(rows)));
        };
        self.ui().set_genres(ModelRc::default());
        if let Some(g) = self.core.cached_genres(account.clone(), library.clone()) {
            render(self, &g);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_genres(account, library, false).await;
            let Some(this) = this.upgrade() else { return };
            if this.ui().get_library_tab() != "categories" {
                return;
            }
            match r {
                Ok(g) => render(&this, &g),
                Err(e) => this.set_error(Some(e)),
            }
        })
        .ok();
    }

    fn query(k: &GridKey, start: u32) -> ItemQuery {
        ItemQuery {
            library_id: Some(k.library.clone()),
            start,
            limit: PAGE,
            sort_by: k.sort,
            descending: k.sort != SortBy::Title,
            unplayed_only: false,
            search: None,
            genre_id: k.genre.clone(),
            collection_id: k.collection.clone(),
        }
    }

    fn render_sorts(&self, sort: SortBy) {
        let sorts = [
            ("title", tr!("A–Z"), SortBy::Title),
            ("added", tr!("Added"), SortBy::DateAdded),
            ("released", tr!("Released"), SortBy::ReleaseDate),
        ];
        self.ui().set_sorts(ModelRc::new(VecModel::from(
            sorts
                .iter()
                .map(|(id, t, s)| NamedId {
                    id: sstr(id),
                    title: sstr(t),
                    selected: *s == sort,
                })
                .collect::<Vec<_>>(),
        )));
    }

    fn open_grid(self: &Rc<Self>, key: GridKey) {
        let Some(account) = self.account_of(&key.library) else {
            return;
        };
        // A collection keeps its own order (saga order); libraries and genres sort.
        if key.collection.is_none() {
            self.render_sorts(key.sort);
        } else {
            self.ui().set_sorts(ModelRc::default());
        }
        let model = self.state.borrow().grid_model.clone();
        model.set_vec(vec![]);
        // Every cached page at once: the grid appears complete instantly.
        let mut loaded = 0u32;
        let mut total = 0u32;
        while let Ok(Some(p)) = self
            .core
            .cached_items(account.clone(), Self::query(&key, loaded))
        {
            self.own(&p.page.items, &account);
            self.fill(
                &model,
                &p.page.items,
                ImageKind::Primary,
                POSTER_PX,
                loaded as usize,
                false,
            );
            total = p.page.total;
            loaded += p.page.items.len() as u32;
            if p.page.items.len() < PAGE as usize {
                break;
            }
        }
        self.state.borrow_mut().grid = Some(GridState {
            key,
            loaded,
            total,
            loading: false,
        });
        self.load_page(0);
    }

    fn load_page(self: &Rc<Self>, start: u32) {
        let key = {
            let mut st = self.state.borrow_mut();
            let Some(g) = st.grid.as_mut() else { return };
            if g.loading {
                return;
            }
            g.loading = true;
            g.key.clone()
        };
        let Some(account) = self.account_of(&key.library) else {
            if let Some(g) = self.state.borrow_mut().grid.as_mut() {
                g.loading = false;
            }
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .refresh_items(account.clone(), Self::query(&key, start), false)
                .await;
            let Some(this) = this.upgrade() else { return };
            let model = this.state.borrow().grid_model.clone();
            match r {
                Ok(p) => {
                    this.own(&p.page.items, &account);
                    let same = this
                        .state
                        .borrow()
                        .grid
                        .as_ref()
                        .is_some_and(|g| g.key == key);
                    if same {
                        this.fill(
                            &model,
                            &p.page.items,
                            ImageKind::Primary,
                            POSTER_PX,
                            start as usize,
                            false,
                        );
                        if let Some(g) = this.state.borrow_mut().grid.as_mut() {
                            g.loaded = g.loaded.max(start + p.page.items.len() as u32);
                            g.total = p.page.total;
                        }
                    }
                }
                Err(e) => this.set_error(Some(e)),
            }
            if let Some(g) = this.state.borrow_mut().grid.as_mut() {
                g.loading = false;
            }
        })
        .ok();
    }

    fn grid_scrolled(self: &Rc<Self>, fraction: f32) {
        let next = {
            let st = self.state.borrow();
            match st.grid.as_ref() {
                Some(g) if !g.loading && g.loaded < g.total && fraction > 0.7 => Some(g.loaded),
                _ => None,
            }
        };
        if let Some(start) = next {
            self.load_page(start);
        }
    }

    // ----- detail ---------------------------------------------------------------------------

    /// Texts of a detail/hero panel (images are filled in asynchronously).
    fn detail_of(i: &Item) -> Detail {
        let playable = matches!(i.kind, ItemKind::Movie | ItemKind::Episode);
        let episode = i.kind == ItemKind::Episode;
        let mut meta = vec![item_subtitle(i)];
        meta.extend(i.official_rating.clone());
        meta.extend(i.community_rating.map(|r| format!("★ {r:.1}")));
        meta.extend(i.genres.first().cloned());
        meta.retain(|m| !m.is_empty());
        Detail {
            id: sstr(&i.id),
            title: sstr(if i.kind == ItemKind::Episode {
                i.series_title.as_deref().unwrap_or(&i.title)
            } else {
                &i.title
            }),
            meta: sstr(meta.join("  ·  ")),
            overview: sstr(i.overview.clone().unwrap_or_default()),
            genres: sstr(i.genres.join(", ")),
            backdrop: Image::default(),
            poster: Image::default(),
            logo: Image::default(),
            playable,
            resumable: playable && i.user.position_ms > 0,
            resume_at: sstr(clock(i.user.position_ms)),
            played: i.user.played,
            is_series: i.kind == ItemKind::Series,
            can_watchlist: matches!(i.kind, ItemKind::Movie | ItemKind::Series),
            watchlisted: i.user.favorite,
            has_trailer: false,
            crew: SharedString::default(),
            saga_id: SharedString::default(),
            saga_label: SharedString::default(),
            series_id: sstr(
                episode
                    .then_some(i.series_id.as_deref())
                    .flatten()
                    .unwrap_or(""),
            ),
            series_title: sstr(
                episode
                    .then_some(i.series_title.as_deref())
                    .flatten()
                    .unwrap_or_default(),
            ),
            season_id: sstr(
                episode
                    .then_some(i.season_id.as_deref())
                    .flatten()
                    .unwrap_or(""),
            ),
            season_label: match (episode, i.season_number) {
                (true, Some(0)) => sstr(tr!("Specials")),
                (true, Some(n)) => sstr(tr!("Season {n}", n = n)),
                _ => SharedString::default(),
            },
            next_id: SharedString::default(),
            next_label: SharedString::default(),
            next_resume_at: SharedString::default(),
        }
    }

    fn render_detail(self: &Rc<Self>, i: &Item) {
        let ui = self.ui();
        self.state
            .borrow_mut()
            .items
            .insert(i.id.clone(), i.clone());
        let playable = matches!(i.kind, ItemKind::Movie | ItemKind::Episode);
        let mut d = Self::detail_of(i);
        d.title = sstr(&i.title);
        let account = self.account_of(&i.id).unwrap_or_default();
        d.watchlisted = self.core.is_watchlisted(account.clone(), i.id.clone());
        let same = ui.get_detail().id == d.id;
        if same {
            let prev = ui.get_detail();
            d.backdrop = prev.backdrop;
            d.poster = prev.poster;
            d.logo = prev.logo;
            d.has_trailer = prev.has_trailer;
            d.crew = prev.crew;
            d.saga_id = prev.saga_id;
            d.saga_label = prev.saga_label;
            d.next_id = prev.next_id;
            d.next_label = prev.next_label;
            d.next_resume_at = prev.next_resume_at;
        } else {
            ui.set_leaving_open(SharedString::new());
        }
        ui.set_detail(d);
        if !same && i.kind != ItemKind::Episode {
            self.load_extras(account, i.id.clone());
        }
        self.render_keep_panel(i);
        if !same {
            self.refresh_keep_panel(i.id.clone());
        }
        self.load_saga(i);
        let id = i.id.clone();
        if i.kind != ItemKind::Episode && image_ref(i, ImageKind::Logo).is_some() {
            let id = id.clone();
            self.load_image(i, ImageKind::Logo, 800, move |this, img| {
                let ui = this.ui();
                let mut d = ui.get_detail();
                if d.id.as_str() == id {
                    d.logo = img;
                    ui.set_detail(d);
                }
            });
        }
        self.load_image(i, ImageKind::Backdrop, BACKDROP_PX, {
            let id = id.clone();
            move |this, img| {
                let ui = this.ui();
                let mut d = ui.get_detail();
                if d.id.as_str() == id {
                    d.backdrop = img;
                    ui.set_detail(d);
                }
            }
        });
        self.load_image(i, ImageKind::Primary, POSTER_PX, move |this, img| {
            let ui = this.ui();
            let mut d = ui.get_detail();
            if d.id.as_str() == id {
                d.poster = img;
                ui.set_detail(d);
            }
        });
        if playable {
            self.prefetch(&i.id);
        }
        if i.kind == ItemKind::Series && self.state.borrow().series.as_deref() != Some(&i.id) {
            self.load_seasons(i.id.clone());
        }
        if i.kind == ItemKind::Series && !same {
            self.load_next_episode(i.id.clone());
        }
    }

    /// A show's Play: the episode in progress, else the next unwatched one (like the phone:
    /// the server's continue watching first, then the first unwatched season's episodes).
    fn load_next_episode(self: &Rc<Self>, series: String) {
        let Some(account) = self.account_of(&series) else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let from_home = core
                .cached_home(account.clone())
                .ok()
                .flatten()
                .and_then(|h| {
                    h.continue_watching
                        .into_iter()
                        .find(|e| e.series_id.as_deref() == Some(&series))
                });
            let next = match from_home {
                Some(e) => Some(e),
                None => {
                    let seasons = match core.cached_seasons(account.clone(), series.clone()) {
                        Ok(Some(r)) => r.items,
                        _ => core
                            .refresh_seasons(account.clone(), series.clone(), false)
                            .await
                            .map(|r| r.items)
                            .unwrap_or_default(),
                    };
                    let mut todo: Vec<&Item> = seasons.iter().filter(|s| !s.user.played).collect();
                    if todo.is_empty() {
                        todo.extend(seasons.first());
                    }
                    let mut found = None;
                    for season in todo {
                        let eps = match core.cached_episodes(
                            account.clone(),
                            series.clone(),
                            season.id.clone(),
                        ) {
                            Ok(Some(r)) => r.items,
                            _ => core
                                .refresh_episodes(
                                    account.clone(),
                                    series.clone(),
                                    season.id.clone(),
                                    false,
                                )
                                .await
                                .map(|r| r.items)
                                .unwrap_or_default(),
                        };
                        found = eps
                            .iter()
                            .find(|e| e.user.position_ms > 0 && !e.user.played)
                            .or_else(|| eps.iter().find(|e| !e.user.played))
                            .cloned();
                        if found.is_some() {
                            break;
                        }
                    }
                    found
                }
            };
            // The episode's own cache is fresher than the home's (watched since on this device).
            let next = next.map(|e| {
                core.cached_item(account.clone(), e.id.clone())
                    .ok()
                    .flatten()
                    .map_or(e, |r| r.item)
            });
            let show = |next: Option<Item>| {
                let Some(this) = this.upgrade() else { return };
                let ui = this.ui();
                let mut d = ui.get_detail();
                if d.id.as_str() != series {
                    return;
                }
                match next {
                    Some(e) => {
                        this.own(std::slice::from_ref(&e), &account);
                        this.state
                            .borrow_mut()
                            .items
                            .insert(e.id.clone(), e.clone());
                        d.next_id = sstr(&e.id);
                        d.next_label = sstr(match (e.season_number, e.episode_number) {
                            (Some(s), Some(n)) => format!("S{s} E{n}"),
                            _ => e.title.clone(),
                        });
                        d.next_resume_at = if e.user.position_ms > 0 {
                            sstr(clock(e.user.position_ms))
                        } else {
                            SharedString::default()
                        };
                    }
                    None => d.next_id = SharedString::default(),
                }
                ui.set_detail(d);
            };
            show(next.clone());
            // Then the server's word on where it stands (watched elsewhere since).
            if let Some(e) = next
                && let Ok(r) = core.refresh_item(account.clone(), e.id, false).await
            {
                show(Some(r.item));
            }
        })
        .ok();
    }

    fn load_detail(self: &Rc<Self>, id: String) {
        let Some(account) = self.account_of(&id) else {
            return;
        };
        {
            let mut st = self.state.borrow_mut();
            st.series = None;
            st.season = None;
            st.episodes_model.set_vec(vec![]);
        }
        self.ui().set_seasons(ModelRc::default());
        match self.core.cached_item(account.clone(), id.clone()) {
            Ok(Some(r)) => self.render_detail(&r.item),
            _ => {
                // Show what the list already knew while the detail loads.
                let known = self.state.borrow().items.get(&id).cloned();
                match known {
                    Some(i) => self.render_detail(&i),
                    // Nothing yet: an empty page of its own (not the previous title's, which
                    // would also make the answer below look stale).
                    None => self.ui().set_detail(Detail {
                        id: sstr(&id),
                        ..Default::default()
                    }),
                }
            }
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_item(account, id.clone(), false).await;
            let Some(this) = this.upgrade() else { return };
            if this.ui().get_page() != Page::Detail
                || this.ui().get_detail().id.as_str() != id && !this.ui().get_detail().id.is_empty()
            {
                return;
            }
            match r {
                Ok(r) => this.render_detail(&r.item),
                Err(e) => this.set_error(Some(e)),
            }
        })
        .ok();
    }

    fn render_seasons(self: &Rc<Self>, seasons: &[Item]) {
        // Opened from a "recently added" card: on the season concerned.
        let pending = {
            let mut st = self.state.borrow_mut();
            let series = st.series.clone();
            st.pending_season
                .take_if(|(show, _)| Some(&*show) == series.as_ref())
                .map(|(_, season)| season)
                .filter(|p| seasons.iter().any(|s| &s.id == p))
        };
        let selected = pending
            .or_else(|| self.state.borrow().season.clone())
            .or_else(|| {
                seasons
                    .iter()
                    .find(|s| !s.user.played)
                    .or(seasons.first())
                    .map(|s| s.id.clone())
            });
        self.ui().set_seasons(ModelRc::new(VecModel::from(
            seasons
                .iter()
                .map(|s| NamedId {
                    id: sstr(&s.id),
                    title: sstr(&s.title),
                    selected: Some(&s.id) == selected.as_ref(),
                })
                .collect::<Vec<_>>(),
        )));
        if let Some(s) = selected
            && self.state.borrow().season.as_deref() != Some(&s)
        {
            self.select_season(s);
        }
    }

    fn load_seasons(self: &Rc<Self>, series: String) {
        let Some(account) = self.account_of(&series) else {
            return;
        };
        self.state.borrow_mut().series = Some(series.clone());
        if let Ok(Some(r)) = self.core.cached_seasons(account.clone(), series.clone()) {
            self.own(&r.items, &account);
            self.render_seasons(&r.items);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .refresh_seasons(account.clone(), series.clone(), false)
                .await;
            let Some(this) = this.upgrade() else { return };
            if this.state.borrow().series.as_deref() != Some(&series) {
                return;
            }
            if let Ok(r) = r {
                this.own(&r.items, &account);
                this.render_seasons(&r.items);
            }
        })
        .ok();
    }

    fn select_season(self: &Rc<Self>, season: String) {
        let Some(series) = self.state.borrow().series.clone() else {
            return;
        };
        let Some(account) = self.account_of(&series) else {
            return;
        };
        self.state.borrow_mut().season = Some(season.clone());
        let ui = self.ui();
        let seasons = ui.get_seasons();
        for i in 0..seasons.row_count() {
            if let Some(mut s) = seasons.row_data(i) {
                s.selected = s.id.as_str() == season;
                seasons.set_row_data(i, s);
            }
        }
        let model = self.state.borrow().episodes_model.clone();
        model.set_vec(vec![]);
        let acct = account.clone();
        let show = move |this: &Rc<Self>, eps: &[Item]| {
            this.own(eps, &acct);
            let model = this.state.borrow().episodes_model.clone();
            this.fill(&model, eps, ImageKind::Primary, EPISODE_PX, 0, false);
            for (n, e) in eps.iter().enumerate() {
                if let Some(mut c) = model.row_data(n) {
                    c.title = sstr(format!("{}. {}", e.episode_number.unwrap_or(0), e.title));
                    c.subtitle = sstr(e.overview.clone().unwrap_or_default());
                    model.set_row_data(n, c);
                }
            }
        };
        if let Ok(Some(r)) =
            self.core
                .cached_episodes(account.clone(), series.clone(), season.clone())
        {
            show(self, &r.items);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .refresh_episodes(account, series, season.clone(), false)
                .await;
            let Some(this) = this.upgrade() else { return };
            if this.state.borrow().season.as_deref() != Some(&season) {
                return;
            }
            if let Ok(r) = r {
                show(&this, &r.items);
            }
        })
        .ok();
    }

    // ----- playback -------------------------------------------------------------------------

    fn profile() -> DeviceProfile {
        // mpv/FFmpeg decode everything we are likely to meet; the GPU decides what is hardware.
        DeviceProfile {
            max_bitrate: Some(400_000_000),
            containers: [
                "mkv", "matroska", "webm", "mp4", "m4v", "mov", "ts", "mpegts", "m2ts", "avi",
                "ogg", "flv", "wmv", "asf",
            ]
            .map(String::from)
            .to_vec(),
            video_codecs: [
                "h264",
                "hevc",
                "vp9",
                "vp8",
                "av1",
                "mpeg2video",
                "mpeg4",
                "vc1",
                "msmpeg4v3",
                "wmv3",
            ]
            .map(String::from)
            .to_vec(),
            // mpv decodes 10-bit everywhere (in hardware or software).
            video_10bit_codecs: ["hevc", "h264", "vp9", "av1"].map(String::from).to_vec(),
            audio_codecs: [
                "aac", "mp3", "mp2", "opus", "vorbis", "flac", "alac", "ac3", "eac3", "dts", "dca",
                "truehd", "mlp", "pcm", "wmav2",
            ]
            .map(String::from)
            .to_vec(),
            subtitle_codecs: [
                "srt", "subrip", "ass", "ssa", "vtt", "webvtt", "pgssub", "dvdsub", "dvbsub",
                "ttml",
            ]
            .map(String::from)
            .to_vec(),
            max_width: None,
            max_height: None,
            hdr: vec![
                HdrKind::Sdr,
                HdrKind::Hdr10,
                HdrKind::Hdr10Plus,
                HdrKind::Hlg,
                HdrKind::DolbyVision,
            ],
        }
    }

    fn prefetch(&self, id: &str) {
        let Some(account) = self.account_of(id) else {
            return;
        };
        let playable = self
            .state
            .borrow()
            .items
            .get(id)
            .is_some_and(|i| matches!(i.kind, ItemKind::Movie | ItemKind::Episode));
        if playable {
            self.core
                .prefetch_playback_info(account, id.to_owned(), Self::profile());
        }
    }

    /// Settings → Audio & subtitles: the next choice of a setting (same values as Android).
    fn cycle_setting(&self, key: String) {
        let codes: Vec<&str> = std::iter::once("")
            .chain(LANGUAGES.iter().copied())
            .collect();
        let options: Vec<&str> = match key.as_str() {
            "player.sub_mode" => vec!["auto", "forced", "always", "off"],
            "player.audio_lang" | "player.sub_lang" => codes,
            _ => return,
        };
        let now = self
            .core
            .get_setting(key.clone())
            .ok()
            .flatten()
            .unwrap_or_default();
        let at = options.iter().position(|o| *o == now).unwrap_or(0);
        let next = options[(at + 1) % options.len()];
        let _ = self.core.set_setting(key, next.to_owned());
        self.load_player_settings();
    }

    /// Settings → Language: the next choice ("same as the computer", then each language).
    /// Text in the .slint files changes at once; text built here follows as pages refresh.
    fn cycle_language(&self) {
        let data = crate::platform::paths().data;
        let choices: Vec<String> = std::iter::once(String::new())
            .chain(i18n::languages().into_iter().map(|(c, _)| c.to_owned()))
            .collect();
        let now = crate::lang::choice(&data);
        let at = choices.iter().position(|c| *c == now).unwrap_or(0);
        crate::lang::choose(&data, &choices[(at + 1) % choices.len()]);
        self.load_language_label();
        self.load_player_settings();
    }

    fn load_language_label(&self) {
        let choice = crate::lang::choice(&crate::platform::paths().data);
        let label = if choice.is_empty() {
            let name = i18n::languages()
                .into_iter()
                .find(|(c, _)| *c == i18n::language())
                .map(|(_, n)| n)
                .unwrap_or("English");
            i18n::tr!("Same as the computer ({name})", name = name)
        } else {
            i18n::languages()
                .into_iter()
                .find(|(c, _)| *c == choice)
                .map_or(choice.clone(), |(_, n)| n.to_owned())
        };
        self.ui().set_language_label(label.into());
    }

    /// Player switches in settings (defaults: auto intro skip off, auto next episode on).
    fn load_player_settings(&self) {
        let get = |k: &str| self.core.get_setting(k.to_owned()).ok().flatten();
        let ui = self.ui();
        ui.set_auto_skip_intro(get("player.auto_skip_intro").as_deref() == Some("1"));
        ui.set_auto_next(get("player.auto_next").as_deref() != Some("0"));
        let lang = |code: &str| language_name(code).unwrap_or_else(|| code.to_owned());
        ui.set_audio_lang_label(sstr(match get("player.audio_lang").as_deref() {
            None | Some("") => tr!("Original"),
            Some(c) => lang(c),
        }));
        ui.set_sub_mode_label(sstr(match get("player.sub_mode").as_deref() {
            Some("forced") => tr!("Only foreign-language parts"),
            Some("always") => tr!("Always"),
            Some("off") => tr!("Off"),
            _ => tr!("Automatic"),
        }));
        ui.set_sub_lang_label(sstr(match get("player.sub_lang").as_deref() {
            None | Some("") => tr!("System language"),

            Some(c) => lang(c),
        }));
        let r = self.core.together_rights();
        ui.set_together_title(r.change_title);
        ui.set_together_play(r.play_pause);
        ui.set_together_seek(r.seek);
    }

    /// A line of the player's quality menu: playback starts again at that size, here.
    fn player_quality(&self, arg: &str) {
        let pick = {
            let st = self.state.borrow();
            arg.parse::<usize>()
                .ok()
                .and_then(|i| st.quality_choices.get(i).copied())
        };
        let Some(q) = pick else { return };
        let mpv = {
            let mut st = self.state.borrow_mut();
            if st.quality == q {
                return;
            }
            st.quality = q;
            st.quality_restart = true;
            st.player_mpv.clone()
        };
        if let Some(m) = mpv {
            let _ = m.command(&["quit"]);
        }
    }

    fn play(self: &Rc<Self>, id: String, from_start: bool) {
        let Some(account) = self.account_of(&id) else {
            return;
        };
        self.play_for(account, id, from_start);
    }

    /// Plays a title; in a watch together room, it becomes the room's title.
    fn play_for(self: &Rc<Self>, account: String, id: String, from_start: bool) {
        let together = if self.in_room() {
            TogetherPlay::Room { announce: true }
        } else {
            TogetherPlay::Alone
        };
        self.play_with(account, id, from_start.then_some(0), together);
    }

    /// Plays from `start` (None: where the viewer left it), alone or in a room.
    fn play_with(
        self: &Rc<Self>,
        account: String,
        id: String,
        start: Option<u64>,
        together: TogetherPlay,
    ) {
        // One player at a time: a second press while the first starts (the server answers in
        // a second or two; a double tap) would open two, both playing.
        {
            let mut st = self.state.borrow_mut();
            if st.player_starting || st.player_item.is_some() {
                return;
            }
            st.player_starting = true;
        }
        let starting = Starting(Rc::downgrade(self));
        let item = self.state.borrow().items.get(&id).cloned().or_else(|| {
            self.core
                .cached_item(account.clone(), id.clone())
                .ok()
                .flatten()
                .map(|r| r.item)
        });
        let start_ms =
            start.unwrap_or_else(|| item.as_ref().map(|i| i.user.position_ms).unwrap_or(0));
        let title = item.as_ref().map(|i| i.title.clone()).unwrap_or_default();
        let title_key = item
            .clone()
            .map_or_else(|| id.clone(), alwywatch_core::track_title_key);
        let spdif = self.ui().get_spdif();
        let setting = |k: &str| self.core.get_setting(k.to_owned()).ok().flatten();
        let track_pref = self
            .core
            .track_pref(account.clone(), title_key.clone())
            .ok()
            .flatten();
        let track_rules = TrackRules {
            audio_language: setting("player.audio_lang").unwrap_or_default(),
            subtitle_mode: setting("player.sub_mode")
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "auto".into()),
            subtitle_language: setting("player.sub_lang")
                .filter(|l| !l.is_empty())
                .unwrap_or_else(platform::system_language),
            default_audio: None,
        };
        let auto_skip_intro = setting("player.auto_skip_intro").as_deref() == Some("1");
        let auto_next = setting("player.auto_next").as_deref() != Some("0");
        let local = self.core.downloaded_file(account.clone(), id.clone());
        // A quality picked in the menu (not in a room: everyone plays the same file there).
        let quality = self
            .state
            .borrow()
            .quality
            .filter(|_| together == TogetherPlay::Alone);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let _starting = starting;
            // Downloaded: play the local original (works offline, progress is queued).
            let info = if let Some(path) = local {
                PlaybackInfo {
                    item_id: id.clone(),
                    play_session_id: None,
                    sources: vec![MediaSource {
                        id: id.clone(),
                        container: None,
                        size: None,
                        bitrate: None,
                        duration_ms: None,
                        file_name: None,
                        streams: vec![],
                        direct: HttpRequest {
                            url: path,
                            headers: vec![],
                        },
                        transcode: None,
                        supports_direct_play: true,
                        default_audio_index: None,
                        default_subtitle_index: None,
                    }],
                }
            } else {
                match core
                    .playback_info(account.clone(), id.clone(), Self::profile())
                    .await
                {
                    Ok(i) => i,
                    Err(e) => {
                        if let Some(t) = this.upgrade() {
                            t.set_error(Some(e));
                        }
                        return;
                    }
                }
            };
            let Some(src) = info.sources.into_iter().next() else {
                if let Some(t) = this.upgrade() {
                    t.ui().set_error(sstr(tr!("Nothing to play")));
                }
                return;
            };
            // The quality menu (a server file that can be converted, watching alone), and the
            // conversion at the picked size.
            // Any server file (not a download): the server converts on request, even when it
            // lists no conversion (Jellyfin, for a file it thinks plays as is).
            let menu = (src.direct.url.starts_with("http") && together == TogetherPlay::Alone)
                .then(|| alwywatch_core::quality_choices(src.clone()));
            let mut converted = None;
            if let Some(q) = quality.filter(|_| menu.is_some()) {
                let sel = TrackSelection {
                    source_id: Some(src.id.clone()),
                    quality: Some(q),
                    ..Default::default()
                };
                converted = core
                    .playback_info_with(account.clone(), id.clone(), Self::profile(), sel)
                    .await
                    .ok()
                    .and_then(|i| {
                        let t = i.sources.first()?.transcode.clone()?;
                        Some((t, i.play_session_id))
                    });
            }
            if let Some(t) = this.upgrade() {
                let ui = t.ui();
                let picked = quality.filter(|_| converted.is_some());
                ui.set_player_quality_label(sstr(alwywatch_core::quality_label(
                    src.clone(),
                    converted.is_some(),
                    picked,
                )));
                let choices = menu.unwrap_or_default();
                t.state.borrow_mut().quality_choices = choices.iter().map(|c| c.quality).collect();
                ui.set_player_qualities(ModelRc::new(VecModel::from(
                    choices
                        .into_iter()
                        .enumerate()
                        .map(|(i, c)| crate::PlayerTrack {
                            id: sstr(i.to_string()),
                            label: sstr(c.label),
                            selected: c.quality == picked,
                        })
                        .collect::<Vec<_>>(),
                )));
            }
            let subs = src
                .streams
                .iter()
                .filter(|s| s.kind == StreamKind::Subtitle)
                .filter_map(|s| {
                    s.external.as_ref().map(|r| ExternalSub {
                        url: r.url.clone(),
                        title: s.title.clone(),
                        language: s.language.clone(),
                    })
                })
                .collect();
            // Episodes around this one and skippable parts (both cached: they work offline).
            let adjacent = core
                .adjacent_episodes(account.clone(), id.clone())
                .await
                .ok();
            // Known by id for when they play (Next, the end of this one): their title on top
            // of the player and where the viewer left them.
            if let (Some(a), Some(t)) = (&adjacent, this.upgrade()) {
                let around: Vec<Item> = a.previous.iter().chain(&a.next).cloned().collect();
                t.own(&around, &account);
                let mut st = t.state.borrow_mut();
                for i in around {
                    st.items.entry(i.id.clone()).or_insert(i);
                }
            }
            let segments = core.segments(account.clone(), id.clone()).await;
            let next = adjacent
                .as_ref()
                .and_then(|a| a.next.as_ref().map(|e| e.id.clone()));
            let previous = adjacent
                .as_ref()
                .and_then(|a| a.previous.as_ref().map(|e| e.id.clone()));
            let play_method = if converted.is_some() {
                PlayMethod::Transcode
            } else {
                PlayMethod::Direct
            };
            let session = match &converted {
                Some((_, s)) => s.clone(),
                None => info.play_session_id.clone(),
            };
            let req = PlayRequest {
                media: converted.map_or_else(|| src.direct.clone(), |(t, _)| t),
                start_ms,
                view: Some(Self::player_view_link(title.clone())),
                title,
                external_subs: subs,
                tracks: TrackSetup {
                    pref: track_pref,
                    rules: track_rules,
                    save: {
                        let (core, account, key) = (core.clone(), account.clone(), title_key);
                        Box::new(move |p| {
                            let _ = core.set_track_pref(account.clone(), key.clone(), p);
                        })
                    },
                },
                spdif,
                segments: segments.unwrap_or_default(),
                has_next: next.is_some(),
                has_previous: previous.is_some(),
                auto_skip_intro,
                auto_next,
                together: None,
                notes: None,
            };
            let mut req = req;
            // Short messages for this player (an invitation arrived meanwhile).
            let (notes_tx, notes_rx) = std::sync::mpsc::channel();
            req.notes = Some(notes_rx);
            if let Some(t) = this.upgrade() {
                t.state.borrow_mut().player_notes = Some(notes_tx);
            }
            if let Some(t) = this.upgrade() {
                req.together = Some(t.player_link(together != TogetherPlay::Alone));
                t.state.borrow_mut().player_item = Some((account.clone(), id.clone()));
            }
            let linked = together != TogetherPlay::Alone;
            let src_id = src.id.clone();
            let rt = core.runtime();
            let (core2, acct2, id2) = (core.clone(), account.clone(), id.clone());
            let result = player::spawn(req, move |p| {
                let report = ProgressReport {
                    item_id: id2.clone(),
                    media_source_id: Some(src_id.clone()),
                    play_session_id: session.clone(),
                    position_ms: p.position_ms,
                    duration_ms: p.duration_ms,
                    event: p.event,
                    audio_index: None,
                    subtitle_index: None,
                    // The original file, unless a quality was picked.
                    play_method,
                };
                // A start that failed played nothing: nothing to report.
                if p.error.is_none() {
                    let (core, acct) = (core2.clone(), acct2.clone());
                    rt.spawn(async move {
                        let _ = core.report_progress(acct, report).await;
                    });
                }
                if p.event == PlaybackEvent::Stop {
                    // Back on the UI thread: refresh the page and autoplay the next episode.
                    let (next, previous, acct) = (next.clone(), previous.clone(), acct2.clone());
                    let (finished, go, error) = (p.finished, p.go, p.error);
                    let (again, at) = (id2.clone(), p.position_ms);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                            // Another quality picked: the same title again, where it was.
                            if std::mem::take(&mut t.state.borrow_mut().quality_restart) {
                                t.state.borrow_mut().player_notes = None;
                                t.player_closed();
                                t.play_with(acct, again, Some(at), TogetherPlay::Alone);
                                return;
                            }
                            let current = t.state.borrow().stack.last().cloned();
                            if let Some(r) = current {
                                t.show(r);
                            }
                            t.state.borrow_mut().player_notes = None;
                            t.player_closed();
                            let in_room = t.in_room();
                            t.room_player_gone();
                            // Said on the page the viewer is back on (after `show` cleared it);
                            // in a room, on the room's page (the room stays).
                            if let Some(e) = error {
                                t.player_failed(e);
                                return;
                            }
                            // Watching together: the room's new title, else ours (the room
                            // follows), else back to the room's page.
                            if let Some((a, i, start)) = t.take_switch() {
                                t.play_with(
                                    a,
                                    i,
                                    Some(start),
                                    TogetherPlay::Room { announce: false },
                                );
                                return;
                            }
                            let then = match go {
                                Some(player::Go::Next) => next.map(|n| (n, false)),
                                Some(player::Go::Previous) => previous.map(|p| (p, false)),
                                None if finished => next.map(|n| (n, true)),
                                None => None,
                            };
                            match then {
                                Some((n, from_start)) => t.play_for(acct, n, from_start),
                                None if in_room => t.room_player_closed(None),
                                None => {}
                            }
                        }
                    });
                }
            });
            match (result, this.upgrade()) {
                (Err(e), Some(t)) => {
                    t.state.borrow_mut().player_notes = None;
                    t.room_player_gone();
                    t.player_failed(e);
                }
                (Ok(()), Some(t)) => {
                    let alone = !t.in_room();
                    t.ui()
                        .set_player_can_together(alone && t.core.keep_active());
                    if linked {
                        t.player_started(account, id, together, start_ms);
                    }
                }
                _ => {}
            }
        })
        .ok();
    }

    /// Cast, crew and trailer of the title page: cached people first, then the server's.
    fn load_extras(self: &Rc<Self>, account: String, id: String) {
        self.ui().set_cast(ModelRc::default());
        self.state.borrow_mut().trailer = None;
        if let Some(p) = self.core.cached_people(account.clone(), id.clone()) {
            self.render_people(&id, p);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let people = core.people(account.clone(), id.clone()).await;
            let trailers = core.trailers(account.clone(), id.clone()).await;
            let Some(this) = this.upgrade() else { return };
            if this.ui().get_detail().id.as_str() != id {
                return;
            }
            if let Ok(p) = people {
                this.render_people(&id, p);
            }
            if let Some(t) = trailers.ok().and_then(|t| t.into_iter().next()) {
                this.state.borrow_mut().trailer = Some((id.clone(), account, t));
                let ui = this.ui();
                let mut d = ui.get_detail();
                d.has_trailer = true;
                ui.set_detail(d);
            }
        })
        .ok();
    }

    fn render_people(self: &Rc<Self>, id: &str, people: Vec<Person>) {
        let ui = self.ui();
        let mut d = ui.get_detail();
        if d.id.as_str() != id {
            return;
        }
        let directors: Vec<&str> = people
            .iter()
            .filter(|p| p.kind == PersonKind::Director)
            .map(|p| p.name.as_str())
            .collect();
        d.crew = sstr(if directors.is_empty() {
            String::new()
        } else {
            tr!("Directed by {names}", names = directors.join(", "))
        });
        ui.set_detail(d);
        let cast = people
            .into_iter()
            .filter(|p| p.kind == PersonKind::Actor)
            .take(30)
            .enumerate()
            .map(|(n, p)| {
                (
                    format!("{n}"),
                    p.name,
                    p.role.unwrap_or_default(),
                    p.photo_url,
                )
            })
            .collect();
        self.seerr_cards(cast, AppWindow::set_cast);
    }

    fn play_trailer(self: &Rc<Self>, id: String) {
        let Some((of, account, t)) = self.state.borrow().trailer.clone() else {
            return;
        };
        if of != id {
            return;
        }
        if let Some(item) = t.item_id {
            self.play_for(account, item, true);
        } else if let Some(url) = t.url {
            platform::open_url(&url);
        }
    }

    /// A movie's saga (TMDB collection, through Seerr), once per page. Seerr may connect a
    /// moment after start (through the AlwyWatch server): looks again for a few seconds.
    fn load_saga(self: &Rc<Self>, i: &Item) {
        let Some(tmdb) = (i.kind == ItemKind::Movie)
            .then(|| i.provider_ids.tmdb.as_deref()?.parse::<i64>().ok())
            .flatten()
        else {
            return;
        };
        {
            let mut st = self.state.borrow_mut();
            if st.saga_for.as_deref() == Some(i.id.as_str()) {
                return;
            }
            st.saga_for = Some(i.id.clone());
        }
        let (core, this, id) = (self.core.clone(), Rc::downgrade(self), i.id.clone());
        slint::spawn_local(async move {
            for _ in 0..10 {
                if core.seerr_active() {
                    break;
                }
                async_sleep(Duration::from_secs(1)).await;
            }
            if !core.seerr_active() {
                return;
            }
            let Ok(t) = core.seerr_title(SeerrMediaType::Movie, tmdb).await else {
                return;
            };
            let (Some(c), Some(this)) = (t.collection, this.upgrade()) else {
                return;
            };
            let ui = this.ui();
            let mut d = ui.get_detail();
            if d.id.as_str() == id {
                d.saga_id = sstr(c.id.to_string());
                d.saga_label = sstr(saga_label(&c.name));
                ui.set_detail(d);
            }
        })
        .ok();
    }

    /// The title page's keep panel, from the cached Leaving soon list (instant).
    fn render_keep_panel(&self, i: &Item) {
        let ui = self.ui();
        let known = self.core.keep_active()
            && self.core.keep_services().leaving_soon
            && matches!(i.kind, ItemKind::Movie | ItemKind::Series)
            && matches!(self.core.cached_leaving_soon(), Ok(Some(_)));
        if !known {
            ui.set_detail_keep(KeepPanel::default());
            return;
        }
        ui.set_detail_keep(match self.core.keep_status_of(i.clone()) {
            // In no list and never looked up: "Keep it" asks the server for its entry.
            None => KeepPanel {
                shown: true,
                title: sstr(tr!("Not scheduled for deletion")),
                choice: sstr(tr!("You haven't chosen anything")),
                ..Default::default()
            },
            Some(e) => {
                let row = Self::leaving_row(&e);
                let planned = keep_planned(&e);
                KeepPanel {
                    shown: true,
                    listed: planned,
                    looking: false,
                    error: SharedString::new(),
                    key: sstr(&e.key),
                    title: if planned {
                        sstr(keep_policy(&e, &row.when))
                    } else {
                        sstr(tr!("Not scheduled for deletion"))
                    },
                    detail: sstr(keep_detail(&e)),
                    choice: if row.vote.is_empty() {
                        sstr(tr!("You haven't chosen anything"))
                    } else {
                        row.vote
                    },
                    voted: e.my_vote.is_some(),
                }
            }
        });
    }

    /// "Keep it" on a title that is in no deletion list: its entry from the server, then the
    /// keep choices.
    fn keep_look_up(self: &Rc<Self>) {
        let id = self.ui().get_detail().id.to_string();
        let Some(item) = self.state.borrow().items.get(&id).cloned() else {
            return;
        };
        let ui = self.ui();
        let mut panel = ui.get_detail_keep();
        panel.looking = true;
        panel.error = SharedString::new();
        ui.set_detail_keep(panel);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_entry(item).await;
            let Some(t) = this.upgrade() else { return };
            let ui = t.ui();
            // Another title's page by now: nothing to show.
            if ui.get_page() != Page::Detail || ui.get_detail().id.as_str() != id {
                return;
            }
            match r {
                Ok(e) => {
                    t.render_keep_panel_of_page();
                    ui.set_leaving_open(sstr(&e.key));
                }
                Err(e) => {
                    let mut panel = ui.get_detail_keep();
                    panel.looking = false;
                    panel.error = sstr(match e {
                        CoreError::NotFound => tr!("This title is never deleted automatically."),
                        e => e.to_string(),
                    });
                    ui.set_detail_keep(panel);
                }
            }
        })
        .ok();
    }

    /// The keep panel of the title page on screen (after a vote or a refresh).
    fn render_keep_panel_of_page(&self) {
        let id = self.ui().get_detail().id.to_string();
        let item = self.state.borrow().items.get(&id).cloned();
        if let Some(i) = item {
            self.render_keep_panel(&i);
        }
    }

    /// Refreshes the Leaving soon list, then the title page's keep panel (errors stay quiet:
    /// the cached answer is still shown).
    fn refresh_keep_panel(self: &Rc<Self>, id: String) {
        if !self.core.keep_active() {
            return;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let ok = core.refresh_leaving_soon(false).await.is_ok();
            if let Some(t) = this.upgrade()
                && ok
                && t.ui().get_page() == Page::Detail
                && t.ui().get_detail().id.as_str() == id
            {
                t.render_keep_panel_of_page();
            }
        })
        .ok();
    }

    /// Adds to or removes from My list (Plex watchlist, Jellyfin favorites); the button
    /// follows at once and goes back if the server refuses.
    fn toggle_watchlist(self: &Rc<Self>, id: String) {
        let Some(account) = self.account_of(&id) else {
            return;
        };
        let set = |this: &Rc<Self>, on: bool| {
            let ui = this.ui();
            let mut d = ui.get_detail();
            if d.id.as_str() == id {
                d.watchlisted = on;
                ui.set_detail(d);
            }
        };
        let on = !self.ui().get_detail().watchlisted;
        set(self, on);
        let (core, this, id2) = (self.core.clone(), Rc::downgrade(self), id.clone());
        slint::spawn_local(async move {
            let r = core.set_watchlisted(account, id2, on).await;
            let Some(this) = this.upgrade() else { return };
            if let Err(e) = r {
                let ui = this.ui();
                let mut d = ui.get_detail();
                if d.id.as_str() == id {
                    d.watchlisted = !on;
                    ui.set_detail(d);
                }
                this.set_error(Some(e));
            }
        })
        .ok();
    }

    /// My list: every server of the home, cached first, then refreshed.
    fn load_watchlist(self: &Rc<Self>) {
        let accounts = self.home_accounts();
        let render = |this: &Rc<Self>, lists: Vec<(String, Vec<Item>)>| {
            let badges = lists.iter().filter(|(_, l)| !l.is_empty()).count() > 1;
            let mut seen = HashSet::new();
            let mut items = Vec::new();
            for (a, list) in lists {
                for i in list {
                    if seen.insert(same_as(&i)) {
                        this.own(std::slice::from_ref(&i), &a);
                        items.push(i);
                    }
                }
            }
            if items.is_empty() {
                this.ui().set_status(sstr(tr!(
                    "Nothing saved yet. On a movie or show, press the bookmark to keep it here."
                )));
            }
            let model = this.state.borrow().grid_model.clone();
            model.set_vec(vec![]);
            this.fill(&model, &items, ImageKind::Primary, POSTER_PX, 0, badges);
        };
        let cached = accounts
            .iter()
            .map(|a| {
                let items = self
                    .core
                    .cached_watchlist(a.clone())
                    .map(|r| r.items)
                    .unwrap_or_default();
                (a.clone(), items)
            })
            .collect();
        render(self, cached);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let mut lists = Vec::new();
            let mut failed = None;
            for a in accounts {
                match core.refresh_watchlist(a.clone(), false).await {
                    Ok(r) => lists.push((a, r.items)),
                    Err(e) => {
                        failed = Some(e);
                        let items = core.cached_watchlist(a.clone()).map(|r| r.items);
                        lists.push((a, items.unwrap_or_default()));
                    }
                }
            }
            let Some(this) = this.upgrade() else { return };
            if this.ui().get_page() != Page::Mylist {
                return;
            }
            if lists.iter().all(|(_, l)| l.is_empty())
                && let Some(e) = failed
            {
                this.set_error(Some(e));
            }
            render(&this, lists);
        })
        .ok();
    }

    /// A card's actions, as on the phone's long press.
    fn card_menu_open(self: &Rc<Self>, id: String, row: String) {
        if id.starts_with("leaving:") {
            return;
        }
        let Some(i) = self.state.borrow().items.get(&id).cloned() else {
            return;
        };
        let series = i.kind == ItemKind::Series;
        let playable = matches!(i.kind, ItemKind::Movie | ItemKind::Episode);
        let started = i.user.position_ms > 0;
        let in_continue = row == "continue";
        let mut actions: Vec<(&str, String)> = Vec::new();
        if playable && started {
            actions.push(("start", tr!("Play from start")));
        }
        if series {
            if !i.user.played {
                actions.push(("all_watched", tr!("Mark all episodes watched")));
            }
            actions.push(("all_unwatched", tr!("Mark all episodes unwatched")));
        } else if playable {
            if !i.user.played {
                actions.push(("watched", tr!("Mark watched")));
            }
            if i.user.played || started {
                actions.push(("unwatched", tr!("Mark unwatched")));
            }
        }
        if playable && (!i.user.played || in_continue) && (started || in_continue) {
            actions.push(("remove_cw", tr!("Remove from Continue watching")));
        }
        if i.kind == ItemKind::Episode && i.series_id.is_some() {
            actions.push(("show", tr!("Go to the show")));
            if i.season_id.is_some() {
                actions.push((
                    "season",
                    match i.season_number {
                        Some(n) => tr!("Go to season {season}", season = n),
                        None => tr!("Go to season"),
                    },
                ));
            }
        }
        if matches!(i.kind, ItemKind::Movie | ItemKind::Series) {
            let account = self.account_of(&id).unwrap_or_default();
            if self.core.is_watchlisted(account, id.clone()) {
                actions.push(("list_remove", tr!("Remove from my list")));
            } else {
                actions.push(("list_add", tr!("Add to my list")));
            }
        }
        if actions.is_empty() {
            return;
        }
        let title = match (&i.series_title, i.kind) {
            (Some(show), ItemKind::Episode) => format!("{show} · {}", i.title),
            _ => i.title.clone(),
        };
        self.state.borrow_mut().card_menu = Some((id, row));
        self.ui().set_card_menu(CardMenu {
            shown: true,
            title: sstr(title),
            actions: ModelRc::new(VecModel::from(
                actions
                    .into_iter()
                    .map(|(id, title)| NamedId {
                        id: sstr(id),
                        title: sstr(title),
                        selected: false,
                    })
                    .collect::<Vec<_>>(),
            )),
        });
    }

    /// A show's Download choices, in the card menu's sheet.
    fn series_download_menu(self: &Rc<Self>) {
        let ui = self.ui();
        let d = ui.get_detail();
        let series = d.id.to_string();
        let account = self.account_of(&series).unwrap_or_default();
        let season = {
            use slint::Model;
            ui.get_seasons().iter().find(|s| s.selected)
        };
        let mut actions: Vec<(&str, String)> = Vec::new();
        if let Some(s) = &season {
            actions.push((
                "dl_season",
                tr!("Download {season}", season = s.title.to_string()),
            ));
        }
        actions.push(("dl_show", tr!("Download the whole show")));
        match self.core.auto_download_of(account, series.clone()) {
            Some(a) if a.count > 0 => actions.push((
                "dl_auto_off",
                tr!("Stop keeping the next {n} episodes", n = a.count),
            )),
            _ => actions.push(("dl_auto", tr!("Keep the next 3 episodes downloaded"))),
        }
        self.state.borrow_mut().card_menu = Some((series, "download".into()));
        ui.set_card_menu(CardMenu {
            shown: true,
            title: d.title.clone(),
            actions: ModelRc::new(VecModel::from(
                actions
                    .into_iter()
                    .map(|(id, title)| NamedId {
                        id: sstr(id),
                        title: sstr(title),
                        selected: false,
                    })
                    .collect::<Vec<_>>(),
            )),
        });
    }

    fn card_menu_pick(self: &Rc<Self>, action: String) {
        self.ui().set_card_menu(CardMenu::default());
        let Some((id, row)) = self.state.borrow_mut().card_menu.take() else {
            return;
        };
        if self.play_on_pick(&id, &row, &action) {
            return;
        }
        let Some(i) = self.state.borrow().items.get(&id).cloned() else {
            return;
        };
        let Some(account) = self.account_of(&id) else {
            return;
        };
        match action.as_str() {
            "start" => {
                if !self.play_or_ask(id.clone(), true) {
                    self.play(id, true);
                }
                return;
            }
            "show" | "season" => {
                let Some(series) = i.series_id.clone() else {
                    return;
                };
                let season = i.season_id.clone().filter(|_| action == "season");
                self.own_ids([&series].into_iter().chain(season.as_ref()), &account);
                self.state.borrow_mut().pending_season = season.map(|s| (series.clone(), s));
                return self.navigate(Route::Detail(series));
            }
            "" => return,
            "dl_season" | "dl_show" | "dl_auto" | "dl_auto_off" => {
                let season = self.state.borrow().season.clone();
                let (core, this, title) = (self.core.clone(), Rc::downgrade(self), i.title.clone());
                slint::spawn_local(async move {
                    let r = match action.as_str() {
                        "dl_season" => match season {
                            Some(s) => core.download_season(account, id, s, None).await,
                            None => Ok(0),
                        },
                        "dl_show" => core.download_series(account, id, None).await,
                        "dl_auto" => {
                            let rule = core.default_delete_rule();
                            core.set_auto_download(account, id, title, 3, rule).await
                        }
                        _ => {
                            let rule = core.default_delete_rule();
                            core.set_auto_download(account, id, title, 0, rule).await
                        }
                    };
                    let Some(t) = this.upgrade() else { return };
                    match r {
                        Ok(n) if n > 0 => {
                            t.kick_downloads();
                            t.navigate(Route::Downloads);
                        }
                        Ok(_) => {}
                        Err(e) => t.set_error(Some(e)),
                    }
                })
                .ok();
                return;
            }
            _ => {}
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = match action.as_str() {
                "watched" | "all_watched" => {
                    core.set_played(account.clone(), id.clone(), true).await
                }
                "unwatched" | "all_unwatched" => {
                    core.set_played(account.clone(), id.clone(), false).await
                }
                "remove_cw" => {
                    core.remove_from_continue_watching(account.clone(), id.clone())
                        .await
                }
                "list_add" => {
                    core.set_watchlisted(account.clone(), id.clone(), true)
                        .await
                }
                "list_remove" => {
                    core.set_watchlisted(account.clone(), id.clone(), false)
                        .await
                }
                _ => Ok(()),
            };
            let Some(t) = this.upgrade() else { return };
            if let Err(e) = r {
                t.set_error(Some(e));
                return;
            }
            // What changed shows at once: the home and this title asked again, the page redrawn.
            let _ = core.refresh_home(account.clone(), true).await;
            if let Ok(r) = core.refresh_item(account, id, true).await {
                t.state.borrow_mut().items.insert(r.item.id.clone(), r.item);
            }
            let route = t.state.borrow().stack.last().cloned();
            if let Some(route) = route {
                t.show(route);
            }
        })
        .ok();
    }

    fn toggle_played(self: &Rc<Self>, id: String) {
        let Some(account) = self.account_of(&id) else {
            return;
        };
        let played = self
            .state
            .borrow()
            .items
            .get(&id)
            .is_some_and(|i| i.user.played);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.set_played(account.clone(), id.clone(), !played).await;
            let Some(this) = this.upgrade() else { return };
            match r {
                Ok(()) => {
                    if let Ok(i) = core.refresh_item(account, id, true).await {
                        this.render_detail(&i.item);
                    }
                }
                Err(e) => this.set_error(Some(e)),
            }
        })
        .ok();
    }

    /// "Mark as" on a show's page: every episode of the selected season (`kind` "season") or
    /// of the show ("show"). Unwatched forgets progress, so it is asked first.
    fn mark_many(self: &Rc<Self>, kind: String, played: bool) {
        let (series, season) = {
            let st = self.state.borrow();
            (st.series.clone(), st.season.clone())
        };
        let Some(series) = series else { return };
        let Some(account) = self.account_of(&series) else {
            return;
        };
        let target = if kind == "season" {
            match season {
                Some(s) => s,
                None => return,
            }
        } else {
            series.clone()
        };
        if played {
            self.mark_now(target, true);
            return;
        }
        // "Mark all 10 episodes of Silo Season 1 unwatched?"
        let show = self.ui().get_detail().title.to_string();
        let seasons = self
            .core
            .cached_seasons(account, series.clone())
            .ok()
            .flatten()
            .map(|r| r.items)
            .unwrap_or_default();
        let (count, what) = if target == series {
            let n: Option<u32> = seasons.iter().map(|s| s.child_count).sum();
            (n, show)
        } else {
            let season = seasons.iter().find(|s| s.id == target);
            let n = season
                .and_then(|s| s.child_count)
                .or_else(|| Some(self.state.borrow().episodes_model.row_count() as u32))
                .filter(|&n| n > 0);
            (
                n,
                format!(
                    "{show} {}",
                    season.map(|s| s.title.clone()).unwrap_or_default()
                ),
            )
        };
        self.state.borrow_mut().mark_pending = Some(target);
        let what = what.trim();
        self.ui().set_mark_confirm(sstr(match count {
            Some(n) => trn!(
                "Mark all {n} episode of {title} unwatched? This forgets where you stopped.",
                "Mark all {n} episodes of {title} unwatched? This forgets where you stopped.",
                n,
                title = what
            ),
            None => tr!(
                "Mark all episodes of {title} unwatched? This forgets where you stopped.",
                title = what
            ),
        }));
    }

    fn mark_confirmed(self: &Rc<Self>, ok: bool) {
        let target = self.state.borrow_mut().mark_pending.take();
        self.ui().set_mark_confirm(SharedString::new());
        if let (true, Some(t)) = (ok, target) {
            self.mark_now(t, false);
        }
    }

    /// The same path as the single-title mark (`set_played` on a season or series id marks
    /// every episode under it), then the page redrawn: show, season marks, episodes.
    fn mark_now(self: &Rc<Self>, target: String, played: bool) {
        let Some(series) = self.state.borrow().series.clone() else {
            return;
        };
        let Some(account) = self.account_of(&series) else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.set_played(account.clone(), target, played).await;
            let Some(t) = this.upgrade() else { return };
            if let Err(e) = r {
                t.set_error(Some(e));
                return;
            }
            let item = core
                .refresh_item(account.clone(), series.clone(), true)
                .await;
            let seasons = core
                .refresh_seasons(account.clone(), series.clone(), true)
                .await;
            let season = t.state.borrow().season.clone();
            if let Some(s) = &season {
                let _ = core
                    .refresh_episodes(account.clone(), series.clone(), s.clone(), true)
                    .await;
            }
            let Some(t) = this.upgrade() else { return };
            if t.state.borrow().series.as_deref() != Some(&series) {
                return;
            }
            if let Ok(i) = item {
                t.render_detail(&i.item);
            }
            if let Ok(r) = seasons {
                t.own(&r.items, &account);
                t.render_seasons(&r.items);
            }
            // Episodes again, from the fresh cache (marks and progress bars).
            if let Some(s) = season {
                t.state.borrow_mut().season = None;
                t.select_season(s);
            }
        })
        .ok();
    }

    // ----- search ---------------------------------------------------------------------------

    fn search(self: &Rc<Self>, text: String) {
        // Every server of the home (preferred first: its copy wins a title on several).
        let accounts = self.home_accounts();
        if accounts.is_empty() {
            return;
        }
        let generation = {
            let mut st = self.state.borrow_mut();
            st.search_gen += 1;
            st.search_gen
        };
        if text.chars().count() < 2 {
            let mut st = self.state.borrow_mut();
            st.search_results = None;
            st.grid_model.set_vec(vec![]);
            return;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            // Debounce: only the last keystroke within 300 ms searches.
            async_sleep(Duration::from_millis(300)).await;
            if this
                .upgrade()
                .is_none_or(|t| t.state.borrow().search_gen != generation)
            {
                return;
            }
            let mut found: Vec<(String, Item)> = Vec::new();
            let mut failed = None;
            for a in accounts {
                match core.search(a.clone(), text.clone(), 60).await {
                    Ok(items) => found.extend(items.into_iter().map(|i| (a.clone(), i))),
                    Err(e) => failed = Some(e),
                }
            }
            let Some(this) = this.upgrade() else { return };
            if this.state.borrow().search_gen != generation {
                return;
            }
            if found.is_empty()
                && let Some(e) = failed
            {
                this.set_error(Some(e));
            }
            // One per title across servers.
            let mut seen = HashSet::new();
            found.retain(|(_, i)| seen.insert(same_as(i)));
            let badges = found.iter().map(|(a, _)| a).collect::<HashSet<_>>().len() > 1;
            for (a, i) in &found {
                this.own(std::slice::from_ref(i), a);
            }
            let items: Vec<Item> = found.into_iter().map(|(_, i)| i).collect();
            let model = this.state.borrow().grid_model.clone();
            model.set_vec(vec![]);
            this.fill(&model, &items, ImageKind::Primary, POSTER_PX, 0, badges);
            this.state.borrow_mut().search_results = Some((text, items, badges));
        })
        .ok();
    }

    // ----- accounts and settings -----------------------------------------------------------

    fn refresh_settings(&self) {
        let ui = self.ui();
        let current = self.account();
        let rows: Vec<AccountRow> = self
            .core
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| {
                let detail = format!("{:?} · {}", a.kind, a.base_url);
                AccountRow {
                    current: Some(&a.id) == current.as_ref(),
                    label: sstr(tr!(
                        "{name} on {server}",
                        name = a.user_name,
                        server = a.server_name
                    )),
                    detail: sstr(detail),
                    id: sstr(a.id),
                }
            })
            .collect();
        ui.set_accounts(ModelRc::new(VecModel::from(rows)));
        ui.set_cache_size(sstr(tr!(
            "{size} MB",
            size = self.core.image_cache_bytes() / (1024 * 1024)
        )));
        ui.set_about(sstr(tr!(
            "AlwyWatch {version} (core {core}) · GNU AGPL v3 or later · player: {player}",
            version = env!("CARGO_PKG_VERSION"),
            core = core_version(),
            player = if crate::mpv::available() {
                tr!("libmpv found")
            } else {
                tr!("libmpv missing")
            }
        )));
    }

    fn signed_in(self: &Rc<Self>, s: SignedIn) {
        if let Some(t) = &s.plex_account_token {
            let _ = self.tokens.put(&format!("{}:plextv", s.account.id), t);
            self.core
                .set_plex_account_token(s.account.id.clone(), t.clone());
        }
        if let Err(e) = self.tokens.put(&s.account.id, &s.token) {
            self.ui()
                .set_status(sstr(tr!("could not store the token: {error}", error = e)));
            return;
        }
        let _ = self
            .core
            .set_setting("current_account".into(), s.account.id.clone());

        // Leaving soon + Seerr through the AlwyWatch server the user started from (if any).
        if let Some(token) = self.keep_token(&s.account.id) {
            let (core, this, id) = (self.core.clone(), Rc::downgrade(self), s.account.id.clone());
            let (id2, token2) = (id.clone(), token.clone());
            slint::spawn_local(async move {
                if let Ok(Some(_)) = core.keep_onboard(id, token).await
                    && let Some(t) = this.upgrade()
                {
                    t.refresh_keep_settings();
                    t.refresh_seerr_settings();
                    t.check_welcome();
                    t.start_rooms_poll();
                }
                // Signed in to an AlwyWatch account: this sign-in joins it.
                let _ = core.account_link_media(id2, token2).await;
                // A new account may be the same person as the synced one.
                if let Some(t) = this.upgrade() {
                    t.check_links(true);
                    // It shows now: AlwyWatch follows it when its server is served there.
                    t.keep_follow_shown();
                }
            })
            .ok();
        }
        self.state.borrow_mut().account = Some(s.account.id);
        self.images.borrow_mut().clear();
        self.reset(Route::Home);
    }

    fn use_account(self: &Rc<Self>, id: String) {
        let _ = self.core.set_setting("current_account".into(), id.clone());
        self.state.borrow_mut().account = Some(id);
        self.images.borrow_mut().clear();
        self.keep_follow_shown();
        self.reset(Route::Home);
    }

    fn remove_account(self: &Rc<Self>, id: String) {
        let _ = self.core.remove_account(id.clone());
        let _ = self.tokens.remove(&id);
        self.links_account_removed(&id);
        self.render_links();
        if self.account().as_deref() == Some(&id) {
            let next = self
                .core
                .accounts()
                .ok()
                .and_then(|a| a.into_iter().next())
                .map(|a| a.id);
            self.state.borrow_mut().account = next.clone();
            if next.is_none() {
                self.reset(Route::Add);
                return;
            }
            self.keep_follow_shown();
        }
        self.refresh_settings();
    }

    /// Moves between the add-server steps. "alwywatch" (sign in to AlwyWatch), "create" (create
    /// an AlwyWatch account) and "direct" (Jellyfin or Plex only) are the three paths from the
    /// welcome step; "back" returns to the path the user came from, or to the app when the page
    /// was opened from it (Settings: Add a server, Connect to AlwyWatch).
    fn add_choose(self: &Rc<Self>, which: String) {
        let ui = self.ui();
        ui.set_status(SharedString::new());
        self.state.borrow_mut().poll_gen += 1;
        let step = ui.get_add_step();
        let in_app = !self.core.accounts().unwrap_or_default().is_empty();
        let first = step == "choose" || step == "direct" || step == "alwywatch";
        if which == "back" && in_app && first && !ui.get_add_from_keep() {
            self.back();
            return;
        }
        let which = match which.as_str() {
            "back" if step == "receive" || step == "account" => "alwywatch".into(),
            "back" if step == "create" => "create".into(),
            "back" if first => String::new(),
            "back" if ui.get_add_from_keep() => String::new(),
            "back" if self.state.borrow().preset_url.is_empty() => "direct".into(),
            "back" => "alwywatch".into(),
            _ => which,
        };
        match which.as_str() {
            "alwywatch" | "create" => {
                // The address another profile of this computer used.
                if ui.get_keep_address().is_empty()
                    && let Some(url) = self.profiles.last_address()
                {
                    ui.set_keep_address(sstr(url.trim_start_matches("https://")));
                }
                ui.set_transfer_status(SharedString::new());
                ui.set_add_create(which == "create");
                ui.set_add_step("alwywatch".into());
            }
            "direct" => {
                // No AlwyWatch server on this path: forget one picked earlier.
                let _ = self.core.keep_clear_preset();
                let mut st = self.state.borrow_mut();
                st.preset_url.clear();
                st.preset_plex = None;
                drop(st);
                ui.set_add_step("direct".into());
            }
            "jellyfin" => ui.set_add_step("jellyfin-url".into()),
            "account" => {
                ui.set_transfer_status(SharedString::new());
                ui.set_add_step("account".into());
            }
            "plex" => {
                ui.set_add_step("plex".into());
                ui.set_plex_code("…".into());
                self.plex_flow();
            }
            _ => ui.set_add_step("choose".into()),
        }
    }

    /// Lists an AlwyWatch server's media servers so nobody types their URLs.
    fn add_keep_servers(self: &Rc<Self>, url: String) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let url = match normalize_server_url(url) {
                Ok(u) => u,
                Err(e) => return report(&this, e),
            };
            let list = match core.keep_servers(url.clone()).await {
                Ok(l) => l,
                Err(e) => return report(&this, e),
            };
            let Some(t) = this.upgrade() else { return };
            let signed_in: Vec<String> = t
                .core
                .accounts()
                .unwrap_or_default()
                .into_iter()
                .map(|a| a.server_id)
                .collect();
            // Adding while connected: only the servers not added yet.
            let hide_added = t.ui().get_add_from_keep();
            t.ui().set_preset_servers(ModelRc::new(VecModel::from(
                list.iter()
                    .enumerate()
                    .filter(|(_, e)| !(hide_added && signed_in.contains(&e.id)))
                    .map(|(i, e)| {
                        let plex = e.kind == ServerType::Plex;
                        ServerChoice {
                            id: sstr(i.to_string()),
                            title: sstr(&e.name),
                            subtitle: sstr(match (plex, signed_in.contains(&e.id)) {
                                (true, true) => tr!("Plex · already signed in"),
                                (false, true) => tr!("Jellyfin · already signed in"),
                                (true, false) => tr!("Plex · sign in on plex.tv"),
                                (false, false) => tr!("Jellyfin · Quick Connect or password"),
                            }),
                            plex,
                        }
                    })
                    .collect::<Vec<_>>(),
            )));
            t.ui().set_status(if list.is_empty() {
                sstr(tr!("This AlwyWatch server lists no media servers."))
            } else {
                SharedString::new()
            });
            let mut st = t.state.borrow_mut();
            st.preset_servers = list;
            st.preset_url = url;
        })
        .ok();
    }

    fn add_keep_pick(self: &Rc<Self>, index: String) {
        let (entry, url) = {
            let st = self.state.borrow();
            let e = index
                .parse::<usize>()
                .ok()
                .and_then(|i| st.preset_servers.get(i).cloned());
            (e, st.preset_url.clone())
        };
        let Some(entry) = entry else { return };
        // Already signed in to that server (Connect to AlwyWatch from Settings): that login
        // connects, no second sign-in.
        if let Some(a) = self
            .core
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.server_id == entry.id)
        {
            // Signed in to an AlwyWatch account just before: this login joins it.
            if let Some(token) = self.keep_token(&a.id)
                && self.core.account().is_some()
            {
                let (core, id) = (self.core.clone(), a.id.clone());
                slint::spawn_local(async move {
                    let _ = core.account_link_media(id, token).await;
                })
                .ok();
            }
            self.keep_sync_as(url, a.id);
            self.back();
            return;
        }
        let _ = self.core.keep_set_preset(url);
        match entry.kind {
            ServerType::Jellyfin => self.jellyfin_url(entry.url),
            ServerType::Plex => {
                self.state.borrow_mut().preset_plex = Some(entry.id);
                self.add_choose("plex".into());
            }
        }
    }

    fn jellyfin_url(self: &Rc<Self>, url: String) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let url = match normalize_server_url(url) {
                Ok(u) => u,
                Err(e) => return report(&this, e),
            };
            if let Err(e) = core.jellyfin_server_info(url.clone()).await {
                return report(&this, e);
            }
            let quick = core.jellyfin_quick_connect_start(url.clone()).await.ok();
            let Some(t) = this.upgrade() else { return };
            let generation = {
                let mut st = t.state.borrow_mut();
                st.jellyfin_url = url.clone();
                st.poll_gen += 1;
                st.poll_gen
            };
            let ui = t.ui();
            ui.set_quick_code(sstr(
                quick.as_ref().map(|q| q.code.clone()).unwrap_or_default(),
            ));
            ui.set_add_step("jellyfin".into());
            ui.set_status(SharedString::new());
            drop(t);
            let Some(mut q) = quick else { return };
            loop {
                async_sleep(Duration::from_secs(3)).await;
                if this
                    .upgrade()
                    .is_none_or(|t| t.state.borrow().poll_gen != generation)
                {
                    return;
                }
                match core
                    .jellyfin_quick_connect_poll(url.clone(), q.secret.clone())
                    .await
                {
                    Ok(s) => {
                        if let Some(t) = this.upgrade() {
                            t.signed_in(s);
                        }
                        return;
                    }
                    Err(CoreError::Pending) => continue,
                    // Codes expire after a few minutes: show a fresh one instead of failing.
                    Err(e) => match core.jellyfin_quick_connect_start(url.clone()).await {
                        Ok(fresh) => {
                            if let Some(t) = this.upgrade() {
                                t.ui().set_quick_code(sstr(&fresh.code));
                            }
                            q = fresh;
                        }
                        Err(_) => return report(&this, e),
                    },
                }
            }
        })
        .ok();
    }

    fn jellyfin_sign_in(self: &Rc<Self>, user: String, pass: String) {
        let url = self.state.borrow().jellyfin_url.clone();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            match core.jellyfin_sign_in(url, user, pass).await {
                Ok(s) => {
                    if let Some(t) = this.upgrade() {
                        t.signed_in(s);
                    }
                }
                Err(e) => report(&this, e),
            }
        })
        .ok();
    }

    fn plex_flow(self: &Rc<Self>) {
        let generation = self.state.borrow().poll_gen;
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let pin = match core.plex_create_pin(true).await {
                Ok(p) => p,
                Err(e) => return report(&this, e),
            };
            platform::open_url(&pin.auth_url);
            if let Some(t) = this.upgrade() {
                t.ui()
                    .set_plex_code(sstr(&pin.code[..pin.code.len().min(8)]));
            }
            loop {
                async_sleep(Duration::from_secs(2)).await;
                if this
                    .upgrade()
                    .is_none_or(|t| t.state.borrow().poll_gen != generation)
                {
                    return;
                }
                match core.plex_check_pin(pin.id).await {
                    Ok(token) => {
                        let servers = match core.plex_servers(token.clone()).await {
                            Ok(s) => s,
                            Err(e) => return report(&this, e),
                        };
                        let Some(t) = this.upgrade() else { return };
                        t.ui().set_plex_servers(ModelRc::new(VecModel::from(
                            servers
                                .iter()
                                .map(|s| NamedId {
                                    id: sstr(&s.machine_id),
                                    title: sstr(if s.owned {
                                        tr!("{name} (yours)", name = s.name)
                                    } else {
                                        s.name.clone()
                                    }),
                                    selected: false,
                                })
                                .collect::<Vec<_>>(),
                        )));
                        let mut st = t.state.borrow_mut();
                        st.plex_token = token;
                        let preset = st
                            .preset_plex
                            .take()
                            .filter(|id| servers.iter().any(|s| &s.machine_id == id));
                        st.plex_servers = servers;
                        drop(st);
                        match preset {
                            Some(id) => t.plex_pick(id),
                            None => t.ui().set_add_step("plex-servers".into()),
                        }
                        return;
                    }
                    Err(CoreError::Pending) => continue,
                    Err(e) => return report(&this, e),
                }
            }
        })
        .ok();
    }

    fn plex_pick(self: &Rc<Self>, machine: String) {
        let (token, server) = {
            let st = self.state.borrow();
            (
                st.plex_token.clone(),
                st.plex_servers
                    .iter()
                    .find(|s| s.machine_id == machine)
                    .cloned(),
            )
        };
        let Some(server) = server else { return };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            match core.plex_connect(token, server, None).await {
                Ok(s) => {
                    if let Some(t) = this.upgrade() {
                        t.signed_in(s);
                    }
                }
                Err(e) => report(&this, e),
            }
        })
        .ok();
    }

    // ----- device transfer (same bundle as Android) -----------------------------------------

    /// What a signed-in device sends: the core's accounts and settings, plus their tokens.
    fn transfer_bundle(&self) -> Result<String, CoreError> {
        let mut tokens = serde_json::Map::new();
        for a in self.core.accounts()? {
            for k in [
                a.id.clone(),
                format!("{}:plextv", a.id),
                format!("{}:plexhome", a.id),
            ] {
                if let Some(t) = self.tokens.get(&k) {
                    tokens.insert(k, t.into());
                }
            }
        }
        if let Some(t) = self.tokens.get("seerr") {
            tokens.insert("seerr".into(), t.into());
        }
        let core = self
            .core
            .export_config(SYNCED_SETTINGS.iter().map(|k| (*k).to_owned()).collect())?;
        Ok(serde_json::json!({ "core": core, "tokens": tokens }).to_string())
    }

    /// New device: stores what arrived and signs in with it. Returns the account to open.
    fn restore_bundle(&self, json: &str) -> Result<Option<String>, CoreError> {
        let invalid = |detail: String| CoreError::Invalid { detail };
        let v: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| invalid(tr!("unreadable transfer: {error}", error = e)))?;
        let core = v["core"]
            .as_str()
            .ok_or_else(|| invalid(tr!("unreadable transfer")))?;
        let ids = self.core.import_config(core.to_owned())?;
        if let Some(t) = v["tokens"].as_object() {
            for (k, t) in t {
                if let Some(t) = t.as_str() {
                    self.tokens.put(k, t).map_err(|e| {
                        invalid(tr!("could not store the token: {error}", error = e))
                    })?;
                }
            }
        }
        activate_saved(&self.core, &self.tokens);
        let current = self
            .core
            .get_setting("current_account".into())
            .ok()
            .flatten()
            .filter(|c| ids.contains(c));
        Ok(current.or_else(|| ids.into_iter().next()))
    }

    /// Signed in to an AlwyWatch account (no media server yet): its server's list, to sign in
    /// to the server the user watches on.
    fn account_ready(self: &Rc<Self>, url: String, name: String) {
        let ui = self.ui();
        ui.set_account_note(sstr(tr!(
            "Signed in as {name}. Now sign in to the server you watch on.",
            name = name
        )));
        ui.set_add_create(false);
        ui.set_add_step("alwywatch".into());
        ui.set_keep_address(sstr(&url));
        self.profiles.set_last_address(&url);
        self.add_keep_servers(url);
        self.refresh_account_settings();
        // The account's settings (and server list) arrive.
        let core = self.core.clone();
        slint::spawn_local(async move {
            let _ = core.sync_settings().await;
        })
        .ok();
    }

    /// "AlwyWatch account" step: username and password.
    fn account_login(self: &Rc<Self>, url: String, user: String, pass: String) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        self.ui().set_transfer_status(SharedString::new());
        slint::spawn_local(async move {
            let url = match normalize_server_url(url) {
                Ok(u) => u,
                Err(e) => return report(&this, e),
            };
            let r = core
                .account_login(url.clone(), user, pass, crate::hostname())
                .await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(s) => {
                    if let Err(e) = t.tokens.put(ACCOUNT_TOKEN, &s.token) {
                        return t.ui().set_transfer_status(sstr(tr!(
                            "could not store the token: {error}",
                            error = e
                        )));
                    }
                    // The Jellyfin login an invitation made, signed in with the same password:
                    // straight to the library, as right after sign-up.
                    t.account_or_library(url, s);
                }
                Err(e) => t.ui().set_transfer_status(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// After an AlwyWatch account sign-in or sign-up: the library when it brought a Jellyfin
    /// login signed in on this device (an invitation's), else the account's next step.
    fn account_or_library(self: &Rc<Self>, url: String, s: AccountSignedIn) {
        match s.jellyfin {
            Some(j) => {
                self.ui().set_keep_address(sstr(&url));
                self.profiles.set_last_address(&url);
                let _ = self.core.keep_set_preset(url);
                self.refresh_account_settings();
                let core = self.core.clone();
                slint::spawn_local(async move {
                    let _ = core.sync_settings().await;
                })
                .ok();
                self.signed_in(j);
            }
            None => self.account_ready(url, s.account.name),
        }
    }

    /// "Forgot password?": asks the server to email a link (the web page sets the new one).
    fn account_forgot(self: &Rc<Self>, url: String, login: String) {
        let ui = self.ui();
        ui.set_transfer_status(SharedString::new());
        ui.set_account_info(SharedString::new());
        if login.trim().is_empty() {
            ui.set_transfer_status(sstr(tr!(
                "Type your username or email first, then choose “Forgot password?”."
            )));
            return;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let url = match normalize_server_url(url) {
                Ok(u) => u,
                Err(e) => return report(&this, e),
            };
            let r = core.account_forgot_password(url, login).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(()) => t.ui().set_account_info(sstr(tr!(
                    "If this account has an email address, a link to set a new password is on its way. It works for an hour."
                ))),
                Err(e) => t.ui().set_transfer_status(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Settings: the AlwyWatch account row.
    fn refresh_account_settings(&self) {
        self.ui().set_alwy_account(sstr(
            self.core.account().map(|a| a.name).unwrap_or_default(),
        ));
    }

    /// Settings → Account → "Sign out of AlwyWatch": the AlwyWatch account and server go for
    /// this profile (choices stay on the server); its Jellyfin and Plex logins stay.
    fn alwywatch_sign_out(self: &Rc<Self>) {
        self.leave_room(None, true);
        let _ = self.core.keep_disconnect();
        self.stop_rooms_poll();
        self.forget_me();
        self.links_reset();
        self.refresh_keep_settings();
        self.refresh_settings();
        // A bridged Seerr goes with it.
        self.refresh_seerr_settings();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            if core.account().is_some() {
                let _ = core.account_sign_out().await;
            }
            if let Some(t) = this.upgrade() {
                let _ = t.tokens.remove(ACCOUNT_TOKEN);
                t.refresh_keep_settings();
            }
        })
        .ok();
    }

    /// Settings → Account → "Connect to AlwyWatch": the same steps as a new profile's.
    fn alwy_connect(self: &Rc<Self>, create: bool) {
        self.navigate(Route::Add);
        self.add_choose(if create { "create" } else { "alwywatch" }.into());
    }

    /// "Create an AlwyWatch account", address step: what the server accepts.
    fn create_check(self: &Rc<Self>, url: String) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        self.ui().set_status(SharedString::new());
        slint::spawn_local(async move {
            let url = match normalize_server_url(url) {
                Ok(u) => u,
                Err(e) => return report(&this, e),
            };
            let r = core.account_options(url).await;
            let Some(t) = this.upgrade() else { return };
            let ui = t.ui();
            match r {
                Ok(o) if o.registration == "closed" => ui.set_status(sstr(tr!(
                    "This server doesn't accept new accounts without an invitation. Ask its owner for one, or sign in with your Jellyfin or Plex login."
                ))),
                Ok(o) => {
                    ui.set_create_invite_needed(o.registration == "invite");
                    t.create_invite_reset();
                    // A code typed earlier: what it gives, for this server.
                    let typed = ui.get_create_invite().to_string();
                    if !typed.trim().is_empty() {
                        t.create_invite_edited(ui.get_keep_address().to_string(), typed);
                    }
                    ui.set_transfer_status(SharedString::new());
                    ui.set_add_step("create".into());
                }
                Err(e) => ui.set_status(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// "Create an AlwyWatch account": invitation, username and password; then, signed in, the
    /// server's list to add a login.
    fn account_create(
        self: &Rc<Self>,
        url: String,
        invite: String,
        user: String,
        pass: String,
        again: String,
    ) {
        let ui = self.ui();
        if pass != again {
            ui.set_transfer_status(sstr(tr!("The two passwords differ.")));
            return;
        }
        ui.set_transfer_status(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let url = match normalize_server_url(url) {
                Ok(u) => u,
                Err(e) => return report(&this, e),
            };
            let invite = Some(invites::invite_code(&invite)).filter(|i| !i.is_empty());
            let email = this
                .upgrade()
                .map(|t| t.ui().get_create_email().trim().to_owned())
                .filter(|e| !e.is_empty());
            let r = core
                .account_register(url.clone(), user, pass, invite, email, crate::hostname())
                .await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(s) => {
                    if let Err(e) = t.tokens.put(ACCOUNT_TOKEN, &s.token) {
                        return t.ui().set_transfer_status(sstr(tr!(
                            "could not store the token: {error}",
                            error = e
                        )));
                    }
                    // The invitation made a Jellyfin login too, already signed in: straight to
                    // the library, like after a Jellyfin sign-in from this server.
                    t.account_or_library(url, s);
                }
                Err(e) => t.ui().set_transfer_status(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Settings → Admin: what the AlwyWatch server is connected to, with what to do about it.
    fn admin_open(self: &Rc<Self>) {
        self.invites_load();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_admin_status().await;
            let Some(t) = this.upgrade() else { return };
            let ui = t.ui();
            match r {
                Ok(st) => {
                    let rows: Vec<StatusRow> = status_lines(&st, now_secs())
                        .into_iter()
                        .map(|(title, text, ok)| StatusRow {
                            title: sstr(title),
                            text: sstr(text),
                            ok,
                        })
                        .collect();
                    ui.set_admin_dry_run(st.dry_run);
                    ui.set_admin_status(ModelRc::new(VecModel::from(rows)));
                    ui.set_admin_status_message(SharedString::new());
                }
                Err(e) => ui.set_admin_status_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// New device, from the AlwyWatch server step: shows a code to type on a signed-in device
    /// and waits for the accounts (end-to-end encrypted: the server only relays them).
    fn transfer_receive(self: &Rc<Self>, url: String) {
        let generation = {
            let mut st = self.state.borrow_mut();
            st.transfer_gen += 1;
            st.transfer_gen
        };
        let ui = self.ui();
        ui.set_add_step("receive".into());
        ui.set_transfer_code("…".into());
        ui.set_transfer_check(SharedString::new());
        ui.set_transfer_status(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        let waiting = move |this: &std::rc::Weak<App>| {
            this.upgrade().is_some_and(|t| {
                t.state.borrow().transfer_gen == generation && t.ui().get_add_step() == "receive"
            })
        };
        slint::spawn_local(async move {
            let url = match normalize_server_url(url) {
                Ok(u) => u,
                Err(e) => return report(&this, e),
            };
            while waiting(&this) {
                let c = match core.transfer_start(url.clone(), crate::hostname()).await {
                    Ok(c) => c,
                    Err(e) => {
                        if let Some(t) = this.upgrade() {
                            t.ui().set_transfer_status(sstr(e.to_string()));
                        }
                        async_sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };
                if let Some(t) = this.upgrade() {
                    let ui = t.ui();
                    ui.set_transfer_code(sstr(format!(
                        "{} {}",
                        &c.code[..3.min(c.code.len())],
                        &c.code[3.min(c.code.len())..]
                    )));
                    ui.set_transfer_check(sstr(&c.check));
                    ui.set_transfer_status(SharedString::new());
                    let page = alwywatch_core::quick_connect_url(url.clone(), c.code.clone());
                    ui.set_transfer_qr(qr_image(&page));
                    ui.set_transfer_url(sstr(page.trim_start_matches("https://")));
                }
                let until = std::time::Instant::now() + Duration::from_secs(c.expires_in);
                // Until it expires (then a new code).
                while std::time::Instant::now() < until {
                    async_sleep(Duration::from_secs(2)).await;
                    if !waiting(&this) {
                        return;
                    }
                    let Ok(Some(got)) = core.transfer_receive(url.clone(), c.handle.clone()).await
                    else {
                        continue;
                    };
                    let Some(t) = this.upgrade() else { return };
                    let mut account = None;
                    if let Some(token) = &got.account_token
                        && t.tokens.put(ACCOUNT_TOKEN, token).is_ok()
                    {
                        account = core
                            .account_use_token(url.clone(), token.clone())
                            .await
                            .ok();
                    }
                    let Some(bundle) = got.bundle else {
                        match account {
                            Some(a) => t.account_ready(url.clone(), a.name),
                            None => t.ui().set_transfer_status(sstr(tr!(
                                "Nothing to copy: that device has no accounts."
                            ))),
                        }
                        return;
                    };
                    t.refresh_account_settings();
                    match t.restore_bundle(&bundle) {
                        Ok(Some(account)) => {
                            let _ = t
                                .core
                                .set_setting("current_account".into(), account.clone());
                            t.state.borrow_mut().account = Some(account);
                            t.refresh_keep_settings();
                            t.refresh_seerr_settings();
                            t.reset(Route::Home);
                        }
                        Ok(None) => t.ui().set_transfer_status(sstr(tr!(
                            "Nothing to copy: that device has no accounts."
                        ))),
                        Err(e) => t.ui().set_transfer_status(sstr(e.to_string())),
                    }
                    return;
                }
            }
        })
        .ok();
    }

    /// Signed-in device, "Add a device": the new device's code → its name and check number.
    fn send_lookup(self: &Rc<Self>, code: String) {
        let code: String = code.chars().filter(char::is_ascii_digit).collect();
        let ui = self.ui();
        if code.len() != 6 {
            ui.set_send_error(sstr(tr!("Type the 6 digits the new device shows.")));
            return;
        }
        ui.set_send_error(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.transfer_peer(code.clone()).await;
            let Some(t) = this.upgrade() else { return };
            let ui = t.ui();
            match r {
                Ok(req) => {
                    ui.set_send_code(sstr(&code));
                    ui.set_send_device(sstr(&req.device));
                    ui.set_send_check(sstr(&req.check));
                    ui.set_send_step("confirm".into());
                }
                Err(CoreError::NotFound) => {
                    ui.set_send_error(sstr(tr!("Unknown or expired code.")));
                }
                Err(e) => ui.set_send_error(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Sends the accounts and settings, sealed to the device whose code was checked.
    fn send_confirm(self: &Rc<Self>) {
        let ui = self.ui();
        let code = ui.get_send_code().to_string();
        let bundle = match self.transfer_bundle() {
            Ok(b) => b,
            Err(e) => return ui.set_send_error(sstr(e.to_string())),
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.transfer_send(code, bundle).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(()) => t.ui().set_send_step("sent".into()),
                Err(e) => t.ui().set_send_error(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Test hook used by headless screenshots: the transfer screens with made-up values.
    pub fn test_transfer(self: &Rc<Self>, step: &str) {
        let ui = self.ui();
        if step == "receive" {
            self.navigate(Route::Add);
            ui.set_add_step("receive".into());
            ui.set_transfer_code("482 913".into());
            ui.set_transfer_check("7261".into());
            ui.set_transfer_qr(qr_image("https://watch.example.org/q/482913"));
            ui.set_transfer_url("watch.example.org/q/482913".into());
        } else {
            self.navigate(Route::Settings);
            ui.set_settings_section("device".into());
            ui.set_keep_active(true);
            ui.set_keep_label("Synced with watch.example.org".into());
            ui.set_send_step("confirm".into());
            ui.set_send_device("Pixel 9".into());
            ui.set_send_check("7261".into());
        }
    }

    /// Test hook used by headless screenshots: open Settings.
    pub fn test_settings(self: &Rc<Self>) {
        self.navigate(Route::Settings);
        // ALWYWATCH_TEST_SECTION: which section of the settings to show.
        if let Ok(section) = std::env::var("ALWYWATCH_TEST_SECTION") {
            self.ui().set_settings_section(sstr(section));
        }
    }

    /// A title page with sample data: trailer and list buttons, crew, cast.
    pub fn test_detail(self: &Rc<Self>) {
        let ui = self.ui();
        ui.set_page(Page::Detail);
        ui.set_detail(Detail {
            id: sstr("t"),
            title: sstr("Sample movie"),
            meta: sstr("2008  ·  1 h 36 min  ·  PG  ·  ★ 7.4"),
            overview: sstr("A dog who believes he has super powers crosses the country."),
            genres: sstr("Family, Animation"),
            playable: true,
            can_watchlist: true,
            watchlisted: true,
            has_trailer: true,
            crew: sstr("Directed by Chris Williams, Byron Howard"),
            saga_id: sstr("1"),
            saga_label: sstr(saga_label("Bolt")),
            ..Default::default()
        });
        ui.set_detail_keep(KeepPanel {
            shown: true,
            listed: true,
            looking: false,
            error: SharedString::new(),
            key: sstr("t"),
            title: sstr("Kept until 2 people have watched it"),
            detail: sstr("In “Old movies” · date 23 Nov 2026 · kept by 2 until watched"),
            choice: sstr("You haven't chosen anything"),
            voted: false,
        });
        let cast = ["John", "Miley", "Susie", "Mark"]
            .iter()
            .enumerate()
            .map(|(n, name)| (n.to_string(), (*name).to_owned(), "Role".to_owned(), None))
            .collect();
        self.seerr_cards(cast, AppWindow::set_cast);
    }

    /// My list on the real profile; with `open_first`, its first title's page a moment later.
    pub fn test_mylist(self: &Rc<Self>, open_first: bool) {
        self.navigate(Route::MyList);
        if open_first {
            let this = Rc::downgrade(self);
            slint::Timer::single_shot(Duration::from_secs(4), move || {
                if let Some(this) = this.upgrade()
                    && let Some(c) = this.state.borrow().grid_model.row_data(0)
                {
                    let id = c.id.to_string();
                    let this = this.clone();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        this.navigate(Route::Detail(id));
                    });
                }
            });
        }
    }

    pub fn test_home(self: &Rc<Self>) {
        let item = |id: &str, kind: ItemKind, title: &str, at: i64| Item {
            id: id.into(),
            kind,
            title: title.into(),
            sort_title: None,
            overview: None,
            year: Some(2024),
            runtime_ms: Some(3_000_000),
            community_rating: None,
            official_rating: None,
            genres: vec![],
            provider_ids: ProviderIds::default(),
            series_id: None,
            series_title: None,
            season_id: None,
            season_number: None,
            episode_number: None,
            child_count: None,
            premiere_date: None,
            images: vec![],
            user: UserData {
                position_ms: 1_000_000,
                last_played_at: Some(at),
                ..Default::default()
            },
        };
        let home = |cw: Vec<Item>, lib: &str, movies: Vec<Item>| {
            let library = MediaLibrary {
                id: lib.into(),
                title: "Movies".into(),
                kind: LibraryKind::Movies,
            };
            Home {
                resume: vec![],
                next_up: vec![],
                continue_watching: cw,
                libraries: vec![library.clone()],
                latest: vec![LibraryRail {
                    library,
                    items: movies,
                    notes: vec![],
                }],
                fetched_at: 0,
            }
        };
        let m = ItemKind::Movie;
        self.ui().set_page(Page::Home);
        self.render_merged(vec![
            AccountHome {
                account_id: "plex:test".into(),
                home: home(
                    vec![item("p1", m, "First", 30)],
                    "pl",
                    vec![item("p2", m, "Plex film", 0), item("p3", m, "Another", 0)],
                ),
            },
            AccountHome {
                account_id: "jellyfin:test".into(),
                home: home(
                    vec![item("j1", m, "Second", 20)],
                    "jl",
                    vec![item("j2", m, "Jellyfin film", 0)],
                ),
            },
        ]);
    }

    /// Test hook used by headless screenshots: the made-up home's Movies library on `tab`.
    pub fn test_library(self: &Rc<Self>, tab: &str) {
        self.navigate(Route::Library("pl".into(), "Movies".into()));
        self.show_library_tab("pl", tab);
        if tab == "categories" {
            let names = ["Action", "Comedy", "Drama", "Science Fiction", "Thriller"];
            self.ui().set_genres(ModelRc::new(VecModel::from(
                names
                    .iter()
                    .map(|n| NamedId {
                        id: sstr(n),
                        title: sstr(n),
                        selected: false,
                    })
                    .collect::<Vec<_>>(),
            )));
        }
    }

    /// Test hook used by headless screenshots: a made-up Seerr title page (layout only).
    pub fn test_seerr_title(self: &Rc<Self>) {
        let ui = self.ui();
        ui.set_page(Page::Seerrtitle);
        ui.set_seerr_title(SeerrPage {
            key: "Tv:1".into(),
            title: "A Made-Up Show".into(),
            meta: "2024  ·  3 seasons  ·  48 min  ·  ★ 7.9".into(),
            tagline: "Nothing is what it seems.".into(),
            overview: "Test text for the layout of a Seerr title page.".into(),
            genres: "Drama  ·  Mystery".into(),
            status: "Partly available".into(),
            saga_id: "1".into(),
            saga_name: "The Made-Up Collection".into(),
            ..Default::default()
        });
        ui.set_seerr_panel(SeerrPanel {
            is_tv: true,
            can_request: true,
            ..Default::default()
        });
        let named = |n: &str, sel| NamedId {
            id: n.into(),
            title: n.into(),
            selected: sel,
        };
        ui.set_seerr_seasons(ModelRc::new(VecModel::from(vec![
            named("Season 1 · Available", false),
            named("Season 2", true),
            named("Season 3", false),
        ])));
        self.seerr_cards(
            (0..8)
                .map(|n| {
                    (
                        format!("cast:{n}"),
                        format!("Actor {n}"),
                        "Role".into(),
                        None,
                    )
                })
                .collect(),
            AppWindow::set_seerr_cast,
        );
    }

    /// Test hook used by headless screenshots.
    pub fn test_seerr(self: &Rc<Self>, url: String) {
        self.navigate(Route::Settings);
        self.seerr_url(url);
    }

    /// Test hook used by headless screenshots.
    pub fn test_jellyfin(self: &Rc<Self>, url: String) {
        self.navigate(Route::Add);
        self.add_choose("jellyfin".into());
        self.jellyfin_url(url);
    }

    // ----- wiring ---------------------------------------------------------------------------

    fn wire(self: &Rc<Self>, ui: &AppWindow) {
        macro_rules! on {
            ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {{
                let w = Rc::downgrade(self);
                ui.$setter(move |$($arg),*| {
                    if let Some($app) = w.upgrade() {
                        $body
                    }
                });
            }};
        }
        on!(on_back, |a| a.back());
        on!(on_dash_pick, |a, id| a.dash_pick(id.into()));
        on!(on_dash_days_pick, |a, days| a.dash_days(days));
        on!(on_move_focus, |a, forward| {
            let key = if forward {
                slint::platform::Key::Tab
            } else {
                slint::platform::Key::Backtab
            };
            let ui = a.ui();
            let text: SharedString = key.into();
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
        });
        on!(on_open_item, |a, id| {
            let collection = a
                .state
                .borrow()
                .items
                .get(id.as_str())
                .filter(|i| i.kind == ItemKind::Collection)
                .map(|i| i.title.clone());
            // A "recently added" show card: its new episode, or the show on the season concerned.
            // Only from the rows that carry notes (the same show elsewhere opens itself).
            let note = matches!(a.ui().get_page(), Page::Home | Page::Library)
                .then(|| a.state.borrow().notes.get(id.as_str()).cloned())
                .flatten();
            if id.starts_with("leaving:") {
                a.navigate(Route::Leaving);
            } else if let Some(n) = note {
                a.state.borrow_mut().pending_season = n
                    .open_season_id
                    .filter(|_| n.open_id == n.item_id)
                    .map(|s| (n.item_id.clone(), s));
                a.navigate(Route::Detail(n.open_id));
            } else if let Some(title) = collection {
                let library = a.ui().get_library_id().to_string();
                a.navigate(Route::Filtered(library, title, None, Some(id.into())));
            } else {
                a.navigate(Route::Detail(id.into()));
            }
        });
        on!(on_prefetch, |a, id| {
            a.prefetch(&id);
        });
        on!(on_open_library, |a, id, title| a
            .navigate(Route::Library(id.into(), title.into())));
        on!(on_set_sort, |a, id| {
            let sort = match id.as_str() {
                "added" => SortBy::DateAdded,
                "released" => SortBy::ReleaseDate,
                _ => SortBy::Title,
            };
            let key = a.state.borrow().grid.as_ref().map(|g| g.key.clone());
            if let Some(key) = key {
                a.open_grid(GridKey { sort, ..key });
            }
        });
        on!(on_grid_scrolled, |a, f| a.grid_scrolled(f));
        on!(on_library_tab_chosen, |a, tab| {
            let library = a.ui().get_library_id().to_string();
            a.show_library_tab(&library, &tab);
        });
        on!(on_open_genre, |a, id, name| {
            let ui = a.ui();
            let (library, title) = (ui.get_library_id().to_string(), ui.get_grid_title());
            a.navigate(Route::Filtered(
                library,
                format!("{name} · {title}"),
                Some(id.into()),
                None,
            ));
        });
        on!(on_play, |a, id, from_start| {
            let id: String = id.into();
            if !a.play_or_ask(id.clone(), from_start) {
                a.play(id, from_start);
            }
        });
        on!(on_remote_open, |a, id| a.remote_open(id.into()));
        on!(on_remote_close, |a| a.remote_close());
        on!(on_remote_action, |a, action| a.remote_action(action.into()));
        on!(on_remote_seek, |a, ms| a.remote_seek(ms));
        on!(on_remote_drag, |a| a.remote_drag());
        on!(on_remote_track, |a, kind, index| a
            .remote_track(kind.into(), index.into()));
        on!(on_card_menu_open, |a, id, row| a
            .card_menu_open(id.into(), row.into()));
        on!(on_card_menu_pick, |a, action| a
            .card_menu_pick(action.into()));
        on!(on_series_download_menu, |a| a.series_download_menu());
        on!(on_toggle_played, |a, id| a.toggle_played(id.into()));
        on!(on_mark_many, |a, kind, played| a
            .mark_many(kind.into(), played));
        on!(on_mark_confirm_ok, |a| a.mark_confirmed(true));
        on!(on_mark_confirm_cancel, |a| a.mark_confirmed(false));
        on!(on_toggle_watchlist, |a, id| a.toggle_watchlist(id.into()));
        on!(on_play_trailer, |a, id| a.play_trailer(id.into()));
        on!(on_select_season, |a, id| a.select_season(id.into()));
        on!(on_detail_open_series, |a, series, season| {
            // The show is on the episode's server (several sign-ins: not the current one).
            let episode = a.ui().get_detail().id.to_string();
            let owner = a.state.borrow().owners.get(&episode).cloned();
            if let Some(acc) = owner {
                let ids = [series.to_string(), season.to_string()];
                a.own_ids(ids.iter().filter(|i| !i.is_empty()), &acc);
            }
            a.state.borrow_mut().pending_season =
                (!season.is_empty()).then(|| (series.to_string(), season.to_string()));
            a.navigate(Route::Detail(series.into()));
        });
        on!(on_search, |a, text| a.search(text.into()));
        on!(on_go, |a, page| a.navigate(match page {
            Page::Search => Route::Search,
            Page::Mylist => Route::MyList,
            Page::Settings => Route::Settings,
            Page::Add => Route::Add,
            Page::Downloads => Route::Downloads,
            Page::Requests => Route::Requests,
            Page::Leaving => Route::Leaving,
            Page::Together => Route::Together,
            Page::Triage => Route::Triage,
            Page::Keepadmin => Route::KeepAdmin,
            Page::Dashboard => Route::Dashboard,
            Page::Profiles => Route::Profiles,
            Page::Historycopy => Route::HistoryCopy,
            _ => Route::Home,
        }));
        on!(on_add_choose, |a, which| a.add_choose(which.into()));
        on!(on_profile_open, |a, id| a.profile_open(id.into()));
        on!(on_profile_dialog_open, |a, kind, id| a
            .profile_dialog(kind.into(), id.into()));
        on!(on_profile_submit, |a| a.profile_submit());
        on!(on_profile_edited, |a, text| a.profile_edited(text.into()));
        on!(on_profile_pin_remove, |a| a.profile_pin_remove());
        on!(on_profile_cancel, |a| a.profile_cancel());
        on!(on_profile_switcher, |a| a.profile_switcher());
        on!(on_cycle_setting, |a, key| a.cycle_setting(key.into()));
        on!(on_profile_auto_toggle, |a| a.profile_auto_toggle());
        on!(on_history_pick, |a, side, id| a
            .history_pick(side.into(), id.into()));
        on!(on_history_check, |a| a.history_check());
        on!(on_player_cmd, |a, cmd, arg| {
            if cmd == "quality" {
                a.player_quality(&arg);
            } else if cmd == "together" {
                a.player_watch_together();
            } else if cmd == "room" {
                a.room_invite();
            } else {
                a.player_cmd(&cmd, &arg);
            }
        });
        on!(on_player_room_start, |a| a.player_room_start());
        on!(on_app_fullscreen_toggle, |a| a.app_fullscreen_toggle());
        on!(on_window_minimize, |a| a.ui().window().set_minimized(true));
        on!(on_window_maximize, |a| {
            let ui = a.ui();
            let w = ui.window();
            w.set_maximized(!w.is_maximized());
        });
        on!(on_window_close, |a| {
            let _ = a.ui().hide();
        });
        // Wayland title bars (drawn by the app through winit) ignore touch.
        self.ui().set_window_controls(
            cfg!(target_os = "linux")
                && std::env::var_os("WAYLAND_DISPLAY").is_some()
                && std::env::var("SLINT_BACKEND").map_or(true, |b| !b.contains("x11")),
        );
        on!(on_player_room_cancel, |a| a.player_room_cancel());
        on!(on_rail_see_all, |a, key, title| {
            match key.as_str() {
                "continue" => a.navigate(Route::Continue),
                "leaving" => a.navigate(Route::Leaving),
                k => {
                    if let Some(lib) = k.strip_prefix("lib:") {
                        // The rail's title is "New in {library}" in the selected language.
                        let tpl = i18n::tr("New in {library}");
                        let (pre, post) = tpl.split_once("{library}").unwrap_or(("", ""));
                        let name = title
                            .strip_prefix(pre)
                            .and_then(|s| s.strip_suffix(post))
                            .unwrap_or(&title)
                            .to_owned();

                        a.navigate(Route::Recent(lib.to_owned(), name));
                    }
                }
            }
        });
        on!(on_history_copy, |a| a.history_copy());
        on!(on_history_toggle_missing, |a| a.history_toggle_missing());
        on!(on_jellyfin_url, |a, url| a.jellyfin_url(url.into()));
        on!(on_add_keep_servers, |a, url| a.add_keep_servers(url.into()));
        on!(on_add_keep_pick, |a, i| a.add_keep_pick(i.into()));
        on!(on_jellyfin_sign_in, |a, u, p| a
            .jellyfin_sign_in(u.into(), p.into()));
        on!(on_plex_pick, |a, id| a.plex_pick(id.into()));
        on!(on_use_account, |a, id| a.use_account(id.into()));
        on!(on_remove_account, |a, id| a.remove_account(id.into()));
        on!(on_clear_cache, |a| {
            let _ = a.core.clear_image_cache();
            a.images.borrow_mut().clear();
            a.refresh_settings();
        });
        on!(on_download, |a, id| a.download(id.into()));
        on!(on_seerr_url, |a, url| a.seerr_url(url.into()));
        on!(on_keep_connect, |a, url| a.keep_connect(url.into()));
        on!(on_alwywatch_sign_out, |a| a.alwywatch_sign_out());
        on!(on_alwy_connect, |a, create| a.alwy_connect(create));
        on!(on_create_check, |a, url| a.create_check(url.into()));
        on!(on_account_create, |a, url, invite, user, pass, again| a
            .account_create(
                url.into(),
                invite.into(),
                user.into(),
                pass.into(),
                again.into()
            ));
        on!(on_admin_open, |a| a.admin_open());
        on!(on_admin_invite, |a| a.admin_invite());
        on!(on_invite_next_server, |a| a.invite_next_server());
        on!(on_invite_next_like, |a| a.invite_next_like());
        on!(on_invite_end, |a, id| a.invite_end(id.into()));
        on!(on_open_server_settings, |a| a.open_server_settings());
        on!(on_create_invite_edited, |a, url, code| a
            .create_invite_edited(url.into(), code.into()));
        on!(on_admin_seerr, |a, user, id| a
            .admin_seerr(user.into(), id.into()));
        on!(on_keep_vote, |a, key, choice| a
            .keep_vote(key.into(), choice.into()));
        on!(on_keep_look_up, |a| a.keep_look_up());
        on!(on_watch_together, |a, id| {
            if let Some(account) = a.account_of(&id) {
                a.watch_together(account, id.into());
            }
        });
        on!(on_together_open, |a, code| a.together_open(code.into()));
        on!(on_together_decline, |a, code| a
            .together_decline(code.into()));
        on!(on_room_start, |a| a.room_start());
        on!(on_together_new, |a| a.together_new());
        on!(on_room_play, |a| a.open_room_player());
        on!(on_room_leave, |a| a.room_leave());
        on!(on_room_invite, |a| a.room_invite());
        on!(on_room_uninvite, |a, key| a.room_uninvite(key.into()));
        on!(on_picker_toggle, |a, key| a.picker_toggle(key.into()));
        on!(on_picker_filter, |a, text| a.picker_filter(text.into()));
        on!(on_picker_send, |a| a.picker_send());
        on!(on_picker_cancel, |a| a.picker_cancel());
        on!(on_invite_join, |a| a.invite_join());
        on!(on_invite_decline, |a| a.invite_decline());
        on!(on_invite_dismiss, |a| a.invite_dismiss());
        on!(on_account_dialog_open, |a, kind| a
            .account_dialog_open(kind.into()));
        on!(on_account_name_edited, |a, text| a
            .account_name_edited(text.into()));
        on!(on_account_name_save, |a| a.account_name_save());
        on!(on_account_name_reset, |a| a.account_name_reset());
        on!(on_account_preset, |a, id| a.account_preset(id.into()));
        on!(on_account_upload, |a| a.account_upload());
        on!(on_account_remove_avatar, |a| a.account_remove_avatar());
        on!(on_account_dialog_close, |a| a.account_dialog_close());
        on!(on_welcome_continue, |a| a.welcome_continue());
        on!(on_welcome_skip, |a| a.welcome_skip());
        on!(on_link_ask, |a, account| a.link_ask(account.into()));
        on!(on_link_confirm, |a| a.link_confirm());
        on!(on_link_dialog_close, |a| a.link_dialog_close());
        on!(on_unlink_ask, |a, key| a.unlink_ask(key.into()));
        on!(on_unlink_confirm, |a| a.unlink_confirm());
        on!(on_crop_drag, |a, dx, dy| a.crop_drag(dx, dy));
        on!(on_crop_zoom_by, |a, steps| a.crop_zoom_by(steps));
        on!(on_crop_set_zoom, |a, z| a.crop_set_zoom(z));
        on!(on_crop_save, |a| a.crop_save());
        on!(on_crop_cancel, |a| a.crop_cancel());
        on!(on_leaving_scrolled, |a, first| a.leaving_scrolled(first));
        on!(on_triage_vote, |a, choice| a.triage_vote(choice.into()));
        on!(on_triage_open, |a| a.triage_open());
        on!(on_admin_rights, |a, user, field| a
            .admin_rights(user.into(), field.into()));
        on!(on_admin_countdown, |a, id, stagger| a
            .admin_countdown(id.into(), stagger));
        on!(on_seerr_login, |a, m| a.seerr_login(m.into()));
        on!(on_seerr_password, |a, u, p| a
            .seerr_password(u.into(), p.into()));
        on!(on_seerr_disconnect, |a| {
            // Not connected again through the server on the next launch.
            let _ = a.core.set_setting(SEERR_OFF.to_owned(), "1".to_owned());
            let _ = a.core.seerr_sign_out();
            let _ = a.tokens.remove("seerr");
            a.refresh_seerr_settings();
        });
        on!(on_seerr_bridge, |a| a.seerr_bridge(false));
        on!(on_seerr_search, |a, t| a.seerr_search(t.into()));
        on!(on_seerr_open, |a, key| a.seerr_open(key.into()));
        on!(on_seerr_toggle_season, |a, n| {
            if let Ok(n) = n.parse::<u32>() {
                for (s, sel) in a.state.borrow_mut().seerr_seasons.iter_mut() {
                    if s.number == n
                        && matches!(
                            s.availability,
                            Availability::Unknown | Availability::Deleted
                        )
                    {
                        *sel = !*sel;
                    }
                }
                a.render_seerr_seasons();
            }
        });
        on!(on_seerr_request, |a, all| a.seerr_request(all));
        on!(on_transfer_receive, |a, url| a.transfer_receive(url.into()));
        on!(on_account_forgot, |a, url, login| a
            .account_forgot(url.into(), login.into()));
        on!(on_account_login, |a, url, u, p| a.account_login(
            url.into(),
            u.into(),
            p.into()
        ));
        on!(on_cycle_language, |a| a.cycle_language());
        on!(on_account_manage, |a| {
            if let Some(acc) = a.core.account() {
                platform::open_url(&format!("{}/account", acc.base_url.trim_end_matches('/')));
            }
        });
        on!(on_send_lookup, |a, code| a.send_lookup(code.into()));
        on!(on_send_confirm, |a| a.send_confirm());
        on!(on_send_start, |a| {
            let ui = a.ui();
            ui.set_send_step("code".into());
            ui.set_send_error(SharedString::new());
        });
        on!(on_send_cancel, |a| a
            .ui()
            .set_send_step(SharedString::new()));
        on!(on_detail_open_saga, |a| {
            let d = a.ui().get_detail();
            // The label stands for the name until the saga's own arrives.
            if let Ok(id) = d.saga_id.parse() {
                a.navigate(Route::SeerrSaga(id, d.saga_label.into()));
            }
        });
        on!(on_seerr_open_saga, |a| {
            let t = a.ui().get_seerr_title();
            if let Ok(id) = t.saga_id.parse() {
                a.navigate(Route::SeerrSaga(id, t.saga_name.into()));
            }
        });
        on!(on_download_action, |a, id, action| {
            let id = i64::from(id);
            let _ = match action.as_str() {
                "pause" => a.core.pause_download(id),
                "resume" => a.core.resume_download(id).map(|_| a.kick_downloads()),
                _ => a.core.remove_download(id),
            };
            a.refresh_downloads();
        });
        on!(on_play_download, |a, account, item| a.play_for(
            account.into(),
            item.into(),
            false
        ));
        on!(on_toggle_setting, |a, key| {
            let key = key.to_string();
            let on = a.core.get_setting(key.clone()).ok().flatten();
            // Defaults: automatic next episode is on, automatic intro skip is off.
            let was = match key.as_str() {
                "player.auto_next" => on.as_deref() != Some("0"),
                k if k.starts_with("together.") => on.as_deref() != Some("0"),
                _ => on.as_deref() == Some("1"),
            };
            let _ = a.core.set_setting(key, if was { "0" } else { "1" }.into());
            a.load_player_settings();
        });
        on!(on_toggle_spdif, |a| {
            let ui = a.ui();
            let v = !ui.get_spdif();
            ui.set_spdif(v);
            let _ = a
                .core
                .set_setting("player.spdif".into(), if v { "1" } else { "0" }.into());
        });
    }
}

/// Languages offered for audio and subtitles (the codes Android stores too).
const LANGUAGES: [&str; 16] = [
    "eng", "fra", "nld", "deu", "spa", "ita", "por", "jpn", "kor", "zho", "ara", "rus", "pol",
    "swe", "tur", "hin",
];

/// The name of one of [`LANGUAGES`] in the selected language.
fn language_name(code: &str) -> Option<String> {
    Some(match code {
        "eng" => tr!("English"),
        "fra" => tr!("French"),
        "nld" => tr!("Dutch"),
        "deu" => tr!("German"),
        "spa" => tr!("Spanish"),
        "ita" => tr!("Italian"),
        "por" => tr!("Portuguese"),
        "jpn" => tr!("Japanese"),
        "kor" => tr!("Korean"),
        "zho" => tr!("Chinese"),
        "ara" => tr!("Arabic"),
        "rus" => tr!("Russian"),
        "pol" => tr!("Polish"),
        "swe" => tr!("Swedish"),
        "tur" => tr!("Turkish"),
        "hin" => tr!("Hindi"),
        _ => return None,
    })
}

/// The AlwyWatch account token, in the token store like media tokens.
const ACCOUNT_TOKEN: &str = "alwywatch_account";

/// A QR code as a picture (one pixel per module plus a quiet zone; the UI scales it up
/// without smoothing).
fn qr_image(text: &str) -> slint::Image {
    let Ok(q) = alwywatch_core::qr_code(text.to_owned()) else {
        return slint::Image::default();
    };
    let n = q.size as usize;
    let side = n + 2;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(side as u32, side as u32);
    let px = buf.make_mut_slice();
    for (i, p) in px.iter_mut().enumerate() {
        let (x, y) = (i % side, i / side);
        let dark = x >= 1 && y >= 1 && x <= n && y <= n && q.modules[(y - 1) * n + (x - 1)];
        let v = if dark { 0 } else { 255 };
        *p = slint::Rgb8Pixel { r: v, g: v, b: v };
    }
    slint::Image::from_rgb8(buf)
}

/// Signs the saved accounts and connections in with their stored tokens (start-up, and after
/// accounts arrived from another device).
pub fn activate_saved(core: &AlwyWatch, tokens: &Tokens) {
    for a in core.accounts().unwrap_or_default() {
        if let Some(t) = tokens.get(&a.id) {
            let _ = core.activate_account(a.id.clone(), t);
        }
        // Plex's watchlist is on plex.tv: the (home) user's account token.
        if let Some(t) = tokens
            .get(&format!("{}:plexhome", a.id))
            .or_else(|| tokens.get(&format!("{}:plextv", a.id)))
        {
            core.set_plex_account_token(a.id.clone(), t);
        }
    }
    if core.account().is_some()
        && let Some(t) = tokens.get(ACCOUNT_TOKEN)
    {
        let _ = core.account_activate(t);
    }
    if core.seerr_connection().is_some()
        && let Some(cookie) = tokens.get("seerr")
    {
        let _ = core.seerr_activate(cookie);
    }
    if let Some(k) = core.keep_connection() {
        let kind = core
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.id == k.account_id)
            .map(|a| a.kind);
        let token = match kind {
            Some(ServerKind::Plex) => tokens.get(&format!("{}:plextv", k.account_id)),
            _ => tokens.get(&k.account_id),
        };
        if let Some(t) = token {
            let _ = core.keep_activate(t);
        }
    }
}

/// Settings that follow the user to a new device (preferences, not caches); same as Android.
const SYNCED_SETTINGS: [&str; 4] = [
    "ui.combined",
    "ui.preferred_account",
    "current_account",
    "downloads.delete_rule",
];

fn report(this: &std::rc::Weak<App>, e: CoreError) {
    if let Some(t) = this.upgrade() {
        t.ui().set_status(sstr(e.to_string()));
    }
}

/// A timer-based sleep usable inside `slint::spawn_local` futures.
/// "Saga · Alien", or the name alone when it already says so ("Very Bad Trip - Saga").
fn saga_label(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.contains("saga") || lower.contains("collection") {
        name.to_owned()
    } else {
        tr!("Saga · {name}", name = name)
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Past its date while deletion is off: listed, but nothing is planned.
fn keep_paused(e: &LeavingItem, now: i64) -> bool {
    !e.collection_active
        && e.protection == Protection::None
        && e.effective_deletion_date.is_some_and(|d| d < now)
}

/// "Kept forever", "Kept until 2 people have watched it", "Leaves in 12 days"… (`leaves`: the
/// Leaving soon row's label).
fn keep_policy(e: &LeavingItem, leaves: &str) -> String {
    match e.protection {
        Protection::Forever => tr!("Kept forever"),
        Protection::UntilWatched => match e.counts.until_watched {
            ..=1 => tr!("Kept until the person who asked has watched it"),
            n => trn!(
                "Kept until {n} person has watched it",
                "Kept until {n} people have watched it",
                n
            ),
        },
        _ if keep_paused(e, now_secs()) => tr!("On the deletion list"),
        _ => leaves.to_owned(),
    }
}

/// Something is planned for the title or someone keeps it. False: it is in no deletion list
/// (kept ahead for "a while" at most, which only counts once it gets into one).
fn keep_planned(e: &LeavingItem) -> bool {
    e.listed || e.protection != Protection::None
}

/// Which list it is in and why nothing happens yet, plus who keeps it.
fn keep_detail(e: &LeavingItem) -> String {
    let mut parts = vec![];
    // (A title taken out of its list by a keep still remembers which one it was in.)
    if let Some(t) = e.collection_title.as_ref().filter(|_| e.listed) {
        parts.push(tr!("In “{list}”", list = t));
    }
    if e.listed && e.protection == Protection::None && !e.collection_active {
        parts.push(tr!("deletion isn't turned on yet"));
    }
    if let Some(d) = e.effective_deletion_date
        && e.listed
        && !keep_paused(e, now_secs())
    {
        parts.push(tr!("date {date}", date = medium_date(d)));
    }
    let c = &e.counts;
    let mut keepers: Vec<String> = vec![];
    if c.forever > 0 {
        keepers.push(trn!("{n} forever", "{n} forever", c.forever));
    }
    if c.until_watched > 0 {
        keepers.push(trn!(
            "{n} until watched",
            "{n} until watched",
            c.until_watched
        ));
    }
    if c.days > 0 {
        keepers.push(trn!("{n} for a while", "{n} for a while", c.days));
    }
    if !keepers.is_empty() {
        parts.push(tr!("kept by {people}", people = keepers.join(", ")));
    }
    let s = parts.join(" · ");
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().chain(chars).collect(),
        None => s,
    }
}

/// "22 Nov 2026" (UTC; the day is what matters).
fn medium_date(ts: i64) -> String {
    // Civil from days (Howard Hinnant's algorithm).
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let month = match month {
        1 => tr!("Jan"),
        2 => tr!("Feb"),
        3 => tr!("Mar"),
        4 => tr!("Apr"),
        5 => tr!("May"),
        6 => tr!("Jun"),
        7 => tr!("Jul"),
        8 => tr!("Aug"),
        9 => tr!("Sep"),
        10 => tr!("Oct"),
        11 => tr!("Nov"),
        _ => tr!("Dec"),
    };
    tr!(
        "{day} {month} {year}",
        day = day,
        month = month,
        year = year
    )
}

async fn async_sleep(d: Duration) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    slint::Timer::single_shot(d, move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}

/// A name for admin lists: the media-server name next to the display name when they differ or
/// when someone else has the same display name.
fn person_label(name: &str, media_name: &str, twin: bool) -> String {
    if !media_name.is_empty() && (twin || !media_name.eq_ignore_ascii_case(name)) {
        format!("{name} ({media_name})")
    } else {
        name.to_owned()
    }
}

/// "9:12" or "1:02:03".
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, sec) = (s / 3600, s % 3600 / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// "5 minutes", "3 hours", "2 days": how long ago something happened.
fn elapsed(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 3600 {
        trn!("{n} minute", "{n} minutes", (secs / 60).max(1))
    } else if secs < 86_400 {
        trn!("{n} hour", "{n} hours", secs / 3600)
    } else {
        trn!("{n} day", "{n} days", secs / 86_400)
    }
}

/// Settings → Admin → Server status: (what, its state and what to do, fine). Read only: all of
/// it is set in Server settings (the web app).
fn status_lines(s: &keep_service::ServerStatus, now: i64) -> Vec<(String, String, bool)> {
    let maintainerr = match s.maintainerr.as_str() {
        "off" => (
            tr!("Off. Set up Maintainerr in Server settings (web app)."),
            true,
        ),
        "starting" => (tr!("Reading Maintainerr for the first time…"), true),
        "ok" => (
            trn!(
                "Maintainerr {version} · read {time} ago · {n} title leaving",
                "Maintainerr {version} · read {time} ago · {n} titles leaving",
                s.leaving_items,
                version = s.maintainerr_version.clone().unwrap_or_default(),
                time = elapsed(now - s.last_sync.unwrap_or(now)),
            ),
            true,
        ),
        _ => (
            tr!(
                "Can't reach Maintainerr. Check its address and API key in Server settings (web app)."
            ),
            false,
        ),
    };
    let requests = if s.seerr_bridge {
        tr!("On: every app requests as each person's Seerr user.")
    } else {
        tr!("Off: only direct request-site logins work. Set up Seerr in Server settings (web app).")
    };
    let mut watched = vec![];
    if !s.jellyfin_key {
        watched.push(tr!(
            "Jellyfin keeps don't end by themselves. Add the Jellyfin server's key in Server settings (web app)."
        ));
    }
    if !s.tautulli {
        watched.push(tr!(
            "Plex keeps don't end by themselves. Set up Tautulli in Server settings (web app)."
        ));
    }
    let watched_ok = watched.is_empty();
    if watched_ok {
        watched.push(tr!("They end by themselves for Jellyfin and Plex users."));
    }
    watched.push(if s.webhooks {
        tr!("Webhooks on: watched titles are noticed at once.")
    } else {
        tr!(
            "Optional: watched titles are noticed within 15 min. For instant, add the webhooks shown in Server settings (web app)."
        )
    });
    let accounts = match s.registration.as_str() {
        "open" => tr!("Anyone can create an account."),
        "closed" => tr!("Nobody can create an account."),
        _ => tr!("New accounts need an invitation."),
    };
    vec![
        (
            tr!("AlwyWatch server"),
            tr!("Version {version}", version = s.version),
            true,
        ),
        (
            tr!("Leaving soon (Maintainerr)"),
            maintainerr.0,
            maintainerr.1,
        ),
        (tr!("Requests (Seerr)"), requests, true),
        (tr!("Keep until watched"), watched.join(" "), watched_ok),
        (tr!("Accounts"), accounts, true),
        (
            tr!("Passkeys"),
            if s.passkeys {
                tr!("On.")
            } else {
                tr!("Off: set this site's address (https) in Server settings (web app).")
            },
            true,
        ),
        (
            tr!("Pocket ID"),
            if s.pocketid { tr!("On.") } else { tr!("Off.") },
            true,
        ),
    ]
}

#[cfg(test)]
mod status_tests {
    use super::*;

    fn status() -> keep_service::ServerStatus {
        keep_service::ServerStatus {
            version: "1.2.0".into(),
            maintainerr: "ok".into(),
            maintainerr_version: Some("3.32".into()),
            last_sync: Some(1_000),
            leaving_items: 12,
            seerr_bridge: true,
            tautulli: true,
            jellyfin_key: true,
            webhooks: false,
            passkeys: true,
            pocketid: false,
            registration: "invite".into(),
            dry_run: false,
        }
    }

    #[test]
    fn server_status_reads_plainly() {
        let lines = status_lines(&status(), 1_000 + 300);
        assert_eq!(
            lines[1].1,
            "Maintainerr 3.32 · read 5 minutes ago · 12 titles leaving"
        );
        assert!(lines.iter().all(|l| l.2));
        assert!(
            lines[3]
                .1
                .contains("add the webhooks shown in Server settings")
        );

        let mut s = status();
        s.maintainerr = "unreachable".into();
        s.tautulli = false;
        let lines = status_lines(&s, 0);
        assert!(!lines[1].2 && !lines[3].2);
        assert!(
            lines[3]
                .1
                .starts_with("Plex keeps don't end by themselves.")
        );
    }

    #[test]
    fn elapsed_rounds_down() {
        assert_eq!(elapsed(10), "1 minute");
        assert_eq!(elapsed(7_200), "2 hours");
        assert_eq!(elapsed(200_000), "2 days");
    }
}

#[cfg(test)]
mod clock_tests {
    #[test]
    fn admin_labels_tell_people_apart() {
        assert_eq!(super::person_label("Sam", "samuel", false), "Sam (samuel)");
        assert_eq!(super::person_label("Sam", "sam", false), "Sam");
        assert_eq!(super::person_label("Sam", "sam", true), "Sam (sam)");
        assert_eq!(super::person_label("Sam", "", true), "Sam");
    }

    #[test]
    fn formats_like_the_android_player() {
        assert_eq!(super::clock(552_000), "9:12");
        assert_eq!(super::clock(3_723_000), "1:02:03");
        assert_eq!(super::clock(0), "0:00");
    }
}

#[cfg(test)]
mod keep_panel_tests {
    use super::*;

    fn entry(protection: Protection, active: bool, date: Option<i64>) -> LeavingItem {
        LeavingItem {
            key: "k".into(),
            title: "T".into(),
            year: None,
            kind: "movie".into(),
            poster_url: None,
            tmdb_id: None,
            tvdb_id: None,
            server_item_id: None,
            collection_id: Some(1),
            collection_title: Some("Old movies".into()),
            collection_active: active,
            deletion_date: date,
            effective_deletion_date: date,
            protection,
            my_vote: None,
            counts: VoteCounts::default(),
            listed: true,
        }
    }

    #[test]
    fn dates_and_saga_labels() {
        assert_eq!(medium_date(0), "1 Jan 1970");
        assert_eq!(medium_date(1_795_392_000), "23 Nov 2026");
        assert_eq!(saga_label("Alien"), "Saga · Alien");
        assert_eq!(saga_label("Very Bad Trip - Saga"), "Very Bad Trip - Saga");
        assert_eq!(saga_label("Star Wars Collection"), "Star Wars Collection");
    }

    #[test]
    fn policy_and_detail_like_android() {
        let mut e = entry(
            Protection::UntilWatched,
            true,
            Some(now_secs() + 86_400 * 20),
        );
        e.counts.until_watched = 2;
        e.counts.forever = 1;
        assert_eq!(keep_policy(&e, "x"), "Kept until 2 people have watched it");
        assert!(keep_detail(&e).starts_with("In “Old movies” · date "));
        assert!(keep_detail(&e).ends_with(" · kept by 1 forever, 2 until watched"));
        let paused = entry(Protection::None, false, Some(10));
        assert_eq!(keep_policy(&paused, "x"), "On the deletion list");
        assert_eq!(
            keep_detail(&paused),
            "In “Old movies” · deletion isn't turned on yet"
        );
        let mut plain = entry(Protection::None, true, Some(now_secs() + 86_400 * 5));
        plain.collection_title = None;
        assert_eq!(keep_policy(&plain, "Leaves in 4 days"), "Leaves in 4 days");
        assert!(keep_detail(&plain).starts_with("Date "));
    }
}
