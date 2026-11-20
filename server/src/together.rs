// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together rooms (see `keep_service::together`), in memory only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use keep_service::together::{
    TogetherCreate, TogetherJoined, TogetherMedia, TogetherMember, TogetherState, TogetherUpdate,
    WAIT_SECS,
};
use tokio::sync::Notify;

use crate::transfer::random_u64;

/// A member who has not polled for this long has left.
const MEMBER_TTL: Duration = Duration::from_secs(60);
/// An empty room is kept this long (someone rejoining after a network change).
const EMPTY_TTL: Duration = Duration::from_secs(300);
const ROOM_TTL: Duration = Duration::from_secs(12 * 3600);
const MAX_ROOMS: usize = 200;
const MAX_MEMBERS: usize = 20;

#[derive(Debug, PartialEq, Eq)]
pub enum TogetherError {
    NotFound,
    Busy,
    Full,
    /// Not the caller's member id.
    Forbidden,
}

struct Member {
    name: String,
    user_key: String,
    seen: Instant,
}

struct Room {
    state: TogetherState,
    members: HashMap<String, Member>,
    created: Instant,
    empty_since: Option<Instant>,
    changed: Arc<Notify>,
}

#[derive(Default)]
pub struct Rooms {
    rooms: Mutex<HashMap<String, Room>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn clean_media(mut m: TogetherMedia) -> TogetherMedia {
    let cut = |s: String| s.chars().filter(|c| !c.is_control()).take(200).collect();
    m.title = cut(m.title);
    m.kind = cut(m.kind);
    m.series_title = m.series_title.map(cut);
    for id in [
        &mut m.tmdb,
        &mut m.imdb,
        &mut m.tvdb,
        &mut m.series_tmdb,
        &mut m.series_tvdb,
    ] {
        *id = id.take().map(|s| s.chars().take(40).collect());
    }
    m
}

impl Room {
    /// Drops members who stopped polling; true when someone left.
    fn expire_members(&mut self) -> bool {
        let before = self.members.len();
        self.members.retain(|_, m| m.seen.elapsed() < MEMBER_TTL);
        if self.members.is_empty() && self.empty_since.is_none() {
            self.empty_since = Some(Instant::now());
        }
        self.members.len() != before
    }

    fn view(&mut self) -> TogetherState {
        let mut members: Vec<TogetherMember> = self
            .members
            .iter()
            .map(|(id, m)| TogetherMember {
                id: id.clone(),
                name: m.name.clone(),
            })
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name));
        self.state.members = members;
        self.state.now_ms = now_ms();
        self.state.clone()
    }

    fn bump(&mut self, by: &str) {
        self.state.seq += 1;
        self.state.by = by.to_owned();
        self.changed.notify_waiters();
    }

    fn add_member(&mut self, name: &str, user_key: &str) -> Result<String, TogetherError> {
        if self.members.len() >= MAX_MEMBERS {
            return Err(TogetherError::Full);
        }
        let id = format!("{:016x}", random_u64());
        self.members.insert(
            id.clone(),
            Member {
                name: name.chars().filter(|c| !c.is_control()).take(60).collect(),
                user_key: user_key.to_owned(),
                seen: Instant::now(),
            },
        );
        self.empty_since = None;
        Ok(id)
    }

    fn check(&mut self, member: &str, user_key: &str) -> Result<(), TogetherError> {
        match self.members.get_mut(member) {
            Some(m) if m.user_key == user_key => {
                m.seen = Instant::now();
                Ok(())
            }
            Some(_) => Err(TogetherError::Forbidden),
            None => Err(TogetherError::NotFound),
        }
    }
}

