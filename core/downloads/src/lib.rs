// SPDX-License-Identifier: AGPL-3.0-or-later
//! Resumable downloads of original media files.
//!
//! Each download writes to `<name>.part` and persists its offset in SQLite after every fsync
//! (every 4 MiB and on pause). A resume:
//! 1. truncates the `.part` file to the last persisted offset (anything after it may not have
//!    reached the disk before a crash),
//! 2. sends `Range: bytes=<offset>-` with `If-Range: <ETag or Last-Modified>`,
//! 3. on `206` checks `Content-Range` starts at the offset and appends; on `200` the server says
//!    the file changed (or ignores ranges), so it restarts from zero.
//!
//! Transient failures (network loss, 5xx, 429) retry with capped exponential backoff and jitter.
//! A full disk pauses the download as [`DownloadState::PausedNoSpace`] and keeps the `.part`
//! file, so freeing space and resuming continues where it stopped.

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use net::reqwest::{Client, StatusCode, header};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const SYNC_EVERY: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum DownloadState {
    Queued,
    Running,
    Paused,
    /// Stopped because the disk is full; the `.part` file is kept.
    PausedNoSpace,
    Failed,
    Completed,
}

impl DownloadState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::PausedNoSpace => "no_space",
            Self::Failed => "failed",
            Self::Completed => "completed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "no_space" => Self::PausedNoSpace,
            "failed" => Self::Failed,
            "completed" => Self::Completed,
            _ => Self::Queued,
        }
    }
}

/// What to fetch. Auth headers are stored with the download so it can resume after a restart;
/// the database lives in the app's private storage, like the rest of the cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub account_id: String,
    pub item_id: String,
    pub title: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub file_name: String,
    pub expected_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Download {
    pub id: i64,
    pub account_id: String,
    pub item_id: String,
    pub title: String,
    pub file_path: String,
    pub state: DownloadState,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("download not found")]
    NotFound,
    #[error("disk full")]
    NoSpace,
    #[error("paused")]
    Paused,
    #[error("server refused the download (HTTP {0})")]
    Refused(u16),
    #[error("{0}")]
    Transient(String),
    #[error("{0}")]
    Fatal(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Fatal(format!("database: {e}"))
    }
}

fn io_err(e: io::Error) -> Error {
    if is_disk_full(&e) {
        Error::NoSpace
    } else {
        Error::Fatal(e.to_string())
    }
}

/// ENOSPC / EDQUOT on Unix, ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL on Windows.
pub fn is_disk_full(e: &io::Error) -> bool {
    if matches!(
        e.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    ) {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(28) | Some(122) => true,
        #[cfg(windows)]
        Some(112) | Some(39) => true,
        _ => false,
    }
}

/// Where downloaded bytes go. A trait so tests can simulate a full disk or a slow device.
pub trait Sink: Send {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    /// Makes everything written so far durable.
    fn sync(&mut self) -> io::Result<()>;
}

pub struct FileSink(pub std::fs::File);

impl Sink for FileSink {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        io::Write::write_all(&mut self.0, buf)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.0.sync_data()
    }
}

pub type SinkFactory = dyn Fn(&Path) -> io::Result<Box<dyn Sink>> + Send + Sync;

/// Retry schedule for transient failures.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub base: Duration,
    pub max: Duration,
    /// Consecutive failures before giving up (the user can resume a failed download).
    pub max_attempts: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            max: Duration::from_secs(300),
            max_attempts: 12,
        }
    }
}

impl RetryPolicy {
    /// Exponential backoff with +/-25% jitter (jitter from the clock, no RNG dependency).
    pub fn delay(&self, attempt: u32) -> Duration {
        let exp = self.base.saturating_mul(1u32 << attempt.min(16));
        let d = exp.min(self.max);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let jitter = (nanos % 500) as f64 / 1000.0 - 0.25; // [-0.25, 0.25)
        d.mul_f64(1.0 + jitter)
    }
}

