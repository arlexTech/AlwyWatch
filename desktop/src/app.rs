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
use crate::{AccountRow, AppWindow, Card, Detail, NamedId, Page, Rail};

const POSTER_PX: u32 = 300;
const EPISODE_PX: u32 = 320;
const BACKDROP_PX: u32 = 1280;
const PAGE: u32 = 60;

#[derive(Clone, PartialEq)]
enum Route {
    Home,
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
    series: Option<String>,
    season: Option<String>,
    search_gen: u64,
    poll_gen: u64,
}

thread_local! {
    /// The UI-thread App, reachable from `invoke_from_event_loop` closures (which must be `Send`).
    static APP: RefCell<std::rc::Weak<App>> = const { RefCell::new(std::rc::Weak::new()) };
}

pub struct App {
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
                series: None,
                season: None,
                search_gen: 0,
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
        let first = if app.state.borrow().account.is_some() {
            Route::Home
        } else {
            Route::Add
        };
        app.navigate(first);
        app
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
            Route::Settings => {
                ui.set_page(Page::Settings);
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
                .map(|r| (format!("Latest in {}", r.library.title), r.items.clone())),
        );
        rails.retain(|(_, items)| !items.is_empty());
        let models: Vec<Rc<VecModel<Card>>> =
            rails.iter().map(|_| Rc::new(VecModel::default())).collect();
        for ((_, items), m) in rails.iter().zip(&models) {
            self.fill(m, items, ImageKind::Primary, POSTER_PX, 0);
        }
        let rail_vec: Vec<Rail> = rails
            .iter()
            .zip(&models)
            .map(|((t, _), m)| Rail {
                title: sstr(t),
                items: ModelRc::from(m.clone()),
            })
            .collect();
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

    fn render_detail(self: &Rc<Self>, i: &Item) {
        let ui = self.ui();
        self.state
            .borrow_mut()
            .items
            .insert(i.id.clone(), i.clone());
        let playable = matches!(i.kind, ItemKind::Movie | ItemKind::Episode);
        let mut meta = vec![item_subtitle(i)];
        meta.extend(i.official_rating.clone());
        meta.extend(i.community_rating.map(|r| format!("★ {r:.1}")));
        meta.retain(|m| !m.is_empty());
        let mut d = Detail {
            id: sstr(&i.id),
            title: sstr(&i.title),
            meta: sstr(meta.join("  ·  ")),
            overview: sstr(i.overview.clone().unwrap_or_default()),
            genres: sstr(i.genres.join(", ")),
            backdrop: Image::default(),
            poster: Image::default(),
            playable,
            resumable: playable && i.user.position_ms > 0,
            played: i.user.played,
            is_series: i.kind == ItemKind::Series,
        };
        if ui.get_detail().id == d.id {
            d.backdrop = ui.get_detail().backdrop;
            d.poster = ui.get_detail().poster;
        }
        ui.set_detail(d);
        let id = i.id.clone();
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
        let item = self.state.borrow().items.get(&id).cloned();
        let start_ms = if from_start {
            0
        } else {
            item.as_ref().map(|i| i.user.position_ms).unwrap_or(0)
        };
        let title = item.as_ref().map(|i| i.title.clone()).unwrap_or_default();
        let next = self.next_episode(&id);
        let spdif = self.ui().get_spdif();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let info = match core
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
        if let Err(e) = self.tokens.put(&s.account.id, &s.token) {
            self.ui()
                .set_status(sstr(format!("could not store the token: {e}")));
            return;
        }
        let _ = self
            .core
            .set_setting("current_account".into(), s.account.id.clone());
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

    fn add_choose(self: &Rc<Self>, which: String) {
        let ui = self.ui();
        ui.set_status(SharedString::new());
        self.state.borrow_mut().poll_gen += 1;
        match which.as_str() {
            "jellyfin" => ui.set_add_step("jellyfin-url".into()),
            "plex" => {
                ui.set_add_step("plex".into());
                ui.set_plex_code("…".into());
                self.plex_flow();
            }
            _ => ui.set_add_step("choose".into()),
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
            let Some(q) = quick else { return };
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
                    Err(e) => return report(&this, e),
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
                        st.plex_servers = servers;
                        drop(st);
                        t.ui().set_add_step("plex-servers".into());
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
        on!(on_open_item, |a, id| a.navigate(Route::Detail(id.into())));
        on!(on_prefetch, |a, id| a.prefetch(&id));
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
            _ => Route::Home,
        }));
        on!(on_add_choose, |a, which| a.add_choose(which.into()));
        on!(on_jellyfin_url, |a, url| a.jellyfin_url(url.into()));
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
