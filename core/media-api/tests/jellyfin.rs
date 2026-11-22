// SPDX-License-Identifier: AGPL-3.0-or-later
use media_api::jellyfin::{JellyfinAuth, JellyfinClient};
use media_api::*;
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_info() -> ClientInfo {
    ClientInfo {
        product: "AlwyWatch".into(),
        version: "0.1.0".into(),
        device_name: "Test".into(),
        device_id: "dev-1".into(),
        platform: "Linux".into(),
    }
}

fn http() -> net::reqwest::Client {
    net::api_client("AlwyWatch-test").unwrap()
}

fn session(base: &str) -> Session {
    Session {
        kind: ServerKind::Jellyfin,
        base_url: base.into(),
        server_id: "srv".into(),
        server_name: "Test".into(),
        user_id: "u1".into(),
        user_name: "alice".into(),
        is_admin: false,
        token: "secret-token".into(),
    }
}

const AUTH: &str = r#"^MediaBrowser Client="AlwyWatch", Device="Test", DeviceId="dev-1", Version="0.1.0", Token="secret-token"$"#;

#[tokio::test]
async fn sign_in_with_password() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Users/AuthenticateByName"))
        .and(body_partial_json(json!({"Username": "alice", "Pw": "pw"})))
        .and(header_regex(
            "Authorization",
            r#"^MediaBrowser Client="AlwyWatch".*DeviceId="dev-1""#,
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "User": {"Id": "u1", "Name": "alice", "Policy": {"IsAdministrator": true}},
            "AccessToken": "tok", "ServerId": "srv"
        })))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/System/Info/Public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Id": "srv", "ServerName": "HomeJelly", "Version": "12.4.0"
        })))
        .mount(&s)
        .await;

    let auth = JellyfinAuth::new(http(), client_info());
    let sess = auth.sign_in(&s.uri(), "alice", "pw").await.unwrap();
    assert_eq!(sess.user_id, "u1");
    assert_eq!(sess.token, "tok");
    assert_eq!(sess.server_name, "HomeJelly");
    assert!(sess.is_admin);
}

#[tokio::test]
async fn wrong_password_is_unauthorized() {
    let s = MockServer::start().await;
    Mock::given(path("/Users/AuthenticateByName"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    let auth = JellyfinAuth::new(http(), client_info());
    assert_eq!(
        auth.sign_in(&s.uri(), "a", "b").await.unwrap_err(),
        Error::Unauthorized
    );
}

#[tokio::test]
async fn quick_connect_pending_then_authorized() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/QuickConnect/Initiate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Authenticated": false, "Secret": "sec", "Code": "123456"
        })))
        .mount(&s)
        .await;
    let auth = JellyfinAuth::new(http(), client_info());
    let code = auth.quick_connect_start(&s.uri()).await.unwrap();
    assert_eq!(code.code, "123456");

    let pending = Mock::given(method("GET"))
        .and(path("/QuickConnect/Connect"))
        .and(query_param("secret", "sec"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Authenticated": false, "Secret": "sec", "Code": "123456"
        })))
        .mount_as_scoped(&s)
        .await;
    assert_eq!(
        auth.quick_connect_poll(&s.uri(), "sec").await.unwrap_err(),
        Error::Pending
    );
    drop(pending);

    Mock::given(method("GET"))
        .and(path("/QuickConnect/Connect"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Authenticated": true, "Secret": "sec", "Code": "123456"
        })))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/Users/AuthenticateWithQuickConnect"))
        .and(body_partial_json(json!({"Secret": "sec"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "User": {"Id": "u2", "Name": "bob"}, "AccessToken": "tok2", "ServerId": "srv"
        })))
        .mount(&s)
        .await;
    let sess = auth.quick_connect_poll(&s.uri(), "sec").await.unwrap();
    assert_eq!(sess.user_name, "bob");
    assert!(!sess.is_admin);
}

