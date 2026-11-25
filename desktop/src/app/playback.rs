// SPDX-License-Identifier: AGPL-3.0-or-later
//! The in-window player: mpv draws the video under the window (video.rs), these are its
//! controls (PlayerOverlay in app.slint) and their commands.

use std::rc::Rc;
use std::sync::Arc;

use i18n::tr;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{APP, App};
use crate::PlayerTrack;
use crate::mpv::Mpv;
use crate::player::{PlayerView, TrackItem, ViewLink};

/// Setting: the app full screen ("1").
const APP_FULLSCREEN: &str = "desktop.fullscreen";

impl App {
    /// The link a new player uses to show itself in the window with `title` on top.
    pub(super) fn player_view_link(title: String) -> ViewLink {
        ViewLink {
            attached: Box::new(move |m: Arc<Mpv>| {
                let title = title.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                        t.state.borrow_mut().player_mpv = Some(m);
                        let ui = t.ui();
                        ui.set_player_title(title.into());
                        ui.set_player_pos(0);
                        ui.set_player_dur(0);
                        ui.set_player_skip(SharedString::default());
                        ui.set_player_countdown(0);
                        ui.set_player_note(SharedString::default());
                        // Already full screen (the app is): the player's button says so.
                        ui.set_player_fullscreen(ui.window().is_fullscreen());
                        ui.set_player_on(true);
                    }
                });
            }),
            show: Box::new(|v: PlayerView| {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                        t.show_player_view(v);
                    }
                });
            }),
        }
    }

    fn show_player_view(&self, v: PlayerView) {
        let ui = self.ui();
        let ms = |x: u64| i32::try_from(x).unwrap_or(i32::MAX);
        ui.set_player_pos(ms(v.position_ms));
        ui.set_player_dur(ms(v.duration_ms));
        ui.set_player_cache(ms(v.cache_ms));
        ui.set_player_paused(v.paused);
        ui.set_player_skip(v.skip.into());
        ui.set_player_countdown(v.countdown as i32);
        ui.set_player_note(v.note.into());
        ui.set_player_has_next(v.has_next);
        ui.set_player_has_previous(v.has_previous);
        ui.set_player_near_end(v.near_end);
        ui.set_player_ends_at(if v.duration_ms > v.position_ms {
            crate::localtime::clock_in((v.duration_ms - v.position_ms) / 1000).into()
        } else {
            SharedString::default()
        });
        ui.set_player_room_label(self.player_room_label().into());
        let label = |list: &[TrackItem]| {
            list.iter()
                .find(|t| t.selected)
                .map(|t| t.label.split(" · ").next().unwrap_or_default().to_owned())
                .unwrap_or_else(|| tr!("Off"))
        };
        ui.set_player_audio_label(label(&v.audio).into());
        ui.set_player_subtitle_label(label(&v.subtitles).into());
        let model = |list: Vec<TrackItem>| {
            ModelRc::new(VecModel::from(
                list.into_iter()
                    .map(|t| PlayerTrack {
                        id: t.id.into(),
                        label: t.label.into(),
                        selected: t.selected,
                    })
                    .collect::<Vec<_>>(),
            ))
        };
        ui.set_player_audio(model(v.audio));
        ui.set_player_subtitles(model(v.subtitles));
    }

    /// The whole app full screen or not (the sidebar's button, F11), kept for the next launch.
    pub(super) fn app_fullscreen_toggle(&self) {
        let ui = self.ui();
        let on = !ui.window().is_fullscreen();
        ui.set_app_fullscreen(on);
        ui.window().set_fullscreen(on);
        let _ = self.core.set_setting(
            APP_FULLSCREEN.to_owned(),
            if on { "1" } else { "0" }.to_owned(),
        );
    }

    /// Progress reports that failed (offline) wait in the cache: retried every minute, as the
    /// phone does when its network comes back. Downloads' housekeeping every 15 minutes.
    pub(crate) fn start_progress_flush(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.progress_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(60),
            move || {
                let Some(a) = w.upgrade() else { return };
                let core = a.core.clone();
                // Every 15 minutes, the downloads' housekeeping too: rules that are due, and
                // shows set to keep their next episodes downloaded.
                let tick = a.progress_ticks.get();
                a.progress_ticks.set(tick.wrapping_add(1));
                let w = Rc::downgrade(&a);
                slint::spawn_local(async move {
                    let _ = core.flush_pending_progress().await;
                    if tick % 15 == 0
                        && core.sync_downloads().await.unwrap_or(0) > 0
                        && let Some(a) = w.upgrade()
                    {
                        a.kick_downloads();
                    }
                })
                .ok();
            },
        );
    }

    /// At startup: full screen when it was left so.
    pub(crate) fn app_fullscreen_restore(&self) {
        if self
            .core
            .get_setting(APP_FULLSCREEN.to_owned())
            .ok()
            .flatten()
            .as_deref()
            == Some("1")
        {
            let ui = self.ui();
            ui.set_app_fullscreen(true);
            ui.window().set_fullscreen(true);
        }
    }

    /// A control was used (PlayerOverlay's `cmd`).
    pub(super) fn player_cmd(&self, cmd: &str, arg: &str) {
        if cmd == "fullscreen" {
            // One switch for the app and the player: kept when the player closes.
            self.app_fullscreen_toggle();
            let ui = self.ui();
            ui.set_player_fullscreen(ui.get_app_fullscreen());
            return;
        }
        let Some(m) = self.state.borrow().player_mpv.clone() else {
            return;
        };
        let _ = match cmd {
            "pause" => m.command(&["cycle", "pause"]),
            "seek" => m.command(&["seek", arg, "absolute"]),
            // While dragging: the nearest keyframe answers fast; the release seeks exactly.
            "seek-drag" => m.command(&["seek", arg, "absolute+keyframes"]),
            "rel" => m.command(&["seek", arg]),
            "aid" => m.command(&["set", "aid", arg]),
            "sid" => m.command(&["set", "sid", arg]),
            "volume" => m.command(&["add", "volume", arg]),
            "mute" => m.command(&["cycle", "mute"]),
            "skip" | "next" | "previous" | "cancel" => m.command(&["script-message", "alwy", cmd]),
            "close" => m.command(&["quit"]),
            _ => Ok(()),
        };
    }

    /// Playback ended (on the UI thread): back to the app.
    pub(super) fn player_closed(self: &Rc<Self>) {
        // "Watch together" pressed, never started: that room closes with the player.
        if self.ui().get_player_room() {
            self.player_room_cancel();
        }
        {
            let mut st = self.state.borrow_mut();
            st.player_mpv = None;
            st.player_item = None;
            st.player_tx = None;
        }
        self.ui().set_player_can_together(false);
        crate::video::detach();
        let ui = self.ui();
        if ui.get_player_on() {
            ui.set_player_on(false);
            // Back to the app as it was: full screen when it is set so (sidebar, F11).
            let app = ui.get_app_fullscreen();
            if ui.window().is_fullscreen() != app {
                ui.window().set_fullscreen(app);
            }
            ui.set_player_fullscreen(false);
        }
    }
}

