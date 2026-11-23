// SPDX-License-Identifier: AGPL-3.0-or-later
//! My AlwyWatch account on the desktop (docs/profile-apps.md): the avatars drawn everywhere a
//! person is shown, Settings → AlwyWatch account (name, avatar), the cropper for an uploaded
//! picture, and the one-time welcome step.

use std::path::PathBuf;
use std::sync::OnceLock;

use super::*;
use crate::avatars::{self, Crop};
use crate::{PersonAvatar, PresetRow};

/// The cropper's viewport and round frame, in pixels (same numbers as app.slint).
const VIEWPORT: f32 = 360.0;
const FRAME: f32 = 300.0;
/// Where my `Me` is cached (Settings renders it instantly).
const ME_KEY: &str = "keep.me";

/// The avatar chosen in the welcome step, saved on Continue.
pub(super) enum Draft {
    Default,
    Preset(String),
    Upload(Vec<u8>, Image),
}

/// A picture being cropped: the file (read again on Save) and where the frame is.
struct CropJob {
    path: PathBuf,
    crop: Crop,
    welcome: bool,
}

#[derive(Default)]
pub(super) struct MeState {
    me: Option<Me>,
    draft: Option<Draft>,
    crop: Option<CropJob>,
    picking: bool,
    /// Avatars arrived: one redraw for the lot.
    redraw_pending: bool,
}

/// The built-in avatars drawn, by id.
type Presets = Rc<Vec<(String, Image)>>;

thread_local! {
    /// The built-in avatars, drawn once.
    static PRESETS: RefCell<Option<Presets>> = const { RefCell::new(None) };
}

fn presets() -> Presets {
    PRESETS.with(|p| {
        p.borrow_mut()
            .get_or_insert_with(|| {
                Rc::new(
                    avatar_presets()
                        .into_iter()
                        .map(|a| {
                            let px = avatars::PRESET_PX;
                            let rgba =
                                avatars::draw_preset(&a.shape, &a.background, &a.foreground, px);
                            let buf =
                                SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, px, px);
                            (a.id, Image::from_rgba8(buf))
                        })
                        .collect(),
                )
            })
            .clone()
    })
}

fn preset_image(id: &str) -> Option<Image> {
    presets()
        .iter()
        .find(|(p, _)| p == id)
        .map(|(_, i)| i.clone())
}

fn color(hex: &str) -> slint::Color {
    let (r, g, b) = avatars::parse_hex(hex);
    slint::Color::from_rgb_u8(r, g, b)
}

/// The initial on its colour.
fn default_avatar(name: &str) -> PersonAvatar {
    PersonAvatar {
        initial: sstr(avatar_initial(name.to_owned())),
        color: color(&avatar_default_color(name.to_owned())),
        picture: Image::default(),
        has_picture: false,
    }
}

fn with_picture(name: &str, picture: Image) -> PersonAvatar {
    PersonAvatar {
        picture,
        has_picture: true,
        ..default_avatar(name)
    }
}

fn http() -> Option<&'static net::reqwest::Client> {
    static CLIENT: OnceLock<Option<net::reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| net::api_client(concat!("AlwyWatch/", env!("CARGO_PKG_VERSION"))).ok())
        .as_ref()
}

/// The same client, for reading the other accounts' `Me` (Linked sign-ins).
pub(super) fn http_client() -> Option<net::reqwest::Client> {
    http().cloned()
}

/// A failed profile call in plain words.
fn profile_error(e: &CoreError) -> String {
    match e {
        CoreError::Server { status: 429 } => tr!("Too many changes: try again in a few minutes."),
        CoreError::Server { status: 413 } => tr!("This picture is too large."),
        CoreError::Server { status: 400 } => tr!("Your AlwyWatch server refused this."),
        CoreError::NotFound | CoreError::Server { status: 405 } => {
            tr!("Your AlwyWatch server doesn't support this yet: it needs an update.")
        }
        CoreError::Network { .. } => tr!("Can't reach your AlwyWatch server right now."),
        e => e.to_string(),
    }
}