#[tokio::test]
async fn libraries_and_paged_items() {
    let s = MockServer::start().await;
    Mock::given(path("/UserViews"))
        .and(header_regex("Authorization", AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Id": "m", "Name": "Films", "CollectionType": "movies"},
            {"Id": "t", "Name": "Séries", "CollectionType": "tvshows"},
            {"Id": "x", "Name": "Music", "CollectionType": "music"}
        ]})))
        .mount(&s)
        .await;
    Mock::given(path("/Items"))
        .and(query_param("parentId", "m"))
        .and(query_param("startIndex", "50"))
        .and(query_param("limit", "50"))
        .and(query_param("sortBy", "SortName"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "TotalRecordCount": 776, "StartIndex": 50,
            "Items": [{
                "Id": "i1", "Name": "Arrival", "Type": "Movie", "ProductionYear": 2016,
                "RunTimeTicks": 69_960_000_000i64, "CommunityRating": 7.6,
                "ProviderIds": {"Tmdb": "329865", "Imdb": "tt2543164", "Tvdb": null},
                "ImageTags": {"Primary": "ptag"},
                "BackdropImageTags": ["btag"],
                "ImageBlurHashes": {"Primary": {"ptag": "LEHV6nWB2yk8"}},
                "UserData": {"Played": false, "PlaybackPositionTicks": 12_000_000_000i64, "PlayCount": 0,
                             "IsFavorite": true, "LastPlayedDate": "2026-11-01T20:00:00Z"}
            }]
        })))
        .mount(&s)
        .await;

    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let libs = c.libraries().await.unwrap();
    assert_eq!(libs.len(), 3);
    assert_eq!(libs[0].kind, LibraryKind::Movies);
    assert_eq!(libs[1].kind, LibraryKind::Shows);
    assert_eq!(libs[2].kind, LibraryKind::Other);

    let page = c.items(&ItemQuery::library("m", 50, 50)).await.unwrap();
    assert_eq!(page.total, 776);
    assert_eq!(page.start, 50);
    let it = &page.items[0];
    assert_eq!(it.kind, ItemKind::Movie);
    assert_eq!(it.runtime_ms, Some(6_996_000));
    assert_eq!(it.provider_ids.tmdb.as_deref(), Some("329865"));
    assert_eq!(it.provider_ids.tvdb, None);
    assert_eq!(it.user.position_ms, 1_200_000);
    assert!(it.user.favorite);
    let poster = it
        .images
        .iter()
        .find(|i| i.kind == ImageKind::Primary)
        .unwrap();
    assert_eq!(poster.path, "/Items/i1/Images/Primary?tag=ptag");
    assert_eq!(poster.blurhash.as_deref(), Some("LEHV6nWB2yk8"));
    let req = c.image_request("i1", poster, 300);
    assert_eq!(
        req.url,
        format!(
            "{}/Items/i1/Images/Primary?tag=ptag&fillWidth=300&quality=90",
            s.uri()
        )
    );
    assert!(
        !req.url.contains("secret-token"),
        "token must not leak into URLs"
    );
}

#[tokio::test]
async fn episode_falls_back_to_series_images() {
    let s = MockServer::start().await;
    Mock::given(path("/Shows/NextUp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": [{
            "Id": "e1", "Name": "Pilot", "Type": "Episode", "SeriesId": "s1", "SeriesName": "Show",
            "SeasonId": "se1", "ParentIndexNumber": 1, "IndexNumber": 1,
            "SeriesPrimaryImageTag": "stag", "ParentBackdropItemId": "s1",
            "ParentBackdropImageTags": ["sb"]
        }]})))
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let items = c.next_up(10).await.unwrap();
    let e = &items[0];
    assert_eq!((e.season_number, e.episode_number), (Some(1), Some(1)));
    let img = |k: ImageKind| {
        e.images
            .iter()
            .find(|i| i.kind == k)
            .map(|i| i.path.as_str())
    };
    assert_eq!(
        img(ImageKind::Primary),
        Some("/Items/s1/Images/Primary?tag=stag")
    );
    assert_eq!(
        img(ImageKind::SeriesPrimary),
        Some("/Items/s1/Images/Primary?tag=stag")
    );
    assert_eq!(
        img(ImageKind::Backdrop),
        Some("/Items/s1/Images/Backdrop?tag=sb")
    );
}

