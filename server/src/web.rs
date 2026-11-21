// SPDX-License-Identifier: AGPL-3.0-or-later
//! Serves the web app (`web/dist`, built by `web/build.sh`) at `/`. Files go out as they are,
//! or their `.gz` copy when the browser accepts gzip; any other path that is not the API gets
//! `index.html`, so app links (`/item/…`, `/play/…`) open directly. Revalidated with an ETag.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

fn root() -> PathBuf {
    std::env::var_os("ALWYWATCH_WEB")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/web".into())
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
    serve_from(&root(), &uri, &headers).await
}

async fn serve_from(dir: &Path, uri: &Uri, headers: &HeaderMap) -> Response {
    let path = uri.path();
    if path == "/v1" || path.starts_with("/v1/") {
        return not_found();
    }
    let rel = path.trim_start_matches('/');
    // No way out of the folder, no hidden files.
    if rel
        .split('/')
        .any(|s| s == ".." || s.starts_with('.') || s.contains('\\'))
    {
        return not_found();
    }
    let asset = Path::new(rel).extension().is_some();
    let file = if asset {
        dir.join(rel)
    } else {
        dir.join("index.html")
    };
    let gzip = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    let gz = PathBuf::from(format!("{}.gz", file.display()));
    let (sent, encoded) = if gzip && gz.is_file() {
        (gz, true)
    } else {
        (file.clone(), false)
    };
    let Ok(meta) = std::fs::metadata(&sent) else {
        return not_found();
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let etag = format!(
        "\"{:x}-{:x}{}\"",
        meta.len(),
        mtime,
        if encoded { "-gz" } else { "" }
    );
    let mut out = HeaderMap::new();
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime(&file)));
    // Fixed file names: always revalidate (a 304 costs one round trip, a stale app costs more).
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    out.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    out.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    out.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    out.insert("x-frame-options", HeaderValue::from_static("DENY"));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        out.insert(header::ETAG, v);
    }
    if encoded {
        out.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == etag)
    {
        return (StatusCode::NOT_MODIFIED, out).into_response();
    }
    match tokio::fs::read(&sent).await {
        Ok(bytes) => (StatusCode::OK, out, bytes).into_response(),
        Err(_) => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(dir: &Path, path: &str, gzip: bool) -> (StatusCode, Option<String>, Vec<u8>) {
        let mut h = HeaderMap::new();
        if gzip {
            h.insert(
                header::ACCEPT_ENCODING,
                HeaderValue::from_static("gzip, br"),
            );
        }
        let r = serve_from(dir, &path.parse().unwrap(), &h).await;
        let status = r.status();
        let enc = r
            .headers()
            .get(header::CONTENT_ENCODING)
            .map(|v| v.to_str().unwrap().to_owned());
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, enc, body.to_vec())
    }

    #[tokio::test]
    async fn serves_files_app_links_and_gzip_but_not_outside_or_the_api() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("index.html"), "<app>").unwrap();
        std::fs::write(d.path().join("style.css"), "css").unwrap();
        std::fs::write(d.path().join("style.css.gz"), "gz").unwrap();
        let p = d.path();
        assert_eq!(get(p, "/", false).await.2, b"<app>");
        assert_eq!(
            get(p, "/item/jf.a.b/123", false).await.2,
            b"<app>",
            "app link"
        );
        assert_eq!(get(p, "/style.css", false).await.2, b"css");
        assert_eq!(
            get(p, "/style.css", true).await,
            (StatusCode::OK, Some("gzip".into()), b"gz".to_vec())
        );
        assert_eq!(get(p, "/missing.js", false).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            get(p, "/../etc/passwd", false).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(get(p, "/.env", false).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            get(p, "/v1/nothing", false).await.0,
            StatusCode::NOT_FOUND,
            "API stays JSON 404"
        );
    }
}
