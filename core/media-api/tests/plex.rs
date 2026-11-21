// SPDX-License-Identifier: AGPL-3.0-or-later
use media_api::plex::{PlexAuth, PlexClient};
use media_api::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_info() -> ClientInfo {
    ClientInfo {
        product: "AlwyWatch".into(),
        version: "0.1.0".into(),
        device_name: "Test".into(),
        device_id: "dev-1".into(),
        platform: "Android".into(),
    }
}

fn http() -> net::reqwest::Client {
    net::api_client("AlwyWatch-test").unwrap()
}

fn session(base: &str) -> Session {
    Session {
        kind: ServerKind::Plex,
        base_url: base.into(),
        server_id: "machine".into(),
        server_name: "HomePlex".into(),
        user_id: "42".into(),
        user_name: "alice".into(),
        is_admin: false,
        token: "server-token".into(),
    }
}

#[tokio::test]
async fn pin_flow_and_server_discovery() {
    let tv = MockServer::start().await;
    let pms = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/pins"))
        .and(query_param("strong", "true"))
        .and(header("X-Plex-Client-Identifier", "dev-1"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"id": 7, "code": "longcode"})),
        )
        .mount(&tv)
        .await;
    let auth = PlexAuth::with_plex_tv(http(), client_info(), &tv.uri());
    let pin = auth.create_pin(true).await.unwrap();
    assert!(
        pin.auth_url
            .starts_with("https://app.plex.tv/auth#?clientID=dev-1&code=longcode")
    );

    let pending = Mock::given(path("/api/v2/pins/7"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": 7, "code": "longcode", "authToken": null})),
        )
        .mount_as_scoped(&tv)
        .await;
    assert_eq!(auth.check_pin(7).await.unwrap_err(), Error::Pending);
    drop(pending);
    Mock::given(path("/api/v2/pins/7"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": 7, "code": "longcode", "authToken": "acct"})),
        )
        .mount(&tv)
        .await;
    let token = auth.check_pin(7).await.unwrap();
    assert_eq!(token, "acct");

    Mock::given(path("/api/v2/user"))
        .and(header("X-Plex-Token", "acct"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": 42, "username": "alice", "title": "Alice"})),
        )
        .mount(&tv)
        .await;
    Mock::given(path("/api/v2/resources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"name": "Phone", "clientIdentifier": "p", "provides": "client,player"},
            {"name": "HomePlex", "clientIdentifier": "machine", "provides": "server", "owned": false,
             "accessToken": "server-token",
             "connections": [
                {"uri": "http://10.255.255.1:9", "local": true, "relay": false},
                {"uri": pms.uri(), "local": false, "relay": false}
             ]}
        ])))
        .mount(&tv)
        .await;
    Mock::given(path("/identity"))
        .and(header("X-Plex-Token", "server-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"MediaContainer": {"machineIdentifier": "machine"}})),
        )
        .mount(&pms)
        .await;

    let user = auth.user(&token).await.unwrap();
    assert_eq!(user.title, "Alice");
    let servers = auth.servers(&token).await.unwrap();
    assert_eq!(servers.len(), 1, "non-server resources are filtered out");
    let sess = auth.connect(&servers[0], &user, None).await.unwrap();
    assert_eq!(
        sess.base_url,
        pms.uri(),
        "remote direct connection tried before local"
    );
    assert_eq!(
        sess.token, "server-token",
        "server token, not the account token"
    );
    assert!(!sess.is_admin);
}

#[tokio::test]
async fn library_items_and_guid_mapping() {
    let s = MockServer::start().await;
    Mock::given(path("/library/sections"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"MediaContainer": {"Directory": [
                {"key": "1", "title": "Films", "type": "movie"},
                {"key": "4", "title": "Séries", "type": "show"}
            ]}}),
        ))
        .mount(&s)
        .await;
    Mock::given(path("/library/sections/1/all"))
        .and(query_param("X-Plex-Container-Start", "0"))
        .and(query_param("sort", "addedAt:desc"))
        .and(header("X-Plex-Token", "server-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {
            "totalSize": 776, "offset": 0,
            "Metadata": [{
                "ratingKey": "175731", "type": "movie", "title": "Arrival", "year": 2016,
                "duration": 6_996_000, "audienceRating": 8.1, "viewOffset": 60_000, "viewCount": 1,
                "lastViewedAt": 1_790_000_000,
                "thumb": "/library/metadata/175731/thumb/1", "art": "/library/metadata/175731/art/1",
                "Genre": [{"tag": "Science-Fiction"}],
                "Guid": [{"id": "imdb://tt2543164"}, {"id": "tmdb://329865"}]
            }]
        }})))
        .mount(&s)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http());
    let libs = c.libraries().await.unwrap();
    assert_eq!(libs[1].kind, LibraryKind::Shows);
    let mut q = ItemQuery::library("1", 0, 50);
    q.sort_by = SortBy::DateAdded;
    q.descending = true;
    let page = c.items(&q).await.unwrap();
    assert_eq!(page.total, 776);
    let it = &page.items[0];
    assert_eq!(it.provider_ids.tmdb.as_deref(), Some("329865"));
    assert_eq!(it.provider_ids.imdb.as_deref(), Some("tt2543164"));
    assert_eq!(it.genres, vec!["Science-Fiction"]);
    assert!(it.user.played);
    assert_eq!(it.user.position_ms, 60_000);
    let img = c.image_request(&it.id, &it.images[0], 200);
    assert_eq!(
        img.url,
        format!(
            "{}/photo/:/transcode?width=200&height=300&minSize=1&upscale=1&url=%2Flibrary%2Fmetadata%2F175731%2Fthumb%2F1",
            s.uri()
        )
    );
    assert!(!img.url.contains("server-token"));
}