#[tokio::test]
async fn playback_info_prefers_direct_play_and_keeps_token_in_headers() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Items/i1/PlaybackInfo"))
        .and(body_partial_json(json!({"EnableDirectPlay": true, "UserId": "u1"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "PlaySessionId": "ps1",
            "MediaSources": [
                {"Id": "t", "SupportsDirectPlay": false, "TranscodingUrl": "/videos/i1/master.m3u8?x=1"},
                {"Id": "ms1", "Container": "mkv", "Size": 4_000_000_000u64, "Bitrate": 20_000_000,
                 "RunTimeTicks": 69_960_000_000i64, "SupportsDirectPlay": true,
                 "Path": "/media/movies/Arrival (2016)/Arrival.mkv",
                 "DefaultAudioStreamIndex": 1,
                 "MediaStreams": [
                    {"Index": 0, "Type": "Video", "Codec": "HEVC", "VideoRangeType": "DOVIWithHDR10",
                     "DvProfile": 8, "Width": 3840, "Height": 2160, "BitDepth": 10},
                    {"Index": 1, "Type": "Audio", "Codec": "truehd", "Channels": 8, "Language": "eng", "IsDefault": true},
                    {"Index": 2, "Type": "Subtitle", "Codec": "srt", "Language": "fre", "IsExternal": true},
                    {"Index": 3, "Type": "Attachment"}
                 ]}
            ]
        })))
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let profile = DeviceProfile {
        max_bitrate: Some(120_000_000),
        containers: vec!["mkv".into(), "mp4".into()],
        video_codecs: vec!["hevc".into(), "h264".into()],
        video_10bit_codecs: vec!["hevc".into()],
        audio_codecs: vec!["truehd".into(), "eac3".into()],
        subtitle_codecs: vec!["srt".into(), "pgssub".into()],
        max_width: None,
        max_height: None,
        hdr: vec![HdrKind::DolbyVision, HdrKind::Hdr10],
    };
    let info = c.playback_info("i1", &profile).await.unwrap();
    assert_eq!(info.play_session_id.as_deref(), Some("ps1"));
    let src = &info.sources[0];
    assert_eq!(src.id, "ms1", "direct-play source sorted first");
    assert_eq!(src.file_name.as_deref(), Some("Arrival.mkv"));
    assert_eq!(src.duration_ms, Some(6_996_000));
    assert_eq!(src.streams.len(), 3, "attachments are dropped");
    assert_eq!(src.streams[0].hdr, Some(HdrKind::DolbyVision));
    assert_eq!(src.streams[0].codec, "hevc");
    let sub = &src.streams[2];
    assert!(sub.is_external);
    assert_eq!(
        sub.external.as_ref().unwrap().url,
        format!("{}/Videos/i1/ms1/Subtitles/2/0/Stream.srt", s.uri())
    );
    assert_eq!(
        src.direct.url,
        format!(
            "{}/Videos/i1/stream?static=true&mediaSourceId=ms1&playSessionId=ps1",
            s.uri()
        )
    );
    assert!(
        src.direct
            .headers
            .iter()
            .any(|h| h.name == "Authorization" && h.value.contains("secret-token"))
    );
    assert_eq!(
        info.sources[1].transcode.as_ref().unwrap().url,
        format!("{}/videos/i1/master.m3u8?x=1", s.uri())
    );
}

