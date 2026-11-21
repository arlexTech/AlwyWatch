// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together rooms (see `keep_service::together`), in memory only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use keep_service::together::{
    KnockStatus, TogetherCreate, TogetherJoined, TogetherKnockState, TogetherMedia, TogetherMember,
    TogetherRoom, TogetherState, TogetherUpdate, WAIT_SECS,
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
/// Join requests waiting in one room at most.
const MAX_REQUESTS: usize = 10;

#[derive(Debug, PartialEq, Eq)]
pub enum TogetherError {
    NotFound,
    Busy,
    Full,
    /// Not the caller's member id (or not the host, for answering requests).
    Forbidden,
}

struct Member {
    name: String,
    user_key: String,
    seen: Instant,
    since: Instant,
}

/// Someone asking to join, until they collect the answer (or stop polling).
struct Request {
    name: String,
    user_key: String,
    seen: Instant,
    status: KnockStatus,
    joined: Option<TogetherJoined>,
}

struct Room {
    state: TogetherState,
    members: HashMap<String, Member>,
    requests: HashMap<String, Request>,
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
        let before = (self.members.len(), self.requests.len());
        self.members.retain(|_, m| m.seen.elapsed() < MEMBER_TTL);
        self.requests.retain(|_, r| r.seen.elapsed() < MEMBER_TTL);
        if self.members.is_empty() && self.empty_since.is_none() {
            self.empty_since = Some(Instant::now());
        }
        self.fix_host();
        (self.members.len(), self.requests.len()) != before
    }

    /// The host left: the longest member takes over.
    fn fix_host(&mut self) {
        if !self.members.contains_key(&self.state.host) {
            self.state.host = self
                .members
                .iter()
                .min_by_key(|(_, m)| m.since)
                .map(|(id, _)| id.clone())
                .unwrap_or_default();
        }
    }

    fn host_name(&self) -> String {
        self.members
            .get(&self.state.host)
            .map(|m| m.name.clone())
            .unwrap_or_default()
    }

    fn knock_view(&self, id: &str) -> Option<TogetherKnockState> {
        self.requests.get(id).map(|r| TogetherKnockState {
            request_id: id.to_owned(),
            status: r.status,
            joined: r.joined.clone(),
        })
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
        let mut requests: Vec<TogetherMember> = self
            .requests
            .iter()
            .filter(|(_, r)| r.status == KnockStatus::Waiting)
            .map(|(id, r)| TogetherMember {
                id: id.clone(),
                name: r.name.clone(),
            })
            .collect();
        requests.sort_by(|a, b| a.name.cmp(&b.name));
        self.state.requests = requests;
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
                since: Instant::now(),
            },
        );
        self.empty_since = None;
        self.fix_host();
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
                host: String::new(),
                requests: vec![],
            },
            members: HashMap::new(),
            requests: HashMap::new(),
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

    /// The open rooms (with someone in them), newest first.
    pub fn list(&self, user_key: &str) -> Vec<TogetherRoom> {
        let mut map = self.lock();
        Self::purge(&mut map);
        let mut rooms: Vec<(Instant, TogetherRoom)> = map
            .values_mut()
            .filter(|r| !r.members.is_empty())
            .map(|r| {
                let mut members: Vec<String> = r.members.values().map(|m| m.name.clone()).collect();
                members.sort();
                let mine = r.members.values().any(|m| m.user_key == user_key);
                let host = r.host_name();
                let s = r.view();
                (
                    r.created,
                    TogetherRoom {
                        code: s.code,
                        host,
                        media: s.media,
                        members,
                        playing: s.playing,
                        position_ms: s.position_ms,
                        at_ms: s.at_ms,
                        now_ms: s.now_ms,
                        mine,
                    },
                )
            })
            .collect();
        rooms.sort_by_key(|r| std::cmp::Reverse(r.0));
        rooms.into_iter().map(|(_, r)| r).collect()
    }

    /// Asks to join. Someone already in the room (another of their devices) gets a seat at once.
    pub fn knock(
        &self,
        code: &str,
        name: &str,
        user_key: &str,
    ) -> Result<TogetherKnockState, TogetherError> {
        let mut map = self.lock();
        Self::purge(&mut map);
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        if room.members.is_empty() {
            return Err(TogetherError::NotFound);
        }
        let request_id = format!("{:016x}", random_u64());
        if room.members.values().any(|m| m.user_key == user_key) {
            let member_id = room.add_member(name, user_key)?;
            room.bump("");
            return Ok(TogetherKnockState {
                request_id,
                status: KnockStatus::Accepted,
                joined: Some(TogetherJoined {
                    member_id,
                    state: room.view(),
                }),
            });
        }
        // The same person asking again (a second press) keeps their place.
        if let Some((id, r)) = room
            .requests
            .iter_mut()
            .find(|(_, r)| r.user_key == user_key && r.status == KnockStatus::Waiting)
        {
            r.seen = Instant::now();
            let id = id.clone();
            return Ok(room.knock_view(&id).expect("just found"));
        }
        if room.requests.len() >= MAX_REQUESTS || room.members.len() >= MAX_MEMBERS {
            return Err(TogetherError::Full);
        }
        room.requests.insert(
            request_id.clone(),
            Request {
                name: name.chars().filter(|c| !c.is_control()).take(60).collect(),
                user_key: user_key.to_owned(),
                seen: Instant::now(),
                status: KnockStatus::Waiting,
                joined: None,
            },
        );
        // Wakes the host.
        room.bump("");
        Ok(room.knock_view(&request_id).expect("just added"))
    }

    /// The request once answered, or after `WAIT_SECS`. An answer is handed over once.
    pub async fn knock_wait(
        &self,
        code: &str,
        request: &str,
        user_key: &str,
    ) -> Result<TogetherKnockState, TogetherError> {
        self.knock_wait_for(code, request, user_key, Duration::from_secs(WAIT_SECS))
            .await
    }

    async fn knock_wait_for(
        &self,
        code: &str,
        request: &str,
        user_key: &str,
        timeout: Duration,
    ) -> Result<TogetherKnockState, TogetherError> {
        if let Some(v) = self.take_answer(code, request, user_key)? {
            return Ok(v);
        }
        let changed = self
            .lock()
            .get(code)
            .ok_or(TogetherError::NotFound)?
            .changed
            .clone();
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(v) = self.take_answer(code, request, user_key)? {
                return Ok(v);
            }
            if tokio::time::timeout_at(deadline, notified.as_mut())
                .await
                .is_err()
            {
                let mut map = self.lock();
                let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
                return room.knock_view(request).ok_or(TogetherError::NotFound);
            }
            // Woken by some change in the room: register again before re-checking.
            notified.set(changed.notified());
            notified.as_mut().enable();
        }
    }

    /// The answer, once there is one (handed over once); notes the asker is still there.
    fn take_answer(
        &self,
        code: &str,
        request: &str,
        user_key: &str,
    ) -> Result<Option<TogetherKnockState>, TogetherError> {
        let mut map = self.lock();
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        let r = room
            .requests
            .get_mut(request)
            .ok_or(TogetherError::NotFound)?;
        if r.user_key != user_key {
            return Err(TogetherError::Forbidden);
        }
        r.seen = Instant::now();
        if r.status == KnockStatus::Waiting {
            return Ok(None);
        }
        let view = room.knock_view(request);
        room.requests.remove(request);
        Ok(view)
    }

    /// The asker gives up.
    pub fn knock_cancel(
        &self,
        code: &str,
        request: &str,
        user_key: &str,
    ) -> Result<(), TogetherError> {
        let mut map = self.lock();
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        match room.requests.get(request) {
            Some(r) if r.user_key == user_key => {}
            Some(_) => return Err(TogetherError::Forbidden),
            None => return Err(TogetherError::NotFound),
        }
        room.requests.remove(request);
        room.bump("");
        Ok(())
    }

    /// The host lets someone in, or not.
    pub fn answer(
        &self,
        code: &str,
        request: &str,
        host: &str,
        accept: bool,
        user_key: &str,
    ) -> Result<TogetherState, TogetherError> {
        let mut map = self.lock();
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        room.check(host, user_key)?;
        if room.state.host != host {
            return Err(TogetherError::Forbidden);
        }
        let (name, asker) = match room.requests.get(request) {
            Some(r) if r.status == KnockStatus::Waiting => (r.name.clone(), r.user_key.clone()),
            _ => return Err(TogetherError::NotFound),
        };
        let member_id = if accept {
            Some(room.add_member(&name, &asker)?)
        } else {
            None
        };
        if let Some(r) = room.requests.get_mut(request) {
            r.status = if accept {
                KnockStatus::Accepted
            } else {
                KnockStatus::Declined
            };
        }
        // One change for everyone; the new member's copy of the room is that same state.
        room.bump("");
        let state = room.view();
        if let (Some(r), Some(member_id)) = (room.requests.get_mut(request), member_id) {
            r.joined = Some(TogetherJoined {
                member_id,
                state: state.clone(),
            });
        }
        Ok(state)
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
                room.bump("");
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
        room.fix_host();
        room.bump("");
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
        let b = let_in(&rooms, &a, "Bo", "jf:bo");
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

    /// Someone leaving is the server's change, not the last member who played or paused: their
    /// players would take it for their own echo and miss it.
    #[tokio::test]
    async fn leaving_is_nobodys_change() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = a.state.code.clone();
        let b = let_in(&rooms, &a, "Bo", "jf:bo");
        let upd = TogetherUpdate {
            member_id: a.member_id.clone(),
            playing: true,
            position_ms: 6_000,
            media: None,
        };
        let after = rooms.update(&code, upd, "plex:ana").unwrap();
        assert_eq!(after.by, a.member_id);
        rooms.leave(&code, &b.member_id, "jf:bo").unwrap();
        let s = rooms
            .wait_for(
                &code,
                &a.member_id,
                "plex:ana",
                after.seq,
                Duration::from_millis(30),
            )
            .await
            .unwrap();
        assert_eq!(s.members.len(), 1);
        assert_eq!(s.by, "");
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
        // Empty: no host to ask any more.
        assert_eq!(
            rooms.knock(&a.state.code, "Bo", "jf:bo").map(|_| ()),
            Err(TogetherError::NotFound)
        );
    }

    /// Bo asks, Ana (host) accepts: Bo's seat.
    fn let_in(rooms: &Rooms, host: &TogetherJoined, name: &str, key: &str) -> TogetherJoined {
        let code = &host.state.code;
        let k = rooms.knock(code, name, key).unwrap();
        assert_eq!(k.status, KnockStatus::Waiting);
        rooms
            .answer(code, &k.request_id, &host.member_id, true, "plex:ana")
            .unwrap();
        let done = rooms
            .take_answer(code, &k.request_id, key)
            .unwrap()
            .unwrap();
        assert_eq!(done.status, KnockStatus::Accepted);
        done.joined.unwrap()
    }

    #[tokio::test]
    async fn a_knock_wakes_the_host_and_waits_for_the_answer() {
        let rooms = Arc::new(Rooms::default());
        let a = create(&rooms);
        let code = a.state.code.clone();
        assert_eq!(a.state.host, a.member_id);
        let host_wait = {
            let (rooms, code, id, seq) = (
                rooms.clone(),
                code.clone(),
                a.member_id.clone(),
                a.state.seq,
            );
            tokio::spawn(async move { rooms.wait(&code, &id, "plex:ana", seq).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let k = rooms.knock(&code, "Bo", "jf:bo").unwrap();
        let s = tokio::time::timeout(Duration::from_secs(2), host_wait)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(s.requests.len(), 1);
        assert_eq!(s.requests[0].name, "Bo");
        // Asking again keeps the same request.
        assert_eq!(
            rooms.knock(&code, "Bo", "jf:bo").unwrap().request_id,
            k.request_id
        );
        let asker = {
            let (rooms, code, id) = (rooms.clone(), code.clone(), k.request_id.clone());
            tokio::spawn(async move { rooms.knock_wait(&code, &id, "jf:bo").await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        // Only the host answers.
        assert_eq!(
            rooms.answer(&code, &k.request_id, &a.member_id, true, "jf:bo"),
            Err(TogetherError::Forbidden)
        );
        let s = rooms
            .answer(&code, &k.request_id, &a.member_id, true, "plex:ana")
            .unwrap();
        assert_eq!(s.members.len(), 2);
        assert!(s.requests.is_empty());
        let got = tokio::time::timeout(Duration::from_secs(2), asker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(got.status, KnockStatus::Accepted);
        assert_eq!(got.joined.unwrap().state.members.len(), 2);
        // Handed over once.
        assert_eq!(
            rooms
                .knock_wait(&code, &k.request_id, "jf:bo")
                .await
                .map(|_| ()),
            Err(TogetherError::NotFound)
        );
    }

    #[tokio::test]
    async fn declined_and_cancelled_requests() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = &a.state.code;
        let k = rooms.knock(code, "Bo", "jf:bo").unwrap();
        rooms
            .answer(code, &k.request_id, &a.member_id, false, "plex:ana")
            .unwrap();
        let got = rooms
            .knock_wait_for(code, &k.request_id, "jf:bo", Duration::from_millis(30))
            .await
            .unwrap();
        assert_eq!(got.status, KnockStatus::Declined);
        assert!(got.joined.is_none());
        let k = rooms.knock(code, "Cy", "jf:cy").unwrap();
        assert_eq!(
            rooms.knock_cancel(code, &k.request_id, "jf:bo"),
            Err(TogetherError::Forbidden)
        );
        rooms.knock_cancel(code, &k.request_id, "jf:cy").unwrap();
        let s = rooms
            .wait_for(code, &a.member_id, "plex:ana", 0, Duration::from_millis(10))
            .await
            .unwrap();
        assert!(s.requests.is_empty());
    }

    #[test]
    fn own_devices_get_in_and_rooms_are_listed() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let k = rooms.knock(&a.state.code, "Ana", "plex:ana").unwrap();
        assert_eq!(k.status, KnockStatus::Accepted);
        assert_eq!(k.joined.unwrap().state.members.len(), 2);
        let listed = rooms.list("jf:bo");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].host, "Ana");
        assert_eq!(listed[0].media.title, "Monsters");
        assert!(!listed[0].mine);
        assert!(rooms.list("plex:ana")[0].mine);
    }

    #[test]
    fn the_longest_member_takes_over_as_host() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let b = let_in(&rooms, &a, "Bo", "jf:bo");
        rooms
            .leave(&a.state.code, &a.member_id, "plex:ana")
            .unwrap();
        let k = rooms.knock(&a.state.code, "Cy", "jf:cy").unwrap();
        assert!(
            rooms
                .answer(&a.state.code, &k.request_id, &b.member_id, true, "jf:bo")
                .is_ok()
        );
        assert_eq!(rooms.list("x")[0].host, "Bo");
    }
}
