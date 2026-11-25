// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Matroska layout check against a server answering range requests.

use media_api::mkv::{Fix, fix};
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
/// With `room`, a seek head points at the tracks and padding before the fonts can take them.
fn file(tracks_first: bool, room: bool) -> Vec<u8> {
    let fonts = el(&[0x19, 0x41, 0xA4, 0x69], &vec![0; 200_000]);
    let tracks = el(&[0x16, 0x54, 0xAE, 0x6B], &[5; 40]);
    let cluster = el(&[0x1F, 0x43, 0xB6, 0x75], &[0; 1000]);
    let body = if tracks_first {
        [tracks, fonts, cluster].concat()
    } else if room {
        // Void of 60 bytes (8-byte size), then the seek head's target: after the cluster.
        let pad = [
            vec![0xEC, 0x01],
            (60u64 - 9).to_be_bytes()[1..].to_vec(),
            vec![0; 51],
        ]
        .concat();
        let seek = |at: u64| {
            let entry = [
                el(&[0x53, 0xAB], &[0x16, 0x54, 0xAE, 0x6B]),
                el(&[0x53, 0xAC], &at.to_be_bytes()),
            ]
            .concat();
            el(&[0x11, 0x4D, 0x9B, 0x74], &el(&[0x4D, 0xBB], &entry))
        };
        let head_len = seek(0).len() as u64;
        let at = head_len + pad.len() as u64 + fonts.len() as u64 + cluster.len() as u64;
        [seek(at), pad, fonts, cluster, tracks].concat()
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
async fn a_file_with_its_tracks_at_the_end_is_patched_or_converted() {
    let s = MockServer::start().await;
    for (name, tracks_first, room) in [
        ("bad.mkv", false, false),
        ("room.mkv", false, true),
        ("good.mkv", true, false),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{name}")))
            .and(header("X-Token", "t"))
            .respond_with(Ranges(file(tracks_first, room)))
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
    assert_eq!(fix(&http, &req("good.mkv")).await, Some(Fix::AsIs));
    // No seek head to find the tracks: the conversion.
    assert_eq!(fix(&http, &req("bad.mkv")).await, Some(Fix::Convert));
    // The tracks fit in the padding: moved there, the old place blanked.
    let Some(Fix::Patch(patches)) = fix(&http, &req("room.mkv")).await else {
        panic!("expected a patch")
    };
    let mut f = file(false, true);
    for p in &patches {
        let o = p.offset as usize;
        f[o..o + p.bytes.len()].copy_from_slice(&p.bytes);
    }
    assert_eq!(f.len(), file(false, true).len());
    let tracks = el(&[0x16, 0x54, 0xAE, 0x6B], &[5; 40]);
    let at = f
        .windows(tracks.len())
        .position(|w| w == tracks.as_slice())
        .unwrap();
    assert!(at < 200, "tracks moved to the front: {at}");
    assert_eq!(
        media_api::mkv::walk(&f, 0, 0),
        media_api::mkv::Layout::TracksFirst
    );
}