#[tokio::test]
async fn progress_reports_use_ticks_and_right_endpoints() {
    let s = MockServer::start().await;
    for (p, paused) in [
        ("/Sessions/Playing", false),
        ("/Sessions/Playing/Progress", true),
        ("/Sessions/Playing/Stopped", false),
    ] {
        Mock::given(method("POST"))
            .and(path(p))
            .and(body_partial_json(
                json!({"ItemId": "i1", "PositionTicks": 600_000_000i64, "IsPaused": paused}),
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
    }
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    for ev in [
        PlaybackEvent::Start,
        PlaybackEvent::Pause,
        PlaybackEvent::Stop,
    ] {
        c.report_progress(&ProgressReport {
            item_id: "i1".into(),
            media_source_id: Some("ms1".into()),
            play_session_id: Some("ps1".into()),
            position_ms: 60_000,
            duration_ms: None,
            event: ev,
            audio_index: None,
            subtitle_index: None,
            play_method: PlayMethod::Direct,
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn report_says_when_the_stream_is_converted() {
    let s = MockServer::start().await;
    for (play_method, m) in [
        ("DirectPlay", PlayMethod::Direct),
        ("Transcode", PlayMethod::Transcode),
    ] {
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing/Progress"))
            .and(body_partial_json(json!({
                "ItemId": "i1", "PlaySessionId": "ps1", "PlayMethod": play_method,
                "AudioStreamIndex": 2, "SubtitleStreamIndex": 5,
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
        let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
        c.report_progress(&ProgressReport {
            item_id: "i1".into(),
            media_source_id: Some("ms1".into()),
            play_session_id: Some("ps1".into()),
            position_ms: 1000,
            duration_ms: None,
            event: PlaybackEvent::Progress,
            audio_index: Some(2),
            subtitle_index: Some(5),
            play_method: m,
        })
        .await
        .unwrap();
    }
}

fn plain_profile() -> DeviceProfile {
    DeviceProfile {
        max_bitrate: None,
        containers: vec!["mp4".into()],
        video_codecs: vec!["h264".into()],
        video_10bit_codecs: vec![],
        audio_codecs: vec!["aac".into()],
        subtitle_codecs: vec!["srt".into()],
        max_width: None,
        max_height: None,
        hdr: vec![],
    }
}

#[tokio::test]
async fn chosen_tracks_go_into_the_playback_info_request() {
    let s = MockServer::start().await;
    let answer = |url: &str| {
        ResponseTemplate::new(200).set_body_json(json!({
            "PlaySessionId": "ps2",
            "MediaSources": [{"Id": "ms1", "SupportsDirectPlay": false, "TranscodingUrl": url}]
        }))
    };
    // A subtitle stream: asked by index, burned into the conversion.
    Mock::given(method("POST"))
        .and(path("/Items/i1/PlaybackInfo"))
        .and(body_partial_json(json!({
            "MediaSourceId": "ms1", "AudioStreamIndex": 2, "SubtitleStreamIndex": 4,
            "AlwaysBurnInSubtitleWhenTranscoding": true, "EnableTranscoding": true,
        })))
        .respond_with(answer(
            "/videos/i1/master.m3u8?AudioStreamIndex=2&SubtitleStreamIndex=4&SubtitleMethod=Encode",
        ))
        .expect(1)
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let info = c
        .playback_info_with(
            "i1",
            &plain_profile(),
            &TrackSelection {
                source_id: Some("ms1".into()),
                audio_index: Some(2),
                subtitle: SubtitleSelection::Stream { index: 4 },
            },
        )
        .await
        .unwrap();
    assert_eq!(info.play_session_id.as_deref(), Some("ps2"));
    assert_eq!(
        info.sources[0].transcode.as_ref().unwrap().url,
        format!(
            "{}/videos/i1/master.m3u8?AudioStreamIndex=2&SubtitleStreamIndex=4&SubtitleMethod=Encode",
            s.uri()
        )
    );
    s.reset().await;

    // Off is Jellyfin's -1, nothing burned; the default selection adds nothing at all.
    Mock::given(method("POST"))
        .and(path("/Items/i1/PlaybackInfo"))
        .respond_with(answer("/videos/i1/master.m3u8"))
        .mount(&s)
        .await;
    let off = TrackSelection {
        subtitle: SubtitleSelection::Off,
        ..Default::default()
    };
    c.playback_info_with("i1", &plain_profile(), &off)
        .await
        .unwrap();
    c.playback_info("i1", &plain_profile()).await.unwrap();
    let bodies: Vec<serde_json::Value> = s
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[0]["SubtitleStreamIndex"], json!(-1));
    for key in [
        "AudioStreamIndex",
        "AlwaysBurnInSubtitleWhenTranscoding",
        "MediaSourceId",
    ] {
        assert!(bodies[0].get(key).is_none(), "{key}");
    }
    for key in [
        "AudioStreamIndex",
        "SubtitleStreamIndex",
        "AlwaysBurnInSubtitleWhenTranscoding",
        "MediaSourceId",
    ] {
        assert!(bodies[1].get(key).is_none(), "default: {key}");
    }
}

#[tokio::test]
async fn played_toggle_and_download_source() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/UserPlayedItems/i1"))
        .and(query_param("userId", "u1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/UserPlayedItems/i1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items/i1"))
        .and(query_param("fields", "MediaSources,Path"))
        .and(header("Accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Id": "i1", "Type": "Movie",
            "MediaSources": [{"Id": "ms1", "Size": 123, "Path": "C:\\media\\Film.mkv"}]
        })))
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    c.set_played("i1", true).await.unwrap();
    c.set_played("i1", false).await.unwrap();
    let d = c.download_source("i1").await.unwrap();
    assert_eq!(d.file_name, "Film.mkv");
    assert_eq!(d.size, Some(123));
    assert_eq!(d.request.url, format!("{}/Items/i1/Download", s.uri()));
}

#[tokio::test]
async fn genres_collections_and_their_items() {
    let s = MockServer::start().await;
    Mock::given(path("/Genres"))
        .and(wiremock::matchers::query_param("parentId", "lib"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Items": [{"Id": "g1", "Name": "Action", "Type": "Genre"}]})),
        )
        .mount(&s)
        .await;
    Mock::given(path("/Items"))
        .and(wiremock::matchers::query_param(
            "includeItemTypes",
            "BoxSet",
        ))
        .and(wiremock::matchers::query_param("parentId", "lib"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Items": [{"Id": "c1", "Name": "Saga", "Type": "BoxSet"}]})),
        )
        .mount(&s)
        .await;
    Mock::given(path("/Items"))
        .and(wiremock::matchers::query_param("parentId", "c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"Items": [{"Id": "m1", "Name": "Part 1", "Type": "Movie"}], "TotalRecordCount": 1}),
        ))
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let g = c.genres("lib").await.unwrap();
    assert_eq!((g[0].id.as_str(), g[0].name.as_str()), ("g1", "Action"));
    let cols = c.collections("lib").await.unwrap();
    assert_eq!(cols[0].kind, ItemKind::Collection);
    let mut q = ItemQuery::library("lib", 0, 20);
    q.collection_id = Some("c1".into());
    assert_eq!(c.items(&q).await.unwrap().items[0].id, "m1");
}

#[tokio::test]
async fn media_segments() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/MediaSegments/e1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Type": "Outro", "StartTicks": 25_000_000_000_i64, "EndTicks": 26_000_000_000_i64},
            {"Type": "Intro", "StartTicks": 0, "EndTicks": 900_000_000},
            {"Type": "Unknown", "StartTicks": 1, "EndTicks": 2}
        ], "TotalRecordCount": 3})))
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let segs = c.segments("e1").await.unwrap();
    assert_eq!(
        segs,
        vec![
            Segment {
                kind: SegmentKind::Intro,
                start_ms: 0,
                end_ms: 90_000
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
async fn people_and_favorites() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/m1"))
        .and(query_param("fields", "People"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Id": "m1", "People": [
                {"Name": "Denis Villeneuve", "Id": "p2", "Type": "Director"},
                {"Name": "Amy Adams", "Id": "p1", "Role": "Louise Banks", "Type": "Actor",
                 "PrimaryImageTag": "t1"}
            ]})),
        )
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("filters", "IsFavorite"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Id": "m1", "Name": "Arrival", "Type": "Movie", "UserData": {"IsFavorite": true}}
        ], "TotalRecordCount": 1})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/UserFavoriteItems/m1"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let people = c.people("m1").await.unwrap();
    assert_eq!(people[0].name, "Amy Adams");
    assert_eq!(
        people[0].photo_url.as_deref(),
        Some(format!("{}/Items/p1/Images/Primary?tag=t1&maxWidth=240", s.uri()).as_str())
    );
    assert_eq!(people[1].kind, PersonKind::Director);
    let fav = c.watchlist(None).await.unwrap();
    assert_eq!(fav[0].id, "m1");
    c.set_watchlisted("m1", true, None).await.unwrap();
}

