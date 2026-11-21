// SPDX-License-Identifier: AGPL-3.0-or-later
//! Glue between the Slint UI and the core. Every page renders what the core has cached in the
//! same frame it is opened, then refreshes in the background (same model as Android).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use alwywatch_core::*;
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
    Weak,
};

use crate::platform::{self, Tokens};
use crate::player::{self, ExternalSub, PlayRequest, TrackSetup};
use crate::{
    AccountRow, AdminRow, AppWindow, Card, Detail, DownloadRow, LeavingRow, NamedId, Page, Rail,
    SeerrPage, SeerrPanel, ServerChoice,
};

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
    /// AlwyWatch server address entered in Settings, while the user picks the account to sync.
    keep_pending_url: String,
    /// Plex server picked from that list: selected automatically after the plex.tv sign-in.
    preset_plex: Option<String>,
    series: Option<String>,
    season: Option<String>,
    search_gen: u64,
    seerr_url: String,
    seerr_selected: Option<SeerrResult>,
    seerr_seasons: Vec<(SeerrSeason, bool)>,
    triage: Vec<LeavingItem>,
    admin_users: Vec<AdminUser>,
    armed: Option<(String, i32)>,
    poll_gen: u64,
    /// Bumped to stop waiting for a device transfer.
    transfer_gen: u64,
    /// The title page's first trailer: (title id, account, trailer).
    trailer: Option<(String, String, Trailer)>,
}

thread_local! {
    /// The UI-thread App, reachable from `invoke_from_event_loop` closures (which must be `Send`).
    static APP: RefCell<std::rc::Weak<App>> = const { RefCell::new(std::rc::Weak::new()) };
}

