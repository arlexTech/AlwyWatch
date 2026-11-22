// SPDX-License-Identifier: AGPL-3.0-or-later
//! Profiles on the desktop (same behaviour and wording as Android's `ProfileScreens`): Settings
//! → Profiles lists them (switch, add, rename, PIN, remove), and "Who's watching?" comes before
//! the app when the computer has several profiles (or the PIN, when the profile has one).
//!
//! One core runs per process and profile (its own database and tokens), so switching to another
//! profile restarts the app into it, like Android does.

use super::*;
use crate::ProfileRow;
use crate::profiles::{self, Profile};

/// Android's `String.hashCode`, so a profile gets the same avatar colours on both.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
}

pub(super) fn profile_row(p: &Profile, current: &str) -> ProfileRow {
    ProfileRow {
        id: sstr(&p.id),
        name: sstr(&p.name),
        initial: sstr(
            p.name
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect::<String>())
                .unwrap_or_else(|| "?".into()),
        ),
        tone: (java_hash(&p.name) & 0x7fff_ffff) % 4,
        has_pin: p.has_pin,
        current: p.id == current,
        // The profile in use and the original one stay.
        removable: p.id != current && p.id != profiles::DEFAULT,
    }
}

impl App {
    fn current_profile(&self) -> String {
        self.core.profile()
    }

    fn find_profile(&self, id: &str) -> Option<Profile> {
        self.profiles.list().into_iter().find(|p| p.id == id)
    }

    pub(super) fn render_profiles(&self) {
        let ui = self.ui();
        let current = self.current_profile();
        let list = self.profiles.list();
        ui.set_profile_name(sstr(
            list.iter()
                .find(|p| p.id == current)
                .map(|p| p.name.as_str())
                .unwrap_or("Me"),
        ));
        let rows: Vec<ProfileRow> = list.iter().map(|p| profile_row(p, &current)).collect();
        ui.set_profiles(ModelRc::new(VecModel::from(rows)));
        ui.set_profile_auto(self.profiles.auto_sign_in());
    }

    /// At start: "Who's watching?" (several profiles) or straight to the PIN (one profile, or
    /// automatic sign-in into a profile with a PIN).
    pub(super) fn show_gate(&self) {
        self.render_profiles();
        let ui = self.ui();
        let list = self.profiles.list();
        let current = self.current_profile();
        let only = if list.len() == 1 {
            list.first().cloned()
        } else if self.profiles.auto_sign_in() {
            list.iter().find(|p| p.id == current && p.has_pin).cloned()
        } else {
            None
        };
        ui.set_profile_gate_back(list.len() > 1);
        ui.set_profile_target(only.map(|p| profile_row(&p, &current)).unwrap_or_default());
        ui.set_profile_input(SharedString::new());
        ui.set_profile_message(SharedString::new());
        ui.set_profile_gate(true);
    }

    /// The menu's profile item: "Who's watching?" over the app (Escape or Close goes back).
    pub(super) fn profile_switcher(&self) {
        self.render_profiles();
        let ui = self.ui();
        ui.set_profile_gate_back(true);
        ui.set_profile_gate_menu(true);
        ui.set_profile_target(ProfileRow::default());
        ui.set_profile_input(SharedString::new());
        ui.set_profile_message(SharedString::new());
        ui.set_profile_gate(true);
    }

    /// A profile was picked: "Switch" in Settings, or a tile of "Who's watching?".
    pub(super) fn profile_open(self: &Rc<Self>, id: String) {
        let Some(p) = self.find_profile(&id) else {
            return;
        };
        let ui = self.ui();
        let current = self.current_profile();
        if ui.get_profile_gate() {
            if p.has_pin {
                ui.set_profile_input(SharedString::new());
                ui.set_profile_message(SharedString::new());
                ui.set_profile_target(profile_row(&p, &current));
            } else {
                self.profile_picked(&id);
            }
        } else if id != current {
            if p.has_pin {
                self.profile_dialog("switch".into(), id);
            } else {
                self.switch_profile(&id);
            }
        }
    }

    /// "Who's watching?" answered (and its PIN checked).
    fn profile_picked(&self, id: &str) {
        if id == self.current_profile() {
            let ui = self.ui();
            ui.set_profile_gate(false);
            ui.set_profile_gate_menu(false);
            ui.set_profile_target(ProfileRow::default());
            ui.set_profile_input(SharedString::new());
        } else {
            self.switch_profile(id);
        }
    }

