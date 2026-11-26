// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings → Servers → "Keep in sync", as on Android and iOS: one switch per pair of servers,
//! when it last ran and what it changed, "Sync now". A pair turned on syncs at once, then every
//! 6 hours while the app runs; what is watched in AlwyWatch is mirrored right away by the core.

use std::cell::Cell;
use std::rc::Rc;

use i18n::tr;
use slint::{ModelRc, VecModel};

use super::{App, sstr};
use crate::SyncRow;

thread_local! {
    /// A sync is running (only one at a time).
    static RUNNING: Cell<bool> = const { Cell::new(false) };
}

/// "21:05" for a Unix time, in the local time zone.
fn local_clock(t: i64) -> String {
    let local = t + i64::from(crate::localtime::utc_offset_minutes()) * 60;
    let day = local.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, day % 3600 / 60)
}

/// What a pair's row says under its name.
fn status_text(on: bool, running: bool, s: &alwywatch_core::SyncStatus, servers: String) -> String {
    if !on {
        return servers;
    }
    if running {
        return tr!("Syncing now…");
    }
    if let Some(e) = &s.error {
        return tr!("Last sync failed: {reason}", reason = e);
    }
    match s.last_run_at {
        Some(t) if s.changes == 0 => {
            tr!(
                "Last synced {time} · nothing to change",
                time = local_clock(t)
            )
        }
        Some(t) => tr!(
            "Last synced {time} · {count} updated",
            time = local_clock(t),
            count = s.changes
        ),
        None => tr!("Waiting for the first sync"),
    }
}

impl App {
    /// Every pair of logins on different servers, with its switch and last run.
    pub(super) fn refresh_sync_settings(&self) {
        let accounts = self.core.accounts().unwrap_or_default();
        let pairs = self.core.sync_pairs();
        let running = RUNNING.with(Cell::get);
        let mut rows = Vec::new();
        for (i, a) in accounts.iter().enumerate() {
            for b in accounts.iter().skip(i + 1) {
                if a.server_id == b.server_id {
                    continue;
                }
                let on = pairs
                    .iter()
                    .any(|p| (p.a == a.id && p.b == b.id) || (p.a == b.id && p.b == a.id));
                let status = self.core.sync_status(a.id.clone(), b.id.clone());
                let servers = tr!(
                    "{server} ({kind}) and {other} ({otherKind})",
                    server = a.server_name,
                    kind = format!("{:?}", a.kind),
                    other = b.server_name,
                    otherKind = format!("{:?}", b.kind)
                );
                rows.push(SyncRow {
                    a: sstr(&a.id),
                    b: sstr(&b.id),
                    label: sstr(format!("{} ⇄ {}", a.server_name, b.server_name)),
                    subtitle: sstr(status_text(on, running, &status, servers)),
                    recent: sstr(if on && !running {
                        status
                            .recent
                            .iter()
                            .take(5)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        String::new()
                    }),
                    on,
                });
            }
        }
        let ui = self.ui();
        ui.set_sync_any_on(!pairs.is_empty());
        ui.set_sync_running(running);
        ui.set_sync_rows(ModelRc::new(VecModel::from(rows)));
    }

    pub(super) fn sync_toggle(self: &Rc<Self>, a: String, b: String, on: bool) {
        if let Err(e) = self.core.set_sync_pair(a, b, on) {
            self.ui().set_status(sstr(e.to_string()));
        }
        self.refresh_sync_settings();
        // Turned on: the first sync right away.
        if on {
            self.sync_now();
        }
    }

    /// Syncs every pair turned on (reads both histories: a minute on big libraries).
    pub(super) fn sync_now(self: &Rc<Self>) {
        if RUNNING.with(Cell::get) || self.core.sync_pairs().is_empty() {
            return;
        }
        RUNNING.with(|r| r.set(true));
        self.refresh_sync_settings();
        let (core, w) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let _ = core.sync_now().await;
            RUNNING.with(|r| r.set(false));
            if let Some(a) = w.upgrade() {
                a.refresh_sync_settings();
            }
        })
        .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_wording() {
        let mut s = alwywatch_core::SyncStatus::default();
        assert_eq!(status_text(false, false, &s, "A and B".into()), "A and B");
        assert_eq!(
            status_text(true, false, &s, String::new()),
            "Waiting for the first sync"
        );
        assert_eq!(status_text(true, true, &s, String::new()), "Syncing now…");
        s.last_run_at = Some(0);
        assert!(status_text(true, false, &s, String::new()).ends_with("nothing to change"));
        s.changes = 3;
        assert!(status_text(true, false, &s, String::new()).ends_with("3 updated"));
        s.error = Some("offline".into());
        assert_eq!(
            status_text(true, false, &s, String::new()),
            "Last sync failed: offline"
        );
    }
}