impl Rooms {
    fn purge(map: &mut HashMap<String, Room>) {
        for r in map.values_mut() {
            r.expire_members();
        }
        map.retain(|_, r| {
            r.created.elapsed() < ROOM_TTL && r.empty_since.is_none_or(|t| t.elapsed() < EMPTY_TTL)
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Room>> {
        self.rooms.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn create(
        &self,
        c: TogetherCreate,
        name: &str,
        user_key: &str,
    ) -> Result<TogetherJoined, TogetherError> {
        let mut map = self.lock();
        Self::purge(&mut map);
        if map.len() >= MAX_ROOMS {
            return Err(TogetherError::Busy);
        }
        let code = loop {
            let c = format!("{:06}", random_u64() % 1_000_000);
            if !map.contains_key(&c) {
                break c;
            }
        };
        let mut room = Room {
            state: TogetherState {
                code: code.clone(),
                seq: 1,
                media: clean_media(c.media),
                playing: c.playing,
                position_ms: c.position_ms,
                at_ms: now_ms(),
                now_ms: 0,
                by: String::new(),
                members: vec![],
            },
            members: HashMap::new(),
            created: Instant::now(),
            empty_since: None,
            changed: Arc::new(Notify::new()),
        };
        let member_id = room.add_member(name, user_key)?;
        room.state.by = member_id.clone();
        let state = room.view();
        map.insert(code, room);
        Ok(TogetherJoined { member_id, state })
    }

    pub fn join(
        &self,
        code: &str,
        name: &str,
        user_key: &str,
    ) -> Result<TogetherJoined, TogetherError> {
        let mut map = self.lock();
        Self::purge(&mut map);
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        let member_id = room.add_member(name, user_key)?;
        // Others see who joined.
        room.state.seq += 1;
        room.changed.notify_waiters();
        Ok(TogetherJoined {
            member_id,
            state: room.view(),
        })
    }

    pub fn update(
        &self,
        code: &str,
        u: TogetherUpdate,
        user_key: &str,
    ) -> Result<TogetherState, TogetherError> {
        let mut map = self.lock();
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        room.check(&u.member_id, user_key)?;
        room.state.playing = u.playing;
        room.state.position_ms = u.position_ms;
        room.state.at_ms = now_ms();
        if let Some(m) = u.media {
            room.state.media = clean_media(m);
        }
        room.bump(&u.member_id);
        Ok(room.view())
    }

    /// The room once `seq` passed `after`, or after `WAIT_SECS` (also the member's heartbeat).
    pub async fn wait(
        &self,
        code: &str,
        member: &str,
        user_key: &str,
        after: u64,
    ) -> Result<TogetherState, TogetherError> {
        self.wait_for(
            code,
            member,
            user_key,
            after,
            Duration::from_secs(WAIT_SECS),
        )
        .await
    }

    async fn wait_for(
        &self,
        code: &str,
        member: &str,
        user_key: &str,
        after: u64,
        timeout: Duration,
    ) -> Result<TogetherState, TogetherError> {
        let changed = {
            let mut map = self.lock();
            let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
            room.check(member, user_key)?;
            if room.expire_members() {
                room.state.seq += 1;
                room.changed.notify_waiters();
            }
            if room.state.seq > after {
                return Ok(room.view());
            }
            room.changed.clone()
        };
        let notified = changed.notified();
        tokio::pin!(notified);
        // Registered before re-checking, so a change in between is not missed.
        notified.as_mut().enable();
        {
            let mut map = self.lock();
            let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
            if room.state.seq > after {
                return Ok(room.view());
            }
        }
        let _ = tokio::time::timeout(timeout, notified).await;
        let mut map = self.lock();
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        room.check(member, user_key)?;
        Ok(room.view())
    }

    pub fn leave(&self, code: &str, member: &str, user_key: &str) -> Result<(), TogetherError> {
        let mut map = self.lock();
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        room.check(member, user_key)?;
        room.members.remove(member);
        if room.members.is_empty() {
            room.empty_since = Some(Instant::now());
        }
        room.state.seq += 1;
        room.changed.notify_waiters();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(title: &str) -> TogetherMedia {
        TogetherMedia {
            kind: "movie".into(),
            title: title.into(),
            year: Some(2001),
            tmdb: Some("585".into()),
            imdb: None,
            tvdb: None,
            series_title: None,
            series_tmdb: None,
            series_tvdb: None,
            season: None,
            episode: None,
        }
    }

    fn create(r: &Rooms) -> TogetherJoined {
        r.create(
            TogetherCreate {
                media: media("Monsters"),
                position_ms: 5_000,
                playing: false,
            },
            "Ana",
            "plex:ana",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_change_wakes_the_other_member() {
        let rooms = Arc::new(Rooms::default());
        let a = create(&rooms);
        let code = a.state.code.clone();
        let b = rooms.join(&code, "Bo", "jf:bo").unwrap();
        assert_eq!(b.state.members.len(), 2);
        let seq = b.state.seq;
        let waiter = {
            let (rooms, code, id) = (rooms.clone(), code.clone(), b.member_id.clone());
            tokio::spawn(async move { rooms.wait(&code, &id, "jf:bo", seq).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        rooms
            .update(
                &code,
                TogetherUpdate {
                    member_id: a.member_id.clone(),
                    playing: true,
                    position_ms: 6_000,
                    media: None,
                },
                "plex:ana",
            )
            .unwrap();
        let s = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("woken")
            .unwrap()
            .unwrap();
        assert!(s.playing);
        assert_eq!(s.position_ms, 6_000);
        assert_eq!(s.by, a.member_id);
    }

    #[tokio::test]
    async fn wait_times_out_with_the_current_state() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let s = rooms
            .wait_for(
                &a.state.code,
                &a.member_id,
                "plex:ana",
                a.state.seq,
                Duration::from_millis(30),
            )
            .await
            .unwrap();
        assert_eq!(s.seq, a.state.seq);
    }

    #[test]
    fn only_the_member_moves_their_own_seat() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let upd = TogetherUpdate {
            member_id: a.member_id.clone(),
            playing: true,
            position_ms: 0,
            media: None,
        };
        assert_eq!(
            rooms.update(&a.state.code, upd.clone(), "jf:someone-else"),
            Err(TogetherError::Forbidden)
        );
        assert_eq!(
            rooms.update("000000x", upd, "plex:ana"),
            Err(TogetherError::NotFound)
        );
        rooms
            .leave(&a.state.code, &a.member_id, "plex:ana")
            .unwrap();
        assert!(rooms.join(&a.state.code, "Bo", "jf:bo").is_ok());
    }
}
