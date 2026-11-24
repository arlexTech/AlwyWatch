// SPDX-License-Identifier: AGPL-3.0-or-later
//! Download manager tests: interruption and resume, ETag change, disk full, retries, pause.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use downloads::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn body(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 251) as u8).collect()
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        base: Duration::from_millis(10),
        max: Duration::from_millis(40),
        max_attempts: 5,
    }
}

fn http() -> net::reqwest::Client {
    net::transfer_client("test").unwrap()
}

fn file_sink() -> Box<SinkFactory> {
    Box::new(|p: &Path| {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)?;
        Ok(Box::new(FileSink(f)) as Box<dyn Sink>)
    })
}

fn manager(dir: &Path, sink: Box<SinkFactory>) -> Downloads {
    Downloads::with_options(
        &dir.join("dl.sqlite"),
        &dir.join("files"),
        http(),
        fast_retry(),
        sink,
    )
    .unwrap()
}

fn request(url: String, size: Option<u64>) -> DownloadRequest {
    DownloadRequest {
        account_id: "jellyfin:s:u".into(),
        item_id: "item1".into(),
        title: "Arrival".into(),
        url,
        headers: vec![("Authorization".into(), "MediaBrowser Token=\"t\"".into())],
        file_name: "Arrival.mkv".into(),
        expected_size: size,
        episode: None,
        delete_rule: DeleteRule::Never,
        owner: "me".into(),
    }
}

/// Serves ranges of a fixed body, honouring If-Range against `etag`, like Jellyfin/Plex do.
struct RangeServer {
    data: Vec<u8>,
    etag: String,
    hits: Arc<AtomicUsize>,
}

impl Respond for RangeServer {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.hits.fetch_add(1, Ordering::SeqCst);
        let range = req.headers.get("range").and_then(|v| v.to_str().ok());
        let if_range = req.headers.get("if-range").and_then(|v| v.to_str().ok());
        let full = || {
            ResponseTemplate::new(200)
                .insert_header("etag", self.etag.as_str())
                .set_body_bytes(self.data.clone())
        };
        match range {
            Some(r) if if_range.is_none_or(|v| v == self.etag) => {
                let start: usize = r
                    .trim_start_matches("bytes=")
                    .trim_end_matches('-')
                    .parse()
                    .unwrap();
                ResponseTemplate::new(206)
                    .insert_header("etag", self.etag.as_str())
                    .insert_header(
                        "content-range",
                        format!("bytes {start}-{}/{}", self.data.len() - 1, self.data.len())
                            .as_str(),
                    )
                    .set_body_bytes(self.data[start..].to_vec())
            }
            _ => full(),
        }
    }
}

fn read_file(m: &Downloads, id: i64) -> Vec<u8> {
    std::fs::read(m.get(id).unwrap().file_path).unwrap()
}

#[tokio::test]
async fn downloads_complete_file_and_reports_progress() {
    let s = MockServer::start().await;
    let data = body(3 * 1024 * 1024 + 17);
    let hits = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/Items/item1/Download"))
        .and(header("authorization", "MediaBrowser Token=\"t\""))
        .respond_with(RangeServer {
            data: data.clone(),
            etag: "\"v1\"".into(),
            hits: hits.clone(),
        })
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m
        .enqueue(request(
            format!("{}/Items/item1/Download", s.uri()),
            Some(data.len() as u64),
        ))
        .unwrap();
    assert_eq!(
        m.enqueue(request(format!("{}/x", s.uri()), None)).unwrap(),
        id,
        "same item is not queued twice"
    );
    assert_eq!(m.pending().unwrap(), vec![id]);
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(read_file(&m, id), data);
    let d = m.get(id).unwrap();
    assert_eq!(
        (d.downloaded, d.total),
        (data.len() as u64, Some(data.len() as u64))
    );
    assert!(d.file_path.ends_with("item1 - Arrival.mkv"));
    assert!(!part_path(Path::new(&d.file_path)).exists());
    assert_eq!(
        m.completed_file("jellyfin:s:u", "item1").unwrap(),
        Some(d.file_path)
    );
    assert!(m.pending().unwrap().is_empty());
}