#[tokio::test]
async fn on_deck_splits_resume_and_next_up() {
    let s = MockServer::start().await;
    Mock::given(path("/library/onDeck"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {"Metadata": [
            {"ratingKey": "1", "type": "movie", "title": "Half watched", "viewOffset": 1000},
            {"ratingKey": "2", "type": "episode", "title": "Next ep", "grandparentRatingKey": "10",
             "parentRatingKey": "11", "grandparentTitle": "Show", "parentIndex": 2, "index": 3,
             "grandparentThumb": "/s/thumb"}
        ]}})))
        .mount(&s)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http());
    let resume = c.resume(10).await.unwrap();
    assert_eq!(resume.len(), 1);
    assert_eq!(resume[0].id, "1");
    let next = c.next_up(10).await.unwrap();
    assert_eq!(next.len(), 1);
    let e = &next[0];
    assert_eq!(e.series_id.as_deref(), Some("10"));
    assert_eq!(e.season_id.as_deref(), Some("11"));
    assert_eq!((e.season_number, e.episode_number), (Some(2), Some(3)));
    assert_eq!(e.series_title.as_deref(), Some("Show"));
    assert_eq!(
        e.images[0].path, "/s/thumb",
        "episode without thumb uses the show poster"
    );
}

#[tokio::test]
async fn playback_info_direct_play_and_download() {
    let s = MockServer::start().await;
    Mock::given(path("/library/metadata/175731"))
        .and(query_param("checkFiles", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {"Metadata": [{
            "ratingKey": "175731", "type": "movie", "title": "Arrival",
            "Media": [{
                "id": 9001, "duration": 6_996_000, "bitrate": 20_000, "container": "mkv",
                "Part": [{
                    "key": "/library/parts/555/1690000/file.mkv", "file": "/media/movies/Arrival.mkv",
                    "size": 4_000_000_000u64, "container": "mkv",
                    "Stream": [
                        {"id": 1, "streamType": 1, "codec": "hevc", "colorTrc": "smpte2084", "width": 3840, "height": 2160},
                        {"id": 2, "streamType": 2, "codec": "eac3", "channels": 6, "languageCode": "eng"},
                        {"id": 3, "streamType": 2, "codec": "dca", "channels": 6, "languageCode": "fra", "default": true},
                        {"id": 4, "streamType": 3, "codec": "srt", "key": "/library/streams/4", "languageCode": "fra"}
                    ]
                }]
            }]
        }]}})))
        .mount(&s)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http());
    let empty = DeviceProfile {
        max_bitrate: None,
        containers: vec![],
        video_codecs: vec![],
        video_10bit_codecs: vec![],
        audio_codecs: vec![],
        subtitle_codecs: vec![],
        max_width: None,
        max_height: None,
        hdr: vec![],
    };
    let info = c.playback_info("175731", &empty).await.unwrap();
    let src = &info.sources[0];
    assert_eq!(src.id, "9001");
    assert_eq!(src.bitrate, Some(20_000_000));
    assert_eq!(
        src.direct.url,
        format!("{}/library/parts/555/1690000/file.mkv", s.uri())
    );
    assert_eq!(src.streams[0].hdr, Some(HdrKind::Hdr10));
    assert_eq!(src.default_audio_index, Some(3));
    assert_eq!(
        src.streams[3].external.as_ref().unwrap().url,
        format!("{}/library/streams/4", s.uri())
    );
    assert!(src.transcode.as_ref().unwrap().url.contains(
        "/video/:/transcode/universal/start.m3u8?hasMDE=1&path=%2Flibrary%2Fmetadata%2F175731"
    ));

    let d = c.download_source("175731").await.unwrap();
    assert_eq!(d.file_name, "Arrival.mkv");
    assert!(
        d.request
            .url
            .ends_with("/library/parts/555/1690000/file.mkv?download=1")
    );
}