pub struct Downloads {
    db: Mutex<Connection>,
    dir: PathBuf,
    http: Client,
    retry: RetryPolicy,
    sink: Box<SinkFactory>,
    /// Live byte counts of running downloads (the DB is only updated on fsync).
    live: Mutex<HashMap<i64, u64>>,
    /// Pause flags of running downloads.
    stop: Mutex<HashMap<i64, Arc<AtomicBool>>>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps a file name safe on every platform (no separators, no reserved characters).
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_matches('.').to_owned();
    if s.is_empty() { "download".into() } else { s }
}

struct Row {
    url: String,
    headers: Vec<(String, String)>,
    path: PathBuf,
    offset: u64,
    total: Option<u64>,
    validator: Option<String>,
    state: DownloadState,
}

impl Downloads {
    pub fn open(db_path: &Path, dir: &Path, http: Client) -> Result<Self> {
        Self::with_options(
            db_path,
            dir,
            http,
            RetryPolicy::default(),
            Box::new(|p: &Path| {
                let f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)?;
                Ok(Box::new(FileSink(f)) as Box<dyn Sink>)
            }),
        )
    }

    pub fn with_options(
        db_path: &Path,
        dir: &Path,
        http: Client,
        retry: RetryPolicy,
        sink: Box<SinkFactory>,
    ) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        if let Some(p) = db_path.parent() {
            std::fs::create_dir_all(p).map_err(io_err)?;
        }
        let db = Connection::open(db_path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                title TEXT NOT NULL,
                url TEXT NOT NULL,
                headers TEXT NOT NULL,
                path TEXT NOT NULL,
                offset INTEGER NOT NULL DEFAULT 0,
                total INTEGER,
                validator TEXT,
                state TEXT NOT NULL,
                error TEXT,
                created_at INTEGER NOT NULL,
                UNIQUE(account_id, item_id)
             );",
        )?;
        // A crash while running leaves 'running' rows: they are simply resumable.
        db.execute(
            "UPDATE downloads SET state = 'queued' WHERE state = 'running'",
            [],
        )?;
        Ok(Self {
            db: Mutex::new(db),
            dir: dir.to_owned(),
            http,
            retry,
            sink,
            live: Mutex::new(HashMap::new()),
            stop: Mutex::new(HashMap::new()),
        })
    }

    /// Adds a download (or returns the existing one for the same item). Does not start it.
    pub fn enqueue(&self, req: DownloadRequest) -> Result<i64> {
        let db = lock(&self.db);
        if let Some(id) = db
            .query_row(
                "SELECT id FROM downloads WHERE account_id = ?1 AND item_id = ?2",
                params![req.account_id, req.item_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        // Unique final path: "<item id> - <file name>" avoids collisions between servers.
        let name = sanitize(&format!("{} - {}", sanitize(&req.item_id), req.file_name));
        let path = self.dir.join(&name);
        db.execute(
            "INSERT INTO downloads(account_id, item_id, title, url, headers, path, total, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8)",
            params![
                req.account_id,
                req.item_id,
                req.title,
                req.url,
                serde_json::to_string(&req.headers).unwrap_or_default(),
                path.to_string_lossy(),
                req.expected_size.map(|s| s as i64),
                now()
            ],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn list(&self) -> Result<Vec<Download>> {
        let db = lock(&self.db);
        let mut stmt = db.prepare(
            "SELECT id, account_id, item_id, title, path, state, offset, total, error, created_at
             FROM downloads ORDER BY created_at DESC, id DESC",
        )?;
        let live = lock(&self.live).clone();
        let rows = stmt.query_map([], |r| {
            let id: i64 = r.get(0)?;
            let offset = r.get::<_, i64>(6)? as u64;
            Ok(Download {
                id,
                account_id: r.get(1)?,
                item_id: r.get(2)?,
                title: r.get(3)?,
                file_path: r.get(4)?,
                state: DownloadState::parse(&r.get::<_, String>(5)?),
                downloaded: live.get(&id).copied().unwrap_or(offset).max(offset),
                total: r.get::<_, Option<i64>>(7)?.map(|t| t as u64),
                error: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get(&self, id: i64) -> Result<Download> {
        self.list()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or(Error::NotFound)
    }

    /// The local file of a completed download, for offline playback.
    pub fn completed_file(&self, account_id: &str, item_id: &str) -> Result<Option<String>> {
        let path: Option<String> = lock(&self.db)
            .query_row(
                "SELECT path FROM downloads WHERE account_id = ?1 AND item_id = ?2 AND state = 'completed'",
                params![account_id, item_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(path.filter(|p| Path::new(p).exists()))
    }

    /// Ids that should run (queued, or interrupted by a crash).
    pub fn pending(&self) -> Result<Vec<i64>> {
        let db = lock(&self.db);
        let mut stmt = db
            .prepare("SELECT id FROM downloads WHERE state IN ('queued', 'running') ORDER BY id")?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Asks a running download to stop at the next chunk; a queued one is just marked paused.
    pub fn pause(&self, id: i64) -> Result<()> {
        if let Some(flag) = lock(&self.stop).get(&id) {
            flag.store(true, Ordering::SeqCst);
        }
        self.set_state(id, DownloadState::Paused, None, &["queued"])
    }

    /// Makes a paused/failed/no-space download eligible to run again.
    pub fn resume(&self, id: i64) -> Result<()> {
        self.set_state(
            id,
            DownloadState::Queued,
            None,
            &["paused", "no_space", "failed"],
        )
    }

    /// Deletes the download and its files.
    pub fn remove(&self, id: i64) -> Result<()> {
        if let Some(flag) = lock(&self.stop).get(&id) {
            flag.store(true, Ordering::SeqCst);
        }
        let path: Option<String> = lock(&self.db)
            .query_row("SELECT path FROM downloads WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(path) = path else {
            return Err(Error::NotFound);
        };
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(part_path(Path::new(&path)));
        lock(&self.db).execute("DELETE FROM downloads WHERE id = ?1", [id])?;
        Ok(())
    }

    fn set_state(
        &self,
        id: i64,
        state: DownloadState,
        error: Option<&str>,
        only_from: &[&str],
    ) -> Result<()> {
        let db = lock(&self.db);
        let current: Option<String> = db
            .query_row("SELECT state FROM downloads WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(current) = current else {
            return Err(Error::NotFound);
        };
        if only_from.is_empty() || only_from.contains(&current.as_str()) {
            db.execute(
                "UPDATE downloads SET state = ?2, error = ?3 WHERE id = ?1",
                params![id, state.as_str(), error],
            )?;
        }
        Ok(())
    }

    fn row(&self, id: i64) -> Result<Row> {
        lock(&self.db)
            .query_row(
                "SELECT url, headers, path, offset, total, validator, state FROM downloads WHERE id = ?1",
                [id],
                |r| {
                    Ok(Row {
                        url: r.get(0)?,
                        headers: serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default(),
                        path: PathBuf::from(r.get::<_, String>(2)?),
                        offset: r.get::<_, i64>(3)? as u64,
                        total: r.get::<_, Option<i64>>(4)?.map(|t| t as u64),
                        validator: r.get(5)?,
                        state: DownloadState::parse(&r.get::<_, String>(6)?),
                    })
                },
            )
            .optional()?
            .ok_or(Error::NotFound)
    }

    fn save_progress(
        &self,
        id: i64,
        offset: u64,
        total: Option<u64>,
        validator: Option<&str>,
    ) -> Result<()> {
        lock(&self.db).execute(
            "UPDATE downloads SET offset = ?2, total = COALESCE(?3, total), validator = COALESCE(?4, validator) WHERE id = ?1",
            params![id, offset as i64, total.map(|t| t as i64), validator],
        )?;
        Ok(())
    }

    /// Runs one download to completion, retrying transient failures. Returns its final state.
    pub async fn run(&self, id: i64) -> Result<DownloadState> {
        let row = self.row(id)?;
        if matches!(row.state, DownloadState::Completed | DownloadState::Paused) {
            return Ok(row.state);
        }
        let flag = Arc::new(AtomicBool::new(false));
        lock(&self.stop).insert(id, flag.clone());
        self.set_state(id, DownloadState::Running, None, &[])?;
        let mut failures = 0u32;
        let result = loop {
            match self.attempt(id, &flag).await {
                Ok(()) => break Ok(DownloadState::Completed),
                Err(Error::Paused) => break Ok(DownloadState::Paused),
                Err(Error::NoSpace) => {
                    self.set_state(id, DownloadState::PausedNoSpace, Some("disk full"), &[])?;
                    break Ok(DownloadState::PausedNoSpace);
                }
                Err(Error::Transient(msg)) if failures + 1 < self.retry.max_attempts => {
                    let wait = self.retry.delay(failures);
                    failures += 1;
                    lock(&self.db).execute(
                        "UPDATE downloads SET error = ?2 WHERE id = ?1",
                        params![id, msg],
                    )?;
                    // Sleep in small steps so a pause is honoured quickly.
                    let deadline = tokio::time::Instant::now() + wait;
                    while tokio::time::Instant::now() < deadline {
                        if flag.load(Ordering::SeqCst) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50).min(wait)).await;
                    }
                    if flag.load(Ordering::SeqCst) {
                        break Ok(DownloadState::Paused);
                    }
                }
                Err(e) => {
                    self.set_state(id, DownloadState::Failed, Some(&e.to_string()), &[])?;
                    break Ok(DownloadState::Failed);
                }
            }
        };
        lock(&self.stop).remove(&id);
        lock(&self.live).remove(&id);
        if let Ok(DownloadState::Paused) = result {
            // A remove() during the run deletes the row; ignore NotFound then.
            let _ = self.set_state(id, DownloadState::Paused, None, &[]);
        }
        result
    }

    async fn attempt(&self, id: i64, stop: &AtomicBool) -> Result<()> {
        let row = self.row(id)?;
        let part = part_path(&row.path);
        // Never trust bytes past the last fsync'd offset.
        let mut offset = row.offset;
        match std::fs::metadata(&part) {
            Ok(m) if m.len() >= offset => {
                if m.len() > offset {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&part)
                        .and_then(|f| f.set_len(offset))
                        .map_err(io_err)?;
                }
            }
            // .part shorter than recorded: it was altered outside the app, start over.
            Ok(_) => offset = 0,
            Err(_) => offset = 0,
        }
        if offset == 0 {
            let _ = std::fs::remove_file(&part);
        }

        let mut req = self.http.get(&row.url);
        for (k, v) in &row.headers {
            req = req.header(k, v);
        }
        if offset > 0 {
            req = req.header(header::RANGE, format!("bytes={offset}-"));
            if let Some(v) = &row.validator {
                req = req.header(header::IF_RANGE, v);
            }
        }
        let mut res = req
            .send()
            .await
            .map_err(|e| Error::Transient(e.to_string()))?;
        let status = res.status();
        let validator = res
            .headers()
            .get(header::ETAG)
            .or_else(|| res.headers().get(header::LAST_MODIFIED))
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let total = match status {
            StatusCode::PARTIAL_CONTENT => {
                let (start, total) = res
                    .headers()
                    .get(header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_content_range)
                    .ok_or_else(|| Error::Transient("206 without a valid Content-Range".into()))?;
                if start != offset {
                    // Server resumed from somewhere else: start over cleanly.
                    self.save_progress(id, 0, None, None)?;
                    return Err(Error::Transient(format!(
                        "server resumed at {start}, expected {offset}"
                    )));
                }
                total
            }
            StatusCode::OK => {
                if offset > 0 {
                    // If-Range mismatch (file changed on the server) or no range support.
                    offset = 0;
                    let _ = std::fs::remove_file(&part);
                    self.save_progress(id, 0, None, None)?;
                }
                res.content_length()
            }
            StatusCode::RANGE_NOT_SATISFIABLE if row.total == Some(offset) && offset > 0 => {
                return self.finish(id, &row.path, &part);
            }
            StatusCode::RANGE_NOT_SATISFIABLE => {
                self.save_progress(id, 0, None, None)?;
                let _ = std::fs::remove_file(&part);
                return Err(Error::Transient("range not satisfiable, restarting".into()));
            }
            s if s.is_server_error()
                || s == StatusCode::TOO_MANY_REQUESTS
                || s == StatusCode::REQUEST_TIMEOUT =>
            {
                return Err(Error::Transient(format!(
                    "server returned HTTP {}",
                    s.as_u16()
                )));
            }
            s => return Err(Error::Refused(s.as_u16())),
        };
        // Keep the validator from the first full response; a 206 must match it anyway.
        let keep_validator = if status == StatusCode::OK {
            validator.as_deref()
        } else {
            None
        };
        self.save_progress(id, offset, total, keep_validator)?;

        let mut sink = (self.sink)(&part).map_err(io_err)?;
        let mut since_sync = 0u64;
        loop {
            if stop.load(Ordering::SeqCst) {
                sink.sync().map_err(io_err)?;
                self.save_progress(id, offset, None, None)?;
                return Err(Error::Paused);
            }
            let chunk = match res.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => {
                    // Keep what reached the disk, then retry from there.
                    if sink.sync().is_ok() {
                        self.save_progress(id, offset, None, None)?;
                    }
                    return Err(Error::Transient(e.to_string()));
                }
            };
            if let Err(e) = sink.write_all(&chunk) {
                let err = io_err(e);
                // Persist only what is known durable.
                if sink.sync().is_ok() {
                    self.save_progress(id, offset, None, None)?;
                }
                return Err(err);
            }
            offset += chunk.len() as u64;
            since_sync += chunk.len() as u64;
            lock(&self.live).insert(id, offset);
            if since_sync >= SYNC_EVERY {
                sink.sync().map_err(io_err)?;
                self.save_progress(id, offset, None, None)?;
                since_sync = 0;
            }
        }
        sink.sync().map_err(io_err)?;
        self.save_progress(id, offset, None, None)?;
        drop(sink);
        if let Some(t) = total
            && offset != t
        {
            return Err(Error::Transient(format!(
                "connection closed at {offset} of {t} bytes"
            )));
        }
        self.finish(id, &row.path, &part)
    }

    fn finish(&self, id: i64, path: &Path, part: &Path) -> Result<()> {
        std::fs::rename(part, path).map_err(io_err)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        lock(&self.db).execute(
            "UPDATE downloads SET state = 'completed', error = NULL, offset = ?2, total = ?2 WHERE id = ?1",
            params![id, size as i64],
        )?;
        Ok(())
    }
}

pub fn part_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// `bytes 100-199/1000` -> (100, Some(1000)); `bytes 100-199/*` -> (100, None).
fn parse_content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    let total = if total.trim() == "*" {
        None
    } else {
        Some(total.trim().parse().ok()?)
    };
    Some((start, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_parsing() {
        assert_eq!(
            parse_content_range("bytes 100-199/1000"),
            Some((100, Some(1000)))
        );
        assert_eq!(parse_content_range("bytes 0-0/*"), Some((0, None)));
        assert_eq!(parse_content_range("items 1-2/3"), None);
    }

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize("a/b\\c:d*?.mkv"), "a_b_c_d__.mkv");
        assert_eq!(sanitize(" .. "), "download");
        assert_eq!(sanitize("Film (2016).mkv"), "Film (2016).mkv");
    }

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
            max_attempts: 5,
        };
        let d0 = p.delay(0).as_secs_f64();
        assert!((0.75..=1.25).contains(&d0));
        let d3 = p.delay(3).as_secs_f64();
        assert!((6.0..=10.0).contains(&d3));
        assert!(p.delay(30).as_secs_f64() <= 75.0);
    }

    #[test]
    fn disk_full_detection() {
        assert!(is_disk_full(&io::Error::from(io::ErrorKind::StorageFull)));
        #[cfg(unix)]
        assert!(is_disk_full(&io::Error::from_raw_os_error(28)));
        assert!(!is_disk_full(&io::Error::from(
            io::ErrorKind::PermissionDenied
        )));
    }
}
