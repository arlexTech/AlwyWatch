// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keeps the screen on and the computer awake while the player is open.
//!
//! mpv does this itself only for a window of its own; drawn in the app's window it doesn't, and
//! the desktop dimmed the screen and warned about suspending in the middle of a film.
//!
//! Linux: the desktop portal's Inhibit (GNOME, KDE and most others; flags idle + suspend), else
//! org.freedesktop.ScreenSaver (older desktops). Windows: `SetThreadExecutionState` on the
//! player's thread. Best effort: when nothing answers, playback goes on as before.

/// Held while the player is open; dropping it lets the computer sleep again.
pub struct Awake {
    #[cfg(target_os = "linux")]
    linux: Option<linux::Held>,
}

impl Awake {
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Power::{
                ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
            };
            // SAFETY: plain flags; applies to this (the player's) thread until reset in Drop.
            unsafe {
                SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)
            };
        }
        Self {
            #[cfg(target_os = "linux")]
            linux: linux::hold(),
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Power::{ES_CONTINUOUS, SetThreadExecutionState};
            // SAFETY: as above, back to normal.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        }
        #[cfg(target_os = "linux")]
        if let Some(h) = self.linux.take() {
            h.release();
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use zbus::blocking::Connection;
    use zbus::zvariant::{OwnedObjectPath, Value};

    pub enum Held {
        /// The portal keeps it until its request object is closed.
        Portal(Connection, OwnedObjectPath),
        /// org.freedesktop.ScreenSaver: until UnInhibit with the cookie.
        ScreenSaver(Connection, u32),
    }

    const REASON: &str = "Playing a video";

    pub fn hold() -> Option<Held> {
        let c = match Connection::session() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[awake] no session bus: {e}");
                return None;
            }
        };
        // Idle (4) + suspend (8).
        let mut options: HashMap<&str, Value> = HashMap::new();
        options.insert("reason", Value::from(REASON));
        let portal = c
            .call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.Inhibit"),
                "Inhibit",
                &("", 4u32 | 8u32, options),
            )
            .and_then(|r| r.body().deserialize::<OwnedObjectPath>());
        match portal {
            Ok(path) => return Some(Held::Portal(c, path)),
            Err(e) => eprintln!("[awake] portal: {e}"),
        }
        let saver = c
            .call_method(
                Some("org.freedesktop.ScreenSaver"),
                "/org/freedesktop/ScreenSaver",
                Some("org.freedesktop.ScreenSaver"),
                "Inhibit",
                &("AlwyWatch", REASON),
            )
            .and_then(|r| r.body().deserialize::<u32>());
        match saver {
            Ok(cookie) => Some(Held::ScreenSaver(c, cookie)),
            Err(e) => {
                eprintln!("[awake] screensaver: {e}");
                None
            }
        }
    }

    impl Held {
        pub fn release(self) {
            let r = match &self {
                Held::Portal(c, path) => c
                    .call_method(
                        Some("org.freedesktop.portal.Desktop"),
                        path.as_str(),
                        Some("org.freedesktop.portal.Request"),
                        "Close",
                        &(),
                    )
                    .map(|_| ()),
                Held::ScreenSaver(c, cookie) => c
                    .call_method(
                        Some("org.freedesktop.ScreenSaver"),
                        "/org/freedesktop/ScreenSaver",
                        Some("org.freedesktop.ScreenSaver"),
                        "UnInhibit",
                        &(*cookie,),
                    )
                    .map(|_| ()),
            };
            if let Err(e) = r {
                eprintln!("[awake] release: {e}");
            }
        }
    }
}