    /// Restarts the app into another profile (its PIN, if any, was already checked here).
    fn switch_profile(&self, id: &str) {
        let previous = self.current_profile();
        self.profiles.set_current(id);
        self.profiles.set_unlocked_once(id);
        let started = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .spawn()
        });
        match started {
            Ok(_) => {
                let _ = slint::quit_event_loop();
            }
            Err(e) => {
                self.profiles.set_current(&previous);
                let _ = self.profiles.take_unlocked_once();
                self.ui()
                    .set_profile_message(sstr(format!("Couldn't restart AlwyWatch: {e}")));
            }
        }
    }

    pub(super) fn profile_dialog(&self, kind: String, id: String) {
        let ui = self.ui();
        let target = self
            .find_profile(&id)
            .map(|p| profile_row(&p, &self.current_profile()))
            .unwrap_or_default();
        ui.set_profile_input(if kind == "rename" {
            target.name.clone()
        } else {
            SharedString::new()
        });
        ui.set_profile_target(target);
        ui.set_profile_message(SharedString::new());
        ui.set_profile_dialog(sstr(kind));
    }

    fn close_profile_dialog(&self) {
        let ui = self.ui();
        ui.set_profile_dialog(SharedString::new());
        ui.set_profile_target(ProfileRow::default());
        ui.set_profile_input(SharedString::new());
        ui.set_profile_message(SharedString::new());
        self.render_profiles();
    }

    fn wrong_pin(&self) {
        let ui = self.ui();
        ui.set_profile_input(SharedString::new());
        ui.set_profile_message("Wrong PIN".into());
    }

    pub(super) fn profile_submit(self: &Rc<Self>) {
        let ui = self.ui();
        let input = ui.get_profile_input().to_string();
        let target = ui.get_profile_target().id.to_string();
        match ui.get_profile_dialog().as_str() {
            "add" => {
                // A new profile starts empty: its own sign-in.
                let id = self.profiles.add(&input);
                self.switch_profile(&id);
            }
            "rename" => {
                self.profiles.rename(&target, &input);
                self.close_profile_dialog();
            }
            "pin" => {
                if profiles::valid_pin(&input) {
                    self.profiles.set_pin(&target, Some(&input));
                    self.close_profile_dialog();
                } else {
                    ui.set_profile_message("The PIN is 4 digits.".into());
                }
            }
            "switch" => {
                if self.profiles.check_pin(&target, &input) {
                    self.switch_profile(&target);
                } else {
                    self.wrong_pin();
                }
            }
            "remove" => {
                self.remove_profile(&target);
                self.close_profile_dialog();
            }
            _ if ui.get_profile_gate() && !target.is_empty() => {
                if self.profiles.check_pin(&target, &input) {
                    self.profile_picked(&target);
                } else {
                    self.wrong_pin();
                }
            }
            _ => {}
        }
    }

    /// A PIN is checked as soon as its 4th digit is typed (as on Android).
    pub(super) fn profile_edited(self: &Rc<Self>, text: String) {
        let ui = self.ui();
        let asking = ui.get_profile_dialog() == "switch"
            || (ui.get_profile_dialog().is_empty() && ui.get_profile_gate());
        if asking && profiles::valid_pin(&text) {
            self.profile_submit();
        }
    }

    /// Their servers, history and settings on this computer go; their downloads too unless
    /// another profile wants them.
    fn remove_profile(&self, id: &str) {
        if id == profiles::DEFAULT || id == self.current_profile() {
            return;
        }
        let _ = self.core.release_profile_downloads(id.to_owned());
        let _ = std::fs::remove_dir_all(self.profiles.data_dir(id));
        self.profiles.remove(id);
    }

    pub(super) fn profile_pin_remove(&self) {
        let target = self.ui().get_profile_target().id.to_string();
        self.profiles.set_pin(&target, None);
        self.close_profile_dialog();
    }

    pub(super) fn profile_cancel(&self) {
        let ui = self.ui();
        if !ui.get_profile_dialog().is_empty() {
            self.close_profile_dialog();
        } else if ui.get_profile_gate()
            && ui.get_profile_gate_menu()
            && ui.get_profile_target().id.is_empty()
        {
            // Opened from the menu, nothing picked: back to the app.
            ui.set_profile_gate_menu(false);
            ui.set_profile_gate(false);
        } else if ui.get_profile_gate() && ui.get_profile_gate_back() {
            ui.set_profile_target(ProfileRow::default());
            ui.set_profile_input(SharedString::new());
            ui.set_profile_message(SharedString::new());
        }
    }

    pub(super) fn profile_auto_toggle(&self) {
        self.profiles
            .set_auto_sign_in(!self.profiles.auto_sign_in());
        self.render_profiles();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_avatar_colours_as_android() {
        // Kotlin: "Me".hashCode() == 2488, "Alex".hashCode() == 2043454.
        assert_eq!(java_hash("Me"), 2488);
        assert_eq!(java_hash("Alex"), 2_043_454);
        assert_eq!(java_hash(""), 0);
    }

    #[test]
    fn rows_mark_the_current_and_removable_profiles() {
        let p = |id: &str, name: &str| Profile {
            id: id.into(),
            name: name.into(),
            has_pin: false,
        };
        let me = profile_row(&p(profiles::DEFAULT, "me"), "kids1234");
        assert_eq!(me.initial, "M");
        assert!(!me.current && !me.removable);
        let kids = profile_row(&p("kids1234", "kids"), "kids1234");
        assert!(kids.current && !kids.removable);
        let other = profile_row(&p("other123", ""), "kids1234");
        assert!(other.removable);
        assert_eq!(other.initial, "?");
    }
}
