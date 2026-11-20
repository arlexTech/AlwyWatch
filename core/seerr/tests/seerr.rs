// SPDX-License-Identifier: AGPL-3.0-or-later
use seerr::*;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http() -> net::reqwest::Client {
    net::api_client("test").unwrap()
}

fn user() -> serde_json::Value {
    json!({"id": 3, "displayName": "", "plexUsername": "alice", "permissions": 32})
}

#[tokio::test]
async fn info_reports_plex_mode_and_logins() {
    let s = MockServer::start().await;
    Mock::given(path("/api/v1/settings/public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "initialized": true, "applicationTitle": "Seerr", "localLogin": false,
            "mediaServerLogin": true, "mediaServerType": 1, "cacheImages": true
        })))
        .mount(&s)
        .await;
    let info = SeerrAuth::new(http()).info(&s.uri()).await.unwrap();
    assert_eq!(info.logins, vec![SeerrLogin::Plex]);
    assert!(info.cache_images);
}

#[tokio::test]
async fn plex_sign_in_keeps_session_cookie() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/plex"))
        .and(body_json(json!({"authToken": "acct"})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "set-cookie",
                    "connect.sid=s%3Aabc.def; Path=/; HttpOnly; SameSite=Lax",
                )
                .set_body_json(user()),
        )
        .mount(&s)
        .await;
    let sess = SeerrAuth::new(http())
        .sign_in_plex(&s.uri(), "acct")
        .await
        .unwrap();
    assert_eq!(sess.cookie, "connect.sid=s%3Aabc.def");
    assert_eq!(sess.user.display_name, "alice");
}

#[tokio::test]
async fn jellyfin_quick_connect_flow() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/jellyfin/quickconnect/initiate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"code": "123456", "secret": "sec"})),
        )
        .mount(&s)
        .await;
    let auth = SeerrAuth::new(http());
    let qc = auth.quick_connect_start(&s.uri()).await.unwrap();
    let waiting = Mock::given(path("/api/v1/auth/jellyfin/quickconnect/check"))
        .and(query_param("secret", "sec"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"authenticated": false})))
        .mount_as_scoped(&s)
        .await;
    assert_eq!(
        auth.quick_connect_poll(&s.uri(), &qc.secret)
            .await
            .unwrap_err(),
        net::Error::Pending
    );
    drop(waiting);
    Mock::given(path("/api/v1/auth/jellyfin/quickconnect/check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"authenticated": true})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/jellyfin/quickconnect/authenticate"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "connect.sid=x; Path=/")
                .set_body_json(user()),
        )
        .mount(&s)
        .await;
    assert_eq!(
        auth.quick_connect_poll(&s.uri(), "sec")
            .await
            .unwrap()
            .cookie,
        "connect.sid=x"
    );
}

#[tokio::test]
async fn search_encodes_query_and_maps_results() {
    let s = MockServer::start().await;
    Mock::given(path("/api/v1/search"))
        .and(query_param("query", "the bear"))
        .and(header("cookie", "connect.sid=x"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"page": 1, "results": [
            {"id": 136315, "mediaType": "tv", "name": "The Bear", "firstAirDate": "2022-06-23",
             "posterPath": "/p.jpg", "mediaInfo": {"status": 4}},
            {"id": 1, "mediaType": "person", "name": "Someone"},
            {"id": 550, "mediaType": "movie", "title": "Fight Club", "releaseDate": "1999-10-15"},
            {"id": 940721, "mediaType": "movie", "title": "Godzilla Minus One", "mediaInfo": {"status": 3,
             "downloadStatus": [{"size": 1000.0, "sizeLeft": 250.0}]}},
            {"id": 7, "mediaType": "movie", "title": "Approved only", "mediaInfo": {"status": 3}}
        ]})))
        .mount(&s)
        .await;
    let c = SeerrClient::new(http(), &s.uri(), "connect.sid=x", true);
    let r = c.search("the bear", 1).await.unwrap();
    assert_eq!(r.len(), 4, "people are skipped");
    // Requested and actually downloading (Radarr's queue) vs requested only.
    assert_eq!(r[2].availability, Availability::Processing);
    assert_eq!(r[2].download_progress, Some(0.75));
    assert_eq!(r[3].download_progress, None);
    assert_eq!(r[0].media_type, SeerrMediaType::Tv);
    assert_eq!(r[0].year.as_deref(), Some("2022"));
    assert_eq!(r[0].availability, Availability::PartiallyAvailable);
    assert_eq!(
        r[0].poster_url.as_deref(),
        Some(format!("{}/imageproxy/tmdb/t/p/w300_and_h450_face/p.jpg", s.uri()).as_str())
    );
    assert_eq!(r[1].availability, Availability::Unknown);
    let raw = &s.received_requests().await.unwrap()[0];
    assert!(
        raw.url.query().unwrap().contains("query=the%20bear"),
        "{}",
        raw.url
    );
}