impl App {
    /// Test hook (`ALWYWATCH_TEST_PLAY=file`): plays a local file in the window with a fake
    /// 30 s intro and a next episode, to check the player without a server.
    pub fn test_play(self: &Rc<Self>, file: String) {
        use crate::player::{self, PlayRequest, TrackSetup};
        let req = PlayRequest {
            media: alwywatch_core::HttpRequest {
                url: file,
                headers: vec![],
            },
            start_ms: 0,
            view: Some(Self::player_view_link("Test pattern S01E01".into())),
            title: "Test pattern S01E01".into(),
            external_subs: vec![],
            tracks: TrackSetup {
                pref: None,
                rules: Default::default(),
                save: Box::new(|_| {}),
            },
            spdif: false,
            segments: vec![alwywatch_core::Segment {
                kind: alwywatch_core::SegmentKind::Intro,
                start_ms: 0,
                end_ms: 30_000,
            }],
            has_next: true,
            has_previous: false,
            auto_skip_intro: false,
            auto_next: true,
            together: None,
            notes: None,
        };
        let _ = player::spawn(req, |p| {
            if p.event == alwywatch_core::PlaybackEvent::Stop {
                let _ = slint::invoke_from_event_loop(|| {
                    if let Some(t) = APP.with(|a| a.borrow().upgrade()) {
                        t.player_closed();
                    }
                });
            }
        });
    }
}