#[tokio::test]
async fn delete_rules_remove_watched_downloads() {
    let s = MockServer::start().await;
    let data = body(1024);
    Mock::given(method("GET"))
        .respond_with(RangeServer {
            data: data.clone(),
            etag: "\"v1\"".into(),
            hits: Arc::new(AtomicUsize::new(0)),
        })
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let mut req = request(format!("{}/Items/item1/Download", s.uri()), None);
    req.delete_rule = DeleteRule::WhenWatched;
    req.episode = Some(EpisodeOf {
        series_id: "show".into(),
        series_title: "Show".into(),
        season_number: Some(1),
        episode_number: Some(2),
    });
    let id = m.enqueue(req).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    let d = m.get(id).unwrap();
    assert!(d.completed_at.is_some());
    assert_eq!(d.episode.unwrap().episode_number, Some(2));
    let now = d.completed_at.unwrap() + 10;
    assert_eq!(m.apply_rules(now, "me").unwrap(), 0, "not watched yet");
    m.mark_watched("jellyfin:s:u", "item1", now).unwrap();
    let file = m.get(id).unwrap().file_path;
    assert_eq!(m.apply_rules(now, "me").unwrap(), 1);
    assert!(m.list().unwrap().is_empty());
    assert!(!Path::new(&file).exists());
}

/// A raw HTTP server: the first connection sends headers for the whole file but only part of the
/// body, then drops (network loss). Later connections answer Range requests properly.
async fn flaky_server(
    data: Vec<u8>,
    cut_at: usize,
    etag: &'static str,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        let mut n = 0;
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            n += 1;
            let mut buf = vec![0u8; 4096];
            let len = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..len]).to_string();
            seen2.lock().unwrap().push(req.clone());
            let range = req.lines().find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("range: bytes=")
                    .map(|r| r.trim_end_matches('-').parse::<usize>().unwrap())
            });
            if n == 1 {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: {etag}\r\nConnection: close\r\n\r\n",
                    data.len()
                );
                sock.write_all(head.as_bytes()).await.unwrap();
                sock.write_all(&data[..cut_at]).await.unwrap();
                sock.flush().await.unwrap();
                // Drop the connection mid-body.
            } else {
                let start = range.unwrap_or(0);
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nETag: {etag}\r\nConnection: close\r\n\r\n",
                    data.len() - start,
                    data.len() - 1,
                    data.len()
                );
                sock.write_all(head.as_bytes()).await.unwrap();
                sock.write_all(&data[start..]).await.unwrap();
            }
        }
    });
    (format!("http://{addr}/file"), seen)
}

#[tokio::test]
async fn resumes_after_network_loss_with_range_and_if_range() {
    // 9 MiB, cut after 6 MiB: one fsync (at 4 MiB) happened before the cut, and the bytes received
    // after it are synced on the error path, so the resume starts exactly at the cut.
    let data = body(9 * 1024 * 1024);
    let cut = 6 * 1024 * 1024;
    let (url, seen) = flaky_server(data.clone(), cut, "\"abc\"").await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(url, None)).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(read_file(&m, id), data, "resumed file is byte-identical");
    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    let second = reqs[1].to_ascii_lowercase();
    assert!(second.contains(&format!("range: bytes={cut}-")), "{second}");
    assert!(second.contains("if-range: \"abc\""), "{second}");
}

#[tokio::test]
async fn restart_from_zero_when_file_changed_on_server() {
    let s = MockServer::start().await;
    let new_data = body(100_000);
    // The server now has a different version: it ignores the stale If-Range and sends 200.
    Mock::given(method("GET"))
        .and(header_exists("range"))
        .and(header("if-range", "\"old\""))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"new\"")
                .set_body_bytes(new_data.clone()),
        )
        .expect(1)
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    // Simulate an earlier interrupted run of the old version: 50 000 synced bytes + one unsynced.
    let final_path = PathBuf::from(m.get(id).unwrap().file_path);
    std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    std::fs::write(part_path(&final_path), vec![0xAA; 50_001]).unwrap();
    seed_progress(dir.path(), id, 50_000, "\"old\"");

    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(
        read_file(&m, id),
        new_data,
        "old bytes discarded, new file downloaded whole"
    );
}

fn seed_progress(dir: &Path, id: i64, offset: u64, validator: &str) {
    let db = rusqlite::Connection::open(dir.join("dl.sqlite")).unwrap();
    db.execute(
        "UPDATE downloads SET offset = ?2, validator = ?3 WHERE id = ?1",
        rusqlite::params![id, offset as i64, validator],
    )
    .unwrap();
}

#[tokio::test]
async fn unsynced_tail_is_truncated_before_resuming() {
    let s = MockServer::start().await;
    let data = body(200_000);
    Mock::given(method("GET"))
        .respond_with(RangeServer {
            data: data.clone(),
            etag: "\"e\"".into(),
            hits: Arc::new(AtomicUsize::new(0)),
        })
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    let final_path = PathBuf::from(m.get(id).unwrap().file_path);
    std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    // 120 000 bytes on disk but only 100 000 were confirmed durable; the rest is garbage.
    let mut part = data[..100_000].to_vec();
    part.extend(vec![0xFF; 20_000]);
    std::fs::write(part_path(&final_path), part).unwrap();
    seed_progress(dir.path(), id, 100_000, "\"e\"");
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(read_file(&m, id), data);
}

