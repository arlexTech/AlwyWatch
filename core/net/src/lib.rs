// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared HTTP plumbing for the core clients.
//!
//! * One `reqwest::Client` per process (connection pooling, HTTP/2), TLS through rustls with the
//!   `ring` provider and the OS trust store (rustls-platform-verifier, so user-installed CAs work).
//! * One error type, mapped from HTTP status codes the same way for every service.
//! * Tokens travel in headers only. Request URLs never contain secrets, so errors (which include
//!   the URL) are safe to log.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Once;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

pub use reqwest;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("not signed in, or the session expired")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("server returned HTTP {status}")]
    Status { status: u16 },
    #[error("network error: {message}")]
    Network { message: String },
    #[error("unexpected response: {message}")]
    Decode { message: String },
    #[error("{message}")]
    Invalid { message: String },
    /// A login flow (PIN, QuickConnect) is still waiting for the user.
    #[error("waiting for authorization")]
    Pending,
}

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
        }
    }

    pub fn decode(message: impl Into<String>) -> Self {
        Self::Decode {
            message: message.into(),
        }
    }

    /// True for errors where retrying later can succeed (network loss, 5xx, 429).
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Network { .. } => true,
            Error::Status { status } => *status >= 500 || *status == 429 || *status == 408,
            _ => false,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        if let Some(status) = e.status() {
            return status_error(status);
        }
        if e.is_decode() {
            return Error::decode(e.without_url().to_string());
        }
        // "error sending request" alone says nothing: add the causes (DNS, TLS, refused…).
        let e = e.without_url();
        let mut message = e.to_string();
        let mut source = std::error::Error::source(&e);
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        Error::Network { message }
    }
}

pub fn status_error(status: StatusCode) -> Error {
    match status.as_u16() {
        401 | 403 => Error::Unauthorized,
        404 => Error::NotFound,
        s => Error::Status { status: s },
    }
}

#[cfg(not(target_arch = "wasm32"))]
static TLS_INIT: Once = Once::new();

/// Browser: TLS is the browser's (fetch); nothing to set up.
#[cfg(target_arch = "wasm32")]
pub fn init_tls() {}

/// Installs the ring crypto provider for rustls once per process.
#[cfg(not(target_arch = "wasm32"))]
pub fn init_tls() {
    TLS_INIT.call_once(|| {
        // Err means another provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Android: TLS against the system CA store read straight from disk, instead of the platform
/// verifier. That one validates through Java, whose revocation check falls back to downloading
/// the issuer's CRL: 11 MB for plex.tv's DigiCert CA, parsed into hundreds of MB of heap (the
/// app froze or ran out of memory on Plex sign-in). Android apps don't trust user-added CAs by
/// default, so the system store is what the platform verifier used anyway.
#[cfg(target_os = "android")]
fn tls(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    use std::sync::OnceLock;
    static CONFIG: OnceLock<Option<rustls::ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let roots = android_roots();
        if roots.is_empty() {
            return None;
        }
        let mut c = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        c.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Some(c)
    });
    match config {
        Some(c) => builder.tls_backend_preconfigured(c.clone()),
        // Nothing readable (should not happen): keep the platform verifier.
        None => builder,
    }
}

#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
fn tls(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    builder
}

/// Android's trusted CAs: the updatable Conscrypt store (Android 14+) or the system one. Files
/// are PEM followed by a text dump, which the PEM reader skips.
#[cfg(target_os = "android")]
fn android_roots() -> rustls::RootCertStore {
    use rustls::pki_types::{CertificateDer, pem::PemObject};
    let mut roots = rustls::RootCertStore::empty();
    for dir in [
        "/apex/com.android.conscrypt/cacerts",
        "/system/etc/security/cacerts",
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(certs) = CertificateDer::pem_file_iter(entry.path()) else {
                continue;
            };
            for cert in certs.flatten() {
                let _ = roots.add(cert);
            }
        }
        if !roots.is_empty() {
            break;
        }
    }
    roots
}

/// Browser: fetch() has no connect/read timeouts or pool settings to tune.
#[cfg(target_arch = "wasm32")]
pub fn api_client(_user_agent: &str) -> Result<reqwest::Client> {
    reqwest::Client::builder().build().map_err(Error::from)
}

#[cfg(target_arch = "wasm32")]
pub fn transfer_client(user_agent: &str) -> Result<reqwest::Client> {
    api_client(user_agent)
}

/// Builds the shared client for API calls (short timeouts, gzip handled by servers' defaults).
#[cfg(not(target_arch = "wasm32"))]
pub fn api_client(user_agent: &str) -> Result<reqwest::Client> {
    init_tls();
    tls(reqwest::Client::builder())
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .map_err(Error::from)
}

/// [`api_client`] for a server process: redirects are followed only within the same host, so
/// an API key in a header never goes to another host a service (or a wrong address) points to.
#[cfg(not(target_arch = "wasm32"))]
pub fn server_client(user_agent: &str) -> Result<reqwest::Client> {
    init_tls();
    let policy = reqwest::redirect::Policy::custom(|a| {
        let same = a
            .previous()
            .first()
            .map(|u| (u.host_str().map(str::to_owned), u.port_or_known_default()))
            == Some((
                a.url().host_str().map(str::to_owned),
                a.url().port_or_known_default(),
            ));
        if same && a.previous().len() < 5 {
            a.follow()
        } else {
            a.stop()
        }
    });
    tls(reqwest::Client::builder())
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .redirect(policy)
        .build()
        .map_err(Error::from)
}

