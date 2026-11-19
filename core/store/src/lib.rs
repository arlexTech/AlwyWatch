// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local persistence for the core.
//!
//! * A JSON cache keyed by string, with the time each entry was fetched, used by the
//!   cache-first / stale-while-revalidate repositories.
//! * Accounts (server, user, never tokens), settings and UI state (scroll positions, last screen).
//! * A queue of playback progress reports made while offline.
//! * An on-disk image cache with a size cap and LRU eviction.
//!
//! One SQLite connection in WAL mode behind a mutex: every statement here is a primary-key
//! lookup or a small write, well under a millisecond.

mod images;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use images::ImageCache;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt cache entry: {0}")]
    Decode(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A cached value and when it was fetched (unix seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Cached<T> {
    pub value: T,
    pub fetched_at: i64,
}

impl<T> Cached<T> {
    pub fn age_secs(&self) -> i64 {
        now() - self.fetched_at
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const SCHEMA_VERSION: i32 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS cache (
    key TEXT PRIMARY KEY,
    value BLOB NOT NULL,
    fetched_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    value BLOB NOT NULL,
    position INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS pending_progress (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL,
    value BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS images (
    key TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    last_access INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS images_lru ON images(last_access);
";

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens (or creates) the database. A corrupt or incompatible cache database is recreated:
    /// it only holds data that can be fetched again (accounts are re-added by sign-in).
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = match Self::open_conn(path) {
            Ok(c) => c,
            Err(_) => {
                let _ = std::fs::remove_file(path);
                Self::open_conn(path)?
            }
        };
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn open_conn(path: &Path) -> Result<Connection> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY;",
        )?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(Error::Db(rusqlite::Error::InvalidQuery));
        }
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))?;
        Ok(conn)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock cannot leave SQLite inconsistent; keep going.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ----- JSON cache ------------------------------------------------------------------------

    pub fn cache_get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<Cached<T>>> {
        let row: Option<(Vec<u8>, i64)> = self
            .conn()
            .query_row(
                "SELECT value, fetched_at FROM cache WHERE key = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((bytes, fetched_at)) => match serde_json::from_slice(&bytes) {
                Ok(value) => Ok(Some(Cached { value, fetched_at })),
                // Shape changed between app versions: treat as a miss and drop it.
                Err(_) => {
                    self.cache_delete(key)?;
                    Ok(None)
                }
            },
        }
    }

    pub fn cache_put<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        self.conn().execute(
            "INSERT INTO cache(key, value, fetched_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, fetched_at = excluded.fetched_at",
            params![key, bytes, now()],
        )?;
        Ok(())
    }

    /// Keeps the value (so screens still render instantly) but forces the next refresh.
    pub fn cache_mark_stale(&self, key: &str) -> Result<()> {
        self.conn()
            .execute("UPDATE cache SET fetched_at = 0 WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn cache_delete(&self, key: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM cache WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Deletes every cache entry whose key starts with `prefix` (e.g. one account's data).
    pub fn cache_delete_prefix(&self, prefix: &str) -> Result<usize> {
        // Range scan on the primary key instead of LIKE (no escaping issues, uses the index).
        let mut end = prefix.to_owned();
        end.push(char::MAX);
        Ok(self.conn().execute(
            "DELETE FROM cache WHERE key >= ?1 AND key < ?2",
            params![prefix, end],
        )?)
    }

    // ----- accounts --------------------------------------------------------------------------

    pub fn accounts<T: DeserializeOwned>(&self) -> Result<Vec<T>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT value FROM accounts ORDER BY position, id")?;
        let rows = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        let mut out = Vec::new();
        for bytes in rows {
            if let Ok(v) = serde_json::from_slice(&bytes?) {
                out.push(v);
            }
        }
        Ok(out)
    }

    pub fn put_account<T: Serialize>(&self, id: &str, account: &T) -> Result<()> {
        let bytes = serde_json::to_vec(account)?;
        let conn = self.conn();
        let next: i64 = conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM accounts",
            [],
            |r| r.get(0),
        )?;
        conn.execute(
            "INSERT INTO accounts(id, value, position) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET value = excluded.value",
            params![id, bytes, next],
        )?;
        Ok(())
    }

    pub fn remove_account(&self, id: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        conn.execute("DELETE FROM pending_progress WHERE account_id = ?1", [id])?;
        Ok(())
    }

    // ----- settings and UI state ------------------------------------------------------------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn remove_setting(&self, key: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    // ----- offline progress queue ----------------------------------------------------------

    pub fn queue_progress<T: Serialize>(&self, account_id: &str, report: &T) -> Result<()> {
        let bytes = serde_json::to_vec(report)?;
        self.conn().execute(
            "INSERT INTO pending_progress(account_id, value, created_at) VALUES (?1, ?2, ?3)",
            params![account_id, bytes, now()],
        )?;
        Ok(())
    }

    /// Oldest first: `(row id, account id, report)`.
    pub fn pending_progress<T: DeserializeOwned>(&self) -> Result<Vec<(i64, String, T)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, account_id, value FROM pending_progress ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, acct, bytes) = row?;
            if let Ok(v) = serde_json::from_slice(&bytes) {
                out.push((id, acct, v));
            }
        }
        Ok(out)
    }

    pub fn ack_progress(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM pending_progress WHERE id = ?1", [id])?;
        Ok(())
    }

    // ----- image index (used by ImageCache) --------------------------------------------------

    pub(crate) fn image_touch(&self, key: &str) -> Result<bool> {
        Ok(self.conn().execute(
            "UPDATE images SET last_access = ?2 WHERE key = ?1",
            params![key, now()],
        )? > 0)
    }

    pub(crate) fn image_insert(&self, key: &str, size: u64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO images(key, size, last_access) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET size = excluded.size, last_access = excluded.last_access",
            params![key, size as i64, now()],
        )?;
        Ok(())
    }

    pub(crate) fn image_total(&self) -> Result<u64> {
        let total: i64 =
            self.conn()
                .query_row("SELECT COALESCE(SUM(size), 0) FROM images", [], |r| {
                    r.get(0)
                })?;
        Ok(total as u64)
    }

    /// Removes and returns the least recently used entries until `bytes_to_free` is covered.
    pub(crate) fn image_evict(&self, bytes_to_free: u64) -> Result<Vec<(String, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT key, size FROM images ORDER BY last_access, key")?;
        let mut freed = 0u64;
        let mut victims = Vec::new();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            if freed >= bytes_to_free {
                break;
            }
            let (k, s) = row?;
            freed += s as u64;
            victims.push((k, s as u64));
        }
        drop(stmt);
        for (k, _) in &victims {
            conn.execute("DELETE FROM images WHERE key = ?1", [k])?;
        }
        Ok(victims)
    }

    pub(crate) fn image_remove(&self, key: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM images WHERE key = ?1", [key])?;
        Ok(())
    }
}

