// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end tests of the facade against a mocked Jellyfin server.

use std::sync::Arc;

use alwywatch_core::*;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
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
            downloads_dir: dir.path().join("dl").to_string_lossy().into(),
            profile: None,
            downloads_db: None,
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
fn shows_rail_is_one_card_per_show_saying_what_is_new() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    env.mount(Mock::given(path("/UserViews")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Id": "m", "Name": "Films", "CollectionType": "movies"},
            {"Id": "tv", "Name": "Shows", "CollectionType": "tvshows"}
        ]})),
    ));
    env.mount(
        Mock::given(path("/UserItems/Resume"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": []}))),
    );
    env.mount(
        Mock::given(path("/Shows/NextUp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": []}))),
    );
    // Movies: 20 entries as before; shows: more, episode by episode.
    env.mount(
        Mock::given(path("/Items/Latest"))
            .and(query_param("parentId", "m"))
            .and(query_param("limit", "20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"Id": "l1", "Name": "New", "Type": "Movie"}
            ]))),
    );
    let ep = |id: &str, show: &str, name: &str, s: u32, e: u32| {
        json!({"Id": id, "Name": name, "Type": "Episode", "SeriesId": show,
               "SeriesName": show.to_uppercase(), "SeasonId": format!("{show}-s{s}"),
               "ParentIndexNumber": s, "IndexNumber": e, "SeriesPrimaryImageTag": "p"})
    };
    env.mount(
        Mock::given(path("/Items/Latest"))
            .and(query_param("parentId", "tv"))
            .and(query_param("limit", "100"))
            .and(query_param("groupItems", "false"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                ep("e6", "dreamland", "Edenia", 1, 6),
                ep("s5", "silo", "c", 2, 5),
                ep("s3", "silo", "a", 2, 3),
                ep("s4", "silo", "b", 2, 4),
            ]))),
    );
    let home = env
        .block(app.refresh_home(s.account.id.clone(), true))
        .unwrap();
    assert_eq!(home.latest.len(), 2);
    assert_eq!(home.latest[0].items[0].id, "l1");
    assert!(home.latest[0].notes.is_empty(), "movies are unchanged");
    let shows = &home.latest[1];
    let ids: Vec<&str> = shows.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["dreamland", "silo"]);
    assert!(shows.items.iter().all(|i| i.kind == ItemKind::Series));
    let notes: Vec<(&str, &str, &str)> = shows
        .notes
        .iter()
        .map(|n| (n.item_id.as_str(), n.label.as_str(), n.open_id.as_str()))
        .collect();
    assert_eq!(
        notes,
        [
            ("dreamland", "S1 · E6 · Edenia", "e6"),
            ("silo", "S2 · E3–E5", "silo"),
        ]
    );
    assert_eq!(shows.notes[1].open_season_id.as_deref(), Some("silo-s2"));
    // The cached copy keeps the notes.
    let cached = app.cached_home(s.account.id.clone()).unwrap().unwrap();
    assert_eq!(cached.latest[1].notes, shows.notes);
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
        play_method: PlayMethod::Direct,
    };
    {
        let _down = env.block(
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(503))
                .mount_as_scoped(&env.server),
        );
        for _ in 0..3 {
            env.block(app.report_progress(s.account.id.clone(), report(PlaybackEvent::Progress)))
                .unwrap();
        }
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
        "position snapshots are replaced by the stop"
    );
    assert_eq!(env.block(app.flush_pending_progress()).unwrap(), 0);
}

