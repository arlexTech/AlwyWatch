// SPDX-License-Identifier: AGPL-3.0-or-later
//! Watch together rooms (see `keep_service::together`), in memory only.
//!
//! A room is private: only its members (by user, on any device) and the people its host invited
//! know it exists; for everyone else every call answers `NotFound`, exactly like an unknown
//! code. It starts as a lobby (nothing plays, only the host changes it) until the host starts it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use keep_service::Avatar;
use keep_service::together::{
    SEEK_TOLERANCE_MS, TogetherCreate, TogetherInvitee, TogetherJoined, TogetherMedia,
    TogetherMember, TogetherRoom, TogetherRooms, TogetherState, TogetherUpdate, WAIT_SECS,
};
use tokio::sync::Notify;

use crate::transfer::random_u64;

/// A member who has not polled for this long is dropped (they may join again).
const MEMBER_TTL: Duration = Duration::from_secs(60);
/// An empty room is kept this long, for someone dropped by a network change to join again.
const EMPTY_TTL: Duration = Duration::from_secs(300);
const ROOM_TTL: Duration = Duration::from_secs(12 * 3600);
const MAX_ROOMS: usize = 200;
/// Seats plus pending invitations in one room at most.
pub const MAX_PEOPLE: usize = 20;

/// Pairs of users (ordered) who just watched together, for the people list's history.
pub type Met = Vec<(String, String)>;

#[derive(Debug, PartialEq, Eq)]
pub enum TogetherError {
    /// Unknown or closed room, or one the caller may not know about.
    NotFound,
    Busy,
    Full,
    /// A member of the room doing what they may not (someone else's seat, host-only change).
    Forbidden,
    /// A guest doing what the host's rights don't allow (pause, seek, another title).
    NotAllowed,
}

struct Member {
    name: String,
    user_key: String,
    avatar: Avatar,
    seen: Instant,
    since: Instant,
}

/// Someone allowed in: invited by the host, or a member (so a member dropped by a network
/// change can join again). Leaving on purpose removes it.
struct Guest {
    name: String,
    avatar: Avatar,
    /// Was a member at some point (may join the room even while it is empty).
    joined_before: bool,
}

struct Room {
    state: TogetherState,
    members: HashMap<String, Member>,
    /// By user key.
    guests: HashMap<String, Guest>,
    created: Instant,
    empty_since: Option<Instant>,
    changed: Arc<Notify>,
    /// Shared by every room: wakes the room lists' long polls.
    lists: Arc<Notify>,
}

#[derive(Default)]
pub struct Rooms {
    rooms: Mutex<HashMap<String, Room>>,
    lists: Arc<Notify>,
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

pub fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(60).collect()
}

