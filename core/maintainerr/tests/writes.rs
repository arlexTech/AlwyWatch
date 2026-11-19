// SPDX-License-Identifier: AGPL-3.0-or-later
use maintainerr::Maintainerr;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(s: &MockServer, key: Option<&str>) -> Maintainerr {
    Maintainerr::new(
        net::api_client("t").unwrap(),
        &s.uri(),
        key.map(str::to_owned),
    )
}

#[tokio::test]
async fn postpone_sends_days_and_clamps_and_reset_omits_them() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/collections/media/postpone"))
        .and(body_json(
            json!({"collectionId": 2, "mediaId": "175731", "days": 3650}),
        ))
        .and(header("x-api-key", "k"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"deletionDate": "2036-01-01T00:00:00.000Z"})),
        )
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/collections/media/postpone"))
        .and(body_json(json!({"collectionId": 2, "mediaId": "175731"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(1)
        .mount(&s)
        .await;
    let m = client(&s, Some("k"));
    let d = m.postpone(2, "175731", Some(99_999)).await.unwrap();
    assert_eq!(d, maintainerr::parse_date("2036-01-01T00:00:00.000Z"));
    assert_eq!(m.postpone(2, "175731", None).await.unwrap(), None);
}

#[tokio::test]
async fn busy_lock_is_a_retryable_error() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(409))
        .mount(&s)
        .await;
    let e = client(&s, None).add_exclusion("1").await.unwrap_err();
    assert_eq!(e, net::Error::Status { status: 409 });
}

#[tokio::test]
async fn exclusions_add_and_remove_only_global_ones() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/rules/exclusion"))
        .and(body_json(json!({"mediaServerId": "42", "action": 0})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"code": 1})))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/rules/exclusion"))
        .and(query_param("mediaServerId", "42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 7, "mediaServerId": "42", "ruleGroupId": null},
            {"id": 8, "mediaServerId": "42", "ruleGroupId": 3},
            {"id": 9, "mediaServerId": "99", "parent": 42, "ruleGroupId": null}
        ])))
        .mount(&s)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/rules/exclusion/7"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    let m = client(&s, None);
    m.add_exclusion("42").await.unwrap();
    assert_eq!(
        m.remove_global_exclusions("42").await.unwrap(),
        1,
        "admin's scoped exclusion and children untouched"
    );
}
