// SPDX-License-Identifier: AGPL-3.0-or-later
//! Copy watch history from one server to another (Plex → Jellyfin when moving), same behaviour
//! and wording as Android's `HistoryCopyScreen`: check first (writes nothing) and show what will
//! change, write only on "Copy". The matching and writing are the core's
//! (`history_copy_plan`, `history_copy_apply`); this page only shows them.
//!
//! The job lives in the app, not the page: leaving the page keeps it going, and coming back
//! shows where it is.

use super::*;
use crate::AccountChoice;

#[derive(Default)]
pub(super) struct HistoryJob {
    from: Option<String>,
    to: Option<String>,
    running: bool,
    plan: Option<HistoryPlan>,
    result: Option<HistoryCopyResult>,
    error: Option<String>,
    show_missing: bool,
    progress: Option<HistoryProgress>,
}

/// "Plex Server · alex (Plex)".
fn account_label(a: &Account) -> String {
    let kind = match a.kind {
        ServerKind::Plex => "Plex",
        ServerKind::Jellyfin => "Jellyfin",
    };
    format!("{} · {} ({kind})", a.server_name, a.user_name)
}

/// Plex to Jellyfin by default: the usual move.
fn default_pair(accounts: &[Account]) -> (String, String) {
    let from = accounts
        .iter()
        .find(|a| a.kind == ServerKind::Plex)
        .or(accounts.first())
        .map(|a| a.id.clone())
        .unwrap_or_default();
    let to = accounts
        .iter()
        .find(|a| a.kind == ServerKind::Jellyfin && a.id != from)
        .or_else(|| accounts.iter().find(|a| a.id != from))
        .map(|a| a.id.clone())
        .unwrap_or_default();
    (from, to)
}