fn pair(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

impl Room {
    fn has_user(&self, user_key: &str) -> bool {
        self.members.values().any(|m| m.user_key == user_key)
    }

    /// Invited (or dropped) users who are not in the room.
    fn pending(&self) -> impl Iterator<Item = (&String, &Guest)> {
        self.guests.iter().filter(|(k, _)| !self.has_user(k))
    }

    fn people(&self) -> usize {
        self.members.len() + self.pending().count()
    }

    /// Shown in the caller's list: they are in it, or invited to a room someone is in.
    fn visible_to(&self, user_key: &str) -> bool {
        self.has_user(user_key) || (!self.members.is_empty() && self.guests.contains_key(user_key))
    }

    fn host_user(&self) -> Option<&str> {
        self.members
            .get(&self.state.host)
            .map(|m| m.user_key.as_str())
    }

    /// Whether a guest's update stays within what the host allows (`TogetherRights`).
    fn allowed(&self, u: &TogetherUpdate) -> Result<(), TogetherError> {
        let r = &self.state.rights;
        let new_title = u
            .media
            .as_ref()
            .is_some_and(|m| clean_media(m.clone()) != self.state.media);
        if new_title {
            return if r.change_title {
                Ok(())
            } else {
                Err(TogetherError::NotAllowed)
            };
        }
        if u.playing != self.state.playing && !r.play_pause {
            return Err(TogetherError::NotAllowed);
        }
        let expected = self.state.position_at(now_ms());
        if u.position_ms.abs_diff(expected) > SEEK_TOLERANCE_MS && !r.seek {
            return Err(TogetherError::NotAllowed);
        }
        Ok(())
    }

    fn is_host_user(&self, user_key: &str) -> bool {
        self.host_user() == Some(user_key)
    }

    /// Drops members who stopped polling; true when someone was dropped.
    fn expire_members(&mut self) -> bool {
        let before = self.members.len();
        self.members.retain(|_, m| m.seen.elapsed() < MEMBER_TTL);
        if self.members.is_empty() && self.empty_since.is_none() {
            self.empty_since = Some(Instant::now());
        }
        self.fix_host();
        self.members.len() != before
    }

    /// The host left: the longest-present member takes over.
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

    fn host_avatar(&self) -> Avatar {
        self.members
            .get(&self.state.host)
            .map(|m| m.avatar.clone())
            .unwrap_or_default()
    }

    fn view(&mut self) -> TogetherState {
        let mut members: Vec<TogetherMember> = self
            .members
            .iter()
            .map(|(id, m)| TogetherMember {
                id: id.clone(),
                name: m.name.clone(),
                user_key: m.user_key.clone(),
                avatar: m.avatar.clone(),
            })
            .collect();
        members.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        self.state.members = members;
        let mut invited: Vec<TogetherInvitee> = self
            .pending()
            .map(|(k, g)| TogetherInvitee {
                user_key: k.clone(),
                name: g.name.clone(),
                avatar: g.avatar.clone(),
            })
            .collect();
        invited.sort_by(|a, b| (&a.name, &a.user_key).cmp(&(&b.name, &b.user_key)));
        self.state.invited = invited;
        self.state.now_ms = now_ms();
        self.state.clone()
    }

    /// The room as `user_key`'s list shows it.
    fn entry(&mut self, user_key: &str) -> TogetherRoom {
        let mut members: Vec<String> = self.members.values().map(|m| m.name.clone()).collect();
        members.sort();
        let mine = self.has_user(user_key);
        let hosting = self.is_host_user(user_key);
        let host = self.host_name();
        let host_avatar = self.host_avatar();
        let host_user_key = self
            .members
            .get(&self.state.host)
            .map(|m| m.user_key.clone())
            .unwrap_or_default();
        let s = self.view();
        TogetherRoom {
            code: s.code,
            host,
            host_avatar,
            host_user_key,
            media: s.media,
            members,
            started: s.started,
            playing: s.playing,
            position_ms: s.position_ms,
            at_ms: s.at_ms,
            now_ms: s.now_ms,
            mine,
            hosting,
            invited: !mine,
        }
    }

    fn bump(&mut self, by: &str) {
        self.state.seq += 1;
        self.state.by = by.to_owned();
        self.changed.notify_waiters();
        self.lists.notify_waiters();
    }

    fn add_member(&mut self, name: &str, user_key: &str, avatar: &Avatar) -> String {
        let id = format!("{:016x}", random_u64());
        let name = clean_name(name);
        self.members.insert(
            id.clone(),
            Member {
                name: name.clone(),
                user_key: user_key.to_owned(),
                avatar: avatar.clone(),
                seen: Instant::now(),
                since: Instant::now(),
            },
        );
        let g = self.guests.entry(user_key.to_owned()).or_insert(Guest {
            name: String::new(),
            avatar: Avatar::default(),
            joined_before: true,
        });
        g.name = name;
        g.avatar = avatar.clone();
        g.joined_before = true;
        self.empty_since = None;
        self.fix_host();
        id
    }

    /// The caller's own seat (and notes they are there). Someone not in the room gets
    /// `NotFound`, as if it did not exist; a member using another user's seat, `Forbidden`.
    fn check(&mut self, member: &str, user_key: &str) -> Result<(), TogetherError> {
        if let Some(m) = self.members.get_mut(member)
            && m.user_key == user_key
        {
            m.seen = Instant::now();
            return Ok(());
        }
        if self.has_user(user_key) {
            Err(TogetherError::Forbidden)
        } else {
            Err(TogetherError::NotFound)
        }
    }

    /// `check`, and the seat must be the host user's.
    fn check_host(&mut self, member: &str, user_key: &str) -> Result<(), TogetherError> {
        self.check(member, user_key)?;
        if self.is_host_user(user_key) {
            Ok(())
        } else {
            Err(TogetherError::Forbidden)
        }
    }

    fn users(&self) -> Vec<&str> {
        let mut u: Vec<&str> = self.members.values().map(|m| m.user_key.as_str()).collect();
        u.sort_unstable();
        u.dedup();
        u
    }

    /// Every pair of distinct users in the room.
    fn all_pairs(&self) -> Met {
        let u = self.users();
        let mut out = Vec::new();
        for (i, a) in u.iter().enumerate() {
            for b in &u[i + 1..] {
                out.push(pair(a, b));
            }
        }
        out
    }

    /// `user_key` with every other user in the room.
    fn pairs_with(&self, user_key: &str) -> Met {
        self.users()
            .into_iter()
            .filter(|u| *u != user_key)
            .map(|u| pair(u, user_key))
            .collect()
    }

    /// What the caller's list shows of this room, for the list's `version`.
    fn fingerprint(&self, user_key: &str, h: &mut DefaultHasher) {
        self.state.code.hash(h);
        self.state.started.hash(h);
        self.has_user(user_key).hash(h);
        self.is_host_user(user_key).hash(h);
        self.host_name().hash(h);
        let m = &self.state.media;
        (&m.title, &m.series_title, m.season, m.episode).hash(h);
        let mut names: Vec<&str> = self.members.values().map(|m| m.name.as_str()).collect();
        names.sort_unstable();
        names.hash(h);
    }
}

impl Rooms {
    /// Drops silent members and dead rooms (waking whoever should know).
    fn purge(&self, map: &mut HashMap<String, Room>) {
        for r in map.values_mut() {
            if r.expire_members() {
                r.bump("");
            }
        }
        let before = map.len();
        map.retain(|_, r| {
            r.created.elapsed() < ROOM_TTL
                && (!r.members.is_empty()
                    || (r.empty_since.is_none_or(|t| t.elapsed() < EMPTY_TTL)
                        && r.guests.values().any(|g| g.joined_before)))
        });
        if map.len() != before {
            self.lists.notify_waiters();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Room>> {
        self.rooms.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on a live room (after a purge).
    fn with<T>(
        &self,
        code: &str,
        f: impl FnOnce(&mut Room) -> Result<T, TogetherError>,
    ) -> Result<T, TogetherError> {
        let mut map = self.lock();
        self.purge(&mut map);
        f(map.get_mut(code).ok_or(TogetherError::NotFound)?)
    }

    /// A new lobby, the caller its host.
    pub fn create(
        &self,
        c: TogetherCreate,
        name: &str,
        user_key: &str,
        avatar: &Avatar,
    ) -> Result<TogetherJoined, TogetherError> {
        let mut map = self.lock();
        self.purge(&mut map);
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
                started: false,
                playing: false,
                position_ms: 0,
                at_ms: now_ms(),
                now_ms: 0,
                by: String::new(),
                members: vec![],
                host: String::new(),
                invited: vec![],
                rights: c.rights,
            },
            members: HashMap::new(),
            guests: HashMap::new(),
            created: Instant::now(),
            empty_since: None,
            changed: Arc::new(Notify::new()),
            lists: self.lists.clone(),
        };
        let member_id = room.add_member(name, user_key, avatar);
        room.state.by = member_id.clone();
        let state = room.view();
        map.insert(code, room);
        self.lists.notify_waiters();
        Ok(TogetherJoined { member_id, state })
    }

    /// The caller's rooms and invitations: invitations first, newest first within each.
    pub fn list(&self, user_key: &str) -> TogetherRooms {
        let mut map = self.lock();
        self.purge(&mut map);
        let mut rooms: Vec<(bool, Instant, &mut Room)> = map
            .values_mut()
            .filter(|r| r.visible_to(user_key))
            .map(|r| (r.has_user(user_key), r.created, r))
            .collect();
        rooms.sort_by_key(|(mine, created, _)| (*mine, std::cmp::Reverse(*created)));
        let mut h = DefaultHasher::new();
        rooms.len().hash(&mut h);
        for (_, _, r) in &rooms {
            r.fingerprint(user_key, &mut h);
        }
        TogetherRooms {
            // Below 2^53 (exact in JavaScript numbers), never 0 (a client's "nothing yet").
            version: (h.finish() & ((1 << 53) - 1)).max(1),
            rooms: rooms
                .into_iter()
                .map(|(_, _, r)| r.entry(user_key))
                .collect(),
        }
    }

    /// The caller's list once its version differs from `after`, or after `WAIT_SECS`.
    pub async fn list_wait(&self, user_key: &str, after: u64) -> TogetherRooms {
        self.list_wait_for(user_key, after, Duration::from_secs(WAIT_SECS))
            .await
    }

    async fn list_wait_for(&self, user_key: &str, after: u64, timeout: Duration) -> TogetherRooms {
        let notified = self.lists.notified();
        tokio::pin!(notified);
        // Registered before looking, so a change in between is not missed.
        notified.as_mut().enable();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let list = self.list(user_key);
            if list.version != after {
                return list;
            }
            if tokio::time::timeout_at(deadline, notified.as_mut())
                .await
                .is_err()
            {
                return self.list(user_key);
            }
            notified.set(self.lists.notified());
            notified.as_mut().enable();
        }
    }

    /// The host invites people; already invited or present ones are left as they are. All or
    /// nothing when the room would get too full.
    pub fn invite(
        &self,
        code: &str,
        member: &str,
        user_key: &str,
        people: &[TogetherInvitee],
    ) -> Result<TogetherState, TogetherError> {
        self.with(code, |room| {
            room.check_host(member, user_key)?;
            let mut new: Vec<&TogetherInvitee> = people
                .iter()
                .filter(|p| !room.has_user(&p.user_key) && !room.guests.contains_key(&p.user_key))
                .collect();
            new.sort_by(|a, b| a.user_key.cmp(&b.user_key));
            new.dedup_by(|a, b| a.user_key == b.user_key);
            if new.is_empty() {
                return Ok(room.view());
            }
            if room.people() + new.len() > MAX_PEOPLE {
                return Err(TogetherError::Full);
            }
            for p in new {
                room.guests.insert(
                    p.user_key.clone(),
                    Guest {
                        name: clean_name(&p.name),
                        avatar: p.avatar.clone(),
                        joined_before: false,
                    },
                );
            }
            room.bump("");
            Ok(room.view())
        })
    }

    /// The host withdraws an invitation (nobody is removed from the room: someone in it just
    /// cannot come back after leaving or being dropped).
    pub fn uninvite(
        &self,
        code: &str,
        member: &str,
        user_key: &str,
        target: &str,
    ) -> Result<TogetherState, TogetherError> {
        self.with(code, |room| {
            room.check_host(member, user_key)?;
            if room.guests.remove(target).is_some() {
                room.bump("");
            }
            Ok(room.view())
        })
    }

    /// An invited user says no.
    pub fn decline(&self, code: &str, user_key: &str) -> Result<(), TogetherError> {
        self.with(code, |room| {
            if room.has_user(user_key) || room.guests.remove(user_key).is_none() {
                return Err(TogetherError::NotFound);
            }
            room.bump("");
            Ok(())
        })
    }

    /// A seat for an invited user, or another device of a member (no approval step). Anyone
    /// else: `NotFound`.
    pub fn join(
        &self,
        code: &str,
        name: &str,
        user_key: &str,
        avatar: &Avatar,
    ) -> Result<(TogetherJoined, Met), TogetherError> {
        self.with(code, |room| {
            let member = room.has_user(user_key);
            let allowed = member
                || room
                    .guests
                    .get(user_key)
                    .is_some_and(|g| !room.members.is_empty() || g.joined_before);
            if !allowed {
                return Err(TogetherError::NotFound);
            }
            // A pending invitation already holds its place; another device takes one more.
            if member && room.people() >= MAX_PEOPLE {
                return Err(TogetherError::Full);
            }
            let member_id = room.add_member(name, user_key, avatar);
            let met = if room.state.started {
                room.pairs_with(user_key)
            } else {
                vec![]
            };
            room.bump("");
            Ok((
                TogetherJoined {
                    member_id,
                    state: room.view(),
                },
                met,
            ))
        })
    }

    /// Play/pause/seek/title. In a lobby only the host changes it (and nothing plays) until
    /// they send `started`.
    pub fn update(
        &self,
        code: &str,
        u: TogetherUpdate,
        user_key: &str,
    ) -> Result<(TogetherState, Met), TogetherError> {
        let mut map = self.lock();
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        room.check(&u.member_id, user_key)?;
        let mut met = vec![];
        if !room.state.started {
            if !room.is_host_user(user_key) {
                return Err(TogetherError::Forbidden);
            }
            room.state.started = u.started;
            room.state.playing = u.started && u.playing;
            if u.started {
                met = room.all_pairs();
            }
        } else {
            if !room.is_host_user(user_key) {
                room.allowed(&u)?;
            }
            room.state.playing = u.playing;
        }
        room.state.position_ms = u.position_ms;
        room.state.at_ms = now_ms();
        if let Some(m) = u.media {
            room.state.media = clean_media(m);
        }
        room.bump(&u.member_id);
        Ok((room.view(), met))
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

    /// Linked identities: the users `from` are now the person `to`. Their seats and
    /// invitations become `to`'s, so a room joined with one sign-in is "mine" with the other.
    pub fn rekey(&self, from: &[String], to: &str) {
        let mut map = self.lock();
        for room in map.values_mut() {
            let mut changed = false;
            for m in room.members.values_mut() {
                if from.contains(&m.user_key) {
                    m.user_key = to.to_owned();
                    changed = true;
                }
            }
            for old in from {
                if let Some(g) = room.guests.remove(old) {
                    changed = true;
                    match room.guests.get_mut(to) {
                        Some(have) => have.joined_before |= g.joined_before,
                        None => {
                            room.guests.insert(to.to_owned(), g);
                        }
                    }
                }
            }
            if changed {
                room.bump("");
            }
        }
    }

    /// Leaves on purpose: the user's last seat going also ends their right to come back.
    pub fn leave(&self, code: &str, member: &str, user_key: &str) -> Result<(), TogetherError> {
        let mut map = self.lock();
        let room = map.get_mut(code).ok_or(TogetherError::NotFound)?;
        room.check(member, user_key)?;
        room.members.remove(member);
        if !room.has_user(user_key) {
            room.guests.remove(user_key);
        }
        if room.members.is_empty() {
            room.empty_since = Some(Instant::now());
        }
        room.fix_host();
        room.bump("");
        self.purge(&mut map);
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
                rights: Default::default(),
            },
            "Ana",
            "plex:ana",
            &Avatar::default(),
        )
        .unwrap()
    }

    fn inv(key: &str, name: &str) -> TogetherInvitee {
        TogetherInvitee {
            user_key: key.into(),
            name: name.into(),
            avatar: Avatar::default(),
        }
    }

    fn upd(member: &str, playing: bool, position_ms: u64, started: bool) -> TogetherUpdate {
        TogetherUpdate {
            member_id: member.into(),
            playing,
            position_ms,
            media: None,
            started,
        }
    }

    /// Ana (host) invites; the invitee joins.
    fn invite_in(rooms: &Rooms, host: &TogetherJoined, name: &str, key: &str) -> TogetherJoined {
        let code = &host.state.code;
        rooms
            .invite(code, &host.member_id, "plex:ana", &[inv(key, name)])
            .unwrap();
        rooms.join(code, name, key, &Avatar::default()).unwrap().0
    }

    #[test]
    fn a_new_room_is_a_lobby_seen_only_by_its_host() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        assert!(!a.state.started && !a.state.playing);
        assert_eq!(a.state.host, a.member_id);
        let mine = rooms.list("plex:ana");
        assert_eq!(mine.rooms.len(), 1);
        assert!(mine.rooms[0].mine && mine.rooms[0].hosting && !mine.rooms[0].invited);
        assert!(rooms.list("jf:bo").rooms.is_empty());
        assert_eq!(
            rooms
                .join(&a.state.code, "Bo", "jf:bo", &Avatar::default())
                .map(|_| ()),
            Err(TogetherError::NotFound)
        );
    }

    #[tokio::test]
    async fn a_change_wakes_the_other_member() {
        let rooms = Arc::new(Rooms::default());
        let a = create(&rooms);
        let code = a.state.code.clone();
        rooms
            .update(&code, upd(&a.member_id, true, 0, true), "plex:ana")
            .unwrap();
        let b = invite_in(&rooms, &a, "Bo", "jf:bo");
        assert_eq!(b.state.members.len(), 2);
        let seq = b.state.seq;
        let waiter = {
            let (rooms, code, id) = (rooms.clone(), code.clone(), b.member_id.clone());
            tokio::spawn(async move { rooms.wait(&code, &id, "jf:bo", seq).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        rooms
            .update(&code, upd(&a.member_id, true, 6_000, false), "plex:ana")
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
        let b = invite_in(&rooms, &a, "Bo", "jf:bo");
        let (after, _) = rooms
            .update(&code, upd(&a.member_id, true, 6_000, true), "plex:ana")
            .unwrap();
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
        // Left on purpose: no way back without a new invitation.
        assert!(s.invited.is_empty());
        assert_eq!(
            rooms
                .join(&code, "Bo", "jf:bo", &Avatar::default())
                .map(|_| ()),
            Err(TogetherError::NotFound)
        );
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
    fn strangers_get_not_found_members_get_forbidden() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = &a.state.code;
        let b = invite_in(&rooms, &a, "Bo", "jf:bo");
        let mv = upd(&a.member_id, true, 0, false);
        assert_eq!(
            rooms.update(code, mv.clone(), "jf:stranger").map(|_| ()),
            Err(TogetherError::NotFound)
        );
        assert_eq!(
            rooms.update(code, mv.clone(), "jf:bo").map(|_| ()),
            Err(TogetherError::Forbidden)
        );
        assert_eq!(
            rooms.update("000000x", mv, "plex:ana").map(|_| ()),
            Err(TogetherError::NotFound)
        );
        assert_eq!(
            rooms
                .invite(code, &b.member_id, "jf:bo", &[inv("jf:cy", "Cy")])
                .map(|_| ()),
            Err(TogetherError::Forbidden),
            "only the host invites"
        );
        assert_eq!(
            rooms
                .invite(code, &b.member_id, "jf:cy", &[inv("jf:cy", "Cy")])
                .map(|_| ()),
            Err(TogetherError::NotFound)
        );
    }

    #[test]
    fn guests_do_only_what_the_host_allows() {
        let rooms = Rooms::default();
        let a = rooms
            .create(
                TogetherCreate {
                    media: media("Monsters"),
                    rights: keep_service::together::TogetherRights {
                        change_title: false,
                        play_pause: false,
                        seek: true,
                    },
                },
                "Ana",
                "plex:ana",
                &Avatar::default(),
            )
            .unwrap();
        let code = &a.state.code;
        let b = invite_in(&rooms, &a, "Bo", "jf:bo");
        rooms
            .update(code, upd(&a.member_id, true, 10_000, true), "plex:ana")
            .unwrap();
        // Bo may not pause, nor change the title; he may move along (playing on).
        let refused = |u| rooms.update(code, u, "jf:bo").map(|_| ());
        assert_eq!(
            refused(upd(&b.member_id, false, 10_000, false)),
            Err(TogetherError::NotAllowed)
        );
        let mut retitle = upd(&b.member_id, true, 0, false);
        retitle.media = Some(media("Up"));
        assert_eq!(refused(retitle), Err(TogetherError::NotAllowed));
        assert_eq!(refused(upd(&b.member_id, true, 60_000, false)), Ok(()));
        // The host can do everything.
        let (s, _) = rooms
            .update(code, upd(&a.member_id, false, 60_000, false), "plex:ana")
            .unwrap();
        assert!(!s.playing);
        assert!(!s.rights.play_pause);
    }

    #[test]
    fn seeking_can_be_reserved_to_the_host() {
        let rooms = Rooms::default();
        let a = rooms
            .create(
                TogetherCreate {
                    media: media("Monsters"),
                    rights: keep_service::together::TogetherRights {
                        change_title: true,
                        play_pause: true,
                        seek: false,
                    },
                },
                "Ana",
                "plex:ana",
                &Avatar::default(),
            )
            .unwrap();
        let code = &a.state.code;
        let b = invite_in(&rooms, &a, "Bo", "jf:bo");
        rooms
            .update(code, upd(&a.member_id, false, 10_000, true), "plex:ana")
            .unwrap();
        // Pausing/playing where the room is: fine. Jumping a minute: refused.
        assert!(
            rooms
                .update(code, upd(&b.member_id, true, 10_500, false), "jf:bo")
                .is_ok()
        );
        assert_eq!(
            rooms
                .update(code, upd(&b.member_id, true, 70_000, false), "jf:bo")
                .map(|_| ()),
            Err(TogetherError::NotAllowed)
        );
    }

    #[test]
    fn lobby_belongs_to_the_host_until_started_and_never_goes_back() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = &a.state.code;
        let b = invite_in(&rooms, &a, "Bo", "jf:bo");
        assert!(!b.state.started);
        // Bo can neither start nor move the lobby.
        assert_eq!(
            rooms
                .update(code, upd(&b.member_id, true, 5, true), "jf:bo")
                .map(|_| ()),
            Err(TogetherError::Forbidden)
        );
        // The host changes the title; nothing plays before the start.
        let mut retitle = upd(&a.member_id, true, 1_000, false);
        retitle.media = Some(media("Up"));
        let (s, met) = rooms.update(code, retitle, "plex:ana").unwrap();
        assert!(!s.started && !s.playing);
        assert_eq!(s.media.title, "Up");
        assert!(met.is_empty());
        // Start: both users met.
        let (s, met) = rooms
            .update(code, upd(&a.member_id, true, 2_000, true), "plex:ana")
            .unwrap();
        assert!(s.started && s.playing);
        assert_eq!(met, vec![("jf:bo".to_owned(), "plex:ana".to_owned())]);
        // Started: anyone plays; `started: false` does not go back.
        let (s, _) = rooms
            .update(code, upd(&b.member_id, false, 3_000, false), "jf:bo")
            .unwrap();
        assert!(s.started && !s.playing);
        assert_eq!(s.by, b.member_id);
    }

    #[test]
    fn invitations_join_decline_withdraw_and_own_devices() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = &a.state.code;
        let s = rooms
            .invite(
                code,
                &a.member_id,
                "plex:ana",
                &[
                    inv("jf:bo", "Bo"),
                    inv("jf:cy", "Cy"),
                    inv("plex:ana", "Ana"),
                ],
            )
            .unwrap();
        let names: Vec<&str> = s.invited.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Bo", "Cy"], "inviting oneself does nothing");
        let listed = rooms.list("jf:bo");
        assert_eq!(listed.rooms.len(), 1);
        assert!(listed.rooms[0].invited && !listed.rooms[0].mine);
        assert_eq!(listed.rooms[0].host, "Ana");
        // Cy declines, then cannot join; Bo joins and leaves the invited list.
        rooms.decline(code, "jf:cy").unwrap();
        assert!(rooms.list("jf:cy").rooms.is_empty());
        assert_eq!(rooms.decline(code, "jf:cy"), Err(TogetherError::NotFound));
        assert_eq!(
            rooms
                .join(code, "Cy", "jf:cy", &Avatar::default())
                .map(|_| ()),
            Err(TogetherError::NotFound)
        );
        let (b, met) = rooms.join(code, "Bo", "jf:bo", &Avatar::default()).unwrap();
        assert!(met.is_empty(), "not started yet");
        assert!(b.state.invited.is_empty());
        assert_eq!(b.state.members.len(), 2);
        assert!(rooms.list("jf:bo").rooms[0].mine);
        // Ana's second device gets in without an invitation and has the host's rights.
        let (a2, _) = rooms
            .join(code, "Ana", "plex:ana", &Avatar::default())
            .unwrap();
        assert!(a2.state.is_host(&a2.member_id));
        let s = rooms
            .invite(code, &a2.member_id, "plex:ana", &[inv("jf:dee", "Dee")])
            .unwrap();
        assert_eq!(s.invited.len(), 1);
        let s = rooms
            .uninvite(code, &a2.member_id, "plex:ana", "jf:dee")
            .unwrap();
        assert!(s.invited.is_empty());
        assert!(rooms.list("jf:dee").rooms.is_empty());
    }

    #[test]
    fn rooms_are_capped_at_twenty_people() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = &a.state.code;
        let many: Vec<TogetherInvitee> = (0..19)
            .map(|i| inv(&format!("jf:{i}"), &format!("P{i}")))
            .collect();
        rooms.invite(code, &a.member_id, "plex:ana", &many).unwrap();
        assert_eq!(
            rooms
                .invite(code, &a.member_id, "plex:ana", &[inv("jf:x", "X")])
                .map(|_| ()),
            Err(TogetherError::Full)
        );
        // An invitee's seat replaces their invitation; another own device would be one too many.
        rooms.join(code, "P0", "jf:0", &Avatar::default()).unwrap();
        assert_eq!(
            rooms
                .join(code, "Ana", "plex:ana", &Avatar::default())
                .map(|_| ()),
            Err(TogetherError::Full)
        );
    }

    #[test]
    fn the_longest_member_takes_over_and_invitations_stay() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = &a.state.code;
        let b = invite_in(&rooms, &a, "Bo", "jf:bo");
        rooms
            .invite(code, &a.member_id, "plex:ana", &[inv("jf:cy", "Cy")])
            .unwrap();
        rooms.leave(code, &a.member_id, "plex:ana").unwrap();
        let l = rooms.list("jf:bo");
        assert_eq!(l.rooms[0].host, "Bo");
        assert!(l.rooms[0].hosting);
        assert!(
            rooms.list("jf:cy").rooms[0].invited,
            "invitation still valid"
        );
        // Bo, now host, may start the lobby.
        let (s, _) = rooms
            .update(code, upd(&b.member_id, true, 0, true), "jf:bo")
            .unwrap();
        assert!(s.started);
        let (_, met) = rooms.join(code, "Cy", "jf:cy", &Avatar::default()).unwrap();
        assert_eq!(met, vec![("jf:bo".to_owned(), "jf:cy".to_owned())]);
    }

    #[test]
    fn an_empty_lobby_closes_when_nobody_can_come_back() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = a.state.code.clone();
        rooms
            .invite(&code, &a.member_id, "plex:ana", &[inv("jf:bo", "Bo")])
            .unwrap();
        rooms.leave(&code, &a.member_id, "plex:ana").unwrap();
        assert!(rooms.list("jf:bo").rooms.is_empty());
        assert_eq!(
            rooms
                .join(&code, "Bo", "jf:bo", &Avatar::default())
                .map(|_| ()),
            Err(TogetherError::NotFound)
        );
        assert!(rooms.lock().is_empty());
    }

    #[test]
    fn a_dropped_member_may_join_an_empty_room_again() {
        let rooms = Rooms::default();
        let a = create(&rooms);
        let code = a.state.code.clone();
        // Silent for longer than the heartbeat allows.
        rooms
            .lock()
            .get_mut(&code)
            .unwrap()
            .members
            .values_mut()
            .for_each(|m| m.seen -= MEMBER_TTL);
        assert!(
            rooms.list("plex:ana").rooms.is_empty(),
            "empty rooms are not listed"
        );
        let (again, _) = rooms
            .join(&code, "Ana", "plex:ana", &Avatar::default())
            .unwrap();
        assert_eq!(again.state.host, again.member_id);
    }

    #[tokio::test]
    async fn an_invitation_wakes_the_invitees_list() {
        let rooms = Arc::new(Rooms::default());
        let a = create(&rooms);
        let before = rooms.list("jf:bo");
        assert!(before.rooms.is_empty());
        // Unchanged: the long poll times out with the same version.
        let same = rooms
            .list_wait_for("jf:bo", before.version, Duration::from_millis(30))
            .await;
        assert_eq!(same.version, before.version);
        let waiter = {
            let rooms = rooms.clone();
            tokio::spawn(async move { rooms.list_wait("jf:bo", before.version).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        // Playback in some other room does not answer it; the invitation does.
        rooms
            .update(
                &a.state.code,
                upd(&a.member_id, false, 9, false),
                "plex:ana",
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        rooms
            .invite(
                &a.state.code,
                &a.member_id,
                "plex:ana",
                &[inv("jf:bo", "Bo")],
            )
            .unwrap();
        let got = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("woken")
            .unwrap();
        assert_ne!(got.version, before.version);
        assert!(got.rooms[0].invited);
        // A version that is not the current one answers at once.
        assert_eq!(rooms.list_wait("jf:bo", 0).await.version, got.version);
    }
}