/// Stable 64-bit FNV-1a hash, used for file names (stable across Rust versions, unlike `Hash`).
pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub(crate) fn image_file(dir: &Path, key: &str) -> PathBuf {
    let h = format!("{:016x}", fnv1a(key));
    dir.join(&h[..2]).join(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Row {
        a: u32,
    }

    #[test]
    fn cache_roundtrip_and_prefix_delete() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.cache_get::<Row>("acct1:home").unwrap().is_none());
        s.cache_put("acct1:home", &Row { a: 1 }).unwrap();
        s.cache_put("acct1:item:x", &Row { a: 2 }).unwrap();
        s.cache_put("acct2:home", &Row { a: 3 }).unwrap();
        let c = s.cache_get::<Row>("acct1:home").unwrap().unwrap();
        assert_eq!(c.value, Row { a: 1 });
        assert!(c.age_secs() <= 1);
        assert_eq!(s.cache_delete_prefix("acct1:").unwrap(), 2);
        assert!(s.cache_get::<Row>("acct1:item:x").unwrap().is_none());
        assert!(s.cache_get::<Row>("acct2:home").unwrap().is_some());
    }

    #[test]
    fn shape_change_is_a_cache_miss() {
        let s = Store::open_in_memory().unwrap();
        s.cache_put("k", &"not a row").unwrap();
        assert!(s.cache_get::<Row>("k").unwrap().is_none());
        assert!(
            s.cache_get::<String>("k").unwrap().is_none(),
            "bad entry was dropped"
        );
    }

    #[test]
    fn accounts_keep_insertion_order() {
        let s = Store::open_in_memory().unwrap();
        s.put_account("b", &Row { a: 1 }).unwrap();
        s.put_account("a", &Row { a: 2 }).unwrap();
        s.put_account("b", &Row { a: 3 }).unwrap(); // update keeps position
        let rows: Vec<Row> = s.accounts().unwrap();
        assert_eq!(rows, vec![Row { a: 3 }, Row { a: 2 }]);
        s.remove_account("b").unwrap();
        assert_eq!(s.accounts::<Row>().unwrap().len(), 1);
    }

    #[test]
    fn settings_and_progress_queue() {
        let s = Store::open_in_memory().unwrap();
        s.set_setting("ui.last_screen", "home").unwrap();
        assert_eq!(
            s.setting("ui.last_screen").unwrap().as_deref(),
            Some("home")
        );
        s.remove_setting("ui.last_screen").unwrap();
        assert_eq!(s.setting("ui.last_screen").unwrap(), None);

        s.queue_progress("a", &Row { a: 1 }).unwrap();
        s.queue_progress("a", &Row { a: 2 }).unwrap();
        let q: Vec<(i64, String, Row)> = s.pending_progress().unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].2, Row { a: 1 });
        s.ack_progress(q[0].0).unwrap();
        assert_eq!(s.pending_progress::<Row>().unwrap().len(), 1);
    }

    #[test]
    fn reopen_file_database_and_recover_from_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("db/cache.sqlite");
        {
            let s = Store::open(&p).unwrap();
            s.set_setting("x", "1").unwrap();
        }
        assert_eq!(
            Store::open(&p).unwrap().setting("x").unwrap().as_deref(),
            Some("1")
        );
        std::fs::write(&p, b"this is not sqlite").unwrap();
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let s = Store::open(&p).unwrap();
        assert_eq!(s.setting("x").unwrap(), None, "garbage file was replaced");
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