impl App {
    // ----- drawing people ------------------------------------------------------------------

    /// How to draw someone: their picture (cached by user and version; the default until it
    /// arrives), their built-in avatar, else their initial on their colour. `user_key` empty:
    /// unknown user (no custom picture can be fetched).
    pub(super) fn person_avatar(&self, user_key: &str, name: &str, a: &Avatar) -> PersonAvatar {
        if let Some(v) = a.custom_version {
            return match self.face(user_key, v) {
                Some(img) => with_picture(name, img),
                None => default_avatar(name),
            };
        }
        match a.preset.as_deref().and_then(preset_image) {
            Some(img) => with_picture(name, img),
            None => default_avatar(name),
        }
    }

    /// A custom picture from memory, else fetched once (disk cache, then the server).
    fn face(&self, user_key: &str, version: u32) -> Option<Image> {
        if user_key.is_empty() {
            return None;
        }
        let key = (user_key.to_owned(), version);
        if let Some(found) = self.faces.borrow().get(&key) {
            return found.clone();
        }
        self.faces.borrow_mut().insert(key.clone(), None);
        let (Ok(req), Some(k)) = (
            self.core.keep_avatar_request(user_key.to_owned(), version),
            self.core.keep_connection(),
        ) else {
            return None;
        };
        let dir = platform::paths().cache.join("avatars");
        let name = avatars::cache_name(&k.base_url, user_key, version);
        let core = self.core.clone();
        slint::spawn_local(async move {
            let job = core.runtime().spawn(async move {
                let file = dir.join(&name);
                let bytes = match std::fs::read(&file) {
                    Ok(b) => b,
                    Err(_) => {
                        let mut get = http()?.get(&req.url);
                        for h in &req.headers {
                            get = get.header(h.name.as_str(), h.value.as_str());
                        }
                        let bytes = net::send(get).await.ok()?.bytes().await.ok()?.to_vec();
                        let _ = std::fs::create_dir_all(&dir);
                        // Older versions of this person's picture are never shown again.
                        let prefix = name.split('-').next().unwrap_or_default().to_owned();
                        if let Ok(entries) = std::fs::read_dir(&dir) {
                            for e in entries.flatten() {
                                if e.file_name().to_string_lossy().starts_with(&prefix) {
                                    let _ = std::fs::remove_file(e.path());
                                }
                            }
                        }
                        let _ = std::fs::write(&file, &bytes);
                        bytes
                    }
                };
                let img = avatars::decode(&bytes)?;
                let (w, h) = img.dimensions();
                Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                    img.as_raw(),
                    w,
                    h,
                ))
            });
            let Some(buf) = job.await.ok().flatten() else {
                return;
            };
            if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                t.faces
                    .borrow_mut()
                    .insert(key, Some(Image::from_rgba8(buf)));
                t.avatars_changed();
            }
        })
        .ok();
        None
    }

    /// Pictures arrived: redraw everything showing people (once for a burst).
    fn avatars_changed(self: &Rc<Self>) {
        if std::mem::replace(&mut self.state.borrow_mut().me.redraw_pending, true) {
            return;
        }
        let w = Rc::downgrade(self);
        slint::Timer::single_shot(Duration::from_millis(40), move || {
            let Some(t) = w.upgrade() else { return };
            t.state.borrow_mut().me.redraw_pending = false;
            t.render_me();
            t.render_presets();
            t.render_room();
            t.render_picker();
            t.render_banner();
            match t.ui().get_page() {
                Page::Together => t.render_rooms_list(),
                Page::Keepadmin => {
                    t.render_admin();
                    t.render_forever();
                }
                _ => {}
            }
        });
    }

    // ----- my account ----------------------------------------------------------------------

    /// Settings: my name and avatar as last seen, then fresh.
    pub(super) fn load_me(self: &Rc<Self>) {
        if !self.core.keep_active() {
            return;
        }
        if self.state.borrow().me.me.is_none() {
            let cached = self
                .core
                .get_setting(ME_KEY.into())
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str::<Me>(&s).ok());
            self.state.borrow_mut().me.me = cached;
        }
        self.render_me();
        self.fetch_me(false);
    }

    /// At start and right after linking: the welcome step while this account hasn't done it.
    pub(super) fn check_welcome(self: &Rc<Self>) {
        if self.core.keep_active() {
            self.fetch_me(true);
        }
    }

    fn fetch_me(self: &Rc<Self>, welcome: bool) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_me().await;
            let Some(t) = this.upgrade() else { return };
            // Settings → Account: the AlwyWatch server's state.
            t.ui().set_alwy_state(sstr(match &r {
                Ok(_) => "ok",
                Err(CoreError::Unauthorized) => "expired",
                Err(_) => "offline",
            }));
            match r {
                Ok(me) => {
                    let show = welcome && !me.profile_set;
                    t.set_me(me);
                    if show && t.ui().get_account_dialog().is_empty() {
                        t.open_welcome();
                    }
                }
                Err(e) => {
                    if t.ui().get_page() == Page::Settings {
                        t.ui().set_me_message(sstr(profile_error(&e)));
                    }
                }
            }
        })
        .ok();
    }

    fn set_me(self: &Rc<Self>, me: Me) {
        if let Ok(json) = serde_json::to_string(&me) {
            let _ = self.core.set_setting(ME_KEY.into(), json);
        }
        self.state.borrow_mut().me.me = Some(me);
        self.ui().set_me_message(SharedString::new());
        self.render_me();
        self.render_presets();
    }

    /// Whether my welcome step is still to come (None: profile not known yet).
    pub(super) fn me_profile_set(&self) -> Option<bool> {
        self.state.borrow().me.me.as_ref().map(|m| m.profile_set)
    }

    /// The AlwyWatch server was unlinked: forget my profile.
    pub(super) fn forget_me(&self) {
        self.state.borrow_mut().me.me = None;
        let _ = self.core.set_setting(ME_KEY.into(), String::new());
        let ui = self.ui();
        ui.set_me_loaded(false);
        ui.set_account_dialog(SharedString::new());
    }

    fn render_me(&self) {
        let ui = self.ui();
        let Some(me) = self.state.borrow().me.me.clone() else {
            ui.set_me_loaded(false);
            ui.set_me_avatar(default_avatar("?"));
            ui.set_me_detail(sstr(tr!("How the people you watch with see you.")));
            return;
        };
        ui.set_me_loaded(true);
        ui.set_me_avatar(self.person_avatar(&me.user_key, &me.name, &me.avatar));
        ui.set_me_name(sstr(&me.name));
        ui.set_me_detail(sstr(
            if me.name_is_custom && !me.media_name.is_empty() && me.media_name != me.name {
                tr!(
                    "How the people you watch with see you ({name} on your media server).",
                    name = me.media_name
                )
            } else {
                tr!("How the people you watch with see you.")
            },
        ));
        ui.set_me_name_custom(me.name_is_custom);
        ui.set_me_media_name(sstr(if me.media_name.is_empty() {
            tr!("your media server")
        } else {
            me.media_name.clone()
        }));
        ui.set_me_has_avatar(me.avatar.preset.is_some() || me.avatar.custom_version.is_some());
    }

    /// The avatar grid of the open dialog (welcome: the default first, then the built-in ones
    /// and the uploaded picture waiting for Continue).
    fn render_presets(&self) {
        let ui = self.ui();
        let dialog = ui.get_account_dialog();
        if dialog != "avatar" && dialog != "welcome" {
            return;
        }
        let st = self.state.borrow();
        let Some(me) = st.me.me.as_ref() else { return };
        let welcome = dialog == "welcome";
        let name = if welcome {
            ui.get_account_name_input().to_string()
        } else {
            me.name.clone()
        };
        // Selected: the draft (welcome), else the current avatar.
        let chosen: Option<String> = match (&st.me.draft, welcome) {
            (Some(Draft::Default), true) => Some("default".into()),
            (Some(Draft::Preset(id)), true) => Some(id.clone()),
            (Some(Draft::Upload(..)), true) => Some("upload".into()),
            _ => match (&me.avatar.preset, me.avatar.custom_version) {
                (Some(p), _) => Some(p.clone()),
                (None, None) => Some("default".into()),
                _ => None,
            },
        };
        let mut rows = Vec::new();
        if welcome {
            rows.push(PresetRow {
                id: "default".into(),
                avatar: default_avatar(&name),
                selected: chosen.as_deref() == Some("default"),
            });
        }
        for (id, img) in presets().iter() {
            rows.push(PresetRow {
                id: sstr(id),
                avatar: with_picture(&name, img.clone()),
                selected: chosen.as_deref() == Some(id.as_str()),
            });
        }
        if let (true, Some(Draft::Upload(_, img))) = (welcome, &st.me.draft) {
            rows.push(PresetRow {
                id: "upload".into(),
                avatar: with_picture(&name, img.clone()),
                selected: true,
            });
        }
        ui.set_account_presets(ModelRc::new(VecModel::from(rows)));
        if welcome {
            ui.set_welcome_avatar(match &st.me.draft {
                Some(Draft::Default) => default_avatar(&name),
                Some(Draft::Preset(id)) => preset_image(id)
                    .map(|i| with_picture(&name, i))
                    .unwrap_or_else(|| default_avatar(&name)),
                Some(Draft::Upload(_, img)) => with_picture(&name, img.clone()),
                None => self.person_avatar(&me.user_key, &name, &me.avatar),
            });
        }
    }

    pub(super) fn account_dialog_open(self: &Rc<Self>, kind: String) {
        let Some(me) = self.state.borrow().me.me.clone() else {
            return;
        };
        let ui = self.ui();
        ui.set_account_message(SharedString::new());
        ui.set_account_name_problem(SharedString::new());
        ui.set_account_busy(false);
        if kind == "name" {
            ui.set_account_name_input(sstr(&me.name));
        }
        ui.set_account_dialog(sstr(&kind));
        self.render_presets();
    }

    fn open_welcome(self: &Rc<Self>) {
        let Some(me) = self.state.borrow().me.me.clone() else {
            return;
        };
        self.state.borrow_mut().me.draft = None;
        let ui = self.ui();
        ui.set_account_message(SharedString::new());
        ui.set_account_name_problem(SharedString::new());
        ui.set_account_busy(false);
        ui.set_account_name_input(sstr(&me.name));
        ui.set_account_dialog("welcome".into());
        self.render_presets();
    }

    pub(super) fn account_dialog_close(&self) {
        let ui = self.ui();
        if ui.get_account_dialog() == "welcome" {
            return;
        }
        ui.set_account_dialog(SharedString::new());
    }

    pub(super) fn account_name_edited(&self, text: String) {
        let ui = self.ui();
        ui.set_account_name_problem(sstr(display_name_problem(text).unwrap_or_default()));
        if ui.get_account_dialog() == "welcome" {
            self.render_presets();
        }
    }

    pub(super) fn account_name_save(self: &Rc<Self>) {
        let ui = self.ui();
        let name = ui.get_account_name_input().to_string();
        if let Some(p) = display_name_problem(name.clone()) {
            ui.set_account_name_problem(sstr(p));
            return;
        }
        let unchanged = self
            .state
            .borrow()
            .me
            .me
            .as_ref()
            .is_some_and(|m| m.name == name.trim());
        if unchanged {
            ui.set_account_dialog(SharedString::new());
            return;
        }
        ui.set_account_busy(true);
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.keep_set_display_name(name).await;
            let Some(t) = this.upgrade() else { return };
            t.ui().set_account_busy(false);
            match r {
                Ok(me) => {
                    t.set_me(me);
                    t.ui().set_account_dialog(SharedString::new());
                }
                Err(e) => t.ui().set_account_message(sstr(profile_error(&e))),
            }
        })
        .ok();
    }

    pub(super) fn account_name_reset(self: &Rc<Self>) {
        self.account_call(
            true,
            |core| async move { core.keep_reset_display_name().await },
        );
    }

    /// A profile call from a dialog: on success my new `Me` (and the dialog closed when
    /// `close`), else the reason in the dialog.
    fn account_call<F, Fut>(self: &Rc<Self>, close: bool, call: F)
    where
        F: FnOnce(Arc<AlwyWatch>) -> Fut + 'static,
        Fut: std::future::Future<Output = Result<Me, CoreError>> + 'static,
    {
        let ui = self.ui();
        ui.set_account_busy(true);
        ui.set_account_message(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = call(core).await;
            let Some(t) = this.upgrade() else { return };
            t.ui().set_account_busy(false);
            match r {
                Ok(me) => {
                    t.set_me(me);
                    if close {
                        t.ui().set_account_dialog(SharedString::new());
                    }
                }
                Err(e) => t.ui().set_account_message(sstr(profile_error(&e))),
            }
        })
        .ok();
    }

    /// A tile of the avatar grid: chosen at once in Settings, kept for Continue in the welcome
    /// step.
    pub(super) fn account_preset(self: &Rc<Self>, id: String) {
        if self.ui().get_account_dialog() == "welcome" {
            let draft = match id.as_str() {
                "default" => Some(Draft::Default),
                // The uploaded picture is already the draft.
                "upload" => None,
                _ => Some(Draft::Preset(id)),
            };
            if let Some(d) = draft {
                self.state.borrow_mut().me.draft = Some(d);
            }
            self.render_presets();
            return;
        }
        if id == "default" {
            self.account_remove_avatar();
        } else {
            self.account_call(false, move |core| async move {
                core.keep_set_avatar_preset(id).await
            });
        }
    }

    pub(super) fn account_remove_avatar(self: &Rc<Self>) {
        self.account_call(false, |core| async move { core.keep_remove_avatar().await });
    }

    /// Continue: what changed is saved, then the step is done for this account. The app never
    /// waits for it: the dialog closes at once and a failure only shows a line (the step comes
    /// back at the next start while the server hasn't recorded it).
    pub(super) fn welcome_continue(self: &Rc<Self>) {
        let ui = self.ui();
        let name = ui.get_account_name_input().to_string();
        if let Some(p) = display_name_problem(name.clone()) {
            ui.set_account_name_problem(sstr(p));
            return;
        }
        let (me, draft) = {
            let mut st = self.state.borrow_mut();
            (st.me.me.clone(), st.me.draft.take())
        };
        let Some(me) = me else {
            ui.set_account_dialog(SharedString::new());
            return;
        };
        ui.set_account_dialog(SharedString::new());
        let rename = (name.trim() != me.name).then_some(name);
        let had_avatar = me.avatar.preset.is_some() || me.avatar.custom_version.is_some();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let mut failed: Option<CoreError> = None;
            let mut latest: Option<Me> = None;
            let mut keep = |r: Result<Me, CoreError>| match r {
                Ok(m) => latest = Some(m),
                Err(e) => failed = Some(e),
            };
            if let Some(n) = rename {
                keep(core.keep_set_display_name(n).await);
            }
            match draft {
                Some(Draft::Preset(id)) => keep(core.keep_set_avatar_preset(id).await),
                Some(Draft::Upload(bytes, _)) => {
                    keep(core.keep_upload_avatar(bytes, "image/jpeg".into()).await)
                }
                Some(Draft::Default) if had_avatar => keep(core.keep_remove_avatar().await),
                _ => {}
            }
            keep(core.keep_mark_profile_set().await);
            let Some(t) = this.upgrade() else { return };
            if let Some(m) = latest {
                t.set_me(m);
            }
            if let Some(e) = failed {
                t.ui().set_error(sstr(tr!(
                    "Your name and avatar couldn't all be saved: {error}",
                    error = profile_error(&e)
                )));
            }
        })
        .ok();
    }

    /// Skip: only the "done" mark (never blocks either).
    pub(super) fn welcome_skip(self: &Rc<Self>) {
        self.state.borrow_mut().me.draft = None;
        self.ui().set_account_dialog(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            if let (Ok(me), Some(t)) = (core.keep_mark_profile_set().await, this.upgrade()) {
                t.set_me(me);
            }
        })
        .ok();
    }

    // ----- upload: file dialog and cropper -----------------------------------------------------

    /// "Upload a picture": the system's file dialog (its own thread), then the cropper.
    pub(super) fn account_upload(self: &Rc<Self>) {
        if std::mem::replace(&mut self.state.borrow_mut().me.picking, true) {
            return;
        }
        let welcome = self.ui().get_account_dialog() == "welcome";
        // Test hook (headless driver): this file instead of the dialog.
        let preset = std::env::var_os("ALWYWATCH_TEST_PICTURE").map(PathBuf::from);
        let spawned = std::thread::Builder::new()
            .name("file-dialog".into())
            .spawn(move || {
                let path = preset.or_else(|| {
                    rfd::FileDialog::new()
                        .set_title(tr!("Choose a picture"))
                        .add_filter(tr!("Pictures"), &["jpg", "jpeg", "png", "webp"])
                        .pick_file()
                });
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                        t.state.borrow_mut().me.picking = false;
                        if let Some(p) = path {
                            t.open_cropper(p, welcome);
                        }
                    }
                });
            });
        if spawned.is_err() {
            self.state.borrow_mut().me.picking = false;
        }
    }

    /// Reads the picture off the UI thread, then shows it under the frame.
    fn open_cropper(self: &Rc<Self>, path: PathBuf, welcome: bool) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        let file = path.clone();
        slint::spawn_local(async move {
            let read = core
                .runtime()
                .spawn_blocking(move || {
                    let img = avatars::open_oriented(&file)?;
                    let (w, h) = (img.width(), img.height());
                    let p = avatars::preview(&img);
                    let (pw, ph) = p.dimensions();
                    Ok::<_, String>((
                        w,
                        h,
                        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(p.as_raw(), pw, ph),
                    ))
                })
                .await
                .unwrap_or_else(|_| Err(tr!("Can't read this picture.")));
            let Some(t) = this.upgrade() else { return };
            let ui = t.ui();
            match read {
                Ok((w, h, buf)) => {
                    t.state.borrow_mut().me.crop = Some(CropJob {
                        path,
                        crop: Crop::new(w, h),
                        welcome,
                    });
                    ui.set_crop_image(Image::from_rgba8(buf));
                    let px = VIEWPORT as u32;
                    let mask = avatars::frame_mask(px, FRAME);
                    ui.set_crop_mask(Image::from_rgba8(
                        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&mask, px, px),
                    ));
                    ui.set_crop_message(SharedString::new());
                    ui.set_crop_busy(false);
                    t.render_crop();
                    ui.set_crop_open(true);
                }
                Err(e) => ui.set_account_message(sstr(e)),
            }
        })
        .ok();
    }

    fn render_crop(&self) {
        let Some(c) = self.state.borrow().me.crop.as_ref().map(|j| j.crop) else {
            return;
        };
        let (x, y, w, h) = c.view(VIEWPORT, FRAME);
        let ui = self.ui();
        ui.set_crop_x(x);
        ui.set_crop_y(y);
        ui.set_crop_w(w);
        ui.set_crop_h(h);
        ui.set_crop_zoom_max(c.max_zoom());
        ui.set_crop_zoom(c.zoom);
    }

    fn with_crop(&self, f: impl FnOnce(&mut Crop)) {
        if let Some(j) = self.state.borrow_mut().me.crop.as_mut() {
            f(&mut j.crop);
        }
        self.render_crop();
    }

    pub(super) fn crop_drag(&self, dx: f32, dy: f32) {
        self.with_crop(|c| c.drag(dx, dy, FRAME));
    }

    pub(super) fn crop_zoom_by(&self, steps: f32) {
        self.with_crop(|c| c.set_zoom(c.zoom * 1.15f32.powf(steps)));
    }

    pub(super) fn crop_set_zoom(&self, zoom: f32) {
        self.with_crop(|c| c.set_zoom(zoom));
    }

    pub(super) fn crop_cancel(&self) {
        self.state.borrow_mut().me.crop = None;
        let ui = self.ui();
        ui.set_crop_open(false);
        ui.set_crop_image(Image::default());
    }

    /// Save: the square under the frame as a JPEG (off the UI thread), then uploaded (Settings)
    /// or kept for Continue (welcome step).
    pub(super) fn crop_save(self: &Rc<Self>) {
        let ui = self.ui();
        if ui.get_crop_busy() {
            return;
        }
        let Some((path, crop, welcome)) = self
            .state
            .borrow()
            .me
            .crop
            .as_ref()
            .map(|j| (j.path.clone(), j.crop, j.welcome))
        else {
            return;
        };
        ui.set_crop_busy(true);
        ui.set_crop_message(SharedString::new());
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let made = core
                .runtime()
                .spawn_blocking(move || {
                    let img = avatars::open_oriented(&path)?;
                    let bytes = avatars::render_upload(&img, &crop)?;
                    let small =
                        avatars::decode(&bytes).ok_or_else(|| tr!("Can't prepare the picture."))?;
                    let (w, h) = small.dimensions();
                    let buf =
                        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(small.as_raw(), w, h);
                    Ok::<_, String>((bytes, buf))
                })
                .await
                .unwrap_or_else(|_| Err(tr!("Can't prepare the picture.")));

            let Some(t) = this.upgrade() else { return };
            let (bytes, buf) = match made {
                Ok(m) => m,
                Err(e) => {
                    t.ui().set_crop_busy(false);
                    t.ui().set_crop_message(sstr(e));
                    return;
                }
            };
            if welcome {
                t.state.borrow_mut().me.draft = Some(Draft::Upload(bytes, Image::from_rgba8(buf)));
                t.crop_cancel();
                t.render_presets();
                return;
            }
            let r = core.keep_upload_avatar(bytes, "image/jpeg".into()).await;
            let Some(t) = this.upgrade() else { return };
            t.ui().set_crop_busy(false);
            match r {
                Ok(me) => {
                    t.crop_cancel();
                    t.set_me(me);
                }
                Err(e) => t.ui().set_crop_message(sstr(profile_error(&e))),
            }
        })
        .ok();
    }
}