#[test]
fn offline_position_survives_without_a_stop() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    let report = |pos| ProgressReport {
        item_id: "i1".into(),
        media_source_id: None,
        play_session_id: None,
        position_ms: pos,
        duration_ms: None,
        event: PlaybackEvent::Progress,
        audio_index: None,
        subtitle_index: None,
        play_method: PlayMethod::Direct,
    };
    {
        let _down = env.block(
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(503))
                .mount_as_scoped(&env.server),
        );
        // App closed mid-playback: no stop ever comes.
        for pos in [10_000, 20_000, 30_000] {
            env.block(app.report_progress(s.account.id.clone(), report(pos)))
                .unwrap();
        }
    }
    env.mount(
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing/Stopped"))
            .and(wiremock::matchers::body_partial_json(
                serde_json::json!({"PositionTicks": 300_000_000u64}),
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1),
    );
    assert_eq!(
        env.block(app.flush_pending_progress()).unwrap(),
        1,
        "the latest position only"
    );
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

#[test]
fn download_item_runs_and_is_available_offline() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    env.mount(Mock::given(path("/Items/i1")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({
            "Id": "i1", "Type": "Movie", "Name": "Arrival",
            "MediaSources": [{"Id": "ms", "Size": 6, "Path": "/m/Arrival.mkv"}]
        })),
    ));
    env.mount(
        Mock::given(path("/Items/i1/Download")).respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"x\"")
                .set_body_bytes(b"movie!".to_vec()),
        ),
    );
    let d = env
        .block(app.download_item(s.account.id.clone(), "i1".into(), None))
        .unwrap();
    assert_eq!(d.state, DownloadState::Queued);
    assert_eq!(d.total, Some(6));
    assert!(
        app.downloaded_file(s.account.id.clone(), "i1".into())
            .is_none()
    );
    assert_eq!(env.block(app.run_downloads()).unwrap(), 1);
    let file = app
        .downloaded_file(s.account.id.clone(), "i1".into())
        .unwrap();
    assert!(file.ends_with("Arrival.mkv"));
    assert_eq!(std::fs::read(file).unwrap(), b"movie!");
    assert_eq!(app.downloads().unwrap()[0].state, DownloadState::Completed);
}

#[test]
fn keep_service_connect_list_vote_and_reminders() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    let keep = env.block(MockServer::start());
    let me = json!({"user_key": "jellyfin:srv:u1", "name": "alice", "server_id": "srv", "is_admin": false,
        "rights": {"can_keep_forever": true, "max_forever": null, "max_keep_days": null}, "forever_count": 0});
    let item = |vote: serde_json::Value| {
        json!({
            "key": "100", "title": "Arrival", "year": 2016, "kind": "movie", "poster_url": null, "tmdb_id": "10",
            "tvdb_id": null, "server_item_id": "jf-1", "collection_id": 2, "collection_title": "Deletion",
            "collection_active": false, "deletion_date": 1000, "effective_deletion_date": 1000,
            "protection": "none", "my_vote": vote, "counts": {"forever": 0, "until_watched": 0, "days": 0, "dont_care": 0}
        })
    };
    env.block(
        Mock::given(path("/v1/me"))
            .and(wiremock::matchers::header("authorization", "Bearer tok"))
            .and(wiremock::matchers::header("x-alwywatch-server", "srv"))
            .respond_with(ResponseTemplate::new(200).set_body_json(me))
            .mount(&keep),
    );
    env.block(
        Mock::given(path("/v1/leaving-soon"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([item(json!(null))])))
            .mount(&keep),
    );
    let created = 1_000_000;
    env.block(
        Mock::given(method("PUT"))
            .and(path("/v1/votes/100"))
            .and(wiremock::matchers::body_json(json!({"kind": "keep_until_watched", "days": null, "reminder_days": 7, "reason": null})))
            .respond_with(ResponseTemplate::new(200).set_body_json(item(json!({
                "kind": "keep_until_watched", "until": null, "reminder_days": 7, "reason": null, "created_at": created
            }))))
            .mount(&keep),
    );
    let conn = env
        .block(app.keep_connect(keep.uri(), s.account.id.clone(), "tok".into()))
        .unwrap();
    assert_eq!(conn.server_id, "srv");
    assert!(app.keep_active());
    assert!(app.cached_leaving_soon().unwrap().is_none());
    let list = env.block(app.refresh_leaving_soon(false)).unwrap();
    assert_eq!(list[0].server_item_id.as_deref(), Some("jf-1"));
    let voted = env
        .block(app.keep_vote(
            "100".into(),
            VoteRequest {
                kind: VoteKind::KeepUntilWatched,
                days: None,
                reminder_days: Some(7),
                reason: None,
            },
        ))
        .unwrap();
    assert_eq!(
        voted.my_vote.as_ref().unwrap().kind,
        VoteKind::KeepUntilWatched
    );
    assert!(
        app.cached_leaving_soon().unwrap().unwrap()[0]
            .my_vote
            .is_some(),
        "cache patched in place"
    );
    // Reminder due every 7 days, not before, once per period.
    assert!(app.keep_due_reminders(created + 3 * 86_400).is_empty());
    assert_eq!(app.keep_due_reminders(created + 7 * 86_400 + 60).len(), 1);
    assert!(
        app.keep_due_reminders(created + 7 * 86_400 + 3600)
            .is_empty(),
        "not twice the same period"
    );
    // A check that comes late (the phone in Doze through the due day) still reminds.
    assert_eq!(app.keep_due_reminders(created + 15 * 86_400 + 60).len(), 1);
    assert!(app.keep_due_reminders(created + 16 * 86_400).is_empty());
    app.keep_disconnect().unwrap();
    assert!(!app.keep_active());
}