/// Builds a client for large transfers: no read deadline beyond stalls, longer pool life.
#[cfg(not(target_arch = "wasm32"))]
pub fn transfer_client(user_agent: &str) -> Result<reqwest::Client> {
    init_tls();
    tls(reqwest::Client::builder())
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(Error::from)
}

/// Sends a request and maps non-success statuses to [`Error`].
pub async fn send(req: RequestBuilder) -> Result<Response> {
    let res = req.send().await?;
    if res.status().is_success() {
        Ok(res)
    } else {
        Err(status_error(res.status()))
    }
}

/// Sends a request and decodes a JSON body.
///
/// When the body is not JSON, the error says why when that can be told: a reverse proxy that
/// only lets the LAN in typically redirects everyone else to another site, and a wrong address
/// returns a web page.
pub async fn send_json<T: DeserializeOwned>(req: RequestBuilder) -> Result<T> {
    let (client, req) = req.build_split();
    let req = req?;
    let asked = req.url().host_str().map(str::to_owned);
    let res = client.execute(req).await?;
    if !res.status().is_success() {
        return Err(status_error(res.status()));
    }
    let landed = res.url().host_str().map(str::to_owned);
    let html = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let bytes = read_capped(res).await?;
    serde_json::from_slice(&bytes).map_err(|e| not_json(asked, landed, html, e))
}

/// The most an API answer may weigh: far above any real one, and a wrong or hostile address
/// can't fill the memory.
#[cfg(not(target_arch = "wasm32"))]
const MAX_JSON: usize = 64 << 20;

#[cfg(not(target_arch = "wasm32"))]
async fn read_capped(mut res: reqwest::Response) -> Result<Vec<u8>> {
    let too_big = || Error::invalid("the answer is too large");
    if res.content_length().is_some_and(|n| n > MAX_JSON as u64) {
        return Err(too_big());
    }
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        if out.len() + chunk.len() > MAX_JSON {
            return Err(too_big());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// In a browser, which holds the answer anyway.
#[cfg(target_arch = "wasm32")]
async fn read_capped(res: reqwest::Response) -> Result<Vec<u8>> {
    Ok(res.bytes().await?.to_vec())
}

fn not_json(
    asked: Option<String>,
    landed: Option<String>,
    html: bool,
    e: serde_json::Error,
) -> Error {
    match (asked, landed) {
        (Some(a), Some(l)) if a != l => Error::invalid(i18n::tr!(
            "{asked} redirected to {landed}: the server is probably only reachable from its own network (LAN or VPN)",
            asked = a,
            landed = l
        )),
        _ if html => Error::invalid(i18n::tr!(
            "got a web page instead of the server's API: check the address"
        )),
        _ => Error::decode(e.to_string()),
    }
}

/// Percent-encodes everything outside the RFC 3986 unreserved set.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Joins a base URL and a server-relative path without doubling or losing slashes.
pub fn join(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_owned()
    } else if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// Normalizes a user-typed server address: trims, adds https:// when no scheme, drops trailing `/`.
pub fn normalize_base_url(input: &str) -> Result<String> {
    let s = input.trim();
    if s.is_empty() {
        return Err(Error::invalid(i18n::tr!("empty server address")));
    }
    let with_scheme = if s.contains("://") {
        s.to_owned()
    } else {
        format!("https://{s}")
    };
    let url = reqwest::Url::parse(&with_scheme).map_err(|e| Error::invalid(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::invalid(i18n::tr!(
            "server address must be an http(s) URL"
        )));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_keeps_unreserved() {
        assert_eq!(
            percent_encode("Pixel 8 \"pro\"/é"),
            "Pixel%208%20%22pro%22%2F%C3%A9"
        );
        assert_eq!(percent_encode("a-b_c.d~"), "a-b_c.d~");
    }

    #[test]
    fn join_handles_slashes() {
        assert_eq!(join("https://a/", "/x"), "https://a/x");
        assert_eq!(join("https://a", "x"), "https://a/x");
        assert_eq!(join("https://a", "https://b/y"), "https://b/y");
    }

    #[test]
    fn normalize_adds_scheme_and_trims() {
        assert_eq!(
            normalize_base_url(" jf.example.org/ ").unwrap(),
            "https://jf.example.org"
        );
        assert_eq!(
            normalize_base_url("http://10.0.0.2:8096/jellyfin/").unwrap(),
            "http://10.0.0.2:8096/jellyfin"
        );
        assert!(normalize_base_url("ftp://x").is_err());
        assert!(normalize_base_url("").is_err());
    }

    #[test]
    fn not_json_explains_redirects_and_web_pages() {
        let e = || serde_json::from_slice::<u8>(b"<html>").unwrap_err();
        let host = |h: &str| Some(h.to_owned());
        let redirected = not_json(host("jf.example.org"), host("example.org"), true, e());
        assert!(redirected.to_string().contains("redirected to example.org"));
        let page = not_json(host("jf.example.org"), host("jf.example.org"), true, e());
        assert!(page.to_string().contains("web page"));
        let other = not_json(host("a"), host("a"), false, e());
        assert!(matches!(other, Error::Decode { .. }));
    }

    #[test]
    fn status_mapping() {
        assert_eq!(status_error(StatusCode::UNAUTHORIZED), Error::Unauthorized);
        assert_eq!(status_error(StatusCode::NOT_FOUND), Error::NotFound);
        assert!(status_error(StatusCode::BAD_GATEWAY).is_transient());
        assert!(!Error::Unauthorized.is_transient());
    }
}