/// Test hooks (headless screenshots, `ALWYWATCH_TEST_PAGE`): a made-up account, nothing sent.
impl App {
    fn test_me(&self) {
        self.state.borrow_mut().me.me = Some(Me {
            user_key: "t:alex".into(),
            name: "Alex".into(),
            media_name: "alexandre".into(),
            name_is_custom: true,
            avatar: Avatar {
                preset: Some("p08".into()),
                custom_version: None,
            },
            profile_set: false,
            server_id: String::new(),
            is_admin: false,
            rights: Rights {
                can_keep_forever: true,
                max_forever: None,
                max_keep_days: None,
            },
            forever_count: 0,
            services: Default::default(),
        });
    }

    /// `welcome`, `avatars` (Change avatar), `presets` (every built-in avatar), `crop` (the
    /// cropper on `ALWYWATCH_TEST_PICTURE`).
    pub fn test_account(self: &Rc<Self>, step: &str) {
        self.test_me();
        self.navigate(Route::Settings);
        self.ui().set_keep_active(true);
        self.render_me();
        match step {
            "welcome" => self.open_welcome(),
            "avatars" => self.account_dialog_open("avatar".into()),
            "crop" => {
                if let Some(p) = std::env::var_os("ALWYWATCH_TEST_PICTURE") {
                    self.open_cropper(PathBuf::from(p), false);
                }
            }
            _ => {}
        }
    }
}