#[test]
fn seerr_through_the_keep_bridge() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    let keep = env.block(MockServer::start());
    let auth = || wiremock::matchers::header("authorization", "Bearer tok");
    env.block(
        Mock::given(path("/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"user_key": "k", "name": "bob",
                "server_id": "srv", "is_admin": false, "forever_count": 0,
                "rights": {"can_keep_forever": true, "max_forever": null, "max_keep_days": null}}),
            ))
            .mount(&keep),
    );
    env.block(
        Mock::given(path("/v1/seerr"))
            .and(auth())
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"seerr_user_id": 9,
                "user_name": "bob", "public_url": null, "cache_images": true})),
            )
            .mount(&keep),
    );
    env.block(
        Mock::given(path("/v1/seerr/api/v1/search"))
            .and(auth())
            .and(wiremock::matchers::header("x-alwywatch-server", "srv"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
                {"id": 603, "mediaType": "movie", "title": "The Matrix", "releaseDate": "1999-03-31", "posterPath": "/m.jpg"}
            ]})))
            .mount(&keep),
    );
    // No preset: onboarding does nothing.
    assert!(
        env.block(app.keep_onboard(s.account.id.clone(), "tok".into()))
            .unwrap()
            .is_none()
    );
    // Server picked from the AlwyWatch server's list, then signed in: both get connected.
    app.keep_set_preset(keep.uri()).unwrap();
    let k = env
        .block(app.keep_onboard(s.account.id.clone(), "tok".into()))
        .unwrap();
    assert_eq!(k.unwrap().user_name, "bob");
    assert!(app.keep_active());
    let conn = app.seerr_connection().unwrap();
    assert!(conn.via_keep);
    assert!(app.seerr_active());
    let r = env.block(app.seerr_search("the matrix".into())).unwrap();
    assert_eq!(r[0].tmdb_id, 603);
    // Bridged posters come from TMDB directly.
    assert!(
        r[0].poster_url
            .as_deref()
            .unwrap()
            .starts_with("https://image.tmdb.org/")
    );
    // Bridged as a chosen account (the one Seerr knows), then re-attached from its token.
    let conn = env
        .block(app.seerr_connect_bridge_as(s.account.id.clone(), "tok".into()))
        .unwrap();
    assert_eq!(conn.bridge_account.as_deref(), Some(s.account.id.as_str()));
    app.seerr_activate_bridge("tok".into()).unwrap();
    assert_eq!(
        env.block(app.seerr_search("the matrix".into())).unwrap()[0].tmdb_id,
        603
    );
    // Switching the synced account (here the same one again) keeps Seerr through the bridge.
    env.block(app.keep_connect(keep.uri(), s.account.id.clone(), "tok".into()))
        .unwrap();
    assert!(app.seerr_connection().unwrap().via_keep);
    assert!(app.seerr_active());
    app.keep_disconnect().unwrap();
    assert!(!app.seerr_active());
    assert!(app.seerr_connection().is_none());
    // A preset the user backed out of is forgotten.
    app.keep_set_preset(keep.uri()).unwrap();
    app.keep_clear_preset().unwrap();
    assert!(
        env.block(app.keep_onboard(s.account.id.clone(), "tok".into()))
            .unwrap()
            .is_none()
    );
}

