// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Matroska layout check against a server answering range requests.

use media_api::mkv::tracks_after_clusters;
use media_api::model::{Header, HttpRequest};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn el(id: &[u8], data: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.push(0x01);
    v.extend_from_slice(&(data.len() as u64).to_be_bytes()[1..]);
    v.extend_from_slice(data);
    v
}

/// A file like the release that never started: 200 KB of fonts, a cluster, then the tracks.
fn file(tracks_first: bool) -> Vec<u8> {
    let fonts = el(&[0x19, 0x41, 0xA4, 0x69], &vec![0; 200_000]);
    let tracks = el(&[0x16, 0x54, 0xAE, 0x6B], &[0; 40]);
    let cluster = el(&[0x1F, 0x43, 0xB6, 0x75], &[0; 1000]);
    let body = if tracks_first {
        [tracks, fonts, cluster].concat()
    } else {
        [fonts, cluster, tracks].concat()
    };
    let mut f = el(&[0x1A, 0x45, 0xDF, 0xA3], &[0x42, 0x86, 0x81, 0x01]);
    f.extend(el(&[0x18, 0x53, 0x80, 0x67], &body));
    f
}

struct Ranges(Vec<u8>);

impl Respond for Ranges {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let range = r
            .headers
            .get("range")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let (a, b) = range
            .trim_start_matches("bytes=")
            .split_once('-')
            .map(|(a, b)| (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()))
            .unwrap();
        let end = (b + 1).min(self.0.len());
        ResponseTemplate::new(206).set_body_bytes(self.0[a.min(end)..end].to_vec())
    }
}

#[tokio::test]
async fn a_file_with_its_tracks_at_the_end_is_found_in_two_small_reads() {
    let s = MockServer::start().await;
    for (name, tracks_first) in [("bad.mkv", false), ("good.mkv", true)] {
        Mock::given(method("GET"))
            .and(path(format!("/{name}")))
            .and(header("X-Token", "t"))
            .respond_with(Ranges(file(tracks_first)))
            .mount(&s)
            .await;
    }
    let http = net::api_client("AlwyWatch-test").unwrap();
    let req = |name: &str| HttpRequest {
        url: format!("{}/{name}", s.uri()),
        headers: vec![Header {
            name: "X-Token".into(),
            value: "t".into(),
        }],
    };
    assert_eq!(
        tracks_after_clusters(&http, &req("bad.mkv")).await,
        Some(true)
    );
    assert_eq!(
        tracks_after_clusters(&http, &req("good.mkv")).await,
        Some(false)
    );
    // Two reads for the bad one (the head, then past the fonts), one for the good one.
    assert_eq!(s.received_requests().await.unwrap().len(), 3);
}