/// What the check found: the headline, the details, and the copy button's label ("" when there
/// is nothing to copy).
fn plan_text(p: &HistoryPlan) -> (String, String, String) {
    let watched = p.changes.iter().filter(|c| c.played).count();
    let started = p.changes.len() - watched;
    let summary = if p.changes.is_empty() {
        "Nothing to copy: everything is already there.".to_owned()
    } else {
        [
            (watched > 0).then(|| format!("{watched} to mark as watched")),
            (started > 0).then(|| format!("{started} resume points")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    };
    let detail = [
        (p.up_to_date > 0).then(|| format!("{} already up to date", p.up_to_date)),
        (!p.not_found.is_empty()).then(|| format!("{} not on that server", p.not_found.len())),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let copy = if p.changes.is_empty() {
        String::new()
    } else {
        format!("Copy {} titles", p.changes.len())
    };
    (summary, detail, copy)
}

fn result_text(r: &HistoryCopyResult) -> String {
    if r.failed > 0 {
        format!(
            "Done: {} titles copied, {} failed (try again).",
            r.applied, r.failed
        )
    } else {
        format!("Done: {} titles copied.", r.applied)
    }
}

/// The step it is on and, when the count is known, how far (and the bar's fraction, else -1).
fn step_text(p: Option<&HistoryProgress>) -> (String, f32) {
    let step = p
        .map(|p| p.step.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Starting");
    match p.filter(|p| p.total > 0) {
        Some(p) => (
            format!(
                "{step} · {} of {} ({} %)",
                p.done,
                p.total,
                u64::from(p.done) * 100 / u64::from(p.total)
            ),
            p.done as f32 / p.total as f32,
        ),
        None => (format!("{step}…"), -1.0),
    }
}

impl App {
    pub(super) fn render_history(&self) {
        let ui = self.ui();
        let accounts = self.core.accounts().unwrap_or_default();
        let (from, to) = {
            let mut st = self.state.borrow_mut();
            let job = &mut st.history;
            let known = |id: &Option<String>| {
                id.as_ref()
                    .filter(|id| accounts.iter().any(|a| &a.id == *id))
                    .cloned()
            };
            let (df, dt) = default_pair(&accounts);
            job.from = Some(known(&job.from).unwrap_or(df));
            job.to = Some(known(&job.to).unwrap_or(dt));
            (
                job.from.clone().unwrap_or_default(),
                job.to.clone().unwrap_or_default(),
            )
        };
        let choices = |chosen: &str| -> ModelRc<AccountChoice> {
            ModelRc::new(VecModel::from(
                accounts
                    .iter()
                    .map(|a| AccountChoice {
                        id: sstr(&a.id),
                        title: sstr(account_label(a)),
                        server: match a.kind {
                            ServerKind::Plex => "plex",
                            ServerKind::Jellyfin => "jellyfin",
                        }
                        .into(),
                        selected: a.id == chosen,
                    })
                    .collect::<Vec<_>>(),
            ))
        };
        ui.set_history_from(choices(&from));
        ui.set_history_to(choices(&to));

        let st = self.state.borrow();
        let job = &st.history;
        let state = if job.running {
            "running"
        } else if job.result.is_some() {
            "done"
        } else if job.plan.is_some() {
            "plan"
        } else {
            "idle"
        };
        ui.set_history_state(state.into());
        let (step, fraction) = step_text(job.progress.as_ref());
        ui.set_history_step(sstr(step));
        ui.set_history_progress(fraction);
        let (summary, detail, copy) = match (&job.plan, &job.result) {
            (_, Some(r)) => (result_text(r), String::new(), String::new()),
            (Some(p), None) => plan_text(p),
            _ => Default::default(),
        };
        ui.set_history_summary(sstr(summary));
        ui.set_history_detail(sstr(detail));
        ui.set_history_copy_label(sstr(copy));
        ui.set_history_missing(sstr(
            job.plan
                .as_ref()
                .map(|p| p.not_found.join("\n"))
                .unwrap_or_default(),
        ));
        ui.set_history_show_missing(job.show_missing);
        let error = match &job.error {
            Some(e) => e.clone(),
            None if accounts.len() >= 2 && from == to => "Pick two different servers.".into(),
            None => String::new(),
        };
        ui.set_history_error(sstr(error));
    }

    pub(super) fn history_pick(&self, side: String, id: String) {
        {
            let mut st = self.state.borrow_mut();
            let job = &mut st.history;
            if job.running {
                return;
            }
            match side.as_str() {
                "from" => job.from = Some(id),
                _ => job.to = Some(id),
            }
            job.plan = None;
            job.result = None;
            job.error = None;
            job.show_missing = false;
        }
        self.render_history();
    }

    /// Polls the core's progress twice a second while the job runs.
    fn history_poll(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.history_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(500),
            move || {
                let Some(a) = w.upgrade() else { return };
                let p = a.core.history_copy_progress();
                a.state.borrow_mut().history.progress = Some(p);
                if a.ui().get_page() == Page::Historycopy {
                    a.render_history();
                }
            },
        );
    }

    fn history_finished(&self) {
        self.history_timer.stop();
        self.state.borrow_mut().history.progress = None;
        if self.ui().get_page() == Page::Historycopy {
            self.render_history();
        }
    }

    pub(super) fn history_check(self: &Rc<Self>) {
        let (from, to) = {
            let mut st = self.state.borrow_mut();
            let job = &mut st.history;
            let (Some(from), Some(to)) = (job.from.clone(), job.to.clone()) else {
                return;
            };
            if job.running || from == to || from.is_empty() || to.is_empty() {
                return;
            }
            job.running = true;
            job.plan = None;
            job.result = None;
            job.error = None;
            job.show_missing = false;
            job.progress = None;
            (from, to)
        };
        self.render_history();
        self.history_poll();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.history_copy_plan(from, to).await;
            let Some(t) = this.upgrade() else { return };
            {
                let mut st = t.state.borrow_mut();
                let job = &mut st.history;
                job.running = false;
                match r {
                    Ok(p) => job.plan = Some(p),
                    Err(e) => job.error = Some(format!("Couldn't check the history: {e}")),
                }
            }
            t.history_finished();
        })
        .ok();
    }

    pub(super) fn history_copy(self: &Rc<Self>) {
        let (to, changes) = {
            let mut st = self.state.borrow_mut();
            let job = &mut st.history;
            let (Some(to), Some(plan)) = (job.to.clone(), job.plan.as_ref()) else {
                return;
            };
            if job.running || plan.changes.is_empty() {
                return;
            }
            let changes = plan.changes.clone();
            job.running = true;
            job.error = None;
            job.progress = None;
            (to, changes)
        };
        self.render_history();
        self.history_poll();
        let (core, this) = (self.core.clone(), Rc::downgrade(self));
        slint::spawn_local(async move {
            let r = core.history_copy_apply(to, changes).await;
            let Some(t) = this.upgrade() else { return };
            {
                let mut st = t.state.borrow_mut();
                let job = &mut st.history;
                job.running = false;
                match r {
                    Ok(res) => {
                        job.result = Some(res);
                        job.plan = None;
                        job.show_missing = false;
                    }
                    Err(e) => job.error = Some(format!("The copy stopped: {e}")),
                }
            }
            t.history_finished();
        })
        .ok();
    }

    pub(super) fn history_toggle_missing(&self) {
        {
            let mut st = self.state.borrow_mut();
            st.history.show_missing = !st.history.show_missing;
        }
        self.render_history();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, kind: ServerKind) -> Account {
        Account {
            id: id.into(),
            kind,
            base_url: String::new(),
            server_id: String::new(),
            server_name: "Home".into(),
            user_id: String::new(),
            user_name: "me".into(),
            is_admin: false,
        }
    }

    fn change(played: bool) -> HistoryChange {
        HistoryChange {
            item_id: "1".into(),
            label: "Arrival (2016)".into(),
            played,
            position_ms: 0,
            last_played_at: None,
        }
    }

    #[test]
    fn plex_to_jellyfin_by_default() {
        let a = [
            account("jf:1", ServerKind::Jellyfin),
            account("plex:1", ServerKind::Plex),
        ];
        assert_eq!(default_pair(&a), ("plex:1".into(), "jf:1".into()));
        let same = [
            account("jf:1", ServerKind::Jellyfin),
            account("jf:2", ServerKind::Jellyfin),
        ];
        assert_eq!(default_pair(&same), ("jf:1".into(), "jf:2".into()));
        assert_eq!(default_pair(&[]), (String::new(), String::new()));
        assert_eq!(account_label(&a[1]), "Home · me (Plex)");
    }

    #[test]
    fn plan_and_result_wording() {
        let p = HistoryPlan {
            changes: vec![change(true), change(true), change(false)],
            up_to_date: 4,
            not_found: vec!["Silo S1 · E3".into()],
        };
        assert_eq!(
            plan_text(&p),
            (
                "2 to mark as watched · 1 resume points".into(),
                "4 already up to date · 1 not on that server".into(),
                "Copy 3 titles".into()
            )
        );
        assert_eq!(
            plan_text(&HistoryPlan::default()),
            (
                "Nothing to copy: everything is already there.".into(),
                String::new(),
                String::new()
            )
        );
        assert_eq!(
            result_text(&HistoryCopyResult {
                applied: 3,
                failed: 0
            }),
            "Done: 3 titles copied."
        );
        assert_eq!(
            result_text(&HistoryCopyResult {
                applied: 2,
                failed: 1
            }),
            "Done: 2 titles copied, 1 failed (try again)."
        );
    }

    #[test]
    fn progress_wording() {
        assert_eq!(step_text(None), ("Starting…".into(), -1.0));
        let p = HistoryProgress {
            running: true,
            step: "Copying to Jellyfin".into(),
            done: 1,
            total: 4,
        };
        assert_eq!(
            step_text(Some(&p)),
            ("Copying to Jellyfin · 1 of 4 (25 %)".into(), 0.25)
        );
    }
}