#[test]
fn next_episodes_are_kept_downloaded_and_watched_ones_deleted() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    let acct = s.account.id.clone();
    env.mount(Mock::given(path("/Shows/show/Seasons")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Id": "sp", "Type": "Season", "Name": "Specials", "IndexNumber": 0},
            {"Id": "s1", "Type": "Season", "Name": "Season 1", "IndexNumber": 1}
        ]})),
    ));
    let ep = |n: u32, played: bool| {
        json!({"Id": format!("e{n}"), "Type": "Episode", "Name": format!("Ep {n}"),
               "SeriesId": "show", "SeriesName": "Show", "ParentIndexNumber": 1, "IndexNumber": n,
               "UserData": {"Played": played}})
    };
    env.mount(
        Mock::given(path("/Shows/show/Episodes"))
            .and(wiremock::matchers::query_param("seasonId", "s1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"Items": [ep(1, true), ep(2, false), ep(3, false), ep(4, false)]}),
            )),
    );
    for n in 1..=4 {
        env.mount(Mock::given(path(format!("/Items/e{n}"))).respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "Id": format!("e{n}"), "Type": "Episode", "Name": format!("Ep {n}"),
                "MediaSources": [{"Id": "ms", "Size": 3, "Path": format!("/m/e{n}.mkv")}]
            })),
        ));
        env.mount(
            Mock::given(path(format!("/Items/e{n}/Download")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ep!".to_vec())),
        );
    }
    // Next 2 after the last watched one (e1): e2 and e3.
    let queued = env
        .block(app.set_auto_download(
            acct.clone(),
            "show".into(),
            "Show".into(),
            2,
            DeleteRule::WhenWatched,
        ))
        .unwrap();
    assert_eq!(queued, 2);
    let mut ids: Vec<String> = app
        .downloads()
        .unwrap()
        .into_iter()
        .map(|d| d.item_id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["e2", "e3"]);
    let d = &app.downloads().unwrap()[0];
    assert_eq!(d.episode.as_ref().unwrap().series_id, "show");
    assert_eq!(d.delete_rule, DeleteRule::WhenWatched);
    assert_eq!(env.block(app.run_downloads()).unwrap(), 2);
    env.mount(
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing/Stopped"))
            .respond_with(ResponseTemplate::new(204)),
    );
    // e2 watched to the end: deleted at the next sync, e4 takes its place.
    env.block(app.report_progress(
        acct.clone(),
        ProgressReport {
            item_id: "e2".into(),
            media_source_id: None,
            play_session_id: None,
            position_ms: 950,
            duration_ms: Some(1000),
            event: PlaybackEvent::Stop,
            audio_index: None,
            subtitle_index: None,
            play_method: PlayMethod::Direct,
        },
    ))
    .unwrap();
    assert_eq!(env.block(app.sync_downloads()).unwrap(), 1);
    let mut ids: Vec<String> = app
        .downloads()
        .unwrap()
        .into_iter()
        .map(|d| d.item_id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["e3", "e4"]);
    assert!(app.auto_download_of(acct.clone(), "show".into()).is_some());
    env.block(app.set_auto_download(
        acct.clone(),
        "show".into(),
        "Show".into(),
        0,
        DeleteRule::Never,
    ))
    .unwrap();
    assert!(app.auto_downloads().is_empty());
}

