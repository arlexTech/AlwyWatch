// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end tests of the facade against a mocked Jellyfin server.

use std::sync::Arc;

use alwywatch_core::*;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Env {
    rt: tokio::runtime::Runtime,
    server: MockServer,
    _dir: tempfile::TempDir,
    app: Option<Arc<AlwyWatch>>,
}

impl Env {
    fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let server = rt.block_on(MockServer::start());
        let dir = tempfile::tempdir().unwrap();
        let app = AlwyWatch::new(CoreConfig {
            data_dir: dir.path().join("data").to_string_lossy().into(),
            cache_dir: dir.path().join("cache").to_string_lossy().into(),
            client: ClientInfo {
                product: "AlwyWatch".into(),
                version: "0.1.0".into(),
                device_name: "Test".into(),
                device_id: "dev".into(),
                platform: "Linux".into(),
            },
            image_cache_mb: 16,
        })
        .unwrap();
        Self {
            rt,
            server,
            _dir: dir,
            app: Some(app),
        }
    }

    fn app(&self) -> Arc<AlwyWatch> {
        self.app.clone().unwrap()
    }

    fn block<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    fn mount(&self, m: Mock) {
        self.block(m.mount(&self.server));
    }

    fn sign_in(&self) -> SignedIn {
        self.mount(
            Mock::given(method("POST"))
                .and(path("/Users/AuthenticateByName"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "User": {"Id": "u1", "Name": "alice"}, "AccessToken": "tok", "ServerId": "srv"
                }))),
        );
        self.mount(Mock::given(path("/System/Info/Public")).respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Id": "srv", "ServerName": "JF"})),
        ));
        self.block(
            self.app()
                .jellyfin_sign_in(self.server.uri(), "alice".into(), "pw".into()),
        )
        .unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // The core owns a runtime: drop it outside any async context.
        drop(self.app.take());
    }
}

fn home_mocks(env: &Env, expect: u64) {
    env.mount(
        Mock::given(path("/UserViews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": [
                {"Id": "m", "Name": "Films", "CollectionType": "movies"},
                {"Id": "x", "Name": "Photos", "CollectionType": "homevideos"}
            ]})))
            .expect(expect),
    );
    env.mount(Mock::given(path("/UserItems/Resume")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Id": "r1", "Name": "Half", "Type": "Movie"}
        ]})),
    ));
    env.mount(Mock::given(path("/Shows/NextUp")).respond_with(ResponseTemplate::new(500)));
    env.mount(Mock::given(path("/Items/Latest")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!([
            {"Id": "l1", "Name": "New", "Type": "Movie", "ImageTags": {"Primary": "t"}}
        ])),
    ));
}

#[test]
fn sign_in_persists_account_without_token() {
    let env = Env::new();
    let s = env.sign_in();
    assert_eq!(s.token, "tok");
    assert_eq!(s.account.id, "jellyfin:srv:u1");
    let accounts = env.app().accounts().unwrap();
    assert_eq!(accounts, vec![s.account.clone()]);
    let db = std::fs::read(env._dir.path().join("data/alwywatch.sqlite")).unwrap();
    assert!(
        !db.windows(3).any(|w| w == b"tok"),
        "token must never be written to the database"
    );
}

#[test]
fn home_is_cache_first_with_ttl_and_partial_failures() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    home_mocks(&env, 2);

    assert!(app.cached_home(s.account.id.clone()).unwrap().is_none());
    let home = env
        .block(app.refresh_home(s.account.id.clone(), false))
        .unwrap();
    assert_eq!(home.resume.len(), 1);
    assert!(home.next_up.is_empty(), "failing rail degrades to empty");
    assert_eq!(home.latest.len(), 1, "only movie/show libraries get rails");
    assert_eq!(home.latest[0].items[0].id, "l1");

    let cached = app.cached_home(s.account.id.clone()).unwrap().unwrap();
    assert_eq!(cached.resume, home.resume);

    // Fresh: no network. Forced: network (the mock expects exactly 2 /UserViews calls).
    env.block(app.refresh_home(s.account.id.clone(), false))
        .unwrap();
    env.block(app.refresh_home(s.account.id.clone(), true))
        .unwrap();
    env.block(env.server.verify());
}

#[test]
fn not_signed_in_after_restart_until_activated() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    app.remove_account(s.account.id.clone()).unwrap();
    assert!(app.accounts().unwrap().is_empty());
    let err = env
        .block(app.refresh_home(s.account.id.clone(), true))
        .unwrap_err();
    assert!(matches!(err, CoreError::NotSignedIn { .. }));
}

#[test]
fn progress_is_queued_offline_and_flushed_later() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    let report = |ev| ProgressReport {
        item_id: "i1".into(),
        media_source_id: None,
        play_session_id: None,
        position_ms: 1000,
        duration_ms: None,
        event: ev,
        audio_index: None,
        subtitle_index: None,
    };
    {
        let _down = env.block(
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(503))
                .mount_as_scoped(&env.server),
        );
        env.block(app.report_progress(s.account.id.clone(), report(PlaybackEvent::Progress)))
            .unwrap();
        env.block(app.report_progress(s.account.id.clone(), report(PlaybackEvent::Stop)))
            .unwrap();
        assert_eq!(
            env.block(app.flush_pending_progress()).unwrap(),
            0,
            "still down"
        );
    }
    env.mount(
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing/Stopped"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1),
    );
    assert_eq!(
        env.block(app.flush_pending_progress()).unwrap(),
        1,
        "only the stop event was queued; periodic progress is not"
    );
    assert_eq!(env.block(app.flush_pending_progress()).unwrap(), 0);
}

#[test]
fn images_download_once_and_are_served_from_disk() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    env.mount(
        Mock::given(path("/Items/i1/Images/Primary"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"jpegbytes".to_vec()))
            .expect(1),
    );
    let img = ImageRef {
        kind: ImageKind::Primary,
        path: "/Items/i1/Images/Primary?tag=t".into(),
        blurhash: None,
    };
    assert!(
        app.cached_image(s.account.id.clone(), "i1".into(), img.clone(), 300)
            .is_none()
    );
    let p1 = env
        .block(app.image(s.account.id.clone(), "i1".into(), img.clone(), 300))
        .unwrap();
    // 300 and 310 land in the same 320px bucket.
    let p2 = env
        .block(app.image(s.account.id.clone(), "i1".into(), img.clone(), 310))
        .unwrap();
    assert_eq!(p1, p2);
    assert_eq!(std::fs::read(&p1).unwrap(), b"jpegbytes");
    assert_eq!(
        app.cached_image(s.account.id.clone(), "i1".into(), img, 320)
            .as_deref(),
        Some(p1.as_str())
    );
    env.block(env.server.verify());
}

#[test]
fn width_buckets() {
    assert_eq!(bucket_width(1), 160);
    assert_eq!(bucket_width(320), 320);
    assert_eq!(bucket_width(321), 480);
    assert_eq!(bucket_width(9999), 1920);
}