/// A disk that fills up after `limit` bytes until `space` is raised.
struct LimitedDisk {
    inner: FileSink,
    used: Arc<AtomicU64>,
    limit: Arc<AtomicU64>,
}

impl Sink for LimitedDisk {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let used = self.used.load(Ordering::SeqCst);
        let room = self.limit.load(Ordering::SeqCst).saturating_sub(used);
        if (buf.len() as u64) > room {
            // Write what fits, then fail like a real full disk.
            self.inner.write_all(&buf[..room as usize])?;
            self.used.fetch_add(room, Ordering::SeqCst);
            return Err(io::Error::from_raw_os_error(if cfg!(windows) {
                112
            } else {
                28
            }));
        }
        self.used.fetch_add(buf.len() as u64, Ordering::SeqCst);
        self.inner.write_all(buf)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.inner.sync()
    }
}

#[tokio::test]
async fn disk_full_pauses_keeps_part_and_resumes_after_freeing_space() {
    let s = MockServer::start().await;
    let data = body(2 * 1024 * 1024);
    let hits = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .respond_with(RangeServer {
            data: data.clone(),
            etag: "\"e\"".into(),
            hits: hits.clone(),
        })
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let used = Arc::new(AtomicU64::new(0));
    let limit = Arc::new(AtomicU64::new(700_000));
    let (u, l) = (used.clone(), limit.clone());
    let m = manager(
        dir.path(),
        Box::new(move |p: &Path| {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)?;
            // The .part file is reopened on resume: count what is already in it.
            u.store(f.metadata()?.len(), Ordering::SeqCst);
            Ok(Box::new(LimitedDisk {
                inner: FileSink(f),
                used: u.clone(),
                limit: l.clone(),
            }) as Box<dyn Sink>)
        }),
    );
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::PausedNoSpace);
    let d = m.get(id).unwrap();
    assert_eq!(d.state, DownloadState::PausedNoSpace);
    assert_eq!(d.error.as_deref(), Some("disk full"));
    let part = part_path(Path::new(&d.file_path));
    assert_eq!(
        std::fs::metadata(&part).unwrap().len(),
        700_000,
        ".part kept with what fit"
    );
    assert!(
        m.pending().unwrap().is_empty(),
        "not retried automatically while the disk is full"
    );

    // User frees space and resumes: continues from 700 000 with a Range request.
    limit.store(u64::MAX, Ordering::SeqCst);
    m.resume(id).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(read_file(&m, id), data);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn transient_errors_retry_then_succeed() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(read_file(&m, id), b"ok");
}

#[tokio::test]
async fn permanent_errors_fail_and_can_be_resumed_later() {
    let s = MockServer::start().await;
    let gone = Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount_as_scoped(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Failed);
    assert!(m.get(id).unwrap().error.unwrap().contains("404"));
    drop(gone);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".to_vec()))
        .mount(&s)
        .await;
    m.resume(id).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
}

#[tokio::test]
async fn exhausted_retries_wait_for_the_network_and_stay_pending() {
    let s = MockServer::start().await;
    let down = Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502))
        .mount_as_scoped(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Waiting);
    assert_eq!(s.received_requests().await.unwrap().len(), 5);
    assert!(m.get(id).unwrap().error.unwrap().contains("502"));
    assert_eq!(
        m.pending().unwrap(),
        vec![id],
        "rerun when the network is back"
    );
    drop(down);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".to_vec()))
        .mount(&s)
        .await;
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
}

#[tokio::test]
async fn pause_stops_at_next_chunk_and_resume_continues() {
    let s = MockServer::start().await;
    let data = body(8 * 1024 * 1024);
    Mock::given(method("GET"))
        .respond_with(RangeServer {
            data: data.clone(),
            etag: "\"e\"".into(),
            hits: Arc::new(AtomicUsize::new(0)),
        })
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    // A slow disk so the pause lands mid-download.
    struct Slow(FileSink);
    impl Sink for Slow {
        fn write_all(&mut self, b: &[u8]) -> io::Result<()> {
            std::thread::sleep(Duration::from_millis(2));
            self.0.write_all(b)
        }
        fn sync(&mut self) -> io::Result<()> {
            self.0.sync()
        }
    }
    let m = Arc::new(manager(
        dir.path(),
        Box::new(|p: &Path| {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)?;
            Ok(Box::new(Slow(FileSink(f))) as Box<dyn Sink>)
        }),
    ));
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    let runner = {
        let m = m.clone();
        tokio::spawn(async move { m.run(id).await })
    };
    tokio::time::sleep(Duration::from_millis(60)).await;
    m.pause(id).unwrap();
    assert_eq!(runner.await.unwrap().unwrap(), DownloadState::Paused);
    let d = m.get(id).unwrap();
    assert_eq!(d.state, DownloadState::Paused);
    assert!(
        d.downloaded > 0 && d.downloaded < data.len() as u64,
        "stopped mid-way: {}",
        d.downloaded
    );
    m.resume(id).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(read_file(&m, id), data);
}