#[test]
fn continue_watching_hides_removed_next_episodes_until_watched_again() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    let acct = s.account.id.clone();
    env.mount(
        Mock::given(path("/UserViews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": []}))),
    );
    env.mount(
        Mock::given(path("/UserItems/Resume"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": []}))),
    );
    env.mount(Mock::given(path("/Shows/NextUp")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Id": "n1", "Name": "Next", "Type": "Episode", "SeriesId": "show", "SeriesName": "Show"}
        ]})),
    ));
    // Show last watched: no finished episode yet.
    env.mount(
        Mock::given(path("/Items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": []}))),
    );
    env.mount(
        Mock::given(method("DELETE"))
            .and(path("/UserPlayedItems/n1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({}))),
    );
    let h = env.block(app.refresh_home(acct.clone(), true)).unwrap();
    assert_eq!(h.continue_watching.len(), 1);
    env.block(app.remove_from_continue_watching(acct.clone(), "n1".into()))
        .unwrap();
    let h = env.block(app.refresh_home(acct.clone(), true)).unwrap();
    assert!(h.continue_watching.is_empty(), "hidden on this device");
    assert!(
        app.cached_home(acct)
            .unwrap()
            .unwrap()
            .continue_watching
            .is_empty()
    );
}

/// A fake relay: remembers the registered key and the sealed bundle like the real server.
struct Relay(std::sync::Arc<std::sync::Mutex<(Option<String>, Option<String>)>>);

impl wiremock::Respond for Relay {
    fn respond(&self, req: &wiremock::Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        let p = req.url.path();
        match (req.method.as_str(), p) {
            ("POST", "/v1/transfer") => {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                st.0 = Some(body["public_key"].as_str().unwrap().to_owned());
                ResponseTemplate::new(200)
                    .set_body_json(json!({"code": "123456", "handle": "h", "expires_in": 600}))
            }
            ("GET", "/v1/transfer/123456") => ResponseTemplate::new(200)
                .set_body_json(json!({"public_key": st.0.clone().unwrap(), "device": "Tablet"})),
            ("PUT", "/v1/transfer/123456") => {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                st.1 = Some(body["sealed"].as_str().unwrap().to_owned());
                ResponseTemplate::new(204)
            }
            ("GET", "/v1/transfer/poll/h") => match st.1.take() {
                Some(s) => ResponseTemplate::new(200).set_body_json(json!({"sealed": s})),
                None => ResponseTemplate::new(202),
            },
            _ => ResponseTemplate::new(404),
        }
    }
}

#[test]
fn accounts_move_to_a_new_device_sealed_end_to_end() {
    let env = Env::new();
    let s = env.sign_in();
    let app = env.app();
    let keep = env.block(MockServer::start());
    env.block(
        Mock::given(path("/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "user_key": "jellyfin:srv:u1", "name": "alice", "server_id": "srv", "is_admin": false,
                "forever_count": 0, "rights": {"can_keep_forever": true, "max_forever": null, "max_keep_days": null}
            })))
            .mount(&keep),
    );
    let state = std::sync::Arc::new(std::sync::Mutex::new((None, None)));
    env.block(
        Mock::given(wiremock::matchers::path_regex("^/v1/transfer"))
            .respond_with(Relay(state.clone()))
            .mount(&keep),
    );
    env.block(app.keep_connect(keep.uri(), s.account.id.clone(), "tok".into()))
        .unwrap();
    app.set_setting("ui.combined".into(), "1".into()).unwrap();

    // New device: a code and a check number; nothing yet.
    let code = env
        .block(app.transfer_start(keep.uri(), "Tablet".into()))
        .unwrap();
    assert_eq!(code.code, "123456");
    assert!(
        env.block(app.transfer_receive(keep.uri(), code.handle.clone()))
            .unwrap()
            .is_none()
    );

    // Signed-in device: same check number, then sends its config sealed to that key.
    let req = env.block(app.transfer_peer("123456".into())).unwrap();
    assert_eq!(
        (req.device.as_str(), req.check.as_str()),
        ("Tablet", code.check.as_str())
    );
    let bundle = app.export_config(vec!["ui.combined".into()]).unwrap();
    env.block(app.transfer_send("123456".into(), bundle.clone()))
        .unwrap();
    let sealed = state.lock().unwrap().1.clone().unwrap();
    assert!(
        !sealed.contains("alice") && !sealed.contains(&s.account.id),
        "the relay sees ciphertext only"
    );

    // New device opens it and restores the accounts.
    let got = env
        .block(app.transfer_receive(keep.uri(), code.handle))
        .unwrap()
        .unwrap();
    assert_eq!(got.bundle.as_deref(), Some(bundle.as_str()));
    let other = Env::new();
    let fresh = other.app();
    assert_eq!(
        fresh.import_config(got.bundle.unwrap()).unwrap(),
        vec![s.account.id.clone()]
    );
    assert_eq!(fresh.accounts().unwrap()[0].id, s.account.id);
    assert_eq!(fresh.keep_connection().unwrap().user_name, "alice");
    assert_eq!(
        fresh.get_setting("ui.combined".into()).unwrap().as_deref(),
        Some("1")
    );
}