#[tokio::test]
async fn timeline_and_scrobble() {
    let s = MockServer::start().await;
    Mock::given(path("/:/timeline"))
        .and(query_param("ratingKey", "1"))
        .and(query_param("state", "paused"))
        .and(query_param("time", "5000"))
        .and(query_param("duration", "10000"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(path("/:/scrobble"))
        .and(query_param("key", "1"))
        .and(query_param("identifier", "com.plexapp.plugins.library"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http());
    c.report_progress(&ProgressReport {
        item_id: "1".into(),
        media_source_id: None,
        play_session_id: None,
        position_ms: 5000,
        duration_ms: Some(10_000),
        event: PlaybackEvent::Pause,
        audio_index: None,
        subtitle_index: None,
    })
    .await
    .unwrap();
    c.set_played("1", true).await.unwrap();
}

#[tokio::test]
async fn expired_token_maps_to_unauthorized() {
    let s = MockServer::start().await;
    Mock::given(path("/library/sections"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http());
    assert_eq!(c.libraries().await.unwrap_err(), Error::Unauthorized);
}

#[tokio::test]
async fn plex_home_users_and_switch_with_pin() {
    let tv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/home/users"))
        .and(header("X-Plex-Token", "owner"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 1, "name": "Home",
            "users": [
                {"id": 1, "uuid": "u-owner", "title": "Owner", "admin": true, "protected": false, "thumb": "https://plex.tv/a.png"},
                {"id": 2, "uuid": "u-me", "title": "", "username": "me", "admin": false, "protected": true}
            ]
        })))
        .mount(&tv)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v2/home/users/u-me/switch"))
        .and(query_param("pin", "1234"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"id": 2, "authToken": "me-token"})),
        )
        .mount(&tv)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v2/home/users/u-me/switch"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&tv)
        .await;
    let auth = PlexAuth::with_plex_tv(http(), client_info(), &tv.uri());
    let users = auth.home_users("owner").await.unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[1].title, "me", "falls back to the username");
    assert!(users[1].protected && users[0].admin);
    assert_eq!(
        auth.switch_home_user("owner", "u-me", Some("1234"))
            .await
            .unwrap(),
        "me-token"
    );
    assert_eq!(
        auth.switch_home_user("owner", "u-me", Some("0000")).await,
        Err(net::Error::Unauthorized),
        "wrong PIN"
    );
}

#[tokio::test]
async fn intro_and_credits_markers() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/library/metadata/77"))
        .and(query_param("includeMarkers", "1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {
                "Metadata": [{"ratingKey": "77", "type": "episode", "title": "Ep", "Marker": [
                    {"type": "credits", "startTimeOffset": 2_500_000, "endTimeOffset": 2_600_000},
                    {"type": "intro", "startTimeOffset": 30_000, "endTimeOffset": 95_000},
                    {"type": "bookmark", "startTimeOffset": 1, "endTimeOffset": 2}
                ]}]
            }})),
        )
        .mount(&s)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http());
    let segs = c.segments("77").await.unwrap();
    assert_eq!(
        segs,
        vec![
            Segment {
                kind: SegmentKind::Intro,
                start_ms: 30_000,
                end_ms: 95_000
            },
            Segment {
                kind: SegmentKind::Credits,
                start_ms: 2_500_000,
                end_ms: 2_600_000
            },
        ]
    );
}

