// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end tests of the keep service against mocked Maintainerr, Jellyfin, plex.tv and Tautulli.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use keep_service::*;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_json, header, header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::config::{Config, Secrets};
use crate::db::{Db, now};
use crate::keep::{Shared, State};

const DAY: i64 = 86_400;

struct Env {
    maint: MockServer,
    jf: MockServer,
    plextv: MockServer,
    tautulli: MockServer,
    seerr: MockServer,
    state: Shared,
}

async fn env() -> Env {
    env_with(false).await
}

async fn env_with(dry_run: bool) -> Env {
    let seerr = MockServer::start().await;
    let (maint, jf, plextv, tautulli) = (
        MockServer::start().await,
        MockServer::start().await,
        MockServer::start().await,
        MockServer::start().await,
    );
    let toml = format!(
        r#"
        [[servers]]
        id = "plexsrv"
        kind = "plex"
        name = "HomePlex"
        public_url = "https://plex.example"
        [[servers]]
        id = "jfsrv"
        kind = "jellyfin"
        name = "HomeJelly"
        public_url = "https://jf.example"
        internal_url = "{}"
        [maintainerr]
        url = "{}"
        server = "plexsrv"
        [tautulli]
        url = "{}"
        [seerr]
        url = "{}"
        "#,
        jf.uri(),
        maint.uri(),
        tautulli.uri(),
        seerr.uri()
    );
    let mut config = Config::parse(&toml).unwrap();
    config.secrets = Secrets::for_tests("jf-admin-key", "taut-key");
    config.dry_run = dry_run;
    let state = Arc::new(State::new(
        config,
        Db::open_in_memory().unwrap(),
        net::api_client("t").unwrap(),
        &plextv.uri(),
    ));
    let e = Env {
        maint,
        jf,
        plextv,
        tautulli,
        seerr,
        state,
    };
    e.mount_maintainerr().await;
    e.mount_users().await;
    e
}

