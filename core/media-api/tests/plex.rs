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
    assert!(
        src.transcode.as_ref().unwrap().url.contains(
            "/video/:/transcode/universal/start.m3u8?path=%2Flibrary%2Fmetadata%2F175731"
        )
    );

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