#[tokio::test]
async fn watch_history_and_setting_watch_state() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("filters", "IsPlayed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Id": "m1", "Name": "Arrival", "Type": "Movie", "UserData": {"Played": true}}
        ]})))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("filters", "IsResumable"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": [
            {"Id": "e1", "Name": "Pilot", "Type": "Episode", "SeriesId": "s1",
             "ParentIndexNumber": 1, "IndexNumber": 1,
             "UserData": {"PlaybackPositionTicks": 1_200_000_000}}
        ]})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/UserItems/e1/UserData"))
        .and(body_partial_json(
            json!({"Played": false, "PlaybackPositionTicks": 1_200_000_000,
            "LastPlayedDate": "2023-11-14T22:13:20.0000000Z"}),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let h = c.watch_history().await.unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[1].user.position_ms, 120_000);
    let st = WatchState {
        played: false,
        position_ms: 120_000,
        last_played_at: Some(1_700_000_000),
    };
    c.set_watch_state("e1", &st).await.unwrap();
}

#[tokio::test]
async fn latest_lists_episodes_one_by_one_and_groups_per_show() {
    let s = MockServer::start().await;
    let ep = |id: &str, show: &str, name: &str, s: u32, e: u32| {
        json!({
            "Id": id, "Name": name, "Type": "Episode", "SeriesId": format!("{show}-id"),
            "SeriesName": show, "SeasonId": format!("{show}-s{s}"), "ParentIndexNumber": s,
            "IndexNumber": e, "SeriesPrimaryImageTag": "stag"
        })
    };
    Mock::given(path("/Items/Latest"))
        .and(query_param("parentId", "shows"))
        .and(query_param("groupItems", "false"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            ep("e1", "Dreamland", "Edenia", 1, 6),
            ep("e2", "Percy", "Two", 3, 2),
            ep("e3", "Percy", "One", 3, 1),
        ])))
        .mount(&s)
        .await;
    let c = JellyfinClient::new(session(&s.uri()), client_info(), http());
    let r = recently_added(c.latest("shows", 100).await.unwrap(), 20);
    let labels: Vec<(&str, &str, &str)> = r
        .notes
        .iter()
        .map(|n| (n.item_id.as_str(), n.label.as_str(), n.open_id.as_str()))
        .collect();
    assert_eq!(
        labels,
        [
            ("Dreamland-id", "S1 · E6 · Edenia", "e1"),
            ("Percy-id", "S3 · E1–E2", "Percy-id"),
        ]
    );
    assert_eq!(r.items[0].kind, ItemKind::Series);
    assert_eq!(r.items[0].title, "Dreamland");
}
