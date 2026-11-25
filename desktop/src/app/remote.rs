// SPDX-License-Identifier: AGPL-3.0-or-later
//! Play on the TV from the desktop (docs/keep-protocol.md "Play on the TV"), as on the phones:
//! my TVs with AlwyWatch open (asked every 20 s while an AlwyWatch server is linked), Play
//! asking "This computer" or a TV (the card menu's panel), and a mini remote over the page.

use super::*;
use crate::RemotePanel;

/// The mini remote's state.
#[derive(Default)]
pub(super) struct RemoteUi {
    /// My TVs there now.
    devices: Vec<RemoteDevice>,
    /// The TV the remote follows (None: the list of TVs, or closed).
    device: Option<String>,
    state: Option<RemoteState>,
    /// The TV is gone (AlwyWatch closed on it).
    gone: bool,
    /// Bumped to stop the remote's long poll and its clock.
    generation: u64,
    /// A seek was sent then (ms, mono): the slider stays where it was dropped meanwhile.
    seek_at: i64,
    /// The last Play or button that failed.
    error: String,
}

/// The card menu row of a "Play on" choice: from the start or not.
const PLAY_ROW: &str = "play_on";
const HERE: &str = "\u{0}here";

impl App {
    /// Polls my TVs while an AlwyWatch server is linked (runs with the rooms' poll).
    pub(super) fn start_remote_poll(self: &Rc<Self>, generation: u64) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            loop {
                let r = core.remote_devices().await;
                let Some(t) = this.upgrade().filter(|t| {
                    t.state.borrow().lists.generation == generation && t.core.keep_active()
                }) else {
                    return;
                };
                if let Ok(d) = r {
                    t.set_remote_devices(d);
                }
                drop(t);
                async_sleep(Duration::from_secs(20)).await;
            }
        })
        .ok();
    }

    pub(super) fn stop_remote(&self) {
        self.set_remote_devices(Vec::new());
    }

    fn set_remote_devices(&self, devices: Vec<RemoteDevice>) {
        self.ui().set_remote_tvs(!devices.is_empty());
        self.state.borrow_mut().remote.devices = devices;
        if self.ui().get_remote().shown && self.state.borrow().remote.device.is_none() {
            self.show_remote();
        }
    }

    /// Play pressed: with a TV there (and not in a room), asks where; else here.
    pub(super) fn play_or_ask(self: &Rc<Self>, id: String, from_start: bool) -> bool {
        let devices = self.state.borrow().remote.devices.clone();
        if devices.is_empty() || self.in_room() {
            return false;
        }
        let title = self
            .state
            .borrow()
            .items
            .get(&id)
            .map(|i| i.title.clone())
            .unwrap_or_default();
        let mut actions = vec![NamedId {
            id: HERE.into(),
            title: tr!("This computer").into(),
            selected: false,
        }];
        // `selected` marks a TV (its icon).
        actions.extend(devices.into_iter().map(|d| NamedId {
            id: d.id.into(),
            title: d.name.into(),
            selected: true,
        }));
        self.state.borrow_mut().card_menu = Some((id, format!("{PLAY_ROW}:{from_start}")));
        self.ui().set_card_menu(CardMenu {
            shown: true,
            title: tr!("Where to play {title}?", title = title).into(),
            actions: ModelRc::new(VecModel::from(actions)),
        });
        true
    }

    /// The card menu's answer to a "Play on": true when it was one.
    pub(super) fn play_on_pick(self: &Rc<Self>, id: &str, row: &str, action: &str) -> bool {
        let Some(from_start) = row.strip_prefix(PLAY_ROW).and_then(|r| r.strip_prefix(':')) else {
            return false;
        };
        let from_start = from_start == "true";
        if action.is_empty() {
            return true;
        }
        let Some(account) = self.account_of(id) else {
            return true;
        };
        if action == HERE {
            self.play_for(account, id.to_owned(), from_start);
            return true;
        }
        // What the button said: from the start, or the resume point.
        let position = if from_start {
            0
        } else {
            self.state
                .borrow()
                .items
                .get(id)
                .map_or(0, |i| i.user.position_ms as i64)
        };
        let (core, this, device, id) = (
            self.core.clone(),
            Rc::downgrade(self),
            action.to_owned(),
            id.to_owned(),
        );
        slint::spawn_local(async move {
            let r = core
                .remote_play(device.clone(), account, id, Some(position))
                .await;
            let Some(t) = this.upgrade() else { return };
            match r {
                Ok(()) => t.remote_open(device),
                Err(e) => {
                    t.remote_open(device);
                    t.remote_error(e.to_string());
                }
            }
        })
        .ok();
        true
    }

    /// Opens the mini remote on TV `device` ("": the list, or the only TV).
    pub(super) fn remote_open(self: &Rc<Self>, device: String) {
        let device = if device.is_empty() {
            let st = self.state.borrow();
            (st.remote.devices.len() == 1).then(|| st.remote.devices[0].id.clone())
        } else {
            Some(device)
        };
        let generation = {
            let mut st = self.state.borrow_mut();
            st.remote.generation += 1;
            st.remote.device = device.clone();
            st.remote.state = None;
            st.remote.gone = false;
            st.remote.error.clear();
            st.remote.generation
        };
        let mut panel = self.ui().get_remote();
        panel.shown = true;
        self.ui().set_remote(panel);
        self.show_remote();
        if let Some(device) = device {
            self.follow_remote(device, generation);
        }
    }

    fn remote_error(&self, e: String) {
        self.state.borrow_mut().remote.error = e;
        self.show_remote();
    }

    pub(super) fn remote_close(&self) {
        let mut st = self.state.borrow_mut();
        st.remote.generation += 1;
        st.remote.device = None;
        st.remote.state = None;
        drop(st);
        self.ui().set_remote(RemotePanel::default());
    }

    /// The TV's reports (a long poll) and, every half second, where it is now.
    fn follow_remote(self: &Rc<Self>, device: String, generation: u64) {
        let live = move |this: &std::rc::Weak<App>| {
            this.upgrade()
                .filter(|t| t.state.borrow().remote.generation == generation)
        };
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let (mut after, mut failures) = (0u64, 0u32);
            loop {
                let r = core.remote_state(device.clone(), after).await;
                let Some(t) = live(&this) else { return };
                let wait = match r {
                    Ok(s) => {
                        after = s.version;
                        failures = 0;
                        let mut st = t.state.borrow_mut();
                        st.remote.state = Some(s);
                        st.remote.gone = false;
                        0
                    }
                    Err(CoreError::NotFound) => {
                        t.state.borrow_mut().remote.gone = true;
                        5
                    }
                    Err(_) => {
                        failures += 1;
                        (2 * failures as u64).min(15)
                    }
                };
                t.show_remote();
                drop(t);
                if wait > 0 {
                    async_sleep(Duration::from_secs(wait)).await;
                }
            }
        })
        .ok();
        let this = Rc::downgrade(self);
        slint::spawn_local(async move {
            loop {
                async_sleep(Duration::from_millis(500)).await;
                let Some(t) = live(&this) else { return };
                t.show_remote();
            }
        })
        .ok();
    }

    /// Draws the panel from the state.
    fn show_remote(&self) {
        let st = self.state.borrow();
        let r = &st.remote;
        let ui = self.ui();
        let mut p = ui.get_remote();
        if !p.shown {
            return;
        }
        let name = |id: &str| {
            r.devices
                .iter()
                .find(|d| d.id == id)
                .map_or_else(|| tr!("TV"), |d| d.name.clone())
        };
        let named = |tracks: &[RemoteTrack], now: Option<i32>| {
            let v: Vec<NamedId> = tracks
                .iter()
                .map(|t| NamedId {
                    id: t.index.to_string().into(),
                    title: t.label.clone().into(),
                    selected: Some(t.index) == now,
                })
                .collect();
            ModelRc::new(VecModel::from(v))
        };
        p.devices = ModelRc::new(VecModel::from(
            r.devices
                .iter()
                .map(|d| NamedId {
                    id: d.id.clone().into(),
                    title: if d.title.is_empty() {
                        d.name.clone()
                    } else {
                        format!("{} · {}", d.name, d.title)
                    }
                    .into(),
                    selected: false,
                })
                .collect::<Vec<_>>(),
        ));
        let Some(device) = &r.device else {
            p.name = tr!("Play on the TV").into();
            p.has_device = false;
            p.status = if r.devices.is_empty() {
                tr!("No TV here. Open AlwyWatch on the TV, with this profile, and it shows here.")
            } else {
                String::new()
            }
            .into();
            ui.set_remote(p);
            return;
        };
        p.has_device = true;
        p.name = name(device).into();
        p.has_title = false;
        p.status = if !r.error.is_empty() {
            r.error.clone()
        } else if r.gone {
            tr!("This TV isn't there anymore: AlwyWatch was closed on it.")
        } else {
            match &r.state {
                None => tr!("Connecting…"),
                Some(s) if s.report.title.is_empty() => {
                    s.report.error.clone().unwrap_or_else(|| {
                        tr!("Nothing playing. Pick a title, then Play and this TV.")
                    })
                }
                Some(s) => s.report.error.clone().unwrap_or_default(),
            }
        }
        .into();
        if let Some(s) = r
            .state
            .as_ref()
            .filter(|s| !s.report.title.is_empty() && !r.gone)
        {
            let rep = &s.report;
            let now = remote_position_now(s).max(0);
            p.has_title = true;
            p.title = rep.title.clone().into();
            p.subtitle = rep
                .subtitle
                .clone()
                .unwrap_or_else(|| {
                    if rep.loading {
                        tr!("Loading…")
                    } else {
                        String::new()
                    }
                })
                .into();
            p.playing = rep.playing;
            p.position_ms = now as f32;
            p.duration_ms = rep.duration_ms.max(1) as f32;
            p.elapsed = clock(now as u64).into();
            p.duration = clock(rep.duration_ms.max(0) as u64).into();
            p.audio = named(&rep.audio, rep.audio_index);
            p.subtitles = named(&rep.subtitles, rep.subtitle_index);
            p.subtitles_off = rep.subtitle_index.is_none();
            // The slider stays where it was dropped until the TV has moved there.
            if together::mono_ms() - r.seek_at > 2500 {
                ui.set_remote_position(now as f32);
            }
        }
        ui.set_remote(p);
    }

    /// A button of the remote: pause, resume, back10, fwd30, stop.
    pub(super) fn remote_action(self: &Rc<Self>, action: String) {
        let (device, now, duration) = {
            let st = self.state.borrow();
            let Some(d) = st.remote.device.clone() else {
                return;
            };
            let (now, duration) = st
                .remote
                .state
                .as_ref()
                .map_or((0, 0), |s| (remote_position_now(s), s.report.duration_ms));
            (d, now, duration)
        };
        let (action, position) = match action.as_str() {
            "pause" => (RemoteAction::Pause, None),
            "resume" => (RemoteAction::Resume, None),
            "stop" => (RemoteAction::Stop, None),
            "back10" => (RemoteAction::Seek, Some((now - 10_000).max(0))),
            "fwd30" => (RemoteAction::Seek, Some((now + 30_000).min(duration))),
            _ => return,
        };
        if position.is_some() {
            self.state.borrow_mut().remote.seek_at = together::mono_ms();
            if let Some(p) = position {
                self.ui().set_remote_position(p as f32);
            }
        }
        self.remote_send(device, action, position, None);
    }

    pub(super) fn remote_seek(self: &Rc<Self>, position_ms: f32) {
        let Some(device) = self.state.borrow().remote.device.clone() else {
            return;
        };
        self.state.borrow_mut().remote.seek_at = together::mono_ms();
        self.remote_send(device, RemoteAction::Seek, Some(position_ms as i64), None);
    }

    /// A track: kind "audio" or "subtitles", index ("": subtitles off).
    pub(super) fn remote_track(self: &Rc<Self>, kind: String, index: String) {
        let Some(device) = self.state.borrow().remote.device.clone() else {
            return;
        };
        let action = if kind == "audio" {
            RemoteAction::Audio
        } else {
            RemoteAction::Subtitles
        };
        self.remote_send(device, action, None, index.parse().ok());
    }

    fn remote_send(
        self: &Rc<Self>,
        device: String,
        action: RemoteAction,
        position: Option<i64>,
        index: Option<i32>,
    ) {
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.remote_control(device, action, position, index).await;
            if let Some(t) = this.upgrade() {
                t.remote_error(r.err().map(|e| e.to_string()).unwrap_or_default());
            }
        })
        .ok();
    }
}
