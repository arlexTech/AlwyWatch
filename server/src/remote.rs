// SPDX-License-Identifier: AGPL-3.0-or-later
//! Play on the TV (see `keep_service::remote`), in memory only.
//!
//! A TV is there while it listens (a long poll at least every `DEVICE_TTL_SECS`). Devices are
//! kept per person (canonical user key): a phone only sees and controls its own person's TVs,
//! and for anyone else a device id answers `None` (404), like an unknown one.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use keep_service::remote::{
    DEVICE_TTL_SECS, RemoteCommand, RemoteCommands, RemoteDevice, RemoteReport, RemoteState,
    WAIT_SECS, clean,
};
use tokio::sync::Notify;

/// Commands kept for a TV that has not picked them up (older ones are dropped).
const MAX_QUEUED: usize = 20;
/// A command not picked up within this is stale (the TV went away and came back).
const COMMAND_TTL: Duration = Duration::from_secs(30);
const MAX_DEVICES_PER_PERSON: usize = 10;
const MAX_DEVICES: usize = 1000;

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteError {
    /// Unknown, gone, or another person's device.
    NotFound,
    Busy,
}

struct Device {
    person: String,
    name: String,
    seen: Instant,
    seq: u64,
    queue: VecDeque<(Instant, RemoteCommand)>,
    state: RemoteState,
    /// Wakes the TV's listen.
    commands: Arc<Notify>,
    /// Wakes the phones' state polls.
    changed: Arc<Notify>,
}

#[derive(Default)]
pub struct Remotes {
    devices: Mutex<HashMap<String, Device>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Device ids are per person: the same id from two people stays two devices.
fn key(person: &str, device: &str) -> String {
    format!("{person}\n{device}")
}

fn next_version(v: u64) -> u64 {
    // Below 2^53 (exact in JavaScript numbers), never 0.
    (v % ((1 << 53) - 1)) + 1
}

impl Remotes {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Device>> {
        self.devices.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn purge(map: &mut HashMap<String, Device>) {
        map.retain(|_, d| {
            let alive = d.seen.elapsed() < Duration::from_secs(DEVICE_TTL_SECS);
            if !alive {
                // Phones watching it learn at once that it is gone.
                d.changed.notify_waiters();
            }
            alive
        });
    }

    /// TV: registers (or refreshes) the device, then waits for commands past `after`.
    pub async fn listen(
        &self,
        person: &str,
        device: &str,
        name: &str,
        after: u64,
    ) -> Result<RemoteCommands, RemoteError> {
        self.listen_for(person, device, name, after, Duration::from_secs(WAIT_SECS))
            .await
    }

    async fn listen_for(
        &self,
        person: &str,
        device: &str,
        name: &str,
        after: u64,
        timeout: Duration,
    ) -> Result<RemoteCommands, RemoteError> {
        let device = clean(device, 80);
        let k = key(person, &device);
        let notify = {
            let mut map = self.lock();
            Self::purge(&mut map);
            if !map.contains_key(&k) {
                let mine = map.values().filter(|d| d.person == person).count();
                if mine >= MAX_DEVICES_PER_PERSON || map.len() >= MAX_DEVICES {
                    return Err(RemoteError::Busy);
                }
                let changed = Arc::new(Notify::new());
                map.insert(
                    k.clone(),
                    Device {
                        person: person.to_owned(),
                        name: String::new(),
                        seen: Instant::now(),
                        // Never 0 once there: `after=0` means a TV just there.
                        seq: 1,
                        queue: VecDeque::new(),
                        state: RemoteState {
                            version: 1,
                            at_ms: now_ms(),
                            now_ms: 0,
                            report: RemoteReport::default(),
                        },
                        commands: Arc::new(Notify::new()),
                        changed,
                    },
                );
            }
            let d = map.get_mut(&k).ok_or(RemoteError::NotFound)?;
            d.seen = Instant::now();
            let name = clean(name, 80);
            d.name = if name.trim().is_empty() {
                "TV".into()
            } else {
                name
            };
            // A TV just there: nothing sent before counts.
            if after == 0 || after > d.seq {
                d.queue.clear();
                return Ok(RemoteCommands {
                    last_seq: d.seq,
                    commands: vec![],
                });
            }
            if let Some(c) = Self::take(d, after) {
                return Ok(c);
            }
            d.commands.clone()
        };
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let mut map = self.lock();
            let d = map.get_mut(&k).ok_or(RemoteError::NotFound)?;
            if let Some(c) = Self::take(d, after) {
                return Ok(c);
            }
        }
        let _ = tokio::time::timeout(timeout, notified).await;
        let mut map = self.lock();
        let d = map.get_mut(&k).ok_or(RemoteError::NotFound)?;
        // Listening that long is presence too.
        d.seen = Instant::now();
        Ok(Self::take(d, after).unwrap_or(RemoteCommands {
            last_seq: d.seq,
            commands: vec![],
        }))
    }