impl Env {
    async fn mount_maintainerr(&self) {
        let now = now();
        let date = |offset_days: i64| {
            // Maintainerr format, UTC.
            let t = now + offset_days * DAY;
            let days = t.div_euclid(DAY);
            let (y, m, d) = civil(days);
            format!("{y:04}-{m:02}-{d:02} 00:00:00.000")
        };
        Mock::given(path("/api/collections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": 2, "title": "Deletion", "isActive": true, "type": "movie", "deleteAfterDays": 30, "mediaCount": 2},
                {"id": 3, "title": "Series out", "isActive": false, "type": "show", "deleteAfterDays": 30, "mediaCount": 1}
            ])))
            .mount(&self.maint)
            .await;
        // Movie 100 leaves in ~5 days, movie 101 in ~20 days, show 200 is overdue (inactive collection).
        Mock::given(path("/api/collections/media/2/content/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"totalSize": 2, "items": [
                {"mediaServerId": "101", "tmdbId": 11, "addDate": date(-10), "image_path": "/b.jpg"},
                {"mediaServerId": "100", "tmdbId": 10, "addDate": date(-25), "image_path": "/a.jpg"}
            ]})))
            .mount(&self.maint)
            .await;
        Mock::given(path("/api/collections/media/3/content/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"totalSize": 1, "items": [
                {"mediaServerId": "200", "tmdbId": 20, "tvdbId": 2000, "addDate": date(-100), "image_path": "https://image.tmdb.org/t/p/w500/s.jpg"}
            ]})))
            .mount(&self.maint)
            .await;
        for (id, title, kind) in [
            ("100", "Arrival", "movie"),
            ("101", "Dune", "movie"),
            ("200", "The Show", "show"),
        ] {
            Mock::given(path(format!("/api/media-server/meta/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id, "title": title, "year": 2016, "type": kind, "providerIds": {}})))
                .mount(&self.maint)
                .await;
        }
    }

    async fn mount_users(&self) {
        // Jellyfin: "jf-alice" (admin), "jf-bob".
        for (tok, id, name, admin) in [
            ("jf-alice", "ja", "alice", true),
            ("jf-bob", "jb", "bob", false),
        ] {
            Mock::given(path("/Users/Me"))
                // wiremock splits header values on commas, so match the token part only.
                .and(header_regex("authorization", &format!("Token=\"{tok}\"$")))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({"Id": id, "Name": name, "Policy": {"IsAdministrator": admin}}),
                ))
                .mount(&self.jf)
                .await;
        }
        Mock::given(path("/Users/Me"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&self.jf)
            .await;
        // Jellyfin provider index (admin API key).
        Mock::given(path("/Items"))
            .and(header(
                "authorization",
                "MediaBrowser Token=\"jf-admin-key\"",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Items": [
                {"Id": "jf-arrival", "Type": "Movie", "ProviderIds": {"Tmdb": "10"}},
                {"Id": "jf-show", "Type": "Series", "ProviderIds": {"Tvdb": "2000", "Tmdb": "20"}}
            ]})))
            .mount(&self.jf)
            .await;
        // Plex: "px-owner" owns the server, "px-friend" has it shared, "px-stranger" has no access.
        for (tok, id, owned, has) in [
            ("px-owner", 1, true, true),
            ("px-friend", 2, false, true),
            ("px-stranger", 3, false, false),
        ] {
            Mock::given(path("/api/v2/user"))
                .and(header("x-plex-token", tok))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({"id": id, "username": format!("u{id}"), "title": format!("User {id}")}),
                ))
                .mount(&self.plextv)
                .await;
            let resources = if has {
                json!([{"clientIdentifier": "plexsrv", "owned": owned, "provides": "server"}])
            } else {
                json!([])
            };
            Mock::given(path("/api/v2/resources"))
                .and(header("x-plex-token", tok))
                .respond_with(ResponseTemplate::new(200).set_body_json(resources))
                .mount(&self.plextv)
                .await;
        }
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        server: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(SERVER_HEADER, server)
            .header("authorization", format!("Bearer {token}"));
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let res = crate::api::router(self.state.clone())
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