#[tokio::test]
async fn remove_deletes_files_and_row() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"data".to_vec()))
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    m.run(id).await.unwrap();
    let p = m.get(id).unwrap().file_path;
    m.remove(id).unwrap();
    assert!(!Path::new(&p).exists());
    assert_eq!(m.get(id).unwrap_err(), Error::NotFound);
}

#[tokio::test]
async fn interrupted_runs_are_requeued_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let m = manager(dir.path(), file_sink());
        let id = m
            .enqueue(request("http://127.0.0.1:9/f".into(), None))
            .unwrap();
        let db = rusqlite::Connection::open(dir.path().join("dl.sqlite")).unwrap();
        db.execute("UPDATE downloads SET state = 'running'", [])
            .unwrap();
        id
    };
    let m = manager(dir.path(), file_sink());
    assert_eq!(m.get(id).unwrap().state, DownloadState::Queued);
    assert_eq!(m.pending().unwrap(), vec![id]);
}

#[tokio::test]
async fn shared_downloads_stay_until_no_profile_wants_them() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(RangeServer {
            data: body(64),
            etag: "\"v1\"".into(),
            hits: Arc::new(AtomicUsize::new(0)),
        })
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let url = format!("{}/Items/item1/Download", s.uri());
    let id = m.enqueue(request(url.clone(), None)).unwrap();
    let mut b = request(url, None);
    b.owner = "kid".into();
    assert_eq!(m.enqueue(b).unwrap(), id, "one file for both profiles");
    assert_eq!(
        m.get(id).unwrap().owners,
        vec!["me".to_owned(), "kid".to_owned()]
    );
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    let file = m.get(id).unwrap().file_path;
    assert!(!m.release(id, "me").unwrap(), "kid still wants it");
    assert!(Path::new(&file).exists());
    assert!(m.release(id, "kid").unwrap());
    assert!(!Path::new(&file).exists());
    assert!(m.list().unwrap().is_empty());
}

/// Drops the connection after `cut` bytes of every response (resuming properly), so a download
/// advances a little at each try.
async fn dripping_server(data: Vec<u8>, cut: usize) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let len = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..len]).to_string();
            let start = req
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("range: bytes=")
                        .map(|r| r.trim_end_matches('-').parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let (status, range) = if start == 0 {
                ("200 OK", String::new())
            } else {
                (
                    "206 Partial Content",
                    format!(
                        "Content-Range: bytes {start}-{}/{}\r\n",
                        data.len() - 1,
                        data.len()
                    ),
                )
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{range}ETag: \"e\"\r\nConnection: close\r\n\r\n",
                data.len() - start
            );
            sock.write_all(head.as_bytes()).await.unwrap();
            let end = (start + cut).min(data.len());
            sock.write_all(&data[start..end]).await.unwrap();
            sock.flush().await.unwrap();
        }
    });
    format!("http://{addr}/file")
}

#[tokio::test]
async fn drops_with_progress_in_between_do_not_use_up_the_retries() {
    // 10 drops, each after some progress: more than the 5 failed attempts allowed in a row.
    let data = body(100_000);
    let url = dripping_server(data.clone(), 10_000).await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(url, None)).unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(read_file(&m, id), data);
}

#[tokio::test]
async fn a_changed_file_without_validator_forgets_the_old_one() {
    let s = MockServer::start().await;
    let new_data = body(30_000);
    // The new version comes without ETag or Last-Modified.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(new_data.clone()))
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), file_sink());
    let id = m.enqueue(request(format!("{}/f", s.uri()), None)).unwrap();
    let final_path = PathBuf::from(m.get(id).unwrap().file_path);
    std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    std::fs::write(part_path(&final_path), vec![0xAA; 50_000]).unwrap();
    seed_progress(dir.path(), id, 50_000, "\"old\"");
    let db = rusqlite::Connection::open(dir.path().join("dl.sqlite")).unwrap();
    db.execute("UPDATE downloads SET total = 90000 WHERE id = ?1", [id])
        .unwrap();
    assert_eq!(m.run(id).await.unwrap(), DownloadState::Completed);
    assert_eq!(read_file(&m, id), new_data);
    let validator: Option<String> = db
        .query_row("SELECT validator FROM downloads WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(validator, None, "the old version's validator is gone");
}