    /// The commands past `after` that are still fresh, if any.
    fn take(d: &mut Device, after: u64) -> Option<RemoteCommands> {
        d.queue
            .retain(|(at, c)| c.seq > after && at.elapsed() < COMMAND_TTL);
        if d.queue.is_empty() {
            return None;
        }
        Some(RemoteCommands {
            last_seq: d.seq,
            commands: d.queue.iter().map(|(_, c)| c.clone()).collect(),
        })
    }

    /// TV: what it plays now.
    pub fn report(
        &self,
        person: &str,
        device: &str,
        mut report: RemoteReport,
    ) -> Result<(), RemoteError> {
        let mut map = self.lock();
        let d = map
            .get_mut(&key(person, device))
            .ok_or(RemoteError::NotFound)?;
        report.title = clean(&report.title, 200);
        report.subtitle = report.subtitle.map(|s| clean(&s, 200));
        report.error = report.error.map(|s| clean(&s, 300));
        report.server_id = report.server_id.map(|s| clean(&s, 80));
        report.item_id = report.item_id.map(|s| clean(&s, 80));
        for t in report.audio.iter_mut().chain(report.subtitles.iter_mut()) {
            t.label = clean(&t.label, 120);
        }
        report.audio.truncate(64);
        report.subtitles.truncate(64);
        d.seen = Instant::now();
        d.state = RemoteState {
            version: next_version(d.state.version),
            at_ms: now_ms(),
            now_ms: 0,
            report,
        };
        d.changed.notify_waiters();
        Ok(())
    }

    /// TV: gone (closed, or another profile on it).
    pub fn leave(&self, person: &str, device: &str) {
        let mut map = self.lock();
        if let Some(d) = map.remove(&key(person, device)) {
            d.changed.notify_waiters();
        }
    }

