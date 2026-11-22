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
    /// Pocket ID.
    oidc: MockServer,
    state: Shared,
    /// Data dir (custom avatars), removed with the environment.
    _data: tempfile::TempDir,
}

async fn env() -> Env {
    env_with(false).await
}

async fn env_with(dry_run: bool) -> Env {
    env_at(Db::open_in_memory().unwrap(), dry_run).await
}

async fn env_at(db: Db, dry_run: bool) -> Env {
    let seerr = MockServer::start().await;
    let oidc = MockServer::start().await;
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
        [[servers]]
        id = "jf2"
        kind = "jellyfin"
        name = "Second fin"
        public_url = "https://jf2.example"
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
        jf.uri(),
        maint.uri(),
        tautulli.uri(),
        seerr.uri()
    );
    let mut config = Config::parse(&toml).unwrap();
    config.secrets = Secrets::for_tests("jf-admin-key", "taut-key");
    config.dry_run = dry_run;
    config.accounts = crate::config::AccountsConf {
        rp: crate::passkey::RelyingParty::from_url("https://watch.example"),
        pocketid: Some(crate::oidc::PocketId::for_tests(
            &oidc.uri(),
            "https://watch.example",
        )),
        registration: crate::config::Registration::Invite,
    };
    let data = tempfile::tempdir().unwrap();
    config.data_dir = data.path().to_owned();
    let state = Arc::new(State::new(
        config,
        db,
        net::api_client("t").unwrap(),
        &plextv.uri(),
    ));
    let e = Env {
        maint,
        jf,
        plextv,
        tautulli,
        seerr,
        oidc,
        state,
        _data: data,
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
            ("jf-carol", "jc", "carol", false),
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

impl Env {
    /// A raw body (pictures); `token` None: not signed in. Returns the headers too.
    async fn call_raw(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        content_type: &str,
        body: Vec<u8>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", content_type);
        if let Some(t) = token {
            req = req
                .header(SERVER_HEADER, "jfsrv")
                .header("authorization", format!("Bearer {t}"));
        }
        let res = crate::api::router(self.state.clone())
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 21)
            .await
            .unwrap();
        (status, headers, bytes.to_vec())
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
    assert_eq!(list.len(), 3);
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
async fn a_title_in_no_deletion_list_can_be_kept_ahead() {
    let e = env().await;
    e.state.sync().await.unwrap();
    // A Jellyfin user: the title is found on Maintainerr's server by its TMDB id.
    Mock::given(method("GET"))
        .and(path("/api/media-server/search/Primetime"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "900", "title": "Primetime", "type": "show", "year": 2020,
             "providerIds": {"tmdb": ["77"]}},
            {"id": "901", "title": "Primetime", "type": "movie", "year": 2026,
             "providerIds": {"tmdb": ["77"], "imdb": ["tt1"]}}
        ])))
        .expect(1)
        .mount(&e.maint)
        .await;
    let lookup =
        json!({"item_id": "jf-primetime", "kind": "movie", "title": "Primetime", "tmdb_id": "77"});
    let (s, item) = e
        .call(
            "POST",
            "/v1/keep-entry",
            "jfsrv",
            "jf-bob",
            Some(lookup.clone()),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{item}");
    assert_eq!(
        item["key"], "901",
        "the movie, not the show of the same name"
    );
    assert_eq!(item["title"], "Primetime");
    assert_eq!(item["listed"], false);
    assert_eq!(item["protection"], "none");
    assert_eq!(item["effective_deletion_date"], Value::Null);
    assert_eq!(item["server_item_id"], "jf-primetime");
    // Asked again: known now, Maintainerr isn't searched a second time.
    let (s, again) = e
        .call("POST", "/v1/keep-entry", "jfsrv", "jf-bob", Some(lookup))
        .await;
    assert_eq!((s, &again["key"]), (StatusCode::OK, &json!("901")));
    // Not listed while nobody keeps it.
    let listed = |list: &Value| list.as_array().unwrap().iter().any(|i| i["key"] == "901");
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "jfsrv", "jf-bob", None)
        .await;
    assert!(!listed(&list));

    // "I don't care" is recorded and answered, although Leaving soon doesn't list the title.
    let (s, item) = e
        .call(
            "PUT",
            "/v1/votes/901",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "dont_care"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{item}");
    assert_eq!(item["my_vote"]["kind"], "dont_care");
    // Their own list carries it (for the title's page), nobody else's does.
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "jfsrv", "jf-bob", None)
        .await;
    assert!(listed(&list));
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "plexsrv", "px-friend", None)
        .await;
    assert!(!listed(&list));
    // Some days: nothing to extend yet, so it isn't "extended" and nothing is written.
    let (_, item) = e
        .call(
            "PUT",
            "/v1/votes/901",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "keep_days", "days": 7})),
        )
        .await;
    assert_eq!(item["protection"], "none");
    assert_eq!(item["my_vote"]["kind"], "keep_days");
    assert_eq!(e.state.reconcile_once().await.unwrap(), 0);

    // Forever: excluded in Maintainerr, so it never enters a deletion list.
    Mock::given(method("POST"))
        .and(path("/api/rules/exclusion"))
        .and(body_json(json!({"mediaServerId": "901", "action": 0})))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&e.maint)
        .await;
    let (_, item) = e
        .call(
            "PUT",
            "/v1/votes/901",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "keep_forever"})),
        )
        .await;
    assert_eq!(item["protection"], "forever");
    assert_eq!(item["listed"], false);
    assert_eq!(e.state.reconcile_once().await.unwrap(), 1);
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "jfsrv", "jf-bob", None)
        .await;
    assert!(listed(&list), "a kept title shows with the other kept ones");
    // A sync (the title still in no collection) changes nothing.
    e.state.sync().await.unwrap();
    assert_eq!(e.state.reconcile_once().await.unwrap(), 0);

    // On Maintainerr's own server the id is the same: checked with Maintainerr, no search.
    Mock::given(method("GET"))
        .and(path("/api/media-server/meta/950"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(
            {"id": "950", "title": "Dreamland", "type": "show", "year": 2024,
             "providerIds": {"tvdb": ["5"]}}
        )))
        .mount(&e.maint)
        .await;
    let (s, item) = e
        .call(
            "POST",
            "/v1/keep-entry",
            "plexsrv",
            "px-friend",
            Some(json!({"item_id": "950", "kind": "show", "title": "Dreamland"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{item}");
    assert_eq!(
        (&item["key"], &item["title"]),
        (&json!("950"), &json!("Dreamland"))
    );
    // An episode, or a title that isn't in that library: refused.
    let (s, _) = e
        .call(
            "POST",
            "/v1/keep-entry",
            "plexsrv",
            "px-friend",
            Some(json!({"item_id": "950", "kind": "episode", "title": "x"})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    Mock::given(method("GET"))
        .and(path("/api/media-server/search/Nowhere"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&e.maint)
        .await;
    let (s, _) = e
        .call(
            "POST",
            "/v1/keep-entry",
            "jfsrv",
            "jf-bob",
            Some(json!({"item_id": "jf-x", "kind": "movie", "title": "Nowhere", "tmdb_id": "1"})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
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

fn arrival() -> Value {
    json!({"kind": "movie", "title": "Arrival", "year": 2016, "tmdb": "329865",
        "imdb": null, "tvdb": null, "series_title": null, "series_tmdb": null,
        "series_tvdb": null, "season": null, "episode": null})
}

const BOB: &str = "jellyfin:jfsrv:jb";
const ALICE: &str = "jellyfin:jfsrv:ja";
const FRIEND: &str = "plex:plexsrv:2";

impl Env {
    /// Signs these users in once, so the server knows them (people list, invitations).
    async fn sign_in(&self, who: &[(&str, &str)]) {
        for (server, token) in who {
            assert_eq!(
                self.call("GET", "/v1/me", server, token, None).await.0,
                StatusCode::OK
            );
        }
    }

    /// Bob opens a lobby: (code, his member id).
    async fn bob_lobby(&self) -> (String, String) {
        let (s, a) = self
            .call(
                "POST",
                "/v1/together",
                "jfsrv",
                "jf-bob",
                Some(json!({"media": arrival()})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(a["state"]["started"], false);
        assert_eq!(a["state"]["playing"], false);
        (
            a["state"]["code"].as_str().unwrap().to_owned(),
            a["member_id"].as_str().unwrap().to_owned(),
        )
    }
}

#[tokio::test]
async fn watch_together_rooms_are_private_and_joined_by_invitation() {
    let e = Arc::new(env().await);
    e.sign_in(&[("plexsrv", "px-friend"), ("jfsrv", "jf-alice")])
        .await;
    let (code, bob) = e.bob_lobby().await;
    assert_eq!(code.len(), 6);
    // Bob sees his lobby; nobody else sees anything, and a guessed code is "not found".
    let (_, mine) = e.call("GET", "/v1/together", "jfsrv", "jf-bob", None).await;
    assert_eq!(mine["rooms"][0]["code"], code.as_str());
    assert_eq!(mine["rooms"][0]["hosting"], true);
    let (s, theirs) = e
        .call("GET", "/v1/together", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(theirs["rooms"], json!([]));
    let version = theirs["version"].as_u64().unwrap();
    for (method, uri, body) in [
        ("POST", format!("/v1/together/{code}/join"), None),
        (
            "GET",
            format!("/v1/together/{code}/wait?after=0&member={bob}"),
            None,
        ),
        (
            "PUT",
            format!("/v1/together/{code}"),
            Some(
                json!({"member_id": bob, "playing": true, "position_ms": 0, "media": null, "started": true}),
            ),
        ),
        (
            "POST",
            format!("/v1/together/{code}/invites"),
            Some(json!({"member_id": bob, "user_keys": [ALICE]})),
        ),
        ("DELETE", format!("/v1/together/{code}/invites/me"), None),
        ("DELETE", format!("/v1/together/{code}/members/{bob}"), None),
    ] {
        assert_eq!(
            e.call(method, &uri, "plexsrv", "px-friend", body).await.0,
            StatusCode::NOT_FOUND,
            "{method} {uri}"
        );
    }
    // Users of no served server get nothing at all.
    assert_eq!(
        e.call("GET", "/v1/together", "plexsrv", "px-stranger", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    // The friend's list long poll wakes when Bob invites them.
    let poll = {
        let e = e.clone();
        tokio::spawn(async move {
            e.call(
                "GET",
                &format!("/v1/together/wait?after={version}"),
                "plexsrv",
                "px-friend",
                None,
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (s, st) = e
        .call(
            "POST",
            &format!("/v1/together/{code}/invites"),
            "jfsrv",
            "jf-bob",
            Some(json!({"member_id": bob, "user_keys": [FRIEND]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(st["invited"][0]["user_key"], FRIEND);
    assert_eq!(st["invited"][0]["name"], "User 2");
    let (s, list) = tokio::time::timeout(std::time::Duration::from_secs(5), poll)
        .await
        .expect("woken by the invitation")
        .unwrap();
    assert_eq!(s, StatusCode::OK);
    assert_ne!(list["version"].as_u64().unwrap(), version);
    let room = &list["rooms"][0];
    assert_eq!(
        (
            &room["code"],
            &room["invited"],
            &room["mine"],
            &room["host"]
        ),
        (&json!(code), &json!(true), &json!(false), &json!("bob"))
    );

    // Joining needs no approval.
    let (s, f) = e
        .call(
            "POST",
            &format!("/v1/together/{code}/join"),
            "plexsrv",
            "px-friend",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let friend = f["member_id"].as_str().unwrap().to_owned();
    assert_eq!(f["state"]["members"].as_array().unwrap().len(), 2);
    assert_eq!(f["state"]["invited"], json!([]));
    assert_eq!(f["state"]["started"], false);
    // Alice still sees nothing.
    let (_, alice) = e
        .call("GET", "/v1/together", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(alice["rooms"], json!([]));

    // Lobby: the friend neither starts nor moves it.
    let put = format!("/v1/together/{code}");
    for started in [true, false] {
        assert_eq!(
            e.call(
                "PUT",
                &put,
                "plexsrv",
                "px-friend",
                Some(json!({"member_id": friend, "playing": true, "position_ms": 9, "media": null, "started": started})),
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let mut last_ready = json!(null);
    // Bob's other device joins without an invitation.
    let (s, b2) = e
        .call(
            "POST",
            &format!("/v1/together/{code}/join"),
            "jfsrv",
            "jf-bob",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b2["state"]["members"].as_array().unwrap().len(), 3);
    // The host starts (from his second seat); the friend's wait returns it.
    let seq = f["state"]["seq"].clone();
    let (s, st) = e
        .call(
            "PUT",
            &put,
            "jfsrv",
            "jf-bob",
            Some(json!({"member_id": b2["member_id"], "playing": true, "position_ms": 1000, "media": null, "started": true})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // Nothing plays until every seat has loaded the title.
    assert_eq!(
        (&st["started"], &st["playing"], &st["round"]),
        (&json!(true), &json!(false), &json!(1))
    );
    assert_eq!(st["loading"].as_array().unwrap().len(), 3);
    let wait = format!("/v1/together/{code}/wait?after={seq}&member={friend}");
    let (s, st) = e.call("GET", &wait, "plexsrv", "px-friend", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(st["started"], true);
    assert_eq!(st["by"], b2["member_id"]);
    let ready = format!("/v1/together/{code}/ready");
    for (srv, tok, seat) in [
        ("plexsrv", "px-friend", &friend),
        ("jfsrv", "jf-bob", &bob),
        (
            "jfsrv",
            "jf-bob",
            &b2["member_id"].as_str().unwrap().to_owned(),
        ),
    ] {
        let (s, st) = e
            .call(
                "POST",
                &ready,
                srv,
                tok,
                Some(json!({"member_id": seat, "round": 1})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        last_ready = st;
    }
    assert_eq!(last_ready["playing"], true);
    // The clock, for the apps' sync.
    let (s, clock) = e
        .call("GET", "/v1/together/clock", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(clock["now_ms"].as_i64().unwrap() > 1_700_000_000_000);
    // Started: anyone plays; "started": false does not go back.
    let (s, st) = e
        .call(
            "PUT",
            &put,
            "plexsrv",
            "px-friend",
            Some(json!({"member_id": friend, "playing": false, "position_ms": 2000, "media": null, "started": false})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (&st["started"], &st["playing"]),
        (&json!(true), &json!(false))
    );
    // Nobody moves someone else's seat.
    assert_eq!(
        e.call(
            "PUT",
            &put,
            "plexsrv",
            "px-friend",
            Some(json!({"member_id": bob, "playing": true, "position_ms": 0, "media": null})),
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    // Alice: invited then declines; invited again then withdrawn.
    let invites = format!("/v1/together/{code}/invites");
    let invite_alice = json!({"member_id": bob, "user_keys": [ALICE]});
    e.call(
        "POST",
        &invites,
        "jfsrv",
        "jf-bob",
        Some(invite_alice.clone()),
    )
    .await;
    let (_, alice) = e
        .call("GET", "/v1/together", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(alice["rooms"][0]["invited"], true);
    assert_eq!(alice["rooms"][0]["started"], true);
    assert_eq!(
        e.call(
            "DELETE",
            &format!("{invites}/me"),
            "jfsrv",
            "jf-alice",
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, alice) = e
        .call("GET", "/v1/together", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(alice["rooms"], json!([]));
    e.call("POST", &invites, "jfsrv", "jf-bob", Some(invite_alice))
        .await;
    // Only the host withdraws.
    let withdraw = format!("{invites}/{ALICE}?member={friend}");
    assert_eq!(
        e.call("DELETE", &withdraw, "plexsrv", "px-friend", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (s, st) = e
        .call(
            "DELETE",
            &format!("{invites}/{ALICE}?member={bob}"),
            "jfsrv",
            "jf-bob",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(st["invited"], json!([]));
    assert_eq!(
        e.call(
            "POST",
            &format!("/v1/together/{code}/join"),
            "jfsrv",
            "jf-alice",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Unknown users cannot be invited; too many at once is refused.
    assert_eq!(
        e.call(
            "POST",
            &invites,
            "jfsrv",
            "jf-bob",
            Some(json!({"member_id": bob, "user_keys": ["jellyfin:jfsrv:nobody"]}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let many: Vec<String> = (0..21).map(|i| format!("x{i}")).collect();
    assert_eq!(
        e.call(
            "POST",
            &invites,
            "jfsrv",
            "jf-bob",
            Some(json!({"member_id": bob, "user_keys": many}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    // The friend leaves: their wait is 404, the room is gone from their list.
    assert_eq!(
        e.call(
            "DELETE",
            &format!("/v1/together/{code}/members/{friend}"),
            "plexsrv",
            "px-friend",
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        e.call("GET", &wait, "plexsrv", "px-friend", None).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, list) = e
        .call("GET", "/v1/together", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(list["rooms"], json!([]));
}

#[tokio::test]
async fn knocking_is_gone() {
    let e = env().await;
    let (code, bob) = e.bob_lobby().await;
    for (method, uri) in [
        ("POST", format!("/v1/together/{code}/knock")),
        ("GET", format!("/v1/together/{code}/knock/abc/wait")),
        ("DELETE", format!("/v1/together/{code}/knock/abc")),
        ("POST", format!("/v1/together/{code}/requests/abc")),
    ] {
        let body = (method == "POST").then(|| json!({"member_id": bob, "accept": true}));
        assert_eq!(
            e.call(method, &uri, "jfsrv", "jf-bob", body).await.0,
            StatusCode::NOT_FOUND,
            "{method} {uri}"
        );
    }
}

#[tokio::test]
async fn people_list_puts_recent_partners_first_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("keep.sqlite");
    {
        let e = env_at(Db::open(&db_path).unwrap(), false).await;
        e.sign_in(&[
            ("plexsrv", "px-friend"),
            ("jfsrv", "jf-alice"),
            ("plexsrv", "px-owner"),
        ])
        .await;
        // Nobody watched with Bob yet: everyone but him, by name (case-insensitive).
        let (s, people) = e
            .call("GET", "/v1/together/people", "jfsrv", "jf-bob", None)
            .await;
        assert_eq!(s, StatusCode::OK);
        let names: Vec<&str> = people
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["alice", "User 1", "User 2"]);
        assert_eq!(people[0]["user_key"], ALICE);
        assert_eq!(people[0]["server_name"], "HomeJelly");
        assert_eq!(people[0]["last_together_at"], Value::Null);
        // Bob watches with the friend: invited, joined, started (2 users in it).
        let (code, bob) = e.bob_lobby().await;
        e.call(
            "POST",
            &format!("/v1/together/{code}/invites"),
            "jfsrv",
            "jf-bob",
            Some(json!({"member_id": bob, "user_keys": [FRIEND, ALICE]})),
        )
        .await;
        e.call(
            "POST",
            &format!("/v1/together/{code}/join"),
            "plexsrv",
            "px-friend",
            None,
        )
        .await;
        let (_, people) = e
            .call("GET", "/v1/together/people", "jfsrv", "jf-bob", None)
            .await;
        assert_eq!(
            people[2]["last_together_at"],
            Value::Null,
            "a lobby is not watching together"
        );
        let (s, _) = e
            .call(
                "PUT",
                &format!("/v1/together/{code}"),
                "jfsrv",
                "jf-bob",
                Some(json!({"member_id": bob, "playing": true, "position_ms": 0, "media": null, "started": true})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
    }
    // Rooms are gone with the old process; the history stays.
    let e = env_at(Db::open(&db_path).unwrap(), false).await;
    let (_, people) = e
        .call("GET", "/v1/together/people", "jfsrv", "jf-bob", None)
        .await;
    let order: Vec<&str> = people
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["user_key"].as_str().unwrap())
        .collect();
    assert_eq!(order, [FRIEND, ALICE, "plex:plexsrv:1"]);
    assert!(people[0]["last_together_at"].as_i64().unwrap() > 0);
    // Seen from the friend's side too; Alice (only invited) met nobody.
    let (_, people) = e
        .call("GET", "/v1/together/people", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(people[0]["user_key"], BOB);
    let (_, people) = e
        .call("GET", "/v1/together/people", "jfsrv", "jf-alice", None)
        .await;
    assert!(
        people
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["last_together_at"].is_null())
    );
    let (_, rooms) = e.call("GET", "/v1/together", "jfsrv", "jf-bob", None).await;
    assert_eq!(rooms["rooms"], json!([]));
}

#[tokio::test]
async fn settings_follow_each_user_and_stay_theirs() {
    let e = env().await;
    let (s, _) = e
        .call(
            "PUT",
            "/v1/me/settings",
            "jfsrv",
            "jf-alice",
            Some(json!({"player.sub_mode": "17\u{1f}always", "tracks.x.y": "18\u{1f}fra"})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    // An update replaces one key, the others stay.
    e.call(
        "PUT",
        "/v1/me/settings",
        "jfsrv",
        "jf-alice",
        Some(json!({"player.sub_mode": "20\u{1f}off"})),
    )
    .await;
    let (s, mine) = e
        .call("GET", "/v1/me/settings", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        mine,
        json!({"player.sub_mode": "20\u{1f}off", "tracks.x.y": "18\u{1f}fra"})
    );
    // Another user sees only theirs.
    let (_, theirs) = e
        .call("GET", "/v1/me/settings", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(theirs, json!({}));
    // Oversized values are refused.
    let big = "x".repeat(5000);
    assert_eq!(
        e.call(
            "PUT",
            "/v1/me/settings",
            "jfsrv",
            "jf-alice",
            Some(json!({"k": big}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn wrong_tokens_and_room_codes_get_a_pause() {
    let e = env().await;
    // 20 rejected tokens from one address, then even a good one waits.
    for _ in 0..20 {
        assert_eq!(
            e.call("GET", "/v1/me", "jfsrv", "nope", None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        e.call("GET", "/v1/me", "jfsrv", "jf-alice", None).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );

    // Invitations: sixty per user in ten minutes, then a pause.
    let e = env().await;
    e.sign_in(&[("plexsrv", "px-friend")]).await;
    let (code, bob) = e.bob_lobby().await;
    let invite = format!("/v1/together/{code}/invites");
    let body = json!({"member_id": bob, "user_keys": [FRIEND]});
    for _ in 0..60 {
        assert_eq!(
            e.call("POST", &invite, "jfsrv", "jf-bob", Some(body.clone()))
                .await
                .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        e.call("POST", &invite, "jfsrv", "jf-bob", Some(body.clone()))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Other users are not affected (and, not in the room, it does not exist for them).
    assert_eq!(
        e.call("POST", &invite, "jfsrv", "jf-alice", Some(body))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn internal_errors_do_not_leak_details() {
    let e = env().await;
    // No Maintainerr mounted: the leaving list fails upstream.
    let (s, body) = e
        .call("GET", "/v1/leaving-soon", "jfsrv", "jf-alice", None)
        .await;
    if s == StatusCode::INTERNAL_SERVER_ERROR {
        assert_eq!(body["error"], "server error, try again later");
    }
}

#[tokio::test]
async fn admin_sees_who_keeps_what_forever_and_the_overlaps() {
    let e = env().await;
    e.state.sync().await.unwrap();
    let forever = || Some(json!({"kind": "keep_forever"}));
    // Alice keeps 100 and 101, Bob keeps 100: one title in common, one Alice's alone.
    for (item, tok) in [("100", "jf-alice"), ("101", "jf-alice"), ("100", "jf-bob")] {
        let (s, _) = e
            .call("PUT", &format!("/v1/votes/{item}"), "jfsrv", tok, forever())
            .await;
        assert_eq!(s, StatusCode::OK);
    }
    // Not a forever keep: not counted.
    e.call(
        "PUT",
        "/v1/votes/101",
        "jfsrv",
        "jf-bob",
        Some(json!({"kind": "keep_days", "days": 7})),
    )
    .await;
    assert_eq!(
        e.call("GET", "/v1/admin/forever", "jfsrv", "jf-bob", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (s, st) = e
        .call("GET", "/v1/admin/forever", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (st["titles"].clone(), st["shared"].clone()),
        (json!(2), json!(1))
    );
    let people = st["people"].as_array().unwrap();
    let alice = &people[0];
    assert_eq!(
        (
            alice["count"].clone(),
            alice["shared"].clone(),
            alice["alone"].clone()
        ),
        (json!(2), json!(1), json!(1))
    );
    assert_eq!(alice["overlaps"][0]["both"], 1);
    let bob = &people[1];
    assert_eq!(
        (
            bob["count"].clone(),
            bob["shared"].clone(),
            bob["alone"].clone()
        ),
        (json!(1), json!(1), json!(0))
    );
    assert_eq!(st["items"][0]["key"], "100");
    assert_eq!(st["items"][0]["keepers"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn display_names_are_chosen_validated_and_shown_to_others() {
    let e = env().await;
    e.sign_in(&[("jfsrv", "jf-alice"), ("plexsrv", "px-friend")])
        .await;
    let (s, me) = e
        .call(
            "PUT",
            "/v1/me/name",
            "jfsrv",
            "jf-bob",
            Some(json!({"name": "  Bobby   the Builder "})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["name"], "Bobby the Builder");
    assert_eq!(me["media_name"], "bob");
    assert_eq!(me["name_is_custom"], true);
    for bad in ["   ", "two\nlines", "tab\there", &"x".repeat(33)] {
        let (s, err) = e
            .call(
                "PUT",
                "/v1/me/name",
                "jfsrv",
                "jf-bob",
                Some(json!({ "name": bad })),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad:?}");
        assert!(err["error"].as_str().unwrap().starts_with("the name"));
    }
    // A later sign-in (the media server says "bob" again) keeps the chosen name.
    let (_, me) = e.call("GET", "/v1/me", "jfsrv", "jf-bob", None).await;
    assert_eq!(me["name"], "Bobby the Builder");
    // Others see it: people list, room members, admin users (with the media-server name).
    let (_, people) = e
        .call("GET", "/v1/together/people", "plexsrv", "px-friend", None)
        .await;
    let bob = people
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["user_key"] == BOB)
        .unwrap();
    assert_eq!(
        (&bob["name"], &bob["media_name"]),
        (&json!("Bobby the Builder"), &json!("bob"))
    );
    let (_, room) = e
        .call(
            "POST",
            "/v1/together",
            "jfsrv",
            "jf-bob",
            Some(json!({"media": arrival()})),
        )
        .await;
    assert_eq!(room["state"]["members"][0]["name"], "Bobby the Builder");
    let (_, users) = e
        .call("GET", "/v1/admin/users", "jfsrv", "jf-alice", None)
        .await;
    let bob = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["user_key"] == BOB)
        .unwrap();
    assert_eq!(
        (&bob["name"], &bob["media_name"]),
        (&json!("Bobby the Builder"), &json!("bob"))
    );
    // Reset.
    let (s, me) = e
        .call("DELETE", "/v1/me/name", "jfsrv", "jf-bob", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (&me["name"], &me["name_is_custom"]),
        (&json!("bob"), &json!(false))
    );
}

const JPEG: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1,
];
const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D,
];
const WEBP: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 ";

#[tokio::test]
async fn avatars_presets_uploads_and_who_sees_them() {
    let e = env().await;
    e.sign_in(&[("jfsrv", "jf-alice"), ("plexsrv", "px-friend")])
        .await;
    // Built-in.
    let (s, me) = e
        .call(
            "PUT",
            "/v1/me/avatar",
            "jfsrv",
            "jf-bob",
            Some(json!({"preset": "p03"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        me["avatar"],
        json!({"preset": "p03", "custom_version": null})
    );
    assert_eq!(
        e.call(
            "PUT",
            "/v1/me/avatar",
            "jfsrv",
            "jf-bob",
            Some(json!({"preset": "p99"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    // Custom pictures of each type come back byte-identical, with safe headers.
    let url = format!("/v1/users/{}/avatar", BOB.replace(':', "%3A"));
    let mut last = 0;
    for (ct, bytes) in [
        ("image/jpeg", JPEG),
        ("image/png", PNG),
        ("image/webp", WEBP),
    ] {
        let (s, _, me) = e
            .call_raw("PUT", "/v1/me/avatar", Some("jf-bob"), ct, bytes.to_vec())
            .await;
        assert_eq!(s, StatusCode::OK, "{ct}");
        let me: Value = serde_json::from_slice(&me).unwrap();
        assert_eq!(me["avatar"]["preset"], Value::Null);
        let v = me["avatar"]["custom_version"].as_u64().unwrap();
        assert!(v > last, "the version bumps on every change");
        last = v;
        // Another user fetches it.
        let (s, h, got) = e
            .call_raw("GET", &format!("{url}?v={v}"), Some("jf-alice"), "", vec![])
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(got, bytes);
        assert_eq!(h["content-type"], ct);
        assert_eq!(h["cache-control"], "private, max-age=31536000, immutable");
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert_eq!(h["content-security-policy"], "default-src 'none'");
    }
    // Not signed in: nothing.
    assert_eq!(
        e.call_raw("GET", &url, None, "", vec![]).await.0,
        StatusCode::UNAUTHORIZED
    );
    // Lies about the type, other types, too big.
    for (ct, bytes, want) in [
        ("image/png", JPEG.to_vec(), StatusCode::BAD_REQUEST),
        (
            "image/webp",
            b"RIFF\0\0\0\0WAVEfmt ".to_vec(),
            StatusCode::BAD_REQUEST,
        ),
        ("image/gif", b"GIF89a....".to_vec(), StatusCode::BAD_REQUEST),
        ("text/html", b"<script>".to_vec(), StatusCode::BAD_REQUEST),
        (
            "image/jpeg",
            [JPEG, &vec![0u8; 512 * 1024]].concat(),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        assert_eq!(
            e.call_raw("PUT", "/v1/me/avatar", Some("jf-bob"), ct, bytes)
                .await
                .0,
            want,
            "{ct}"
        );
    }
    // The last good picture is still there; it shows next to Bob's name for others.
    let (_, people) = e
        .call("GET", "/v1/together/people", "plexsrv", "px-friend", None)
        .await;
    let bob = people
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["user_key"] == BOB)
        .unwrap();
    assert_eq!(bob["avatar"]["custom_version"], json!(last));
    let (_, room) = e
        .call(
            "POST",
            "/v1/together",
            "jfsrv",
            "jf-bob",
            Some(json!({"media": arrival()})),
        )
        .await;
    assert_eq!(
        room["state"]["members"][0]["avatar"]["custom_version"],
        json!(last)
    );
    let (_, list) = e.call("GET", "/v1/together", "jfsrv", "jf-bob", None).await;
    assert_eq!(
        list["rooms"][0]["host_avatar"]["custom_version"],
        json!(last)
    );
    let (_, users) = e
        .call("GET", "/v1/admin/users", "jfsrv", "jf-alice", None)
        .await;
    assert!(
        users
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["user_key"] == BOB && u["avatar"]["custom_version"] == json!(last))
    );
    // Removed: back to the default, the picture is gone.
    let (s, me) = e
        .call("DELETE", "/v1/me/avatar", "jfsrv", "jf-bob", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        me["avatar"],
        json!({"preset": null, "custom_version": null})
    );
    assert_eq!(
        e.call_raw("GET", &url, Some("jf-alice"), "", vec![])
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn profile_changes_are_rate_limited() {
    let e = env().await;
    for _ in 0..20 {
        assert_eq!(
            e.call(
                "PUT",
                "/v1/me/avatar",
                "jfsrv",
                "jf-bob",
                Some(json!({"preset": "p01"}))
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        e.call_raw(
            "PUT",
            "/v1/me/avatar",
            Some("jf-bob"),
            "image/jpeg",
            JPEG.to_vec()
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        e.call(
            "PUT",
            "/v1/me/name",
            "jfsrv",
            "jf-bob",
            Some(json!({"name": "B"}))
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Someone else is not affected.
    assert_eq!(
        e.call(
            "PUT",
            "/v1/me/name",
            "jfsrv",
            "jf-alice",
            Some(json!({"name": "Al"}))
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_welcome_step_is_once_per_account() {
    let e = env().await;
    let (_, me) = e.call("GET", "/v1/me", "jfsrv", "jf-bob", None).await;
    assert_eq!(me["profile_set"], false);
    // A name or an avatar alone does not end it.
    e.call(
        "PUT",
        "/v1/me/name",
        "jfsrv",
        "jf-bob",
        Some(json!({"name": "Bobby"})),
    )
    .await;
    let (_, me) = e
        .call(
            "PUT",
            "/v1/me/avatar",
            "jfsrv",
            "jf-bob",
            Some(json!({"preset": "p02"})),
        )
        .await;
    assert_eq!(me["profile_set"], false);
    for _ in 0..2 {
        let (s, me) = e
            .call("POST", "/v1/me/profile-done", "jfsrv", "jf-bob", None)
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(me["profile_set"], true);
    }
    // Bob's TV (another token of the same Jellyfin user) does not ask again.
    Mock::given(path("/Users/Me"))
        .and(header_regex("authorization", "Token=\"jf-bob-tv\"$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"Id": "jb", "Name": "bob", "Policy": {"IsAdministrator": false}}),
        ))
        .with_priority(1)
        .mount(&e.jf)
        .await;
    let (s, me) = e.call("GET", "/v1/me", "jfsrv", "jf-bob-tv", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["profile_set"], true);
    assert_eq!(me["name"], "Bobby");
    // Others start without it.
    let (_, me) = e.call("GET", "/v1/me", "jfsrv", "jf-alice", None).await;
    assert_eq!(me["profile_set"], false);
}

// ----- linked identities --------------------------------------------------------------------------

const BOB_JF: &str = "jellyfin:jfsrv:jb";
const BOB_PX: &str = "plex:plexsrv:2";

fn my_vote(kind: VoteKind, until: Option<i64>, created_at: i64) -> MyVote {
    MyVote {
        kind,
        until,
        reminder_days: None,
        reason: None,
        created_at,
    }
}

impl Env {
    /// Signs `token` in on `server` (creates the user).
    async fn me_as(&self, server: &str, token: &str) -> Value {
        let (s, me) = self.call("GET", "/v1/me", server, token, None).await;
        assert_eq!(s, StatusCode::OK, "{token} signs in");
        me
    }

    /// Signed in as (server, token), links the other sign-in.
    async fn link(&self, server: &str, token: &str, other: (&str, &str)) -> (StatusCode, Value) {
        self.call(
            "POST",
            "/v1/me/links",
            server,
            token,
            Some(json!({"server_id": other.0, "token": other.1})),
        )
        .await
    }

    fn vote_of(&self, item: &str, user: &str) -> Option<crate::db::VoteRow> {
        self.state
            .db
            .votes()
            .unwrap()
            .into_iter()
            .find(|v| v.item_key == item && v.user_key == user)
    }
}

#[tokio::test]
async fn linking_merges_two_accounts_into_one_jellyfin_person() {
    let e = env().await;
    e.state.sync().await.unwrap();
    let alice = "jellyfin:jfsrv:ja";
    e.me_as("jfsrv", "jf-alice").await;
    e.me_as("plexsrv", "px-friend").await;
    e.me_as("jfsrv", "jf-bob").await;
    let now = now();
    let db = &e.state.db;
    // Profile: the Plex side (the caller) chose a name, the Jellyfin side an avatar and did
    // the welcome step.
    e.call(
        "PUT",
        "/v1/me/name",
        "plexsrv",
        "px-friend",
        Some(json!({"name": "Bobby"})),
    )
    .await;
    e.call(
        "PUT",
        "/v1/me/name",
        "jfsrv",
        "jf-bob",
        Some(json!({"name": "Robert"})),
    )
    .await;
    e.call(
        "PUT",
        "/v1/me/avatar",
        "jfsrv",
        "jf-bob",
        Some(json!({"preset": "p03"})),
    )
    .await;
    e.call("POST", "/v1/me/profile-done", "jfsrv", "jf-bob", None)
        .await;
    // Votes, each conflict case (A = Plex side, B = Jellyfin side).
    let set = |item: &str, user: &str, v: MyVote| db.set_vote(item, user, &v).unwrap();
    set(
        "100",
        BOB_PX,
        my_vote(VoteKind::KeepDays, Some(now + 10 * DAY), 1),
    );
    set("100", BOB_JF, my_vote(VoteKind::KeepForever, None, 2)); // B forever > A days
    set("101", BOB_PX, my_vote(VoteKind::KeepForever, None, 3));
    set("101", BOB_JF, my_vote(VoteKind::KeepUntilWatched, None, 4)); // A forever > B watched
    set("200", BOB_PX, my_vote(VoteKind::DontCare, None, 5));
    set("200", BOB_JF, my_vote(VoteKind::KeepUntilWatched, None, 6)); // watched > don't care
    set(
        "d1",
        BOB_PX,
        my_vote(VoteKind::KeepDays, Some(now + 5 * DAY), 7),
    );
    set(
        "d1",
        BOB_JF,
        my_vote(VoteKind::KeepDays, Some(now + 50 * DAY), 8),
    ); // longest days
    set(
        "d2",
        BOB_PX,
        my_vote(VoteKind::KeepDays, Some(now - DAY), 9),
    ); // expired
    set("d2", BOB_JF, my_vote(VoteKind::DontCare, None, 10)); // don't care > an expired keep
    set("w1", BOB_PX, my_vote(VoteKind::KeepUntilWatched, None, 11));
    db.fulfill("w1", BOB_PX).unwrap(); // watched already
    set(
        "w1",
        BOB_JF,
        my_vote(VoteKind::KeepDays, Some(now + 3 * DAY), 12),
    ); // active > fulfilled
    set("t1", BOB_PX, my_vote(VoteKind::KeepForever, None, 13));
    set("t1", BOB_JF, my_vote(VoteKind::KeepForever, None, 14)); // tie: A's
    set("only-b", BOB_JF, my_vote(VoteKind::KeepForever, None, 15));
    // Rights: an admin restricted the Jellyfin side only; that choice stays.
    let (s, _) = e
        .call(
            "PUT",
            &format!("/v1/admin/users/{}/rights", BOB_JF.replace(':', "%3A")),
            "jfsrv",
            "jf-alice",
            Some(json!({"can_keep_forever": true, "max_forever": null, "max_keep_days": 60})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // Seerr: the Plex side got a bridge-made user, the Jellyfin side is linked to a real one.
    db.set_seerr_link(BOB_PX, 9, true).unwrap();
    db.set_seerr_link(BOB_JF, 7, false).unwrap();
    // Settings: newer value per key.
    db.set_settings(
        BOB_PX,
        &[
            ("a".into(), "5\u{241F}x".into()),
            ("b".into(), "1\u{241F}old".into()),
        ],
    )
    .unwrap();
    db.set_settings(
        BOB_JF,
        &[
            ("b".into(), "9\u{241F}new".into()),
            ("c".into(), "1\u{241F}y".into()),
        ],
    )
    .unwrap();
    // Watch together history with alice from both sides, and with oneself.
    db.record_together(&[(BOB_PX.into(), alice.into())], 100)
        .unwrap();
    db.record_together(&[(BOB_JF.into(), alice.into())], 200)
        .unwrap();
    db.record_together(&[(BOB_PX.into(), BOB_JF.into())], 300)
        .unwrap();

    // Signed in with Plex, prove the Jellyfin sign-in.
    let (s, links) = e.link("plexsrv", "px-friend", ("jfsrv", "jf-bob")).await;
    assert_eq!(s, StatusCode::OK, "{links}");
    let links: Vec<LinkedIdentity> = serde_json::from_value(links).unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(
        (
            links[0].user_key.as_str(),
            links[0].canonical,
            links[0].current
        ),
        (BOB_JF, true, false),
        "Jellyfin holds the data, listed first"
    );
    assert_eq!(
        (
            links[1].user_key.as_str(),
            links[1].canonical,
            links[1].current
        ),
        (BOB_PX, false, true)
    );
    assert_eq!(links[1].server_name, "HomePlex");

    // Both sign-ins are now the same account.
    for (server, token) in [("plexsrv", "px-friend"), ("jfsrv", "jf-bob")] {
        let me = e.me_as(server, token).await;
        assert_eq!(me["user_key"], BOB_JF);
        assert_eq!(me["name"], "Bobby", "the caller's chosen name wins");
        assert_eq!(
            me["avatar"]["preset"], "p03",
            "the other side fills the avatar"
        );
        assert_eq!(me["profile_set"], true);
        assert_eq!(
            me["server_id"], server,
            "the server this device signed in to"
        );
        assert_eq!(me["rights"]["max_keep_days"], 60);
        assert_eq!(me["identities"].as_array().unwrap().len(), 2);
        assert_eq!(me["is_admin"], false);
    }
    let kind = |item: &str| e.vote_of(item, BOB_JF).map(|v| v.vote.kind);
    assert_eq!(kind("100"), Some(VoteKind::KeepForever));
    assert_eq!(kind("101"), Some(VoteKind::KeepForever));
    assert_eq!(kind("200"), Some(VoteKind::KeepUntilWatched));
    assert_eq!(
        e.vote_of("d1", BOB_JF).unwrap().vote.until,
        Some(now + 50 * DAY)
    );
    assert_eq!(kind("d2"), Some(VoteKind::DontCare));
    assert_eq!(kind("w1"), Some(VoteKind::KeepDays));
    assert_eq!(
        e.vote_of("t1", BOB_JF).unwrap().vote.created_at,
        13,
        "tie: the caller's"
    );
    assert_eq!(kind("only-b"), Some(VoteKind::KeepForever));
    assert!(
        db.votes().unwrap().iter().all(|v| v.user_key != BOB_PX),
        "nothing left under the alias"
    );
    // One vote per person in the counts, seen from either sign-in.
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "plexsrv", "px-friend", None)
        .await;
    let item = |k: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|i| i["key"] == k)
            .cloned()
            .unwrap()
    };
    assert_eq!(item("100")["my_vote"]["kind"], "keep_forever");
    assert_eq!(
        item("100")["counts"],
        json!({"forever": 1, "until_watched": 0, "days": 0, "dont_care": 0})
    );
    assert_eq!(item("200")["my_vote"]["kind"], "keep_until_watched");
    assert_eq!(
        item("200")["server_item_id"],
        "200",
        "ids of the server signed in to (Plex)"
    );
    let (_, list) = e
        .call("GET", "/v1/leaving-soon", "jfsrv", "jf-bob", None)
        .await;
    let show = list
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == "200")
        .unwrap();
    assert_eq!(
        show["server_item_id"], "jf-show",
        "mapped for the Jellyfin sign-in"
    );
    assert_eq!(show["my_vote"]["kind"], "keep_until_watched");

    assert_eq!(
        db.seerr_link(BOB_JF).unwrap(),
        Some(7),
        "the real Seerr user wins"
    );
    assert_eq!(db.seerr_link(BOB_PX).unwrap(), None);
    let settings: std::collections::HashMap<String, String> =
        db.settings(BOB_JF).unwrap().into_iter().collect();
    assert_eq!(settings["a"], "5\u{241F}x");
    assert_eq!(settings["b"], "9\u{241F}new");
    assert_eq!(settings["c"], "1\u{241F}y");

    // Alice sees Bob once, with the latest time they watched together.
    let (_, people) = e
        .call("GET", "/v1/together/people", "jfsrv", "jf-alice", None)
        .await;
    let people = people.as_array().unwrap();
    assert_eq!(people.len(), 1, "{people:?}");
    assert_eq!(people[0]["user_key"], BOB_JF);
    assert_eq!(people[0]["name"], "Bobby");
    assert_eq!(people[0]["last_together_at"], 200);
    assert_eq!(people[0]["server_name"], "HomeJelly");
    let (_, people) = e
        .call("GET", "/v1/together/people", "plexsrv", "px-friend", None)
        .await;
    let people = people.as_array().unwrap();
    assert_eq!(people.len(), 1, "only alice, never oneself: {people:?}");
    assert_eq!(people[0]["last_together_at"], 200);
    let (_, users) = e
        .call("GET", "/v1/admin/users", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(users.as_array().unwrap().len(), 2, "alice and bob: {users}");
    // Admin calls naming the alias reach the person.
    let (s, u) = e
        .call(
            "PUT",
            &format!("/v1/admin/users/{}/seerr", BOB_PX.replace(':', "%3A")),
            "jfsrv",
            "jf-alice",
            Some(json!({"seerr_user_id": 12})),
        )
        .await;
    assert_eq!((s, u["user_key"].clone()), (StatusCode::OK, json!(BOB_JF)));

    // Idempotent: again, and from the other side.
    let (s, again) = e.link("plexsrv", "px-friend", ("jfsrv", "jf-bob")).await;
    assert_eq!((s, again.as_array().unwrap().len()), (StatusCode::OK, 2));
    let (s, _) = e.link("jfsrv", "jf-bob", ("plexsrv", "px-friend")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = e.link("jfsrv", "jf-bob", ("jfsrv", "jf-bob")).await;
    assert_eq!(s, StatusCode::OK, "linking yourself is a no-op");
    assert_eq!(e.vote_of("t1", BOB_JF).unwrap().vote.created_at, 13);
    assert_eq!(db.settings(BOB_JF).unwrap().len(), 3);

    // Tokens never reach the log.
    let log = db.log_lines();
    assert!(log.iter().any(|l| l.contains(" link ")), "{log:?}");
    assert!(
        log.iter()
            .all(|l| !l.contains("jf-bob") && !l.contains("px-friend"))
    );
}

#[tokio::test]
async fn link_refusals_change_nothing() {
    let e = env().await;
    e.me_as("plexsrv", "px-friend").await;
    e.me_as("jfsrv", "jf-bob").await;
    let unchanged = |e: &Env| {
        assert_eq!(e.state.db.identities_of(BOB_PX).unwrap().len(), 1);
        assert_eq!(e.state.db.identities_of(BOB_JF).unwrap().len(), 1);
    };
    for (other, status) in [
        (("jfsrv", "tok-zz9-expired"), StatusCode::UNAUTHORIZED),
        (("jfsrv", ""), StatusCode::UNAUTHORIZED),
        (("plexsrv", "px-stranger"), StatusCode::UNAUTHORIZED),
        (("elsewhere", "jf-bob"), StatusCode::FORBIDDEN),
    ] {
        let (s, body) = e.link("plexsrv", "px-friend", other).await;
        assert_eq!(s, status, "{other:?}");
        assert!(
            !body.to_string().contains("tok-zz9"),
            "the token is never echoed"
        );
        unchanged(&e);
    }
    // Two different users of one server never become one person.
    e.link("plexsrv", "px-friend", ("jfsrv", "jf-bob")).await;
    let (s, body) = e.link("jfsrv", "jf-alice", ("plexsrv", "px-friend")).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("HomeJelly"));
    assert_eq!(e.state.db.identities_of(BOB_JF).unwrap().len(), 2);
    assert_eq!(
        e.state.db.identities_of("jellyfin:jfsrv:ja").unwrap().len(),
        1
    );
    // Not signed in: nothing.
    let (s, _) = e.link("jfsrv", "nope", ("plexsrv", "px-friend")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let tokens_in_log = e
        .state
        .db
        .log_lines()
        .into_iter()
        .any(|l| l.contains("tok-zz9") || l.contains("px-stranger"));
    assert!(!tokens_in_log);
}

#[tokio::test]
async fn link_attempts_are_rate_limited_per_person() {
    let e = env().await;
    for _ in 0..10 {
        let (s, _) = e.link("jfsrv", "jf-carol", ("plexsrv", "wrong")).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    // Even a good token now waits, and the media server is not asked.
    let (s, _) = e.link("jfsrv", "jf-carol", ("plexsrv", "px-friend")).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        e.state.db.identities_of("jellyfin:jfsrv:jc").unwrap().len(),
        1
    );
    // Someone else is not affected.
    let (s, _) = e.link("jfsrv", "jf-bob", ("plexsrv", "px-friend")).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn admin_follows_the_person_but_only_from_fresh_served_sign_ins() {
    let e = env().await;
    // Bob's Jellyfin user is not an admin; the Plex owner is.
    assert_eq!(e.me_as("jfsrv", "jf-bob").await["is_admin"], false);
    let (s, links) = e.link("jfsrv", "jf-bob", ("plexsrv", "px-owner")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        links[0]["user_key"], BOB_JF,
        "the Jellyfin side stays canonical"
    );
    assert_eq!(e.me_as("jfsrv", "jf-bob").await["is_admin"], true);
    let (s, _) = e
        .call("GET", "/v1/admin/users", "jfsrv", "jf-bob", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    // The Plex sign-in has not been used for long: its admin flag no longer counts.
    e.state
        .db
        .exec("UPDATE identities SET last_seen = 0 WHERE user_key = 'plex:plexsrv:1'");
    assert_eq!(e.me_as("jfsrv", "jf-bob").await["is_admin"], false);
    assert_eq!(
        e.call("GET", "/v1/admin/users", "jfsrv", "jf-bob", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Signing in with Plex again refreshes it.
    assert_eq!(e.me_as("plexsrv", "px-owner").await["is_admin"], true);
    assert_eq!(e.me_as("jfsrv", "jf-bob").await["is_admin"], true);
}

#[tokio::test]
async fn linking_into_a_person_with_several_sign_ins_merges_both_persons() {
    let e = env().await;
    e.me_as("jfsrv", "jf-bob").await;
    e.link("jfsrv", "jf-bob", ("plexsrv", "px-friend")).await;
    e.state
        .db
        .set_vote("100", BOB_JF, &my_vote(VoteKind::KeepForever, None, 1))
        .unwrap();
    // Carol on the second Jellyfin is the same person, with data of her own.
    let carol = "jellyfin:jf2:jc";
    e.me_as("jf2", "jf-carol").await;
    e.state
        .db
        .set_vote("101", carol, &my_vote(VoteKind::DontCare, None, 2))
        .unwrap();
    // From the second Jellyfin, prove the Plex sign-in (it belongs to Bob's person).
    let (s, links) = e.link("jf2", "jf-carol", ("plexsrv", "px-friend")).await;
    assert_eq!(s, StatusCode::OK, "{links}");
    let keys: Vec<&str> = links
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["user_key"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), 3);
    assert_eq!(
        keys[0], carol,
        "the caller's canonical Jellyfin key holds everything"
    );
    assert!(keys.contains(&BOB_JF) && keys.contains(&BOB_PX));
    for (server, token) in [
        ("jfsrv", "jf-bob"),
        ("plexsrv", "px-friend"),
        ("jf2", "jf-carol"),
    ] {
        assert_eq!(e.me_as(server, token).await["user_key"], carol);
    }
    assert!(e.vote_of("100", carol).is_some() && e.vote_of("101", carol).is_some());
    assert!(
        e.state
            .db
            .votes()
            .unwrap()
            .iter()
            .all(|v| v.user_key == carol)
    );
}

#[tokio::test]
async fn rooms_of_one_sign_in_are_mine_with_the_other() {
    let e = env().await;
    e.me_as("jfsrv", "jf-alice").await;
    e.me_as("jfsrv", "jf-bob").await;
    // Before linking: a lobby opened with Plex, and an invitation from alice to the Plex side.
    let (_, joined) = e
        .call(
            "POST",
            "/v1/together",
            "plexsrv",
            "px-friend",
            Some(json!({"media": arrival()})),
        )
        .await;
    let own = joined["state"]["code"].as_str().unwrap().to_owned();
    let (_, a) = e
        .call(
            "POST",
            "/v1/together",
            "jfsrv",
            "jf-alice",
            Some(json!({"media": arrival()})),
        )
        .await;
    let theirs = a["state"]["code"].as_str().unwrap().to_owned();
    let (s, _) = e
        .call(
            "POST",
            &format!("/v1/together/{theirs}/invites"),
            "jfsrv",
            "jf-alice",
            Some(json!({"member_id": a["member_id"], "user_keys": [BOB_PX]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (_, rooms) = e.call("GET", "/v1/together", "jfsrv", "jf-bob", None).await;
    assert!(rooms["rooms"].as_array().unwrap().is_empty());

    e.link("jfsrv", "jf-bob", ("plexsrv", "px-friend")).await;
    let (_, rooms) = e.call("GET", "/v1/together", "jfsrv", "jf-bob", None).await;
    let rooms = rooms["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 2, "{rooms:?}");
    let room = |code: &str| rooms.iter().find(|r| r["code"] == code).unwrap();
    assert_eq!(
        (room(&own)["mine"].clone(), room(&own)["hosting"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(room(&theirs)["invited"], true);
    // Join the own room from the other sign-in without an invitation, as the host.
    let (s, j) = e
        .call(
            "POST",
            &format!("/v1/together/{own}/join"),
            "jfsrv",
            "jf-bob",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let users: Vec<&str> = j["state"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user_key"].as_str().unwrap())
        .collect();
    assert_eq!(users, [BOB_JF, BOB_JF], "one person on two devices");
    let (s, _) = e
        .call(
            "POST",
            &format!("/v1/together/{theirs}/join"),
            "plexsrv",
            "px-friend",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "the invitation follows the person");
    // Inviting an alias key invites the person; already in: nothing new.
    let (_, st) = e
        .call(
            "POST",
            &format!("/v1/together/{theirs}/invites"),
            "jfsrv",
            "jf-alice",
            Some(json!({"member_id": a["member_id"], "user_keys": [BOB_PX]})),
        )
        .await;
    assert!(st["invited"].as_array().unwrap().is_empty(), "{st}");
}

#[tokio::test]
async fn unlinking_gives_back_a_fresh_account() {
    let e = env().await;
    e.me_as("plexsrv", "px-friend").await;
    e.call(
        "PUT",
        "/v1/me/name",
        "plexsrv",
        "px-friend",
        Some(json!({"name": "Bobby"})),
    )
    .await;
    e.link("plexsrv", "px-friend", ("jfsrv", "jf-bob")).await;
    e.state
        .db
        .set_vote("100", BOB_JF, &my_vote(VoteKind::KeepForever, None, 1))
        .unwrap();
    let path = |k: &str| format!("/v1/me/links/{}", k.replace(':', "%3A"));
    // The canonical sign-in cannot go; someone else's is unknown.
    let (s, body) = e
        .call("DELETE", &path(BOB_JF), "plexsrv", "px-friend", None)
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("cannot be unlinked")
    );
    let (s, _) = e
        .call(
            "DELETE",
            &path("jellyfin:jfsrv:ja"),
            "plexsrv",
            "px-friend",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // The Plex side leaves (asked from that very sign-in).
    let (s, left) = e
        .call("DELETE", &path(BOB_PX), "plexsrv", "px-friend", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(left.as_array().unwrap().len(), 1);
    assert_eq!(left[0]["user_key"], BOB_JF);
    let me = e.me_as("plexsrv", "px-friend").await;
    assert_eq!(me["user_key"], BOB_PX);
    assert_eq!(
        (me["name"].clone(), me["name_is_custom"].clone()),
        (json!("User 2"), json!(false))
    );
    assert_eq!(me["identities"].as_array().unwrap().len(), 1);
    assert!(e.vote_of("100", BOB_PX).is_none());
    let me = e.me_as("jfsrv", "jf-bob").await;
    assert_eq!(
        (me["user_key"].clone(), me["name"].clone()),
        (json!(BOB_JF), json!("Bobby"))
    );
    assert!(
        e.vote_of("100", BOB_JF).is_some(),
        "the data stays with the canonical sign-in"
    );
    // Alone: nothing to unlink.
    let (s, _) = e
        .call("DELETE", &path(BOB_PX), "plexsrv", "px-friend", None)
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn watched_on_any_linked_sign_in_fulfils_the_vote() {
    let e = env().await;
    e.state.sync().await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/rules/exclusion"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&e.maint)
        .await;
    e.me_as("jfsrv", "jf-bob").await;
    e.link("jfsrv", "jf-bob", ("plexsrv", "px-friend")).await;
    let (s, _) = e
        .call(
            "PUT",
            "/v1/votes/100",
            "jfsrv",
            "jf-bob",
            Some(json!({"kind": "keep_until_watched"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // Not played on Jellyfin, nor yet on Plex.
    Mock::given(path("/Items/jf-arrival"))
        .and(query_param("userId", "jb"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Id": "jf-arrival", "UserData": {"Played": false}})),
        )
        .mount(&e.jf)
        .await;
    let plex = Mock::given(path("/api/v2"))
        .and(query_param("user_id", "2"))
        .and(query_param("rating_key", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"response": {"result": "success", "data": {"data": [{"watched_status": 0.2}]}}}),
        ))
        .mount_as_scoped(&e.tautulli)
        .await;
    assert_eq!(e.state.check_watched().await.unwrap(), 0);
    drop(plex);
    // Watched on Plex: fulfilled for the person.
    Mock::given(path("/api/v2"))
        .and(query_param("user_id", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"response": {"result": "success", "data": {"data": [{"watched_status": 1}]}}}),
        ))
        .mount(&e.tautulli)
        .await;
    assert_eq!(e.state.check_watched().await.unwrap(), 1);
    assert!(e.vote_of("100", BOB_JF).unwrap().fulfilled_at.is_some());
}

#[tokio::test]
async fn seerr_link_reuses_the_seerr_user_of_any_linked_sign_in() {
    let e = env().await;
    // Seerr (Plex mode) knows Plex user 2 as Seerr user 7.
    Mock::given(method("GET"))
        .and(path("/api/v1/user"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"results": [{"id": 7, "plexId": 2, "displayName": "friend"}]}),
            ),
        )
        .mount(&e.seerr)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/user"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 99})))
        .expect(0)
        .mount(&e.seerr)
        .await;
    Mock::given(path("/api/v1/auth/me"))
        .and(header("x-api-user", "7"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": 7, "displayName": "friend"})),
        )
        .mount(&e.seerr)
        .await;
    e.me_as("jfsrv", "jf-bob").await;
    e.link("jfsrv", "jf-bob", ("plexsrv", "px-friend")).await;
    let (s, info) = e.call("GET", "/v1/seerr", "jfsrv", "jf-bob", None).await;
    assert_eq!(
        (s, info["seerr_user_id"].clone()),
        (StatusCode::OK, json!(7))
    );
    e.seerr.verify().await;
}

#[tokio::test]
async fn custom_avatar_follows_the_merge() {
    let e = env().await;
    e.me_as("jfsrv", "jf-bob").await;
    let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3].to_vec();
    // The Plex side (the caller) has a custom picture; the Jellyfin side will hold the data.
    let req = Request::builder()
        .method("PUT")
        .uri("/v1/me/avatar")
        .header("content-type", "image/png")
        .header(SERVER_HEADER, "plexsrv")
        .header("authorization", "Bearer px-friend");
    let res = crate::api::router(e.state.clone())
        .oneshot(req.body(Body::from(png.clone())).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let before = e.me_as("plexsrv", "px-friend").await["avatar"]["custom_version"]
        .as_u64()
        .unwrap();
    e.link("plexsrv", "px-friend", ("jfsrv", "jf-bob")).await;
    let me = e.me_as("jfsrv", "jf-bob").await;
    let v = me["avatar"]["custom_version"].as_u64().unwrap();
    assert!(v > before, "a new version: no stale cached picture");
    let (s, _, bytes) = e
        .call_raw(
            "GET",
            &format!("/v1/users/{}/avatar?v={v}", BOB_JF.replace(':', "%3A")),
            Some("jf-bob"),
            "",
            vec![],
        )
        .await;
    assert_eq!((s, bytes.clone()), (StatusCode::OK, png.clone()));
    // An old link naming the alias still finds the picture.
    let (s, _, bytes) = e
        .call_raw(
            "GET",
            &format!("/v1/users/{}/avatar?v={v}", BOB_PX.replace(':', "%3A")),
            Some("jf-bob"),
            "",
            vec![],
        )
        .await;
    assert_eq!((s, bytes), (StatusCode::OK, png));
}

#[tokio::test]
async fn older_databases_get_identities_and_keep_their_users() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("keep.sqlite");
    // The schema before linked identities, with a user who chose a name and voted.
    rusqlite::Connection::open(&file)
        .unwrap()
        .execute_batch(
            "CREATE TABLE users (user_key TEXT PRIMARY KEY, name TEXT NOT NULL, server_id TEXT NOT NULL,
               is_admin INTEGER NOT NULL DEFAULT 0, last_seen INTEGER NOT NULL,
               custom_name TEXT, avatar_preset TEXT, avatar_type TEXT,
               avatar_version INTEGER NOT NULL DEFAULT 0, profile_set INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE votes (item_key TEXT NOT NULL, user_key TEXT NOT NULL, kind TEXT NOT NULL,
               until_ts INTEGER, reminder_days INTEGER, reason TEXT, created_at INTEGER NOT NULL,
               fulfilled_at INTEGER, PRIMARY KEY (item_key, user_key));
             INSERT INTO users(user_key, name, server_id, last_seen, custom_name, profile_set)
               VALUES ('plex:plexsrv:2', 'User 2', 'plexsrv', 5, 'Bobby', 1);
             INSERT INTO votes VALUES ('100', 'plex:plexsrv:2', 'keep_forever', NULL, NULL, NULL, 1, NULL);",
        )
        .unwrap();
    drop(Db::open(&file).unwrap());
    let e = env_at(Db::open(&file).unwrap(), false).await; // opening twice is a no-op
    let (s, links) = e
        .call("GET", "/v1/me/links", "plexsrv", "px-friend", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        links,
        json!([{"user_key": BOB_PX, "server_id": "plexsrv", "server_name": "HomePlex",
                "kind": "plex", "media_name": "User 2", "canonical": true, "current": true}])
    );
    let me = e.me_as("plexsrv", "px-friend").await;
    assert_eq!(
        (me["user_key"].clone(), me["name"].clone()),
        (json!(BOB_PX), json!("Bobby"))
    );
    let (s, _) = e.link("plexsrv", "px-friend", ("jfsrv", "jf-bob")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        e.vote_of("100", BOB_JF).unwrap().vote.kind,
        VoteKind::KeepForever
    );
    assert_eq!(e.me_as("jfsrv", "jf-bob").await["name"], "Bobby");
}

#[tokio::test]
async fn a_person_whose_canonical_server_is_no_longer_served_keeps_working() {
    // Before Jellyfin preference existed (or with two Plex servers), a Plex key may be
    // canonical; with Plex gone from the allow-list the person still signs in with Jellyfin.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("keep.sqlite");
    let db = Db::open(&file).unwrap();
    db.touch_user("plex:oldplex:5", "Ann", "oldplex", false)
        .unwrap();
    db.set_custom_name("plex:oldplex:5", Some("Annie")).unwrap();
    db.exec(
        "INSERT INTO identities VALUES ('plex:oldplex:5', 'plex:oldplex:5', 'oldplex', 'plex', '5', 'Ann', 0, 1, 1);
         INSERT INTO identities VALUES ('jellyfin:jfsrv:jb', 'plex:oldplex:5', 'jfsrv', 'jellyfin', 'jb', 'bob', 0, 1, 2);",
    );
    let e = env_at(db, false).await;
    let me = e.me_as("jfsrv", "jf-bob").await;
    assert_eq!(
        (me["user_key"].clone(), me["name"].clone()),
        (json!("plex:oldplex:5"), json!("Annie"))
    );
    let links = me["identities"].as_array().unwrap().clone();
    assert_eq!(
        links[0]["server_name"], "oldplex",
        "no longer served: its id"
    );
    // Others still find and invite them.
    e.me_as("jfsrv", "jf-alice").await;
    let (_, people) = e
        .call("GET", "/v1/together/people", "jfsrv", "jf-alice", None)
        .await;
    assert_eq!(people[0]["user_key"], "plex:oldplex:5");
    assert_eq!(people[0]["server_name"], "HomeJelly");
}

// ----- AlwyWatch accounts -------------------------------------------------------------------

impl Env {
    /// A request without a media sign-in: `bearer` is an account session (or nothing).
    async fn anon(
        &self,
        method: &str,
        uri: &str,
        bearer: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = bearer {
            req = req.header("authorization", format!("Bearer {t}"));
        }
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
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// An account made with an invitation from alice (admin): its session token.
    async fn sign_up(&self, username: &str) -> String {
        let (st, inv) = self
            .call(
                "POST",
                "/v1/account/invites",
                "jfsrv",
                "jf-alice",
                Some(json!({})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{inv}");
        let (st, _, r) = self
            .anon(
                "POST",
                "/v1/account/register",
                None,
                Some(json!({"username": username, "password": "password1", "invite": inv["code"], "device": "Pixel"})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{r}");
        r["token"].as_str().unwrap().to_owned()
    }
}

#[tokio::test]
async fn accounts_sign_up_with_an_invitation_and_sign_in() {
    let e = env().await;
    let (st, _, o) = e.anon("GET", "/v1/account/options", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        o,
        json!({"registration": "invite", "passkeys": true, "pocketid": true})
    );

    // No invitation: refused. Only admins make invitations.
    let (st, _, _) = e
        .anon(
            "POST",
            "/v1/account/register",
            None,
            Some(json!({"username": "sam", "password": "password1"})),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = e
        .call(
            "POST",
            "/v1/account/invites",
            "jfsrv",
            "jf-bob",
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let token = e.sign_up("sam").await;
    assert!(token.starts_with("aws_"));
    let (st, _, me) = e.anon("GET", "/v1/account", Some(&token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me["username"], "sam");
    assert_eq!(me["name"], "sam");
    assert!(me["user_key"].as_str().unwrap().starts_with("account:"));
    assert_eq!(me["sessions"][0]["device"], "Pixel");
    assert_eq!(me["sessions"][0]["current"], true);

    // Username taken (any case); wrong password; then right.
    let (st, inv) = e
        .call(
            "POST",
            "/v1/account/invites",
            "jfsrv",
            "jf-alice",
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = e
        .anon(
            "POST",
            "/v1/account/register",
            None,
            Some(json!({"username": "SAM", "password": "password1", "invite": inv["code"]})),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _, _) = e
        .anon(
            "POST",
            "/v1/account/login",
            None,
            Some(json!({"username": "sam", "password": "nope-nope"})),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, r) = e
        .anon(
            "POST",
            "/v1/account/login",
            None,
            Some(json!({"username": "Sam", "password": "password1", "device": "TV"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let tv = r["token"].as_str().unwrap().to_owned();

    // Settings follow the account; keep votes and rooms still need a media sign-in.
    let (st, _, _) = e
        .anon(
            "PUT",
            "/v1/me/settings",
            Some(&tv),
            Some(json!({"player.auto_next": "0"})),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, _, set) = e.anon("GET", "/v1/me/settings", Some(&token), None).await;
    assert_eq!(set["player.auto_next"], "0");
    let (st, _, _) = e.anon("GET", "/v1/leaving-soon", Some(&tv), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Signing out ends that session only.
    let (st, _, _) = e.anon("POST", "/v1/account/logout", Some(&tv), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = e.anon("GET", "/v1/account", Some(&tv), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = e.anon("GET", "/v1/account", Some(&token), None).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn quick_connect_signs_a_new_device_in() {
    let e = env().await;
    let token = e.sign_up("sam").await;
    let (_, _, me) = e.anon("GET", "/v1/account", Some(&token), None).await;

    // From the web (an account session): the new device gets a session of the same account.
    let (st, _, t) = e
        .anon(
            "POST",
            "/v1/transfer",
            None,
            Some(json!({"public_key": "ab".repeat(32), "device": "Living room TV"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (code, handle) = (t["code"].as_str().unwrap(), t["handle"].as_str().unwrap());
    let (st, _, peer) = e
        .anon("GET", &format!("/v1/transfer/{code}"), Some(&token), None)
        .await;
    assert_eq!(
        (st, peer["device"].as_str()),
        (StatusCode::OK, Some("Living room TV"))
    );
    let (st, _, a) = e
        .anon(
            "PUT",
            &format!("/v1/transfer/{code}/approve"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(
        (st, a["device"].as_str()),
        (StatusCode::OK, Some("Living room TV"))
    );
    let (st, _, got) = e
        .anon("GET", &format!("/v1/transfer/poll/{handle}"), None, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["sealed"], "");
    let tv = got["account_token"].as_str().unwrap();
    let (_, _, tv_me) = e.anon("GET", "/v1/account", Some(tv), None).await;
    assert_eq!(tv_me["id"], me["id"]);
    assert!(
        tv_me["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["device"] == "Living room TV")
    );

    // From an app signed in to Jellyfin: its sealed sign-ins, and a session of that person.
    let (_, _, t) = e
        .anon(
            "POST",
            "/v1/transfer",
            None,
            Some(json!({"public_key": "cd".repeat(32), "device": "Phone"})),
        )
        .await;
    let (code, handle) = (t["code"].as_str().unwrap(), t["handle"].as_str().unwrap());
    let (st, _) = e
        .call(
            "PUT",
            &format!("/v1/transfer/{code}"),
            "jfsrv",
            "jf-bob",
            Some(json!({"sealed": "00ff"})),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, _, got) = e
        .anon("GET", &format!("/v1/transfer/poll/{handle}"), None, None)
        .await;
    assert_eq!(got["sealed"], "00ff");
    let (_, _, phone) = e
        .anon(
            "GET",
            "/v1/account",
            Some(got["account_token"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(phone["user_key"], BOB);
    assert_eq!(phone["username"], Value::Null);
}

#[tokio::test]
async fn an_account_takes_a_jellyfin_sign_in_and_a_media_user_adds_a_password() {
    let e = env().await;
    let token = e.sign_up("sam").await;
    e.anon(
        "PUT",
        "/v1/me/name",
        Some(&token),
        Some(json!({"name": "Samuel"})),
    )
    .await;
    let (st, _, me) = e
        .anon(
            "POST",
            "/v1/account/link",
            Some(&token),
            Some(json!({"server_id": "jfsrv", "token": "jf-carol"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{me}");
    assert_eq!(me["user_key"], "jellyfin:jfsrv:jc");
    assert_eq!(me["linked"][0]["name"], "carol");
    assert_eq!(me["name"], "Samuel", "the account's chosen name stays");
    // Carol's Jellyfin sign-in is now the same person.
    let (_, carol) = e.call("GET", "/v1/me", "jfsrv", "jf-carol", None).await;
    assert_eq!(carol["user_key"], "jellyfin:jfsrv:jc");

    // Bob (Jellyfin only) adds a username and password: same person, signs in without Jellyfin.
    let (st, _) = e
        .call(
            "POST",
            "/v1/account/claim",
            "jfsrv",
            "jf-bob",
            Some(json!({"username": "bob", "password": "password2"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, r) = e
        .anon(
            "POST",
            "/v1/account/login",
            None,
            Some(json!({"username": "bob", "password": "password2"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(r["account"]["user_key"], BOB);
}

#[tokio::test]
async fn pocket_id_creates_the_account_then_signs_it_in() {
    let e = env().await;
    let base = e.oidc.uri();
    Mock::given(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/api/oidc/token"),
            "userinfo_endpoint": format!("{base}/api/oidc/userinfo"),
        })))
        .mount(&e.oidc)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/oidc/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "at", "token_type": "Bearer"})),
        )
        .mount(&e.oidc)
        .await;
    Mock::given(path("/api/oidc/userinfo"))
        .and(header("authorization", "Bearer at"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sub": "p-1", "name": "Pat", "preferred_username": "pat"})),
        )
        .mount(&e.oidc)
        .await;

    let sign_in = || async {
        let (st, h, _) = e
            .anon("GET", "/v1/account/oidc/start?next=/q/123456", None, None)
            .await;
        assert_eq!(st, StatusCode::SEE_OTHER);
        let to = h["location"].to_str().unwrap().to_owned();
        assert!(to.starts_with(&format!("{base}/authorize?")), "{to}");
        assert!(to.contains("code_challenge_method=S256"));
        let state = to
            .split("state=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned();
        let (st, h, _) = e
            .anon(
                "GET",
                &format!("/v1/account/oidc/callback?code=c&state={state}"),
                None,
                None,
            )
            .await;
        assert_eq!(st, StatusCode::SEE_OTHER);
        let back = h["location"].to_str().unwrap().to_owned();
        assert!(back.starts_with("/account/signed-in#token=aws_"), "{back}");
        assert!(back.ends_with("&next=/q/123456"), "{back}");
        let token = back
            .split("token=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned();
        let (_, _, me) = e.anon("GET", "/v1/account", Some(&token), None).await;
        me
    };
    let first = sign_in().await;
    assert_eq!(
        (first["username"].as_str(), first["name"].as_str()),
        (Some("pat"), Some("Pat"))
    );
    assert_eq!(first["pocketid"], true);
    let again = sign_in().await;
    assert_eq!(again["id"], first["id"], "the same account the second time");

    // A replayed state is refused.
    let (_, h, _) = e
        .anon(
            "GET",
            "/v1/account/oidc/callback?code=c&state=unknown",
            None,
            None,
        )
        .await;
    assert!(
        h["location"]
            .to_str()
            .unwrap()
            .starts_with("/account?error=")
    );
}
