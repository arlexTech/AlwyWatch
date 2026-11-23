// SPDX-License-Identifier: AGPL-3.0-or-later
//! SQLite storage of the keep service. Tokens are never stored.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use keep_service::{AdminUser, Avatar, MyVote, Rights, ServerType, VoteCounts, VoteKind};
use rusqlite::{Connection, OptionalExtension, params};

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
CREATE TABLE IF NOT EXISTS users (
    user_key TEXT PRIMARY KEY, name TEXT NOT NULL, server_id TEXT NOT NULL,
    is_admin INTEGER NOT NULL DEFAULT 0, last_seen INTEGER NOT NULL,
    custom_name TEXT, avatar_preset TEXT, avatar_type TEXT,
    avatar_version INTEGER NOT NULL DEFAULT 0, profile_set INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS rights (
    user_key TEXT PRIMARY KEY, can_keep_forever INTEGER NOT NULL,
    max_forever INTEGER, max_keep_days INTEGER
);
CREATE TABLE IF NOT EXISTS items (
    key TEXT PRIMARY KEY, collection_id INTEGER, collection_title TEXT,
    collection_active INTEGER NOT NULL DEFAULT 0, in_collection INTEGER NOT NULL DEFAULT 0,
    title TEXT, year INTEGER, kind TEXT, poster TEXT, tmdb TEXT, tvdb TEXT,
    deletion_date INTEGER, meta_at INTEGER, last_seen INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS votes (
    item_key TEXT NOT NULL, user_key TEXT NOT NULL, kind TEXT NOT NULL,
    until_ts INTEGER, reminder_days INTEGER, reason TEXT,
    created_at INTEGER NOT NULL, fulfilled_at INTEGER,
    PRIMARY KEY (item_key, user_key)
);
CREATE TABLE IF NOT EXISTS applied (
    item_key TEXT PRIMARY KEY, excluded INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS seerr_links (
    user_key TEXT PRIMARY KEY, seerr_user_id INTEGER NOT NULL, created INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS settings (
    user_key TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (user_key, key)
);
CREATE TABLE IF NOT EXISTS together_pairs (
    a TEXT NOT NULL, b TEXT NOT NULL, last_at INTEGER NOT NULL,
    PRIMARY KEY (a, b)
);
CREATE TABLE IF NOT EXISTS identities (
    user_key TEXT PRIMARY KEY, person TEXT NOT NULL, server_id TEXT NOT NULL, kind TEXT NOT NULL,
    user_id TEXT NOT NULL, media_name TEXT NOT NULL, is_admin INTEGER NOT NULL DEFAULT 0,
    last_seen INTEGER NOT NULL, linked_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS identities_person ON identities(person);
-- AlwyWatch accounts: sign-in methods of a person (see accounts.rs). A person with no media
-- sign-in is `account:<id>`.
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY, person TEXT NOT NULL, username TEXT UNIQUE COLLATE NOCASE,
    password TEXT, created INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS accounts_person ON accounts(person);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY, account TEXT NOT NULL, device TEXT NOT NULL,
    created INTEGER NOT NULL, last_seen INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS passkeys (
    cred_id TEXT PRIMARY KEY, account TEXT NOT NULL, name TEXT NOT NULL, alg INTEGER NOT NULL,
    public_key BLOB NOT NULL, sign_count INTEGER NOT NULL, created INTEGER NOT NULL,
    last_used INTEGER
);
CREATE TABLE IF NOT EXISTS oidc_links (
    issuer TEXT NOT NULL, sub TEXT NOT NULL, account TEXT NOT NULL, created INTEGER NOT NULL,
    PRIMARY KEY (issuer, sub)
);
CREATE TABLE IF NOT EXISTS invites (
    code_hash TEXT PRIMARY KEY, created_by TEXT NOT NULL, created INTEGER NOT NULL,
    expires INTEGER NOT NULL, uses_left INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS log (
    id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, actor TEXT NOT NULL,
    action TEXT NOT NULL, detail TEXT NOT NULL
);
";

/// Columns added after the first release (older databases get them on open).
const ADDED_COLUMNS: [(&str, &str, &str); 6] = [
    ("users", "custom_name", "TEXT"),
    ("users", "avatar_preset", "TEXT"),
    ("users", "avatar_type", "TEXT"),
    ("users", "avatar_version", "INTEGER NOT NULL DEFAULT 0"),
    ("users", "profile_set", "INTEGER NOT NULL DEFAULT 0"),
    // 1: Plex/Jellyfin sign-ins of the person no longer act on the account (accounts.rs).
    ("accounts", "media_locked", "INTEGER NOT NULL DEFAULT 0"),
];

fn migrate(c: &Connection) -> rusqlite::Result<()> {
    for (table, column, ty) in ADDED_COLUMNS {
        let has: bool = c.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if !has {
            c.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"))?;
        }
    }
    backfill_identities(c)
}

/// Users from before linked identities each become a one-identity person (idempotent: only
/// users without an identity row; after a merge the aliases have no `users` row left).
fn backfill_identities(c: &Connection) -> rusqlite::Result<()> {
    let legacy: Vec<(String, String, String, bool, i64)> = {
        let mut st = c.prepare(
            "SELECT u.user_key, u.name, u.server_id, u.is_admin, u.last_seen FROM users u
             WHERE NOT EXISTS (SELECT 1 FROM identities i WHERE i.user_key = u.user_key)",
        )?;
        st.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    for (key, name, server_id, is_admin, last_seen) in legacy {
        let (kind, user_id) = split_user_key(&key);
        c.execute(
            "INSERT OR IGNORE INTO identities(user_key, person, server_id, kind, user_id, media_name,
                                              is_admin, last_seen, linked_at)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![key, server_id, kind, user_id, name, is_admin, last_seen],
        )?;
    }
    Ok(())
}

/// `kind:server:user id` -> (kind, user id).
fn split_user_key(key: &str) -> (String, String) {
    let mut it = key.splitn(3, ':');
    let kind = it.next().unwrap_or_default().to_owned();
    let _server = it.next();
    (kind, it.next().unwrap_or_default().to_owned())
}

fn kind_name(k: ServerType) -> &'static str {
    match k {
        ServerType::Jellyfin => "jellyfin",
        ServerType::Plex => "plex",
    }
}

fn parse_server_kind(s: &str) -> ServerType {
    if s == "plex" {
        ServerType::Plex
    } else {
        ServerType::Jellyfin
    }
}

/// One media-server user (a sign-in) and the person (canonical user key) it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityRow {
    pub user_key: String,
    pub person: String,
    pub server_id: String,
    pub kind: ServerType,
    pub user_id: String,
    pub media_name: String,
    /// Admin by the media server's rule, as of `last_seen`.
    pub is_admin: bool,
    pub last_seen: i64,
    pub linked_at: i64,
}

const IDENTITY_COLS: &str =
    "user_key, person, server_id, kind, user_id, media_name, is_admin, last_seen, linked_at";

fn row_identity(r: &rusqlite::Row<'_>) -> rusqlite::Result<IdentityRow> {
    Ok(IdentityRow {
        user_key: r.get(0)?,
        person: r.get(1)?,
        server_id: r.get(2)?,
        kind: parse_server_kind(&r.get::<_, String>(3)?),
        user_id: r.get(4)?,
        media_name: r.get(5)?,
        is_admin: r.get(6)?,
        last_seen: r.get(7)?,
        linked_at: r.get(8)?,
    })
}

/// A sign-in just validated by its media server.
pub struct SignIn<'a> {
    pub user_key: &'a str,
    pub server_id: &'a str,
    pub kind: ServerType,
    pub user_id: &'a str,
    pub media_name: &'a str,
    pub is_admin: bool,
}

/// What a link did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LinkOutcome {
    /// The person's user key after the link.
    pub canonical: String,
    /// Person keys merged into `canonical` (empty: nothing changed, already one person).
    pub merged: Vec<String>,
    /// The custom avatar picture kept is stored under this old key: move it to `canonical`.
    pub avatar_from: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum LinkError {
    /// Both persons have a (different) sign-in on this server.
    SameServer(String),
    Db(String),
}

impl From<rusqlite::Error> for LinkError {
    fn from(e: rusqlite::Error) -> Self {
        LinkError::Db(e.to_string())
    }
}

#[derive(Debug, PartialEq)]
pub enum UnlinkError {
    /// Not one of the caller's sign-ins.
    NotFound,
    /// The canonical sign-in (holds the data) cannot leave.
    Canonical,
    Db(String),
}

impl From<rusqlite::Error> for UnlinkError {
    fn from(e: rusqlite::Error) -> Self {
        UnlinkError::Db(e.to_string())
    }
}

/// Order of strength of two votes of one person on one item: an active keep (forever > until
/// watched > longest days), then "don't care", then keeps that are over (fulfilled, expired).
fn vote_strength(v: &VoteRow, now: i64) -> (u8, u8, i64) {
    if v.active_keep(now) {
        let kind = match v.vote.kind {
            VoteKind::KeepForever => 3,
            VoteKind::KeepUntilWatched => 2,
            _ => 1,
        };
        (2, kind, v.vote.until.unwrap_or(0))
    } else if v.vote.kind == VoteKind::DontCare {
        (1, 0, v.vote.created_at)
    } else {
        (0, 0, v.vote.created_at)
    }
}

/// The more permissive of two explicit rights (an explicit admin choice beats an untouched
/// default, so linking never lifts a restriction by itself).
fn merge_rights(a: Option<Rights>, b: Option<Rights>) -> Option<Rights> {
    let wider = |x: Option<u32>, y: Option<u32>| match (x, y) {
        (Some(x), Some(y)) => Some(x.max(y)),
        _ => None,
    };
    match (a, b) {
        (Some(a), Some(b)) => Some(Rights {
            can_keep_forever: a.can_keep_forever || b.can_keep_forever,
            max_forever: wider(a.max_forever, b.max_forever),
            max_keep_days: wider(a.max_keep_days, b.max_keep_days),
        }),
        (a, b) => a.or(b),
    }
}

/// Settings values are "change time ␟ value" (the apps merge per key, newer wins).
fn setting_time(v: &str) -> Option<i64> {
    v.split_once('\u{241F}')?.0.trim().parse().ok()
}

/// One `users` row, everything a merge needs.
#[derive(Debug, Clone, Default)]
struct UserRow {
    name: String,
    server_id: String,
    is_admin: bool,
    last_seen: i64,
    custom_name: Option<String>,
    avatar_preset: Option<String>,
    avatar_type: Option<String>,
    avatar_version: u32,
    profile_set: bool,
}

fn user_row(tx: &Connection, key: &str) -> rusqlite::Result<Option<UserRow>> {
    tx.query_row(
        "SELECT name, server_id, is_admin, last_seen, custom_name, avatar_preset, avatar_type,
                avatar_version, profile_set FROM users WHERE user_key = ?1",
        [key],
        |r| {
            Ok(UserRow {
                name: r.get(0)?,
                server_id: r.get(1)?,
                is_admin: r.get(2)?,
                last_seen: r.get(3)?,
                custom_name: r.get(4)?,
                avatar_preset: r.get(5)?,
                avatar_type: r.get(6)?,
                avatar_version: r.get(7)?,
                profile_set: r.get(8)?,
            })
        },
    )
    .optional()
}

fn identities_in(tx: &Connection, person: &str) -> rusqlite::Result<Vec<IdentityRow>> {
    let mut st = tx.prepare(&format!(
        "SELECT {IDENTITY_COLS} FROM identities WHERE person = ?1 ORDER BY linked_at, user_key"
    ))?;
    st.query_map([person], row_identity)?.collect()
}

fn votes_of(tx: &Connection, user: &str) -> rusqlite::Result<Vec<VoteRow>> {
    let mut st = tx.prepare(
        "SELECT item_key, user_key, kind, until_ts, reminder_days, reason, created_at, fulfilled_at
         FROM votes WHERE user_key = ?1",
    )?;
    st.query_map([user], row_vote)?.collect()
}

fn row_vote(r: &rusqlite::Row<'_>) -> rusqlite::Result<VoteRow> {
    Ok(VoteRow {
        item_key: r.get(0)?,
        user_key: r.get(1)?,
        vote: MyVote {
            kind: parse_kind(&r.get::<_, String>(2)?),
            until: r.get(3)?,
            reminder_days: r.get(4)?,
            reason: r.get(5)?,
            created_at: r.get(6)?,
        },
        fulfilled_at: r.get(7)?,
    })
}

/// Display name (chosen, else the media-server one) and avatar, as SQL over `users u`.
const PROFILE_COLS: &str =
    "COALESCE(u.custom_name, u.name), u.name, u.avatar_preset, u.avatar_type, u.avatar_version,
     u.custom_name IS NOT NULL, u.profile_set";

/// A user's public profile (`PROFILE_COLS`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Profile {
    pub name: String,
    pub media_name: String,
    pub name_is_custom: bool,
    pub avatar: Avatar,
    /// The one-time welcome step was done (or skipped).
    pub profile_set: bool,
}

/// Reads `PROFILE_COLS` starting at column `at`.
fn row_profile(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Profile> {
    let name: String = r.get(at)?;
    let media_name: String = r.get(at + 1)?;
    let preset: Option<String> = r.get(at + 2)?;
    let custom: Option<String> = r.get(at + 3)?;
    let version: u32 = r.get(at + 4)?;
    Ok(Profile {
        name_is_custom: r.get(at + 5)?,
        profile_set: r.get(at + 6)?,
        name,
        media_name,
        avatar: Avatar {
            preset: if custom.is_some() { None } else { preset },
            custom_version: custom.map(|_| version),
        },
    })
}

fn kind_str(k: VoteKind) -> &'static str {
    match k {
        VoteKind::DontCare => "dont_care",
        VoteKind::KeepDays => "keep_days",
        VoteKind::KeepUntilWatched => "keep_until_watched",
        VoteKind::KeepForever => "keep_forever",
    }
}

fn parse_kind(s: &str) -> VoteKind {
    match s {
        "keep_days" => VoteKind::KeepDays,
        "keep_until_watched" => VoteKind::KeepUntilWatched,
        "keep_forever" => VoteKind::KeepForever,
        _ => VoteKind::DontCare,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemRow {
    pub key: String,
    pub collection_id: Option<i64>,
    pub collection_title: Option<String>,
    pub collection_active: bool,
    pub in_collection: bool,
    pub title: Option<String>,
    pub year: Option<i32>,
    pub kind: Option<String>,
    pub poster: Option<String>,
    pub tmdb: Option<String>,
    pub tvdb: Option<String>,
    pub deletion_date: Option<i64>,
    pub meta_at: Option<i64>,
}

/// One collection item as seen in a Maintainerr sync.
pub struct NewItem<'a> {
    pub key: &'a str,
    pub collection_id: i64,
    pub collection_title: &'a str,
    pub active: bool,
    pub deletion_date: Option<i64>,
    pub poster: Option<&'a str>,
    pub tmdb: Option<String>,
    pub tvdb: Option<String>,
    pub kind: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeverVote {
    pub item_key: String,
    pub user_key: String,
    pub title: Option<String>,
    pub year: Option<i32>,
    pub poster: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoteRow {
    pub item_key: String,
    pub user_key: String,
    pub vote: MyVote,
    pub fulfilled_at: Option<i64>,
}

impl VoteRow {
    /// Still asking to keep (not fulfilled, not expired). "Don't care" never protects.
    pub fn active_keep(&self, now: i64) -> bool {
        self.fulfilled_at.is_none()
            && match self.vote.kind {
                VoteKind::DontCare => false,
                VoteKind::KeepDays => self.vote.until.is_some_and(|u| u > now),
                VoteKind::KeepUntilWatched | VoteKind::KeepForever => true,
            }
    }
}

pub struct Db(Mutex<Connection>);

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let c = Connection::open(path)?;
        c.execute_batch(SCHEMA)?;
        migrate(&c)?;
        Ok(Self(Mutex::new(c)))
    }

    #[cfg(test)]
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let c = Connection::open_in_memory()?;
        c.execute_batch(SCHEMA)?;
        migrate(&c)?;
        Ok(Self(Mutex::new(c)))
    }

    #[cfg(test)]
    pub fn exec(&self, sql: &str) {
        self.c().execute_batch(sql).unwrap();
    }

    /// Every audit log line (actor action detail), oldest first.
    #[cfg(test)]
    pub fn log_lines(&self) -> Vec<String> {
        let c = self.c();
        let mut st = c
            .prepare("SELECT actor || ' ' || action || ' ' || detail FROM log ORDER BY id")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    pub(crate) fn c(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ----- Seerr bridge ------------------------------------------------------------------------

    pub fn seerr_link(&self, user_key: &str) -> rusqlite::Result<Option<i64>> {
        self.c()
            .query_row(
                "SELECT seerr_user_id FROM seerr_links WHERE user_key = ?1",
                [user_key],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn set_seerr_link(
        &self,
        user_key: &str,
        seerr_id: i64,
        created: bool,
    ) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT OR REPLACE INTO seerr_links(user_key, seerr_user_id, created) VALUES (?1, ?2, ?3)",
            params![user_key, seerr_id, created],
        )?;
        Ok(())
    }

    pub fn forget_seerr_link(&self, user_key: &str) -> rusqlite::Result<()> {
        self.c()
            .execute("DELETE FROM seerr_links WHERE user_key = ?1", [user_key])?;
        Ok(())
    }

    // ----- users and rights -----------------------------------------------------------------

    /// Notes a sign-in (`name` is the media-server name; a chosen display name stays) and
    /// returns the user's profile.
    pub fn touch_user(
        &self,
        user_key: &str,
        name: &str,
        server_id: &str,
        is_admin: bool,
    ) -> rusqlite::Result<Profile> {
        self.c().execute(
            "INSERT INTO users(user_key, name, server_id, is_admin, last_seen) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(user_key) DO UPDATE SET name = excluded.name, is_admin = excluded.is_admin, last_seen = excluded.last_seen",
            params![user_key, name, server_id, is_admin, now()],
        )?;
        Ok(self.profile(user_key)?.unwrap_or_default())
    }

    pub fn profile(&self, user_key: &str) -> rusqlite::Result<Option<Profile>> {
        self.c()
            .query_row(
                &format!("SELECT {PROFILE_COLS} FROM users u WHERE u.user_key = ?1"),
                [user_key],
                |r| row_profile(r, 0),
            )
            .optional()
    }

    /// The one-time profile step is done (idempotent).
    pub fn mark_profile_set(&self, user_key: &str) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE users SET profile_set = 1 WHERE user_key = ?1",
            [user_key],
        )?;
        Ok(())
    }

    /// Sets (Some) or clears (None) the chosen display name.
    pub fn set_custom_name(&self, user_key: &str, name: Option<&str>) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE users SET custom_name = ?2 WHERE user_key = ?1",
            params![user_key, name],
        )?;
        Ok(())
    }

    /// A built-in avatar (Some), a custom picture of this type (`custom`), or the default
    /// (both None). Every change bumps the version.
    pub fn set_avatar(
        &self,
        user_key: &str,
        preset: Option<&str>,
        custom: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE users SET avatar_preset = ?2, avatar_type = ?3, avatar_version = avatar_version + 1
             WHERE user_key = ?1",
            params![user_key, preset, custom],
        )?;
        Ok(())
    }

    /// The custom avatar's content type and version, if any.
    pub fn avatar_custom(&self, user_key: &str) -> rusqlite::Result<Option<(String, u32)>> {
        Ok(self
            .c()
            .query_row(
                "SELECT avatar_type, avatar_version FROM users WHERE user_key = ?1",
                [user_key],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get(1)?)),
            )
            .optional()?
            .and_then(|(t, v)| t.map(|t| (t, v))))
    }

    pub fn rights(&self, user_key: &str) -> rusqlite::Result<Rights> {
        Ok(self
            .c()
            .query_row(
                "SELECT can_keep_forever, max_forever, max_keep_days FROM rights WHERE user_key = ?1",
                [user_key],
                |r| {
                    Ok(Rights {
                        can_keep_forever: r.get(0)?,
                        max_forever: r.get(1)?,
                        max_keep_days: r.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn set_rights(&self, user_key: &str, r: &Rights) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO rights(user_key, can_keep_forever, max_forever, max_keep_days) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_key) DO UPDATE SET can_keep_forever = excluded.can_keep_forever,
               max_forever = excluded.max_forever, max_keep_days = excluded.max_keep_days",
            params![user_key, r.can_keep_forever, r.max_forever, r.max_keep_days],
        )?;
        Ok(())
    }

    /// A user's settings shared across their devices (values are opaque to the server).
    pub fn settings(&self, user_key: &str) -> rusqlite::Result<Vec<(String, String)>> {
        let c = self.c();
        let mut st = c.prepare("SELECT key, value FROM settings WHERE user_key = ?1")?;
        st.query_map([user_key], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }

    pub fn settings_count(&self, user_key: &str) -> rusqlite::Result<u32> {
        self.c().query_row(
            "SELECT COUNT(*) FROM settings WHERE user_key = ?1",
            [user_key],
            |r| r.get(0),
        )
    }

    /// Upserts some of a user's settings (one transaction).
    pub fn set_settings(
        &self,
        user_key: &str,
        values: &[(String, String)],
    ) -> rusqlite::Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for (k, v) in values {
            tx.execute(
                "INSERT INTO settings(user_key, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_key, key) DO UPDATE SET value = excluded.value",
                params![user_key, k, v],
            )?;
        }
        tx.commit()
    }

    pub fn forever_count(&self, user_key: &str) -> rusqlite::Result<u32> {
        self.c().query_row(
            "SELECT COUNT(*) FROM votes WHERE user_key = ?1 AND kind = 'keep_forever' AND fulfilled_at IS NULL",
            [user_key],
            |r| r.get(0),
        )
    }

    pub fn users(&self) -> rusqlite::Result<Vec<AdminUser>> {
        let rows: Vec<(String, String, bool, i64, Profile)> = {
            let c = self.c();
            let mut st = c.prepare(&format!(
                "SELECT u.user_key, u.server_id, u.is_admin, u.last_seen, {PROFILE_COLS} FROM users u
                 ORDER BY COALESCE(u.custom_name, u.name)"
            ))?;
            st.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    row_profile(r, 4)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        rows.into_iter()
            .map(|(k, server_id, is_admin, last_seen, p)| {
                Ok(AdminUser {
                    rights: self.rights(&k)?,
                    forever_count: self.forever_count(&k)?,
                    seerr_user_id: self.seerr_link(&k)?,
                    user_key: k,
                    name: p.name,
                    media_name: p.media_name,
                    avatar: p.avatar,
                    server_id,
                    is_admin,
                    last_seen,
                })
            })
            .collect()
    }

    /// Active keep-forever votes as (item, user, item title, year, poster), for the admin stats.
    pub fn forever_votes(&self) -> rusqlite::Result<Vec<ForeverVote>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT v.item_key, v.user_key, i.title, i.year, i.poster FROM votes v
             LEFT JOIN items i ON i.key = v.item_key
             WHERE v.kind = 'keep_forever' AND v.fulfilled_at IS NULL",
        )?;
        st.query_map([], |r| {
            Ok(ForeverVote {
                item_key: r.get(0)?,
                user_key: r.get(1)?,
                title: r.get(2)?,
                year: r.get(3)?,
                poster: r.get(4)?,
            })
        })?
        .collect()
    }

    pub fn user_exists(&self, user_key: &str) -> rusqlite::Result<bool> {
        Ok(self
            .c()
            .query_row(
                "SELECT 1 FROM users WHERE user_key = ?1",
                [user_key],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// A known user's profile and server (to invite them).
    pub fn user_profile(&self, user_key: &str) -> rusqlite::Result<Option<(Profile, String)>> {
        self.c()
            .query_row(
                &format!("SELECT {PROFILE_COLS}, u.server_id FROM users u WHERE u.user_key = ?1"),
                [user_key],
                |r| Ok((row_profile(r, 0)?, r.get(7)?)),
            )
            .optional()
    }

    // ----- linked identities ----------------------------------------------------------------

    /// Notes a validated sign-in and returns its person (canonical user key) with every
    /// identity of that person. A first sign-in is its own one-identity person.
    pub fn sign_in(&self, s: &SignIn<'_>) -> rusqlite::Result<(String, Vec<IdentityRow>)> {
        let c = self.c();
        let now = now();
        c.execute(
            "INSERT INTO identities(user_key, person, server_id, kind, user_id, media_name, is_admin,
                                    last_seen, linked_at)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(user_key) DO UPDATE SET media_name = excluded.media_name,
               is_admin = excluded.is_admin, last_seen = excluded.last_seen",
            params![
                s.user_key,
                s.server_id,
                kind_name(s.kind),
                s.user_id,
                s.media_name,
                s.is_admin,
                now
            ],
        )?;
        let person: String = c.query_row(
            "SELECT person FROM identities WHERE user_key = ?1",
            [s.user_key],
            |r| r.get(0),
        )?;
        let ids = identities_in(&c, &person)?;
        Ok((person, ids))
    }

    /// Notes a request of `person` made with `signed_in` and returns its profile. Signed in
    /// with the canonical identity, its media-server name is stored (as `touch_user`); with an
    /// alias, the canonical's name stays.
    pub fn touch_person(
        &self,
        person: &str,
        signed_in: &SignIn<'_>,
        is_admin: bool,
    ) -> rusqlite::Result<Profile> {
        if person == signed_in.user_key {
            return self.touch_user(person, signed_in.media_name, signed_in.server_id, is_admin);
        }
        let n = self.c().execute(
            "UPDATE users SET is_admin = ?2, last_seen = ?3 WHERE user_key = ?1",
            params![person, is_admin, now()],
        )?;
        if n == 0 {
            // Should not happen (a person always has its row); rebuild it from the canonical
            // identity rather than failing the request.
            let c = self.c();
            let (name, server): (String, String) = c
                .query_row(
                    "SELECT media_name, server_id FROM identities WHERE user_key = ?1",
                    [person],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .unwrap_or_else(|| (signed_in.media_name.into(), signed_in.server_id.into()));
            c.execute(
                "INSERT OR IGNORE INTO users(user_key, name, server_id, is_admin, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![person, name, server, is_admin, now()],
            )?;
        }
        Ok(self.profile(person)?.unwrap_or_default())
    }

    /// The person a user key belongs to: itself when it is a person or unknown.
    pub fn person_of(&self, user_key: &str) -> rusqlite::Result<String> {
        Ok(self
            .c()
            .query_row(
                "SELECT person FROM identities WHERE user_key = ?1",
                [user_key],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| user_key.to_owned()))
    }

    /// Every identity of a person, oldest link first.
    pub fn identities_of(&self, person: &str) -> rusqlite::Result<Vec<IdentityRow>> {
        identities_in(&self.c(), person)
    }

    /// Every identity, by person.
    pub fn all_identities(
        &self,
    ) -> rusqlite::Result<std::collections::HashMap<String, Vec<IdentityRow>>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {IDENTITY_COLS} FROM identities ORDER BY linked_at, user_key"
        ))?;
        let mut out: std::collections::HashMap<String, Vec<IdentityRow>> = Default::default();
        for r in st.query_map([], row_identity)? {
            let r = r?;
            out.entry(r.person.clone()).or_default().push(r);
        }
        Ok(out)
    }

    /// Makes `b` (a sign-in just validated, noted like a sign-in) one person with `a_person`,
    /// in one transaction. Both persons' data move to the canonical key chosen by
    /// `preferred_canonical` among the identities of the merged person (first match in the
    /// order: `a_person`, `b`'s person, the other identities; else `a_person`). Merge rules:
    /// docs/keep-protocol.md, "Linked identities". Already one person: nothing changes.
    pub fn link(
        &self,
        a_person: &str,
        b: &SignIn<'_>,
        preferred_canonical: &dyn Fn(&IdentityRow) -> bool,
    ) -> Result<LinkOutcome, LinkError> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let now = now();
        tx.execute(
            "INSERT INTO identities(user_key, person, server_id, kind, user_id, media_name, is_admin,
                                    last_seen, linked_at)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(user_key) DO UPDATE SET media_name = excluded.media_name,
               is_admin = excluded.is_admin, last_seen = excluded.last_seen",
            params![
                b.user_key,
                b.server_id,
                kind_name(b.kind),
                b.user_id,
                b.media_name,
                b.is_admin,
                now
            ],
        )?;
        let b_person: String = tx.query_row(
            "SELECT person FROM identities WHERE user_key = ?1",
            [b.user_key],
            |r| r.get(0),
        )?;
        if b_person == a_person {
            return Ok(LinkOutcome {
                canonical: a_person.to_owned(),
                ..Default::default()
            });
        }
        let a_ids = identities_in(&tx, a_person)?;
        let b_ids = identities_in(&tx, &b_person)?;
        if let Some(clash) = a_ids
            .iter()
            .find(|x| b_ids.iter().any(|y| y.server_id == x.server_id))
        {
            return Err(LinkError::SameServer(clash.server_id.clone()));
        }
        let first = |k: &str| a_ids.iter().chain(&b_ids).find(|i| i.user_key == k);
        let canonical = [a_person, b_person.as_str()]
            .into_iter()
            .filter_map(first)
            .chain(a_ids.iter().chain(&b_ids))
            .find(|i| preferred_canonical(i))
            .map(|i| i.user_key.clone())
            .unwrap_or_else(|| a_person.to_owned());
        let canon_id = first(&canonical).cloned();
        let olds = [a_person.to_owned(), b_person.clone()];

        // Profile: A's choices win, B fills what A never chose.
        let ua = user_row(&tx, a_person)?.unwrap_or_default();
        let ub = user_row(&tx, &b_person)?.unwrap_or_else(|| UserRow {
            name: b.media_name.into(),
            server_id: b.server_id.into(),
            is_admin: b.is_admin,
            last_seen: now,
            ..Default::default()
        });
        let a_has_avatar = ua.avatar_preset.is_some() || ua.avatar_type.is_some();
        let (avatar_src, preset, avatar_type) = if a_has_avatar {
            (a_person, ua.avatar_preset.clone(), ua.avatar_type.clone())
        } else {
            (
                b_person.as_str(),
                ub.avatar_preset.clone(),
                ub.avatar_type.clone(),
            )
        };
        let avatar_from =
            (avatar_type.is_some() && avatar_src != canonical).then(|| avatar_src.to_owned());
        let (name, server_id) = match &canon_id {
            Some(i) => (i.media_name.clone(), i.server_id.clone()),
            None => (ua.name.clone(), ua.server_id.clone()),
        };
        for k in &olds {
            tx.execute("DELETE FROM users WHERE user_key = ?1", [k])?;
        }
        tx.execute(
            "INSERT INTO users(user_key, name, server_id, is_admin, last_seen, custom_name,
                               avatar_preset, avatar_type, avatar_version, profile_set)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                canonical,
                name,
                server_id,
                ua.is_admin || ub.is_admin,
                ua.last_seen.max(ub.last_seen),
                ua.custom_name.clone().or(ub.custom_name.clone()),
                preset,
                avatar_type,
                // Above both: a cached (user key, version) never shows the wrong picture.
                ua.avatar_version.max(ub.avatar_version) + 1,
                ua.profile_set || ub.profile_set,
            ],
        )?;

        // Rights: the more permissive of two explicit choices; one explicit choice stays.
        let rights_of = |k: &str| -> rusqlite::Result<Option<Rights>> {
            tx.query_row(
                "SELECT can_keep_forever, max_forever, max_keep_days FROM rights WHERE user_key = ?1",
                [k],
                |r| {
                    Ok(Rights {
                        can_keep_forever: r.get(0)?,
                        max_forever: r.get(1)?,
                        max_keep_days: r.get(2)?,
                    })
                },
            )
            .optional()
        };
        let rights = merge_rights(rights_of(a_person)?, rights_of(&b_person)?);
        for k in &olds {
            tx.execute("DELETE FROM rights WHERE user_key = ?1", [k])?;
        }
        if let Some(r) = rights {
            tx.execute(
                "INSERT INTO rights(user_key, can_keep_forever, max_forever, max_keep_days)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    canonical,
                    r.can_keep_forever,
                    r.max_forever,
                    r.max_keep_days
                ],
            )?;
        }

        // Votes: one per item, the stronger one (A's on a tie).
        let mut votes: std::collections::BTreeMap<String, VoteRow> = Default::default();
        for v in votes_of(&tx, a_person)?
            .into_iter()
            .chain(votes_of(&tx, &b_person)?)
        {
            match votes.get(&v.item_key) {
                Some(have) if vote_strength(have, now) >= vote_strength(&v, now) => {}
                _ => {
                    votes.insert(v.item_key.clone(), v);
                }
            }
        }
        for k in &olds {
            tx.execute("DELETE FROM votes WHERE user_key = ?1", [k])?;
        }
        for v in votes.values() {
            tx.execute(
                "INSERT INTO votes(item_key, user_key, kind, until_ts, reminder_days, reason,
                                   created_at, fulfilled_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    v.item_key,
                    canonical,
                    kind_str(v.vote.kind),
                    v.vote.until,
                    v.vote.reminder_days,
                    v.vote.reason,
                    v.vote.created_at,
                    v.fulfilled_at
                ],
            )?;
        }

        // Seerr: a Seerr user that existed (the person's real account, with their requests)
        // beats one the bridge created; then A's.
        let seerr_of = |k: &str| -> rusqlite::Result<Option<(i64, bool)>> {
            tx.query_row(
                "SELECT seerr_user_id, created FROM seerr_links WHERE user_key = ?1",
                [k],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        };
        let seerr = match (seerr_of(a_person)?, seerr_of(&b_person)?) {
            (Some(a), Some(b)) if a.1 && !b.1 => Some(b),
            (a, b) => a.or(b),
        };
        for k in &olds {
            tx.execute("DELETE FROM seerr_links WHERE user_key = ?1", [k])?;
        }
        if let Some((id, created)) = seerr {
            tx.execute(
                "INSERT INTO seerr_links(user_key, seerr_user_id, created) VALUES (?1, ?2, ?3)",
                params![canonical, id, created],
            )?;
        }

        // Settings: union, newer value per key (A's on a tie or without times).
        let settings_of = |k: &str| -> rusqlite::Result<Vec<(String, String)>> {
            let mut st = tx.prepare("SELECT key, value FROM settings WHERE user_key = ?1")?;
            st.query_map([k], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        };
        let mut settings: std::collections::BTreeMap<String, String> =
            settings_of(a_person)?.into_iter().collect();
        for (k, v) in settings_of(&b_person)? {
            let newer = settings
                .get(&k)
                .is_none_or(|have| setting_time(&v) > setting_time(have));
            if newer {
                settings.insert(k, v);
            }
        }
        for k in &olds {
            tx.execute("DELETE FROM settings WHERE user_key = ?1", [k])?;
        }
        for (k, v) in &settings {
            tx.execute(
                "INSERT INTO settings(user_key, key, value) VALUES (?1, ?2, ?3)",
                params![canonical, k, v],
            )?;
        }

        // Watch together history: re-pointed, latest time per pair, no pair with oneself.
        let pairs: Vec<(String, String, i64)> = {
            let mut st = tx.prepare(
                "SELECT a, b, last_at FROM together_pairs WHERE a IN (?1, ?2) OR b IN (?1, ?2)",
            )?;
            st.query_map(params![a_person, b_person], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        tx.execute(
            "DELETE FROM together_pairs WHERE a IN (?1, ?2) OR b IN (?1, ?2)",
            params![a_person, b_person],
        )?;
        let map = |k: String| {
            if olds.contains(&k) {
                canonical.clone()
            } else {
                k
            }
        };
        for (a, b, at) in pairs {
            let (a, b) = (map(a), map(b));
            if a == b {
                continue;
            }
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            tx.execute(
                "INSERT INTO together_pairs(a, b, last_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(a, b) DO UPDATE SET last_at = MAX(last_at, excluded.last_at)",
                params![a, b, at],
            )?;
        }

        tx.execute(
            "UPDATE identities SET person = ?1 WHERE person IN (?2, ?3)",
            params![canonical, a_person, b_person],
        )?;
        // AlwyWatch accounts (password, passkeys, Pocket ID) follow the person.
        tx.execute(
            "UPDATE accounts SET person = ?1 WHERE person IN (?2, ?3)",
            params![canonical, a_person, b_person],
        )?;
        tx.execute(
            "UPDATE identities SET linked_at = ?2 WHERE user_key = ?1",
            params![b.user_key, now],
        )?;
        tx.commit()?;
        Ok(LinkOutcome {
            merged: olds.into_iter().filter(|k| *k != canonical).collect(),
            canonical,
            avatar_from,
        })
    }

    /// `user_key` leaves `person` and becomes its own account again, with no data.
    pub fn unlink(&self, person: &str, user_key: &str) -> Result<(), UnlinkError> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let id = tx
            .query_row(
                &format!("SELECT {IDENTITY_COLS} FROM identities WHERE user_key = ?1"),
                [user_key],
                row_identity,
            )
            .optional()?
            .filter(|i| i.person == person)
            .ok_or(UnlinkError::NotFound)?;
        if user_key == person {
            return Err(UnlinkError::Canonical);
        }
        for table in ["users", "rights", "votes", "seerr_links", "settings"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE user_key = ?1"),
                [user_key],
            )?;
        }
        tx.execute(
            "DELETE FROM together_pairs WHERE a = ?1 OR b = ?1",
            [user_key],
        )?;
        tx.execute(
            "UPDATE identities SET person = user_key, linked_at = ?2 WHERE user_key = ?1",
            params![user_key, now()],
        )?;
        tx.execute(
            "INSERT INTO users(user_key, name, server_id, is_admin, last_seen) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user_key, id.media_name, id.server_id, id.is_admin, id.last_seen],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ----- watch together history -----------------------------------------------------------

    /// Notes that these pairs of users (each `(a, b)` with `a < b`) were just in a started
    /// room together.
    pub fn record_together(&self, pairs: &[(String, String)], at: i64) -> rusqlite::Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for (a, b) in pairs {
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            tx.execute(
                "INSERT INTO together_pairs(a, b, last_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(a, b) DO UPDATE SET last_at = MAX(last_at, excluded.last_at)",
                params![a, b, at],
            )?;
        }
        tx.commit()
    }

    /// Everyone but `user_key`, with when they last watched together with `user_key`:
    /// `(user_key, server_id, last_together_at, profile)`, unordered.
    #[allow(clippy::type_complexity)]
    pub fn together_people(
        &self,
        user_key: &str,
    ) -> rusqlite::Result<Vec<(String, String, Option<i64>, Profile)>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT u.user_key, u.server_id, p.last_at, {PROFILE_COLS} FROM users u
             LEFT JOIN together_pairs p
               ON (p.a = ?1 AND p.b = u.user_key) OR (p.b = ?1 AND p.a = u.user_key)
             WHERE u.user_key != ?1"
        ))?;
        st.query_map([user_key], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, row_profile(r, 3)?))
        })?
        .collect()
    }

    // ----- items ----------------------------------------------------------------------------

    /// Marks every item as out of collection before a sync re-adds the current ones.
    pub fn begin_sync(&self) -> rusqlite::Result<()> {
        self.c().execute("UPDATE items SET in_collection = 0", [])?;
        Ok(())
    }

    pub fn upsert_collection_item(&self, i: &NewItem<'_>) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO items(key, collection_id, collection_title, collection_active, in_collection, deletion_date,
                               poster, tmdb, tvdb, kind, last_seen)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(key) DO UPDATE SET collection_id = excluded.collection_id,
               collection_title = excluded.collection_title, collection_active = excluded.collection_active,
               in_collection = 1, deletion_date = excluded.deletion_date,
               poster = COALESCE(excluded.poster, poster), tmdb = COALESCE(excluded.tmdb, tmdb),
               tvdb = COALESCE(excluded.tvdb, tvdb), kind = excluded.kind, last_seen = excluded.last_seen",
            params![i.key, i.collection_id, i.collection_title, i.active, i.deletion_date, i.poster, i.tmdb, i.tvdb, i.kind, now()],
        )?;
        Ok(())
    }

    pub fn set_meta(
        &self,
        key: &str,
        title: &str,
        year: Option<i32>,
        tmdb: Option<String>,
        tvdb: Option<String>,
    ) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE items SET title = ?2, year = ?3, tmdb = COALESCE(?4, tmdb), tvdb = COALESCE(?5, tvdb), meta_at = ?6 WHERE key = ?1",
            params![key, title, year, tmdb, tvdb, now()],
        )?;
        Ok(())
    }

    pub fn set_deletion_date(&self, key: &str, date: Option<i64>) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE items SET deletion_date = ?2 WHERE key = ?1",
            params![key, date],
        )?;
        Ok(())
    }

    pub fn items_missing_meta(&self, limit: u32) -> rusqlite::Result<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT key FROM items WHERE meta_at IS NULL ORDER BY deletion_date LIMIT ?1",
        )?;
        st.query_map([limit], |r| r.get(0))?.collect()
    }

    /// Items to show: everything in a Maintainerr collection plus anything someone still keeps.
    /// What Leaving soon answers to `user`: the titles in a deletion list, those someone
    /// keeps, and those with a choice of their own (also "I don't care" on a title in no list:
    /// its page shows the choice).
    pub fn visible_items(&self, user: &str) -> rusqlite::Result<Vec<ItemRow>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT key, collection_id, collection_title, collection_active, in_collection, title, year, kind, poster,
                    tmdb, tvdb, deletion_date, meta_at
             FROM items i
             WHERE in_collection = 1 OR EXISTS (
                SELECT 1 FROM votes v WHERE v.item_key = i.key AND v.fulfilled_at IS NULL
                  AND (v.kind != 'dont_care' OR v.user_key = ?1))",
        )?;
        st.query_map([user], row_item)?.collect()
    }

    pub fn item(&self, key: &str) -> rusqlite::Result<Option<ItemRow>> {
        self.c()
            .query_row(
                "SELECT key, collection_id, collection_title, collection_active, in_collection, title, year, kind, poster,
                        tmdb, tvdb, deletion_date, meta_at FROM items WHERE key = ?1",
                [key],
                row_item,
            )
            .optional()
    }

    /// The item of a title known by its provider ids (TVDB first for shows, like the mapper).
    pub fn item_by_ids(
        &self,
        kind: &str,
        tmdb: Option<&str>,
        tvdb: Option<&str>,
    ) -> rusqlite::Result<Option<ItemRow>> {
        let by = |column: &str, id: Option<&str>| -> rusqlite::Result<Option<ItemRow>> {
            let Some(id) = id.filter(|v| !v.is_empty()) else {
                return Ok(None);
            };
            self.c()
                .query_row(
                    &format!(
                        "SELECT key, collection_id, collection_title, collection_active, in_collection, title, year, kind, poster,
                                tmdb, tvdb, deletion_date, meta_at FROM items WHERE kind = ?1 AND {column} = ?2
                         ORDER BY in_collection DESC LIMIT 1"
                    ),
                    params![kind, id],
                    row_item,
                )
                .optional()
        };
        let (first, second) = if kind == "show" {
            (("tvdb", tvdb), ("tmdb", tmdb))
        } else {
            (("tmdb", tmdb), ("tvdb", tvdb))
        };
        match by(first.0, first.1)? {
            Some(row) => Ok(Some(row)),
            None => by(second.0, second.1),
        }
    }

    /// Remembers a library title that is in no collection, so it can be voted on. An item
    /// already known is left as it is.
    pub fn adopt_item(
        &self,
        key: &str,
        kind: &str,
        title: &str,
        year: Option<i32>,
        tmdb: Option<&str>,
        tvdb: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO items(key, in_collection, collection_active, title, year, kind, tmdb, tvdb, meta_at, last_seen)
             VALUES (?1, 0, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(key) DO NOTHING",
            params![key, title, year, kind, tmdb, tvdb, now()],
        )?;
        Ok(())
    }

    // ----- votes ----------------------------------------------------------------------------

    pub fn set_vote(&self, item: &str, user: &str, v: &MyVote) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO votes(item_key, user_key, kind, until_ts, reminder_days, reason, created_at, fulfilled_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)
             ON CONFLICT(item_key, user_key) DO UPDATE SET kind = excluded.kind, until_ts = excluded.until_ts,
               reminder_days = excluded.reminder_days, reason = excluded.reason, created_at = excluded.created_at,
               fulfilled_at = NULL",
            params![item, user, kind_str(v.kind), v.until, v.reminder_days, v.reason, v.created_at],
        )?;
        Ok(())
    }

    pub fn delete_vote(&self, item: &str, user: &str) -> rusqlite::Result<bool> {
        Ok(self.c().execute(
            "DELETE FROM votes WHERE item_key = ?1 AND user_key = ?2",
            params![item, user],
        )? > 0)
    }

    pub fn fulfill(&self, item: &str, user: &str) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE votes SET fulfilled_at = ?3 WHERE item_key = ?1 AND user_key = ?2",
            params![item, user, now()],
        )?;
        Ok(())
    }

    pub fn votes(&self) -> rusqlite::Result<Vec<VoteRow>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT item_key, user_key, kind, until_ts, reminder_days, reason, created_at, fulfilled_at FROM votes",
        )?;
        st.query_map([], row_vote)?.collect()
    }

    // ----- applied state and log ------------------------------------------------------------

    pub fn excluded(&self, item: &str) -> rusqlite::Result<bool> {
        Ok(self
            .c()
            .query_row(
                "SELECT excluded FROM applied WHERE item_key = ?1",
                [item],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn set_excluded(&self, item: &str, excluded: bool) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO applied(item_key, excluded, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(item_key) DO UPDATE SET excluded = excluded.excluded, updated_at = excluded.updated_at",
            params![item, excluded, now()],
        )?;
        Ok(())
    }

    pub fn excluded_items(&self) -> rusqlite::Result<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare("SELECT item_key FROM applied WHERE excluded = 1")?;
        st.query_map([], |r| r.get(0))?.collect()
    }

    pub fn log(&self, actor: &str, action: &str, detail: &str) {
        let _ = self.c().execute(
            "INSERT INTO log(ts, actor, action, detail) VALUES (?1, ?2, ?3, ?4)",
            params![now(), actor, action, detail],
        );
        eprintln!("[keep] {actor} {action} {detail}");
    }
}