/// Civil date from days since epoch (Howard Hinnant).
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[tokio::test]
async fn authentication_against_allow_listed_servers() {
    let e = env().await;
    let (s, me) = e.call("GET", "/v1/me", "jfsrv", "jf-alice", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["user_key"], "jellyfin:jfsrv:ja");
    assert_eq!(me["is_admin"], true);
    assert_eq!(me["rights"]["can_keep_forever"], true);
    assert_eq!(
        e.call("GET", "/v1/me", "jfsrv", "nope", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        e.call("GET", "/v1/me", "other", "jf-alice", None).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, me) = e.call("GET", "/v1/me", "plexsrv", "px-owner", None).await;
    assert_eq!(
        (s, me["is_admin"].clone(), me["user_key"].clone()),
        (StatusCode::OK, json!(true), json!("plex:plexsrv:1"))
    );
    assert_eq!(
        e.call("GET", "/v1/me", "plexsrv", "px-friend", None)
            .await
            .1["is_admin"],
        false
    );
    assert_eq!(
        e.call("GET", "/v1/me", "plexsrv", "px-stranger", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    // Public server list.
    let res = crate::api::router(e.state.clone())
        .oneshot(Request::get("/v1/servers").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let list: Vec<ServerEntry> = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(list.len(), 2);
}

#[tokio::test]
async fn leaving_soon_sorted_by_urgency_with_titles_and_mapped_ids() {
    let e = env().await;
    assert_eq!(e.state.sync().await.unwrap(), 3);
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "jfsrv", "jf-bob", None)
        .await;
    let items: Vec<LeavingItem> = serde_json::from_value(list).unwrap();
    let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["200", "100", "101"],
        "overdue first, then soonest"
    );
    assert_eq!(items[0].title, "The Show");
    assert!(!items[0].collection_active, "inactive collection flagged");
    assert_eq!(
        items[1].poster_url.as_deref(),
        Some("https://image.tmdb.org/t/p/w300/a.jpg")
    );
    assert_eq!(
        items[1].server_item_id.as_deref(),
        Some("jf-arrival"),
        "mapped through TMDB"
    );
    assert_eq!(
        items[0].server_item_id.as_deref(),
        Some("jf-show"),
        "mapped through TVDB"
    );
    assert_eq!(items[2].server_item_id, None, "not on this Jellyfin");
    // Plex users of the managed server get the same ids.
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(list[1]["server_item_id"], "100");
}

#[tokio::test]
async fn keep_days_postpones_by_the_right_amount() {
    let e = env().await;
    e.state.sync().await.unwrap();
    // Movie 100 is deleted in ~5 days; keeping it 30 more days asks for +30.
    Mock::given(method("POST"))
        .and(path("/api/collections/media/postpone"))
        .and(body_json(
            json!({"collectionId": 2, "mediaId": "100", "days": 30}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(1)
        .mount(&e.maint)
        .await;
    let (s, item) = e
        .call(
            "PUT",
            "/v1/votes/100",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "keep_days", "days": 30})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{item}");
    assert_eq!(item["protection"], "extended");
    assert_eq!(item["my_vote"]["kind"], "keep_days");
    assert_eq!(e.state.reconcile_once().await.unwrap(), 1);
    assert_eq!(e.state.reconcile_once().await.unwrap(), 0, "idempotent");
    e.maint.verify().await;
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "jfsrv", "jf-bob", None)
        .await;
    let first: LeavingItem = serde_json::from_value(list[1].clone()).unwrap();
    assert_eq!(first.key, "101", "kept item moved down the urgency order");
}

#[tokio::test]
async fn keep_forever_from_two_users_excludes_once_until_both_withdraw() {
    let e = env().await;
    e.state.sync().await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/rules/exclusion"))
        .and(body_json(json!({"mediaServerId": "101", "action": 0})))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&e.maint)
        .await;
    let forever = json!({"kind": "keep_forever", "reason": "classic"});
    assert_eq!(
        e.call(
            "PUT",
            "/v1/votes/101",
            "jfsrv",
            "jf-bob",
            Some(forever.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, item) = e
        .call(
            "PUT",
            "/v1/votes/101",
            "plexsrv",
            "px-friend",
            Some(forever),
        )
        .await;
    assert_eq!(item["counts"]["forever"], 2);
    assert_eq!(item["protection"], "forever");
    assert_eq!(item["effective_deletion_date"], Value::Null);
    e.state.reconcile_once().await.unwrap();
    e.state.reconcile_once().await.unwrap();

    // Bob changes his mind: the friend still keeps it, nothing is removed.
    e.call("DELETE", "/v1/votes/101", "jfsrv", "jf-bob", None)
        .await;
    assert_eq!(e.state.reconcile_once().await.unwrap(), 0);
    // Last keeper withdraws: our global exclusion is removed (and only ours).
    Mock::given(method("GET"))
        .and(path("/api/rules/exclusion"))
        .and(query_param("mediaServerId", "101"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{"id": 5, "mediaServerId": "101", "ruleGroupId": null}])),
        )
        .mount(&e.maint)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/rules/exclusion/5"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&e.maint)
        .await;
    e.call("DELETE", "/v1/votes/101", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(e.state.reconcile_once().await.unwrap(), 1);
    e.maint.verify().await;
}

#[tokio::test]
async fn admin_rights_limit_keeps() {
    let e = env().await;
    e.state.sync().await.unwrap();
    e.call("GET", "/v1/me", "jfsrv", "jf-bob", None).await; // register bob
    let deny = json!({"can_keep_forever": false, "max_forever": null, "max_keep_days": 7});
    assert_eq!(
        e.call(
            "PUT",
            "/v1/admin/users/jellyfin:jfsrv:jb/rights",
            "jfsrv",
            "jf-bob",
            Some(deny.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "not an admin"
    );
    let (s, u) = e
        .call(
            "PUT",
            "/v1/admin/users/jellyfin:jfsrv:jb/rights",
            "jfsrv",
            "jf-alice",
            Some(deny),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(u["rights"]["max_keep_days"], 7);
    let (s, err) = e
        .call(
            "PUT",
            "/v1/votes/100",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "keep_forever"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(err["error"].as_str().unwrap().contains("disabled"));
    assert_eq!(
        e.call(
            "PUT",
            "/v1/votes/100",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "keep_days", "days": 30}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        e.call(
            "PUT",
            "/v1/votes/100",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "keep_days", "days": 7}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        e.call(
            "PUT",
            "/v1/votes/999",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "dont_care"}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, users) = e
        .call("GET", "/v1/admin/users", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(users.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn until_watched_is_fulfilled_by_tautulli_then_released() {
    let e = env().await;
    e.state.sync().await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/rules/exclusion"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&e.maint)
        .await;
    let (s, _) = e
        .call(
            "PUT",
            "/v1/votes/100",
            "plexsrv",
            "px-friend",
            Some(json!({"kind": "keep_until_watched", "reminder_days": 7})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    e.state.reconcile_once().await.unwrap();
    // Not watched yet.
    let not_yet = Mock::given(path("/api/v2"))
        .and(query_param("cmd", "get_history"))
        .and(query_param("user_id", "2"))
        .and(query_param("rating_key", "100"))
        .and(query_param("apikey", "taut-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"response": {"result": "success", "data": {"data": [{"watched_status": 0.5}]}}}),
        ))
        .mount_as_scoped(&e.tautulli)
        .await;
    assert_eq!(e.state.check_watched().await.unwrap(), 0);
    drop(not_yet);
    Mock::given(path("/api/v2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"response": {"result": "success", "data": {"data": [{"watched_status": 1}]}}}),
        ))
        .mount(&e.tautulli)
        .await;
    assert_eq!(e.state.check_watched().await.unwrap(), 1);
    // Released: the exclusion goes away.
    Mock::given(method("GET"))
        .and(path("/api/rules/exclusion"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"id": 9, "mediaServerId": "100"}])),
        )
        .mount(&e.maint)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/rules/exclusion/9"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&e.maint)
        .await;
    assert_eq!(e.state.reconcile_once().await.unwrap(), 1);
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "plexsrv", "px-friend", None)
        .await;
    let item = list
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == "100")
        .unwrap();
    assert_eq!(item["protection"], "none");
    assert_eq!(
        item["my_vote"],
        Value::Null,
        "fulfilled votes are no longer mine"
    );
    e.maint.verify().await;
}

#[tokio::test]
async fn jellyfin_until_watched_uses_admin_key_and_played_flag() {
    let e = env().await;
    e.state.sync().await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/rules/exclusion"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&e.maint)
        .await;
    e.call(
        "PUT",
        "/v1/votes/200",
        "jfsrv",
        "jf-bob",
        Some(json!({"kind": "keep_until_watched"})),
    )
    .await;
    Mock::given(path("/Items/jf-show"))
        .and(query_param("userId", "jb"))
        .and(header(
            "authorization",
            "MediaBrowser Token=\"jf-admin-key\"",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Id": "jf-show", "UserData": {"Played": true}})),
        )
        .expect(1)
        .mount(&e.jf)
        .await;
    assert_eq!(e.state.check_watched().await.unwrap(), 1);
    e.jf.verify().await;
}

#[tokio::test]
async fn triage_lists_unvoted_urgent_items() {
    let e = env().await;
    e.state.sync().await.unwrap();
    e.call(
        "PUT",
        "/v1/votes/200",
        "jfsrv",
        "jf-bob",
        Some(json!({"kind": "dont_care"})),
    )
    .await;
    let (_, list) = e
        .call("GET", "/v1/triage?limit=1", "jfsrv", "jf-bob", None)
        .await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["key"], "100");
    let (_, all) = e
        .call("GET", "/v1/leaving-soon", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(all[0]["counts"]["dont_care"], 1, "admins see the votes");
}

#[tokio::test]
async fn countdown_is_admin_only_and_resets_every_item() {
    let e = env().await;
    e.state.sync().await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/collections/media/postpone"))
        .and(body_json(json!({"collectionId": 3, "mediaId": "200"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(1)
        .mount(&e.maint)
        .await;
    assert_eq!(
        e.call(
            "POST",
            "/v1/admin/collections/3/countdown",
            "jfsrv",
            "jf-bob",
            Some(json!({"stagger_days": 0}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (s, r) = e
        .call(
            "POST",
            "/v1/admin/collections/3/countdown",
            "jfsrv",
            "jf-alice",
            Some(json!({"stagger_days": 0})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["reset"], 1);
    e.maint.verify().await;
}

#[tokio::test]
async fn webhooks_need_the_secret() {
    let e = env().await;
    let post = |uri: &'static str| Request::post(uri).body(Body::from("{}")).unwrap();
    let r = crate::api::router(e.state.clone())
        .oneshot(post("/v1/hooks/tautulli/wrong"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = crate::api::router(e.state.clone())
        .oneshot(post("/v1/hooks/tautulli/hook"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let _ = &e.plextv;
}

#[tokio::test]
async fn seerr_bridge_links_existing_users_and_creates_missing_ones() {
    let e = env().await;
    let key = || header("x-api-key", "seerr-key");
    // Seerr (Plex mode) knows Plex user 2 as Seerr user 7; nobody from Jellyfin.
    Mock::given(method("GET"))
        .and(path("/api/v1/user"))
        .and(key())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": 1, "plexId": 1, "displayName": "owner"},
            {"id": 7, "plexId": 2, "displayName": "friend"}
        ]})))
        .mount(&e.seerr)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/user"))
        .and(key())
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"id": 9, "displayName": "bob"})),
        )
        .expect(1)
        .mount(&e.seerr)
        .await;
    for (seerr_id, name) in [(7, "friend"), (9, "bob")] {
        Mock::given(path("/api/v1/auth/me"))
            .and(header("x-api-user", seerr_id.to_string().as_str()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id": seerr_id, "displayName": name})),
            )
            .mount(&e.seerr)
            .await;
    }
    Mock::given(path("/api/v1/settings/public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"cacheImages": true})))
        .mount(&e.seerr)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/request"))
        .and(header("x-api-user", "9"))
        .and(body_json(json!({"mediaType": "movie", "mediaId": 603})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 55, "status": 1})))
        .expect(1)
        .mount(&e.seerr)
        .await;
    Mock::given(path("/api/v1/search"))
        .and(query_param("query", "the matrix"))
        .and(header("x-api-user", "9"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
        .mount(&e.seerr)
        .await;

    // Plex friend: reuses their Seerr account.
    let (s, info) = e
        .call("GET", "/v1/seerr", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (info["seerr_user_id"].clone(), info["cache_images"].clone()),
        (json!(7), json!(true))
    );

    // Jellyfin bob: created once, then reused for every call.
    let (s, info) = e.call("GET", "/v1/seerr", "jfsrv", "jf-bob", None).await;
    assert_eq!(
        (s, info["user_name"].clone()),
        (StatusCode::OK, json!("bob"))
    );
    let (s, r) = e
        .call(
            "POST",
            "/v1/seerr/api/v1/request",
            "jfsrv",
            "jf-bob",
            Some(json!({"mediaType": "movie", "mediaId": 603})),
        )
        .await;
    assert_eq!((s, r["id"].clone()), (StatusCode::CREATED, json!(55)));
    let (s, _) = e
        .call(
            "GET",
            "/v1/seerr/api/v1/search?query=the%20matrix&page=1",
            "jfsrv",
            "jf-bob",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // Admin endpoints are not forwarded, and unauthenticated callers get nothing.
    let (s, _) = e
        .call(
            "GET",
            "/v1/seerr/api/v1/settings/main",
            "jfsrv",
            "jf-bob",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = e
        .call(
            "POST",
            "/v1/seerr/api/v1/request/55/approve",
            "jfsrv",
            "jf-bob",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = e.call("GET", "/v1/seerr", "jfsrv", "nope", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dry_run_writes_nothing_to_maintainerr_or_seerr() {
    let e = env_with(true).await;
    e.state.sync().await.unwrap();
    // Any write would hit these and fail the test (expect 0).
    for (m, p) in [
        ("POST", "/api/rules/exclusion"),
        ("POST", "/api/collections/media/postpone"),
    ] {
        Mock::given(method(m))
            .and(path(p))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&e.maint)
            .await;
    }
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&e.seerr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
        .mount(&e.seerr)
        .await;
    let (s, item) = e
        .call(
            "PUT",
            "/v1/votes/101",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "keep_forever"})),
        )
        .await;
    assert_eq!(
        (s, item["protection"].clone()),
        (StatusCode::OK, json!("forever"))
    );
    let (s, _) = e
        .call(
            "PUT",
            "/v1/votes/100",
            "jfsrv",
            "jf-alice",
            Some(json!({"kind": "keep_days", "days": 40})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        e.state.reconcile_once().await.unwrap(),
        2,
        "simulated, not sent"
    );
    let (s, r) = e
        .call(
            "POST",
            "/v1/admin/collections/2/countdown",
            "jfsrv",
            "jf-alice",
            Some(json!({"stagger_days": 0})),
        )
        .await;
    assert_eq!((s, r["reset"].clone()), (StatusCode::OK, json!(0)));
    // Seerr: no user creation, no requests.
    assert_eq!(
        e.call("GET", "/v1/seerr", "jfsrv", "jf-bob", None).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, _) = e
        .call(
            "POST",
            "/v1/seerr/api/v1/request",
            "jfsrv",
            "jf-bob",
            Some(json!({"mediaType": "movie", "mediaId": 1})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_links_a_user_to_an_existing_seerr_user_without_creating_one() {
    // Dry run: Seerr may not be written to, yet linking an existing user works.
    let e = env_with(true).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&e.seerr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": 13, "displayName": "alex", "email": "alex@example.org"}
        ]})))
        .mount(&e.seerr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": 13, "displayName": "alex"})),
        )
        .mount(&e.seerr)
        .await;
    // Bob has no Seerr account of his own: refused until linked.
    let (_, bob) = e.call("GET", "/v1/me", "jfsrv", "jf-bob", None).await;
    let bob_key = bob["user_key"].as_str().unwrap().to_owned();
    assert_eq!(
        e.call("GET", "/v1/seerr", "jfsrv", "jf-bob", None).await.0,
        StatusCode::FORBIDDEN
    );
    // Only admins see Seerr's users and link.
    assert_eq!(
        e.call("GET", "/v1/admin/seerr-users", "jfsrv", "jf-bob", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (s, users) = e
        .call("GET", "/v1/admin/seerr-users", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(
        (s, users[0]["name"].clone()),
        (StatusCode::OK, json!("alex"))
    );
    let (s, u) = e
        .call(
            "PUT",
            &format!("/v1/admin/users/{bob_key}/seerr"),
            "jfsrv",
            "jf-alice",
            Some(json!({"seerr_user_id": 13})),
        )
        .await;
    assert_eq!((s, u["seerr_user_id"].clone()), (StatusCode::OK, json!(13)));
    let (s, info) = e.call("GET", "/v1/seerr", "jfsrv", "jf-bob", None).await;
    assert_eq!(
        (s, info["seerr_user_id"].clone()),
        (StatusCode::OK, json!(13))
    );
}