    /// Phone: the person's TVs there now, by name.
    pub fn devices(&self, person: &str) -> Vec<RemoteDevice> {
        let mut map = self.lock();
        Self::purge(&mut map);
        let mut out: Vec<RemoteDevice> = map
            .iter()
            .filter(|(_, d)| d.person == person)
            .map(|(k, d)| RemoteDevice {
                id: k
                    .split_once('\n')
                    .map(|(_, id)| id)
                    .unwrap_or("")
                    .to_owned(),
                name: d.name.clone(),
                title: d.state.report.title.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        out
    }

    /// Phone: a command for one of the person's TVs.
    pub fn command(
        &self,
        person: &str,
        device: &str,
        mut command: RemoteCommand,
    ) -> Result<(), RemoteError> {
        let mut map = self.lock();
        Self::purge(&mut map);
        let d = map
            .get_mut(&key(person, device))
            .ok_or(RemoteError::NotFound)?;
        d.seq += 1;
        command.seq = d.seq;
        command.server_id = command.server_id.map(|s| clean(&s, 80));
        command.item_id = command.item_id.map(|s| clean(&s, 80));
        if let Some(m) = command.media.as_mut() {
            m.title = clean(&m.title, 200);
            m.series_title = m.series_title.take().map(|s| clean(&s, 200));
        }
        d.queue.push_back((Instant::now(), command));
        while d.queue.len() > MAX_QUEUED {
            d.queue.pop_front();
        }
        d.commands.notify_waiters();
        Ok(())
    }

    /// Phone: the TV's state once its version differs from `after`.
    pub async fn state(
        &self,
        person: &str,
        device: &str,
        after: u64,
    ) -> Result<RemoteState, RemoteError> {
        self.state_for(person, device, after, Duration::from_secs(WAIT_SECS))
            .await
    }

    async fn state_for(
        &self,
        person: &str,
        device: &str,
        after: u64,
        timeout: Duration,
    ) -> Result<RemoteState, RemoteError> {
        let k = key(person, device);
        let view = |d: &Device| RemoteState {
            now_ms: now_ms(),
            ..d.state.clone()
        };
        let changed = {
            let mut map = self.lock();
            Self::purge(&mut map);
            let d = map.get(&k).ok_or(RemoteError::NotFound)?;
            if d.state.version != after {
                return Ok(view(d));
            }
            d.changed.clone()
        };
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let map = self.lock();
            let d = map.get(&k).ok_or(RemoteError::NotFound)?;
            if d.state.version != after {
                return Ok(view(d));
            }
        }
        let _ = tokio::time::timeout(timeout, notified).await;
        let mut map = self.lock();
        Self::purge(&mut map);
        map.get(&k).map(view).ok_or(RemoteError::NotFound)
    }

    /// Linked identities: the users `from` are now the person `to` (their TVs follow).
    pub fn rekey(&self, from: &[String], to: &str) {
        let mut map = self.lock();
        let moved: Vec<String> = map
            .iter()
            .filter(|(_, d)| from.contains(&d.person))
            .map(|(k, _)| k.clone())
            .collect();
        for k in moved {
            if let Some(mut d) = map.remove(&k) {
                let id = k
                    .split_once('\n')
                    .map(|(_, id)| id)
                    .unwrap_or("")
                    .to_owned();
                d.person = to.to_owned();
                map.insert(key(to, &id), d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keep_service::remote::RemoteAction;

    fn cmd(action: RemoteAction) -> RemoteCommand {
        RemoteCommand {
            seq: 0,
            action,
            media: None,
            server_id: None,
            item_id: None,
            position_ms: None,
            index: None,
        }
    }

    #[tokio::test]
    async fn a_tv_gets_its_persons_commands_only() {
        let r = Remotes::default();
        let first = r.listen("alice", "tv1", "Living room", 0).await.unwrap();
        assert_eq!(first.last_seq, 1);
        assert!(first.commands.is_empty());
        assert_eq!(r.devices("alice")[0].name, "Living room");
        assert!(r.devices("bob").is_empty());
        assert_eq!(
            r.command("bob", "tv1", cmd(RemoteAction::Pause)),
            Err(RemoteError::NotFound)
        );
        r.command("alice", "tv1", cmd(RemoteAction::Pause)).unwrap();
        let got = r.listen("alice", "tv1", "Living room", 0).await.unwrap();
        // after=0 is a fresh TV: nothing sent before it listened counts.
        assert!(got.commands.is_empty());
        r.command("alice", "tv1", cmd(RemoteAction::Resume))
            .unwrap();
        let got = r
            .listen_for(
                "alice",
                "tv1",
                "Living room",
                got.last_seq,
                Duration::from_millis(50),
            )
            .await
            .unwrap();
        assert_eq!(got.commands.len(), 1);
        assert_eq!(got.commands[0].action, RemoteAction::Resume);
        let next = r
            .listen_for(
                "alice",
                "tv1",
                "Living room",
                got.last_seq,
                Duration::from_millis(20),
            )
            .await
            .unwrap();
        assert!(next.commands.is_empty());
    }

    #[tokio::test]
    async fn a_waiting_tv_wakes_on_a_command() {
        let r = Arc::new(Remotes::default());
        let s = r.listen("alice", "tv1", "TV", 0).await.unwrap().last_seq;
        let r2 = r.clone();
        let waiting = tokio::spawn(async move {
            r2.listen_for("alice", "tv1", "TV", s, Duration::from_secs(5))
                .await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        let mut play = cmd(RemoteAction::Play);
        play.item_id = Some("42".into());
        r.command("alice", "tv1", play).unwrap();
        let got = waiting.await.unwrap().unwrap();
        assert_eq!(got.commands[0].item_id.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn phones_follow_the_tvs_reports() {
        let r = Arc::new(Remotes::default());
        r.listen("alice", "tv1", "TV", 0).await.unwrap();
        let s = r.state("alice", "tv1", 0).await.unwrap();
        assert!(s.report.title.is_empty());
        let r2 = r.clone();
        let v = s.version;
        let waiting = tokio::spawn(async move {
            r2.state_for("alice", "tv1", v, Duration::from_secs(5))
                .await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        r.report(
            "alice",
            "tv1",
            RemoteReport {
                title: "Silo".into(),
                playing: true,
                position_ms: 5_000,
                duration_ms: 60_000,
                ..Default::default()
            },
        )
        .unwrap();
        let got = waiting.await.unwrap().unwrap();
        assert_eq!(got.report.title, "Silo");
        assert_ne!(got.version, v);
        assert_eq!(r.devices("alice")[0].title, "Silo");
        assert_eq!(r.state("bob", "tv1", 0).await, Err(RemoteError::NotFound));
        r.leave("alice", "tv1");
        assert!(r.devices("alice").is_empty());
        assert_eq!(r.state("alice", "tv1", 0).await, Err(RemoteError::NotFound));
    }

    #[tokio::test]
    async fn linking_moves_the_tvs() {
        let r = Remotes::default();
        r.listen("plex:a", "tv1", "TV", 0).await.unwrap();
        r.rekey(&["plex:a".into()], "jellyfin:a");
        assert!(r.devices("plex:a").is_empty());
        assert_eq!(r.devices("jellyfin:a").len(), 1);
    }
}