#[tokio::test]
async fn people_and_the_plex_tv_watchlist() {
    let s = MockServer::start().await;
    let tv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/library/metadata/77"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {
                "Metadata": [{"ratingKey": "77", "type": "movie", "title": "Arrival",
                    "guid": "plex://movie/5d776b59ad5437001f79c6f8",
                    "Role": [{"tag": "Amy Adams", "role": "Louise Banks",
                        "thumb": "https://metadata-static.plex.tv/a.jpg"},
                        {"tag": "Jeremy Renner", "role": "Ian", "thumb": "/library/x"}],
                    "Director": [{"tag": "Denis Villeneuve"}]}]
            }})),
        )
        .mount(&s)
        .await;
    // The watchlist comes from plex.tv with the account token, not the server's.
    Mock::given(method("GET"))
        .and(path("/library/sections/watchlist/all"))
        .and(header("X-Plex-Token", "account-token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {"Metadata": [
                {"ratingKey": "5d776b59ad5437001f79c6f8", "type": "movie", "title": "Arrival",
                 "guid": "plex://movie/5d776b59ad5437001f79c6f8"},
                {"ratingKey": "abc", "type": "movie", "title": "Not here", "guid": "plex://movie/abc"}
            ]}})),
        )
        .mount(&tv)
        .await;
    Mock::given(method("GET"))
        .and(path("/library/all"))
        .and(query_param("guid", "plex://movie/5d776b59ad5437001f79c6f8"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {
            "Metadata": [{"ratingKey": "77", "type": "movie", "title": "Arrival"}]}})),
        )
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/library/all"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {}})))
        .mount(&s)
        .await;
    Mock::given(method("PUT"))
        .and(path("/actions/removeFromWatchlist"))
        .and(query_param("ratingKey", "5d776b59ad5437001f79c6f8"))
        .and(header("X-Plex-Token", "account-token"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&tv)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http()).with_discover(&tv.uri());
    let people = c.people("77").await.unwrap();
    assert_eq!(people.len(), 3);
    assert_eq!(people[0].name, "Amy Adams");
    assert_eq!(people[0].role.as_deref(), Some("Louise Banks"));
    // Server-relative photos need a token: dropped.
    assert_eq!(people[1].photo_url, None);
    assert_eq!(people[2].kind, PersonKind::Director);

    let list = c.watchlist(Some("account-token")).await.unwrap();
    assert_eq!(
        list.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        ["77"]
    );
    assert!(c.watchlist(None).await.unwrap().is_empty());
    c.set_watchlisted("77", false, Some("account-token"))
        .await
        .unwrap();
}

#[tokio::test]
async fn trailers_and_theme_song() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/library/metadata/9/extras"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {
            "Metadata": [
                {"ratingKey": "101", "type": "clip", "title": "Trailer", "extraType": 1},
                {"ratingKey": "102", "type": "clip", "title": "Behind the scenes", "extraType": 5}
            ]}})),
        )
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/library/metadata/9"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {
            "Metadata": [{"ratingKey": "9", "type": "show", "title": "Silo",
                "theme": "/library/metadata/9/theme/1700000000"}]}})),
        )
        .mount(&s)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http());
    let t = c.trailers("9").await.unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].item_id.as_deref(), Some("101"));
    let theme = c.theme_song("9").await.unwrap().unwrap();
    assert_eq!(
        theme.url,
        format!("{}/library/metadata/9/theme/1700000000", s.uri())
    );
    // The token stays in headers, never in the URL.
    assert!(theme.headers.iter().any(|h| h.name == "X-Plex-Token"));
}

#[tokio::test]
async fn watch_history_and_setting_watch_state() {
    let s = MockServer::start().await;
    Mock::given(path("/library/sections"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"MediaContainer": {"Directory": [
                {"key": "1", "title": "Films", "type": "movie"},
                {"key": "4", "title": "Séries", "type": "show"}
            ]}}),
        ))
        .mount(&s)
        .await;
    Mock::given(path("/library/sections/1/all"))
        .and(query_param("type", "1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {
            "Metadata": [
                {"ratingKey": "10", "type": "movie", "title": "Seen", "viewCount": 2,
                 "Guid": [{"id": "tmdb://1"}]},
                {"ratingKey": "11", "type": "movie", "title": "Never"}
            ]}})),
        )
        .mount(&s)
        .await;
    Mock::given(path("/library/sections/4/all"))
        .and(query_param("type", "4"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": {
            "Metadata": [
                {"ratingKey": "20", "type": "episode", "title": "Pilot", "viewOffset": 120_000,
                 "grandparentRatingKey": "200", "grandparentTitle": "Show",
                 "parentIndex": 1, "index": 1}
            ]}})),
        )
        .mount(&s)
        .await;
    Mock::given(path("/:/scrobble"))
        .and(query_param("key", "10"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(path("/:/unscrobble"))
        .and(query_param("key", "20"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(path("/:/progress"))
        .and(query_param("key", "20"))
        .and(query_param("time", "120000"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    let c = PlexClient::new(session(&s.uri()), client_info(), http());
    let h = c.watch_history().await.unwrap();
    assert_eq!(h.len(), 2, "only watched or started items");
    assert!(h[0].user.played);
    assert_eq!(h[1].series_id.as_deref(), Some("200"));
    assert_eq!(
        (h[1].season_number, h[1].episode_number),
        (Some(1), Some(1))
    );
    let played = WatchState {
        played: true,
        position_ms: 0,
        last_played_at: None,
    };
    c.set_watch_state("10", &played).await.unwrap();
    let started = WatchState {
        played: false,
        position_ms: 120_000,
        last_played_at: None,
    };
    c.set_watch_state("20", &started).await.unwrap();
}