fn row_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok(ItemRow {
        key: r.get(0)?,
        collection_id: r.get(1)?,
        collection_title: r.get(2)?,
        collection_active: r.get(3)?,
        in_collection: r.get(4)?,
        title: r.get(5)?,
        year: r.get(6)?,
        kind: r.get(7)?,
        poster: r.get(8)?,
        tmdb: r.get(9)?,
        tvdb: r.get(10)?,
        deletion_date: r.get(11)?,
        meta_at: r.get(12)?,
    })
}

/// Effective state of one item from all votes on it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Desired {
    pub forever: bool,
    pub until_watched: bool,
    /// Latest "keep N days" target among active votes.
    pub extend_until: Option<i64>,
    pub counts: VoteCounts,
}

impl Desired {
    pub fn from_votes<'a>(votes: impl Iterator<Item = &'a VoteRow>, now: i64) -> Self {
        let mut d = Desired::default();
        for v in votes {
            match v.vote.kind {
                VoteKind::DontCare => d.counts.dont_care += 1,
                _ if !v.active_keep(now) => {}
                VoteKind::KeepForever => {
                    d.forever = true;
                    d.counts.forever += 1;
                }
                VoteKind::KeepUntilWatched => {
                    d.until_watched = true;
                    d.counts.until_watched += 1;
                }
                VoteKind::KeepDays => {
                    d.counts.days += 1;
                    d.extend_until = d.extend_until.max(v.vote.until);
                }
            }
        }
        d
    }

    pub fn needs_exclusion(&self) -> bool {
        self.forever || self.until_watched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vote(kind: VoteKind, until: Option<i64>, fulfilled: bool) -> VoteRow {
        VoteRow {
            item_key: "1".into(),
            user_key: "u".into(),
            vote: MyVote {
                kind,
                until,
                reminder_days: None,
                reason: None,
                created_at: 0,
            },
            fulfilled_at: fulfilled.then_some(1),
        }
    }

    #[test]
    fn resolution_forever_beats_watched_beats_days_and_longest_days_win() {
        let now = 1000;
        let votes = [
            vote(VoteKind::KeepDays, Some(2000), false),
            vote(VoteKind::KeepDays, Some(5000), false),
            vote(VoteKind::KeepDays, Some(500), false), // expired
            vote(VoteKind::DontCare, None, false),
        ];
        let d = Desired::from_votes(votes.iter(), now);
        assert!(!d.needs_exclusion());
        assert_eq!(d.extend_until, Some(5000));
        assert_eq!(d.counts.days, 2);
        assert_eq!(d.counts.dont_care, 1);

        let watched = [
            vote(VoteKind::KeepUntilWatched, None, true),
            vote(VoteKind::KeepDays, Some(3000), false),
        ];
        let d = Desired::from_votes(watched.iter(), now);
        assert!(
            !d.until_watched,
            "a fulfilled until-watched vote no longer protects"
        );
        assert_eq!(d.extend_until, Some(3000));

        let forever = [
            vote(VoteKind::KeepForever, None, false),
            vote(VoteKind::KeepUntilWatched, None, false),
        ];
        let d = Desired::from_votes(forever.iter(), now);
        assert!(d.forever && d.until_watched && d.needs_exclusion());
    }

    #[test]
    fn older_databases_get_the_profile_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE users (user_key TEXT PRIMARY KEY, name TEXT NOT NULL, server_id TEXT NOT NULL,
                 is_admin INTEGER NOT NULL DEFAULT 0, last_seen INTEGER NOT NULL);
                 INSERT INTO users VALUES ('u', 'Ann', 's', 0, 1);",
            )
            .unwrap();
        let db = Db::open(&path).unwrap();
        let p = db.profile("u").unwrap().unwrap();
        assert_eq!(
            (p.name.as_str(), p.name_is_custom, p.profile_set),
            ("Ann", false, false)
        );
        db.set_custom_name("u", Some("Annie")).unwrap();
        // A sign-in with the media server's name keeps the chosen one.
        let p = db.touch_user("u", "Ann", "s", false).unwrap();
        assert_eq!((p.name.as_str(), p.media_name.as_str()), ("Annie", "Ann"));
        drop(db);
        assert!(Db::open(&path).is_ok(), "opening again is a no-op");
    }

    #[test]
    fn votes_are_per_user_and_replace_on_update() {
        let db = Db::open_in_memory().unwrap();
        let v = |kind| MyVote {
            kind,
            until: None,
            reminder_days: None,
            reason: None,
            created_at: 1,
        };
        db.set_vote("i", "alice", &v(VoteKind::KeepForever))
            .unwrap();
        db.set_vote("i", "bob", &v(VoteKind::KeepForever)).unwrap();
        assert_eq!(db.forever_count("alice").unwrap(), 1);
        db.set_vote("i", "alice", &v(VoteKind::DontCare)).unwrap();
        assert_eq!(db.forever_count("alice").unwrap(), 0);
        let all = db.votes().unwrap();
        let d = Desired::from_votes(all.iter(), 0);
        assert!(d.forever, "bob still keeps it after alice changed her mind");
        assert!(db.delete_vote("i", "bob").unwrap());
        assert!(!Desired::from_votes(db.votes().unwrap().iter(), 0).forever);
    }
}
