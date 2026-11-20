// SPDX-License-Identifier: AGPL-3.0-or-later
//! Glue between the Slint UI and the core. Every page renders what the core has cached in the
//! same frame it is opened, then refreshes in the background (same model as Android).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use alwywatch_core::*;
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
    Weak,
};

use crate::platform::{self, Tokens};
use crate::player::{self, PlayRequest};
use crate::{
    AccountRow, AdminRow, AppWindow, Card, Detail, DownloadRow, LeavingRow, NamedId, Page, Rail,
    SeerrPanel, ServerChoice,
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
    Detail(String),
    Search,
    Settings,
    Add,
}

struct GridState {
    library: String,
    sort: SortBy,
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
    seerr_results: Vec<SeerrResult>,
    seerr_selected: Option<SeerrResult>,
    seerr_seasons: Vec<(SeerrSeason, bool)>,
    triage: Vec<LeavingItem>,
    admin_users: Vec<AdminUser>,
    armed: Option<(String, i32)>,
    poll_gen: u64,
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
    i.images.iter().find(|r| r.kind == kind).cloned()
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
                seerr_results: vec![],
                seerr_selected: None,
                seerr_seasons: vec![],
                triage: vec![],
                admin_users: vec![],
                armed: None,
                poll_gen: 0,
            }),
            images: RefCell::new(HashMap::new()),
        });
        APP.with(|a| *a.borrow_mut() = Rc::downgrade(&app));
        ui.set_grid(ModelRc::from(app.state.borrow().grid_model.clone()));
        ui.set_episodes(ModelRc::from(app.state.borrow().episodes_model.clone()));
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
        let Some(account) = self.account() else {
            return;
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.download_item(account, id).await;
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
                        format!("req:{}", r.id),
                        r.title.unwrap_or_else(|| format!("TMDB {}", r.tmdb_id)),
                        status.to_owned(),
                        r.poster_url,
                    )
                })
                .collect();
            this.seerr_cards(cards, AppWindow::set_seerr_mine);
        };
        if let Ok(Some(list)) = self.core.cached_seerr_requests() {
            render(self, list);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_seerr_requests(false).await;
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
            self.state.borrow_mut().seerr_results.clear();
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
                    let cards = results
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
                        .collect();
                    t.state.borrow_mut().seerr_results = results;
                    t.seerr_cards(cards, AppWindow::set_seerr_results);
                }
                Err(e) => t.ui().set_seerr_message(sstr(e.to_string())),
            }
        })
        .ok();
    }

    fn seerr_open(self: &Rc<Self>, key: String) {
        let r = self
            .state
            .borrow()
            .seerr_results
            .iter()
            .find(|r| format!("{:?}:{}", r.media_type, r.tmdb_id) == key)
            .cloned();
        let Some(r) = r else { return };
        let is_tv = r.media_type == SeerrMediaType::Tv;
        self.ui().set_seerr_panel(SeerrPanel {
            open: true,
            key: sstr(&key),
            title: sstr(
                [r.title.clone(), r.year.clone().unwrap_or_default()]
                    .join(" ")
                    .trim(),
            ),
            overview: sstr(r.overview.clone().unwrap_or_default()),
            status: sstr(Self::availability_label(r.availability)),
            is_tv,
            can_request: r.availability != Availability::Available,
        });
        {
            let mut st = self.state.borrow_mut();
            st.seerr_selected = Some(r.clone());
            st.seerr_seasons.clear();
        }
        self.ui().set_seerr_seasons(ModelRc::default());
        if is_tv {
            let (core, this) = (self.core.clone(), Rc::downgrade(self));
            slint::spawn_local(async move {
                let seasons = core.seerr_seasons(r.tmdb_id).await.unwrap_or_default();
                if let Some(t) = this.upgrade() {
                    t.state.borrow_mut().seerr_seasons =
                        seasons.into_iter().map(|s| (s, false)).collect();
                    t.render_seerr_seasons();
                }
            })
            .ok();
        }
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
            t.ui().set_seerr_panel(SeerrPanel::default());
            let _ = core.refresh_seerr_requests(true).await;
            t.load_seerr_requests();
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
                self.open_library(id, SortBy::Title);
            }
            Route::Detail(id) => {
                ui.set_page(Page::Detail);
                self.load_detail(id);
            }
            Route::Search => {
                ui.set_page(Page::Search);
                self.state.borrow().grid_model.set_vec(vec![]);
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
                ui.set_add_step("choose".into());
                ui.set_status(SharedString::new());
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
        let Some(account) = self.account() else {
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
            let key = image_ref(i, kind)
                .map(|r| format!("{}|{}|{px}", self.account().unwrap_or_default(), r.path));
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

    fn render_home(self: &Rc<Self>, home: &Home) {
        let ui = self.ui();
        let mut rails: Vec<(String, Vec<Item>)> = vec![
            ("Continue watching".into(), home.resume.clone()),
            ("Next up".into(), home.next_up.clone()),
        ];
        rails.extend(
            home.latest
                .iter()
                .map(|r| (format!("New in {}", r.library.title), r.items.clone())),
        );
        rails.retain(|(_, items)| !items.is_empty());
        let models: Vec<Rc<VecModel<Card>>> =
            rails.iter().map(|_| Rc::new(VecModel::default())).collect();
        // What the user is watching: landscape stills; the rest: posters.
        let landscape = |t: &str| t == "Continue watching" || t == "Next up";
        for ((t, items), m) in rails.iter().zip(&models) {
            if landscape(t) {
                self.fill(m, items, ImageKind::Backdrop, 640, 0);
            } else {
                self.fill(m, items, ImageKind::Primary, POSTER_PX, 0);
            }
        }
        let mut rail_vec: Vec<Rail> = rails
            .iter()
            .zip(&models)
            .map(|((t, _), m)| Rail {
                title: sstr(t),
                items: ModelRc::from(m.clone()),
                landscape: landscape(t),
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
                let at = rail_vec.iter().take_while(|r| r.landscape).count();
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

    fn load_home(self: &Rc<Self>) {
        let Some(account) = self.account() else {
            return;
        };
        match self.core.cached_home(account.clone()) {
            Ok(Some(h)) => self.render_home(&h),
            _ => self.ui().set_status("Loading…".into()),
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_home(account, false).await;
            let Some(this) = this.upgrade() else { return };
            if this.ui().get_page() != Page::Home {
                return;
            }
            match r {
                Ok(h) => this.render_home(&h),
                Err(e) => this.set_error(Some(e)),
            }
        })
        .ok();
    }

    // ----- library grid ---------------------------------------------------------------------

    fn query(library: &str, sort: SortBy, start: u32) -> ItemQuery {
        ItemQuery {
            library_id: Some(library.to_owned()),
            start,
            limit: PAGE,
            sort_by: sort,
            descending: sort != SortBy::Title,
            unplayed_only: false,
            search: None,
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

    fn open_library(self: &Rc<Self>, library: String, sort: SortBy) {
        let Some(account) = self.account() else {
            return;
        };
        self.render_sorts(sort);
        let model = self.state.borrow().grid_model.clone();
        model.set_vec(vec![]);
        // Every cached page at once: the grid appears complete instantly.
        let mut loaded = 0u32;
        let mut total = 0u32;
        while let Ok(Some(p)) = self
            .core
            .cached_items(account.clone(), Self::query(&library, sort, loaded))
        {
            self.fill(
                &model,
                &p.page.items,
                ImageKind::Primary,
                POSTER_PX,
                loaded as usize,
            );
            total = p.page.total;
            loaded += p.page.items.len() as u32;
            if p.page.items.len() < PAGE as usize {
                break;
            }
        }
        self.state.borrow_mut().grid = Some(GridState {
            library,
            sort,
            loaded,
            total,
            loading: false,
        });
        self.load_page(0);
    }

    fn load_page(self: &Rc<Self>, start: u32) {
        let Some(account) = self.account() else {
            return;
        };
        let (library, sort) = {
            let mut st = self.state.borrow_mut();
            let Some(g) = st.grid.as_mut() else { return };
            if g.loading {
                return;
            }
            g.loading = true;
            (g.library.clone(), g.sort)
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core
                .refresh_items(account, Self::query(&library, sort, start), false)
                .await;
            let Some(this) = this.upgrade() else { return };
            let model = this.state.borrow().grid_model.clone();
            match r {
                Ok(p) => {
                    let same = this
                        .state
                        .borrow()
                        .grid
                        .as_ref()
                        .is_some_and(|g| g.library == library && g.sort == sort);
                    if same {
                        this.fill(
                            &model,
                            &p.page.items,
                            ImageKind::Primary,
                            POSTER_PX,
                            start as usize,
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
            played: i.user.played,
            is_series: i.kind == ItemKind::Series,
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
        if ui.get_detail().id == d.id {
            d.backdrop = ui.get_detail().backdrop;
            d.poster = ui.get_detail().poster;
            d.logo = ui.get_detail().logo;
        }
        ui.set_detail(d);
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
        let Some(account) = self.account() else {
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
        let Some(account) = self.account() else {
            return;
        };
        self.state.borrow_mut().series = Some(series.clone());
        if let Ok(Some(r)) = self.core.cached_seasons(account.clone(), series.clone()) {
            self.render_seasons(&r.items);
        }
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.refresh_seasons(account, series.clone(), false).await;
            let Some(this) = this.upgrade() else { return };
            if this.state.borrow().series.as_deref() != Some(&series) {
                return;
            }
            if let Ok(r) = r {
                this.render_seasons(&r.items);
            }
        })
        .ok();
    }

    fn select_season(self: &Rc<Self>, season: String) {
        let Some(account) = self.account() else {
            return;
        };
        let Some(series) = self.state.borrow().series.clone() else {
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
        let show = |this: &Rc<Self>, eps: &[Item]| {
            let model = this.state.borrow().episodes_model.clone();
            this.fill(&model, eps, ImageKind::Primary, EPISODE_PX, 0);
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
        let Some(account) = self.account() else {
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

    fn play(self: &Rc<Self>, id: String, from_start: bool) {
        let Some(account) = self.account() else {
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
        let next = self.next_episode(&id);
        let spdif = self.ui().get_spdif();
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
                .filter_map(|s| s.external.as_ref().map(|r| r.url.clone()))
                .collect();
            let req = PlayRequest {
                media: src.direct.clone(),
                start_ms,
                title,
                external_subs: subs,
                spdif,
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
                    let next = next.clone();
                    let finished = p.finished;
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                            let current = t.state.borrow().stack.last().cloned();
                            if let Some(r) = current {
                                t.show(r);
                            }
                            if finished && let Some(n) = next {
                                t.play(n, true);
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

    fn next_episode(&self, id: &str) -> Option<String> {
        let st = self.state.borrow();
        let item = st.items.get(id)?;
        if item.kind != ItemKind::Episode {
            return None;
        }
        let eps = self
            .core
            .cached_episodes(
                self.account()?,
                item.series_id.clone()?,
                item.season_id.clone()?,
            )
            .ok()??;
        let idx = eps.items.iter().position(|e| e.id == id)?;
        eps.items.get(idx + 1).map(|e| e.id.clone())
    }

    fn toggle_played(self: &Rc<Self>, id: String) {
        let Some(account) = self.account() else {
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
        let Some(account) = self.account() else {
            return;
        };
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
            let r = core.search(account, text, 60).await;
            let Some(this) = this.upgrade() else { return };
            if this.state.borrow().search_gen != generation {
                return;
            }
            match r {
                Ok(items) => {
                    let model = this.state.borrow().grid_model.clone();
                    model.set_vec(vec![]);
                    this.fill(&model, &items, ImageKind::Primary, POSTER_PX, 0);
                }
                Err(e) => this.set_error(Some(e)),
            }
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
            .map(|a| AccountRow {
                current: Some(&a.id) == current.as_ref(),
                label: sstr(format!("{} @ {}", a.user_name, a.server_name)),
                detail: sstr(format!("{:?} · {}", a.kind, a.base_url)),
                id: sstr(a.id),
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
            "back" if step == "alwywatch" || step == "direct" || step == "choose" => String::new(),
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
            t.ui().set_preset_servers(ModelRc::new(VecModel::from(
                list.iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let plex = e.kind == ServerType::Plex;
                        ServerChoice {
                            id: sstr(i.to_string()),
                            title: sstr(&e.name),
                            subtitle: if plex {
                                "Plex · sign in on plex.tv"
                            } else {
                                "Jellyfin · Quick Connect or password"
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

    /// Test hook used by headless screenshots: open settings and start connecting Seerr.
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
            if id.starts_with("leaving:") {
                a.navigate(Route::Leaving);
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
            let lib = a.state.borrow().grid.as_ref().map(|g| g.library.clone());
            if let Some(lib) = lib {
                a.open_library(lib, sort);
            }
        });
        on!(on_grid_scrolled, |a, f| a.grid_scrolled(f));
        on!(on_play, |a, id, from_start| a.play(id.into(), from_start));
        on!(on_toggle_played, |a, id| a.toggle_played(id.into()));
        on!(on_select_season, |a, id| a.select_season(id.into()));
        on!(on_search, |a, text| a.search(text.into()));
        on!(on_go, |a, page| a.navigate(match page {
            Page::Search => Route::Search,
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
        on!(on_seerr_close, |a| a
            .ui()
            .set_seerr_panel(SeerrPanel::default()));
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