#[tokio::test]
async fn request_tv_all_seasons_and_movie() {
    let s = MockServer::start().await;
    let created = json!({"id": 9, "status": 1, "createdAt": "2026-11-19T10:00:00.000Z",
        "media": {"mediaType": "tv", "tmdbId": 136315, "status": 2}, "requestedBy": user()});
    Mock::given(method("POST"))
        .and(path("/api/v1/request"))
        .and(body_json(
            json!({"mediaType": "tv", "mediaId": 136315, "is4k": false, "seasons": "all"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/request"))
        .and(body_json(
            json!({"mediaType": "movie", "mediaId": 550, "is4k": false}),
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"id": 10, "status": 2,
            "media": {"mediaType": "movie", "tmdbId": 550, "status": 3}})),
        )
        .expect(1)
        .mount(&s)
        .await;
    let c = SeerrClient::new(http(), &s.uri(), "connect.sid=x", false);
    let r = c.request(SeerrMediaType::Tv, 136315, &[]).await.unwrap();
    assert_eq!(
        (r.id, r.status, r.availability),
        (9, RequestStatus::Pending, Availability::Pending)
    );
    assert_eq!(r.requested_by, "alice");
    let m = c.request(SeerrMediaType::Movie, 550, &[]).await.unwrap();
    assert_eq!(m.status, RequestStatus::Approved);
}

#[tokio::test]
async fn my_requests_with_titles_and_season_list() {
    let s = MockServer::start().await;
    Mock::given(path("/api/v1/auth/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(user()))
        .mount(&s)
        .await;
    Mock::given(path("/api/v1/request"))
        .and(query_param("requestedBy", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"pageInfo": {}, "results": [
            {"id": 9, "status": 5, "createdAt": "x", "media": {"mediaType": "movie", "tmdbId": 550, "status": 5}}
        ]})))
        .mount(&s)
        .await;
    Mock::given(path("/api/v1/movie/550"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"title": "Fight Club", "posterPath": "/f.jpg"})),
        )
        .mount(&s)
        .await;
    Mock::given(path("/api/v1/tv/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "seasons": [{"seasonNumber": 0, "name": "Specials"}, {"seasonNumber": 1, "name": "Season 1"}, {"seasonNumber": 2}],
            "mediaInfo": {"status": 4, "seasons": [{"seasonNumber": 1, "status": 5}]}
        })))
        .mount(&s)
        .await;
    let c = SeerrClient::new(http(), &s.uri(), "connect.sid=x", false);
    let reqs = c.requests(20, false).await.unwrap();
    assert_eq!(reqs[0].status, RequestStatus::Completed);
    assert_eq!(reqs[0].title.as_deref(), Some("Fight Club"));
    assert_eq!(
        reqs[0].poster_url.as_deref(),
        Some("https://image.tmdb.org/t/p/w300_and_h450_face/f.jpg")
    );
    let seasons = c.seasons(1).await.unwrap();
    assert_eq!(seasons.len(), 2, "specials hidden");
    assert_eq!(seasons[0].availability, Availability::Available);
    assert_eq!(seasons[1].name, "Season 2");
    assert_eq!(seasons[1].availability, Availability::Unknown);
}

#[tokio::test]
async fn expired_session_is_unauthorized() {
    let s = MockServer::start().await;
    Mock::given(path("/api/v1/auth/me"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&s)
        .await;
    let c = SeerrClient::new(http(), &s.uri(), "connect.sid=old", false);
    assert_eq!(c.me().await.unwrap_err(), net::Error::Unauthorized);
}