pub struct App {
    downloads_timer: slint::Timer,
    runner_timer: slint::Timer,
    core: Arc<AlwyWatch>,
    ui: Weak<AppWindow>,
    tokens: Tokens,
    state: RefCell<State>,
    images: RefCell<HashMap<String, Image>>,
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
            i.runtime_ms.map(|r| format!("{} min", r / 60_000)),
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
    pub fn new(core: Arc<AlwyWatch>, ui: &AppWindow, tokens: Tokens) -> Rc<Self> {
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
            core,
            ui: ui.as_weak(),
            tokens,
            state: RefCell::new(State {
                account,
                stack: vec![],
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
                keep_pending_url: String::new(),
                preset_plex: None,
                series: None,
                season: None,
                search_gen: 0,
                seerr_url: String::new(),
                seerr_selected: None,
                seerr_seasons: vec![],
                triage: vec![],
                admin_users: vec![],
                armed: None,
                poll_gen: 0,
                transfer_gen: 0,
                trailer: None,
            }),
            images: RefCell::new(HashMap::new()),
        });
        APP.with(|a| *a.borrow_mut() = Rc::downgrade(&app));
        ui.set_grid(ModelRc::from(app.state.borrow().grid_model.clone()));
        ui.set_episodes(ModelRc::from(app.state.borrow().episodes_model.clone()));
        app.load_player_settings();
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
        app.show_reminders();
        app.start_download_runner();
        let first = if app.state.borrow().account.is_some() {
            Route::Home
        } else {
            Route::Add
        };
        app.navigate(first);
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

    fn kick_downloads(&self) {
        let core = self.core.clone();
        self.core.runtime().spawn(async move {
            let _ = core.run_downloads().await;
        });
    }

    fn refresh_downloads(&self) {
        let gb = |b: u64| format!("{:.1} GB", b as f64 / 1e9);
        let rows: Vec<DownloadRow> = self
            .core
            .downloads()
            .unwrap_or_default()
            .into_iter()
            .map(|d| {
                let size = d
                    .total
                    .map(|t| format!("{} / {}", gb(d.downloaded), gb(t)))
                    .unwrap_or_else(|| gb(d.downloaded));
                let status = match d.state {
                    DownloadState::Queued => format!("Queued · {size}"),
                    DownloadState::Running => format!("Downloading · {size}"),
                    DownloadState::Paused => format!("Paused · {size}"),
                    DownloadState::PausedNoSpace => {
                        "Paused: disk full. Free some space, then resume.".into()
                    }
                    DownloadState::Waiting => format!("Waiting for the network · {size}"),
                    DownloadState::Failed => {
                        format!("Failed: {}", d.error.clone().unwrap_or_default())
                    }
                    DownloadState::Completed => format!("Downloaded · {}", d.file_path),
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
        self.ui().set_downloads(ModelRc::new(VecModel::from(rows)));
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
            Protection::Forever => "Kept forever".to_owned(),
            Protection::UntilWatched => "Kept until watched".to_owned(),
            p => match i.effective_deletion_date {
                None => "No date".into(),
                // Date passed but the owner has not turned deletion on: nothing happens yet.
                Some(d) if d < now && !i.collection_active && p == Protection::None => {
                    "Deletion paused".to_owned()
                }
                Some(d) => {
                    let days = (d - now).div_euclid(86_400);
                    let mut s = match days {
                        d if d < 0 => "Overdue".to_owned(),
                        0 => "Leaves today".into(),
                        1 => "Leaves tomorrow".into(),
                        d => format!("Leaves in {d} days"),
                    };
                    if p == Protection::Extended {
                        s.push_str(" (extended)");
                    }
                    s
                }
            },
        };
        let vote = i
            .my_vote
            .as_ref()
            .map(|v| match v.kind {
                VoteKind::DontCare => "You don't care".to_owned(),
                VoteKind::KeepDays => "You keep it a while".into(),
                VoteKind::KeepUntilWatched => "You keep it until you watch it".into(),
                VoteKind::KeepForever => format!(
                    "You keep it forever{}",
                    v.reason
                        .as_ref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default()
                ),
            })
            .unwrap_or_default();
        let others = i.counts.forever + i.counts.until_watched + i.counts.days;
        LeavingRow {
            key: sstr(&i.key),
            title: sstr(
                [
                    i.title.clone(),
                    i.year.map(|y| y.to_string()).unwrap_or_default(),
                ]
                .join(" ")
                .trim(),
            ),
            when: sstr(when),
            vote: sstr(vote),
            others: sstr(if others > 0 {
                format!("{others} keep request(s)")
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
        for (row, i) in list.iter().enumerate() {
            let Some(url) = i.poster_url.clone() else {
                continue;
            };
            let (m, key) = (Rc::downgrade(&model), i.key.clone());
            self.load_url_image(url, move |_, img| {
                if let Some(m) = m.upgrade()
                    && let Some(mut r) = m.row_data(row)
                    && r.key.as_str() == key
                {
                    r.poster = img;
                    m.set_row_data(row, r);
                }
            });
        }
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
            "love" => r(VoteKind::KeepForever, None, None, Some("love it")),
            "classic" => r(VoteKind::KeepForever, None, None, Some("classic")),
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
                    if let Ok(Some(list)) = t.core.cached_leaving_soon() {
                        t.render_leaving(&list);
                    }
                }
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
        let rows: Vec<AdminRow> = self
            .state
            .borrow()
            .admin_users
            .iter()
            .map(|u| AdminRow {
                key: sstr(&u.user_key),
                name: sstr(&u.name),
                detail: sstr(format!(
                    "{}{} kept forever",
                    if u.is_admin { "admin · " } else { "" },
                    u.forever_count
                )),
                forever: sstr(if u.rights.can_keep_forever {
                    "Forever: allowed"
                } else {
                    "Forever: blocked"
                }),
                max_forever: sstr(format!(
                    "Max forever: {}",
                    u.rights
                        .max_forever
                        .map(|v| v.to_string())
                        .unwrap_or("∞".into())
                )),
                max_days: sstr(format!(
                    "Max days: {}",
                    u.rights
                        .max_keep_days
                        .map(|v| v.to_string())
                        .unwrap_or("∞".into())
                )),
            })
            .collect();
        self.ui()
            .set_admin_users(ModelRc::new(VecModel::from(rows)));
    }

    fn load_admin(self: &Rc<Self>) {
        self.load_leaving();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_admin_users().await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(u) => {
                    t.state.borrow_mut().admin_users = u;
                    t.render_admin();
                }
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
            }
        })
        .ok();
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
            self.ui()
                .set_keep_message("Click the same button again to confirm the countdown.".into());
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
                    Ok(r) => format!(
                        "Countdown restarted: {} items, {} skipped.",
                        r.reset, r.skipped
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

    fn refresh_keep_settings(&self) {
        let ui = self.ui();
        ui.set_keep_active(self.core.keep_active());
        ui.set_keep_can_switch(self.core.accounts().map(|a| a.len() > 1).unwrap_or(false));
        if let Some(k) = self.core.keep_connection() {
            ui.set_keep_admin(k.is_admin);
            ui.set_keep_label(sstr(format!(
                "Syncing as {} @ {}{}",
                k.user_name,
                k.base_url,
                if k.is_admin { " (admin)" } else { "" }
            )));
        }
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
                Ok((url, list)) => t.keep_offer(url, &list, None),
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    /// Accounts on a server the AlwyWatch server serves; one is synced right away.
    fn keep_offer(self: &Rc<Self>, url: String, list: &[ServerEntry], current: Option<String>) {
        let usable: Vec<Account> = self
            .core
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| list.iter().any(|e| e.id == a.server_id))
            .collect();
        let ui = self.ui();
        if usable.is_empty() {
            let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
            ui.set_keep_message(sstr(format!(
                "None of your accounts is on a server it serves ({}). Add one of those first.",
                names.join(", ")
            )));
            return;
        }
        if usable.len() == 1 && current.is_none() {
            self.keep_sync_as(url, usable[0].id.clone());
            return;
        }
        self.state.borrow_mut().keep_pending_url = url;
        ui.set_keep_choices(ModelRc::new(VecModel::from(
            usable
                .iter()
                .map(|a| NamedId {
                    id: sstr(&a.id),
                    title: sstr(format!(
                        "{} · {} ({})",
                        a.user_name,
                        a.server_name,
                        match a.kind {
                            ServerKind::Plex => "Plex",
                            ServerKind::Jellyfin => "Jellyfin",
                        }
                    )),
                    selected: current.as_deref() == Some(a.id.as_str()),
                })
                .collect::<Vec<_>>(),
        )));
    }

    /// "Sync another account": the choices for the connected AlwyWatch server.
    fn keep_choose(self: &Rc<Self>) {
        let Some(k) = self.core.keep_connection() else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_servers(k.base_url.clone()).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(list) => t.keep_offer(k.base_url, &list, Some(k.account_id)),
                Err(e) => t.ui().set_keep_message(sstr(e.to_string())),
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
            self.ui().set_keep_message(sstr(format!(
                "Sign in to {} again first.",
                account.server_name
            )));
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_connect(url, account_id, token).await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(_) => {
                    t.ui().set_keep_message(SharedString::new());
                    t.ui().set_keep_choices(ModelRc::default());
                    t.refresh_keep_settings();
                    t.refresh_settings();
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
            self.ui().set_keep_message(sstr(format!(
                "Reminder: you wanted to watch {} before it leaves.",
                titles.join(", ")
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
                    let img = image::open(path).ok()?.into_rgba8();
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

    fn availability_label(a: Availability) -> &'static str {
        match a {
            Availability::Available => "Available",
            Availability::PartiallyAvailable => "Partly available",
            Availability::Processing => "Downloading",
            Availability::Pending => "Requested",
            Availability::Blocklisted => "Blocked",
            _ => "",
        }
    }

    fn load_seerr_requests(self: &Rc<Self>) {
        let render = |this: &Rc<Self>, list: Vec<SeerrRequest>| {
            let cards = list
                .into_iter()
                .map(|r| {
                    let status = match r.status {
                        RequestStatus::Pending => "Waiting for approval",
                        RequestStatus::Approved => "Approved",
                        RequestStatus::Declined => "Declined",
                        RequestStatus::Failed => "Failed",
                        RequestStatus::Completed => "Available",
                    };
                    (
                        format!("{:?}:{}", r.media_type, r.tmdb_id),
                        r.title.unwrap_or_else(|| format!("TMDB {}", r.tmdb_id)),
                        status.to_owned(),
                        r.poster_url,
                    )
                })
                .collect();
            this.seerr_cards(cards, AppWindow::set_seerr_mine);
        };
        if let Ok(Some(list)) = self.core.cached_seerr_requests(false) {
            render(self, list);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_seerr_requests(false, false).await;
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

    fn seerr_open(self: &Rc<Self>, key: String) {
        self.navigate(Route::SeerrTitle(key));
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
                    Some(1) => "1 season".into(),
                    Some(n) => format!("{n} seasons"),
                    None => "Show".into(),
                });
            }
            meta.extend(t.runtime_min.map(|m| {
                if m >= 60 {
                    format!("{} h {} min", m / 60, m % 60)
                } else {
                    format!("{m} min")
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
                meta: sstr(format!("{} films", c.parts.len())),
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
                    Self::availability_label(r.availability).to_owned(),
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
                .set_seerr_message("Pick at least one season.".into());
            return;
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let res = core.seerr_request(r.media_type, r.tmdb_id, seasons).await;
            let Some(t) = this.upgrade() else { return };
            t.ui().set_seerr_message(sstr(match res {
                Ok(_) => format!("Requested: {}", r.title),
                Err(e) => format!("Could not request {}: {e}", r.title),
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
        ui.set_seerr_active(self.core.seerr_active());
        if let Some(c) = self.core.seerr_connection() {
            ui.set_seerr_label(sstr(if c.via_keep {
                format!("{} (through your AlwyWatch server)", c.user_name)
            } else {
                format!("{} @ {}", c.user_name, c.base_url)
            }));
        }
    }

    /// Seerr through the AlwyWatch server (any media server type). `quiet`: the server may
    /// simply not offer it, which is not worth a message after connecting it.
    fn seerr_bridge(self: &Rc<Self>, quiet: bool) {
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
                    self.ui()
                        .set_seerr_message(sstr(format!("could not store the session: {e}")));
                    return;
                }
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
                                SeerrLogin::Plex => ("plex", "Sign in with Plex"),
                                SeerrLogin::Jellyfin => ("jellyfin", "Sign in with Jellyfin"),
                                SeerrLogin::Emby => ("emby", "Sign in with Emby"),
                                SeerrLogin::Local => ("local", "Seerr account"),
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

    fn set_error(&self, e: Option<CoreError>) {
        self.ui()
            .set_error(sstr(e.map(|e| e.to_string()).unwrap_or_default()));
    }

    // ----- navigation -----------------------------------------------------------------------

    fn navigate(self: &Rc<Self>, route: Route) {
        {
            let mut st = self.state.borrow_mut();
            if st.stack.last() != Some(&route) {
                st.stack.push(route.clone());
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
            st.stack.last().cloned()
        };
        if let Some(r) = route {
            self.show(r);
        }
    }

    fn reset(self: &Rc<Self>, route: Route) {
        self.state.borrow_mut().stack.clear();
        self.navigate(route);
    }

    fn show(self: &Rc<Self>, route: Route) {
        let ui = self.ui();
        ui.set_error(SharedString::new());
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
                self.state.borrow().grid_model.set_vec(vec![]);
            }
            Route::MyList => {
                ui.set_page(Page::Mylist);
                self.load_watchlist();
            }
            Route::Leaving => {
                ui.set_page(Page::Leaving);
                self.load_leaving();
            }
            Route::Triage => {
                ui.set_page(Page::Triage);
                self.load_triage();
            }
            Route::KeepAdmin => {
                ui.set_page(Page::Keepadmin);
                self.load_admin();
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
            }
            Route::Add => {
                ui.set_page(Page::Add);
                ui.set_status(SharedString::new());
                // Connected to an AlwyWatch server: offer its servers not added yet, then
                // Jellyfin or Plex directly.
                ui.set_add_step("choose".into());
                if let Some(k) = self.core.keep_connection() {
                    ui.set_add_from_keep(true);
                    ui.set_preset_servers(ModelRc::default());
                    self.add_keep_servers(k.base_url);
                } else {
                    ui.set_add_from_keep(false);
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
                    let img = image::open(path).ok()?.into_rgba8();
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
        let mut rails: Vec<(String, Vec<Item>)> = vec![("Continue watching".into(), cw)];
        rails.extend(
            home.latest
                .iter()
                .map(|r| (format!("New in {}", r.library.title), r.items.clone())),
        );
        rails.retain(|(_, items)| !items.is_empty());
        let models: Vec<Rc<VecModel<Card>>> =
            rails.iter().map(|_| Rc::new(VecModel::default())).collect();
        // Posters everywhere, like Plex: an episode shows its show's poster.
        for ((t, items), m) in rails.iter().zip(&models) {
            if t == "Continue watching" {
                self.fill(m, items, ImageKind::SeriesPrimary, POSTER_PX, 0, badges);
            } else {
                self.fill(m, items, ImageKind::Primary, POSTER_PX, 0, badges);
            }
        }
        let mut rail_vec: Vec<Rail> = rails
            .iter()
            .zip(&models)
            .map(|((t, _), m)| Rail {
                title: sstr(t),
                items: ModelRc::from(m.clone()),
                landscape: false,
            })
            .collect();
        // Leaving soon (from the keep service cache), after what the user is watching.
        if self.core.keep_active()
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
                    .take_while(|r| r.title == "Continue watching")
                    .count();
                rail_vec.insert(
                    at,
                    Rail {
                        title: "Leaving soon".into(),
                        items: ModelRc::from(model),
                        landscape: false,
                    },
                );
            }
        }
        // Hero: the first title with artwork (then whatever the user points at).
        if let Some(first) = rails
            .iter()
            .flat_map(|(_, items)| items.iter())
            .find(|i| image_ref(i, ImageKind::Backdrop).is_some())
            .cloned()
            && self.ui().get_hero().id.is_empty()
        {
            self.set_hero(&first);
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
            "Nothing here yet.".into()
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
            self.ui().set_status("Loading…".into());
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
            let added = home
                .latest
                .iter()
                .find(|r| &r.library.id == library)
                .map(|r| r.items.clone())
                .unwrap_or_default();
            this.own(&cw, account);
            this.own(&added, account);
            let mut rails = vec![];
            for (title, items, landscape) in [
                ("Continue watching", cw, false),
                ("Recently added", added, false),
            ] {
                if items.is_empty() {
                    continue;
                }
                let m = Rc::new(VecModel::default());
                // Posters, an episode showing its show's.
                this.fill(&m, &items, ImageKind::SeriesPrimary, POSTER_PX, 0, false);
                rails.push(Rail {
                    title: sstr(title),
                    items: ModelRc::from(m),
                    landscape,
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
                "No collections in this library."
            } else {
                ""
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
            ("title", "A–Z", SortBy::Title),
            ("added", "Added", SortBy::DateAdded),
            ("released", "Released", SortBy::ReleaseDate),
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
        }
    }

    /// The home hero: follows the hovered/focused card (only items with a backdrop).
    fn set_hero(self: &Rc<Self>, i: &Item) {
        if image_ref(i, ImageKind::Backdrop).is_none() {
            return;
        }
        let ui = self.ui();
        if ui.get_hero().id.as_str() == i.id {
            return;
        }
        let mut h = Self::detail_of(i);
        // Keep the previous artwork until the new one is decoded (no flash to black).
        let prev = ui.get_hero();
        h.backdrop = prev.backdrop;
        ui.set_hero(h);
        let id = i.id.clone();
        self.load_image(i, ImageKind::Backdrop, BACKDROP_PX, {
            let id = id.clone();
            move |this, img| {
                let ui = this.ui();
                let mut h = ui.get_hero();
                if h.id.as_str() == id {
                    h.backdrop = img;
                    ui.set_hero(h);
                }
            }
        });
        if image_ref(i, ImageKind::Logo).is_some() {
            self.load_image(i, ImageKind::Logo, 800, move |this, img| {
                let ui = this.ui();
                let mut h = ui.get_hero();
                if h.id.as_str() == id {
                    h.logo = img;
                    ui.set_hero(h);
                }
            });
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
        }
        ui.set_detail(d);
        if !same && i.kind != ItemKind::Episode {
            self.load_extras(account, i.id.clone());
        }
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
                if let Some(i) = known {
                    self.render_detail(&i);
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
        let selected = self.state.borrow().season.clone().or_else(|| {
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

    /// Player switches in settings (defaults: auto intro skip off, auto next episode on).
    fn load_player_settings(&self) {
        let get = |k: &str| self.core.get_setting(k.to_owned()).ok().flatten();
        let ui = self.ui();
        ui.set_auto_skip_intro(get("player.auto_skip_intro").as_deref() == Some("1"));
        ui.set_auto_next(get("player.auto_next").as_deref() != Some("0"));
    }

    fn play(self: &Rc<Self>, id: String, from_start: bool) {
        let Some(account) = self.account_of(&id) else {
            return;
        };
        self.play_for(account, id, from_start);
    }

    fn play_for(self: &Rc<Self>, account: String, id: String, from_start: bool) {
        let item = self.state.borrow().items.get(&id).cloned();
        let start_ms = if from_start {
            0
        } else {
            item.as_ref().map(|i| i.user.position_ms).unwrap_or(0)
        };
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
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
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
                return;
            };
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
            let segments = core.segments(account.clone(), id.clone()).await;
            let next = adjacent
                .as_ref()
                .and_then(|a| a.next.as_ref().map(|e| e.id.clone()));
            let previous = adjacent
                .as_ref()
                .and_then(|a| a.previous.as_ref().map(|e| e.id.clone()));
            let req = PlayRequest {
                media: src.direct.clone(),
                start_ms,
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
            };
            let (src_id, session) = (src.id.clone(), info.play_session_id.clone());
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
                };
                let (core, acct) = (core2.clone(), acct2.clone());
                rt.spawn(async move {
                    let _ = core.report_progress(acct, report).await;
                });
                if p.event == PlaybackEvent::Stop {
                    // Back on the UI thread: refresh the page and autoplay the next episode.
                    let (next, previous, acct) = (next.clone(), previous.clone(), acct2.clone());
                    let (finished, go) = (p.finished, p.go);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                            let current = t.state.borrow().stack.last().cloned();
                            if let Some(r) = current {
                                t.show(r);
                            }
                            match go {
                                Some(player::Go::Next) => {
                                    if let Some(n) = next {
                                        t.play_for(acct, n, false);
                                    }
                                }
                                Some(player::Go::Previous) => {
                                    if let Some(p) = previous {
                                        t.play_for(acct, p, false);
                                    }
                                }
                                None if finished => {
                                    if let Some(n) = next {
                                        t.play_for(acct, n, true);
                                    }
                                }
                                None => {}
                            }
                        }
                    });
                }
            });
            if let (Err(e), Some(t)) = (result, this.upgrade()) {
                t.ui().set_error(sstr(e));
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
            format!("Directed by {}", directors.join(", "))
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
                this.ui().set_status(sstr(
                    "Nothing saved yet. On a movie or show, press the bookmark to keep it here.",
                ));
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
            self.state.borrow().grid_model.set_vec(vec![]);
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
        })
        .ok();
    }

    // ----- accounts and settings -----------------------------------------------------------

    fn refresh_settings(&self) {
        let ui = self.ui();
        let current = self.account();
        let keep = self.core.keep_connection();
        let rows: Vec<AccountRow> = self
            .core
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| {
                let sync = match &keep {
                    Some(k) if k.account_id == a.id => "synced with your AlwyWatch server",
                    Some(_) => "not synced with your AlwyWatch server",
                    None => "only on this device",
                };
                AccountRow {
                    current: Some(&a.id) == current.as_ref(),
                    label: sstr(format!("{} @ {}", a.user_name, a.server_name)),
                    detail: sstr(format!("{:?} · {} · {sync}", a.kind, a.base_url)),
                    id: sstr(a.id),
                }
            })
            .collect();
        ui.set_accounts(ModelRc::new(VecModel::from(rows)));
        ui.set_cache_size(sstr(format!(
            "{} MB",
            self.core.image_cache_bytes() / (1024 * 1024)
        )));
        ui.set_about(sstr(format!(
            "AlwyWatch {} (core {}) · GNU AGPL v3 or later · player: {}",
            env!("CARGO_PKG_VERSION"),
            core_version(),
            if crate::mpv::available() {
                "libmpv found"
            } else {
                "libmpv missing"
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
                .set_status(sstr(format!("could not store the token: {e}")));
            return;
        }
        let _ = self
            .core
            .set_setting("current_account".into(), s.account.id.clone());
        // Leaving soon + Seerr through the AlwyWatch server the user started from (if any).
        if let Some(token) = self.keep_token(&s.account.id) {
            let (core, this, id) = (self.core.clone(), Rc::downgrade(self), s.account.id.clone());
            slint::spawn_local(async move {
                if let Ok(Some(_)) = core.keep_onboard(id, token).await
                    && let Some(t) = this.upgrade()
                {
                    t.refresh_keep_settings();
                    t.refresh_seerr_settings();
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
        self.reset(Route::Home);
    }

    fn remove_account(self: &Rc<Self>, id: String) {
        let _ = self.core.remove_account(id.clone());
        let _ = self.tokens.remove(&id);
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
        }
        self.refresh_settings();
    }

    /// Moves between the add-server steps. "alwywatch" and "direct" are the two paths from
    /// the welcome step; "back" returns to the path the user came from.
    fn add_choose(self: &Rc<Self>, which: String) {
        let ui = self.ui();
        ui.set_status(SharedString::new());
        self.state.borrow_mut().poll_gen += 1;
        let step = ui.get_add_step();
        let which = match which.as_str() {
            "back" if step == "receive" => "alwywatch".into(),
            "back" if step == "alwywatch" || step == "direct" || step == "choose" => String::new(),
            "back" if ui.get_add_from_keep() => String::new(),
            "back" if self.state.borrow().preset_url.is_empty() => "direct".into(),
            "back" => "alwywatch".into(),
            _ => which,
        };
        match which.as_str() {
            "alwywatch" => ui.set_add_step("alwywatch".into()),
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
                            subtitle: match (plex, signed_in.contains(&e.id)) {
                                (true, true) => "Plex · already signed in",
                                (false, true) => "Jellyfin · already signed in",
                                (true, false) => "Plex · sign in on plex.tv",
                                (false, false) => "Jellyfin · Quick Connect or password",
                            }
                            .into(),
                            plex,
                        }
                    })
                    .collect::<Vec<_>>(),
            )));
            t.ui().set_status(if list.is_empty() {
                "This AlwyWatch server lists no media servers.".into()
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
                                        format!("{} (yours)", s.name)
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
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| invalid(format!("unreadable transfer: {e}")))?;
        let core = v["core"]
            .as_str()
            .ok_or_else(|| invalid("unreadable transfer".into()))?;
        let ids = self.core.import_config(core.to_owned())?;
        if let Some(t) = v["tokens"].as_object() {
            for (k, t) in t {
                if let Some(t) = t.as_str() {
                    self.tokens
                        .put(k, t)
                        .map_err(|e| invalid(format!("could not store the token: {e}")))?;
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
                }
                let until = std::time::Instant::now() + Duration::from_secs(c.expires_in);
                // Until it expires (then a new code).
                while std::time::Instant::now() < until {
                    async_sleep(Duration::from_secs(2)).await;
                    if !waiting(&this) {
                        return;
                    }
                    let Ok(Some(bundle)) =
                        core.transfer_receive(url.clone(), c.handle.clone()).await
                    else {
                        continue;
                    };
                    let Some(t) = this.upgrade() else { return };
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
                        Ok(None) => t.ui().set_transfer_status(
                            "Nothing to copy: that device has no accounts.".into(),
                        ),
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
            ui.set_send_error("Type the 6 digits the new device shows.".into());
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
                    ui.set_send_error("Unknown or expired code.".into());
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
        } else {
            self.navigate(Route::Settings);
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
    }

    /// Test hook used by headless screenshots: a home from two made-up servers (server marks).
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
            ..Default::default()
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
            if id.starts_with("leaving:") {
                a.navigate(Route::Leaving);
            } else if let Some(title) = collection {
                let library = a.ui().get_library_id().to_string();
                a.navigate(Route::Filtered(library, title, None, Some(id.into())));
            } else {
                a.navigate(Route::Detail(id.into()));
            }
        });
        on!(on_prefetch, |a, id| {
            a.prefetch(&id);
            if a.ui().get_page() == Page::Home {
                let item = a.state.borrow().items.get(id.as_str()).cloned();
                if let Some(i) = item {
                    a.set_hero(&i);
                }
            }
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
        on!(on_play, |a, id, from_start| a.play(id.into(), from_start));
        on!(on_toggle_played, |a, id| a.toggle_played(id.into()));
        on!(on_toggle_watchlist, |a, id| a.toggle_watchlist(id.into()));
        on!(on_play_trailer, |a, id| a.play_trailer(id.into()));
        on!(on_select_season, |a, id| a.select_season(id.into()));
        on!(on_search, |a, text| a.search(text.into()));
        on!(on_go, |a, page| a.navigate(match page {
            Page::Search => Route::Search,
            Page::Mylist => Route::MyList,
            Page::Settings => Route::Settings,
            Page::Add => Route::Add,
            Page::Downloads => Route::Downloads,
            Page::Requests => Route::Requests,
            Page::Leaving => Route::Leaving,
            Page::Triage => Route::Triage,
            Page::Keepadmin => Route::KeepAdmin,
            _ => Route::Home,
        }));
        on!(on_add_choose, |a, which| a.add_choose(which.into()));
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
        on!(on_keep_choose, |a| a.keep_choose());
        on!(on_keep_sync, |a, id| {
            let url = a
                .core
                .keep_connection()
                .map(|k| k.base_url)
                .unwrap_or_else(|| a.state.borrow().keep_pending_url.clone());
            a.keep_sync_as(url, id.into());
        });
        on!(on_keep_disconnect, |a| {
            let _ = a.core.keep_disconnect();
            a.ui().set_keep_choices(ModelRc::default());
            a.refresh_keep_settings();
            a.refresh_settings();
            // A bridged Seerr goes with it.
            a.refresh_seerr_settings();
        });
        on!(on_keep_vote, |a, key, choice| a
            .keep_vote(key.into(), choice.into()));
        on!(on_triage_vote, |a, choice| a.triage_vote(choice.into()));
        on!(on_admin_rights, |a, user, field| a
            .admin_rights(user.into(), field.into()));
        on!(on_admin_countdown, |a, id, stagger| a
            .admin_countdown(id.into(), stagger));
        on!(on_seerr_login, |a, m| a.seerr_login(m.into()));
        on!(on_seerr_password, |a, u, p| a
            .seerr_password(u.into(), p.into()));
        on!(on_seerr_disconnect, |a| {
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
async fn async_sleep(d: Duration) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    slint::Timer::single_shot(d, move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
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

#[cfg(test)]
mod clock_tests {
    #[test]
    fn formats_like_the_android_player() {
        assert_eq!(super::clock(552_000), "9:12");
        assert_eq!(super::clock(3_723_000), "1:02:03");
        assert_eq!(super::clock(0), "0:00");
    }
}
