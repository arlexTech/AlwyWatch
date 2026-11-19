// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared HTTP plumbing for the core clients.
//!
//! * One `reqwest::Client` per process (connection pooling, HTTP/2), TLS through rustls with the
//!   `ring` provider and the OS trust store (rustls-platform-verifier, so user-installed CAs work).
//! * One error type, mapped from HTTP status codes the same way for every service.
//! * Tokens travel in headers only. Request URLs never contain secrets, so errors (which include
//!   the URL) are safe to log.

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

use std::sync::Once;
use std::time::Duration;

pub use reqwest;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
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
        Error::Network {
            message: e.to_string(),
        }
    }
}

pub fn status_error(status: StatusCode) -> Error {
    match status.as_u16() {
        401 | 403 => Error::Unauthorized,
        404 => Error::NotFound,
        s => Error::Status { status: s },
    }
}

static TLS_INIT: Once = Once::new();

/// Installs the ring crypto provider for rustls once per process.
pub fn init_tls() {
    TLS_INIT.call_once(|| {
        // Err means another provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Builds the shared client for API calls (short timeouts, gzip handled by servers' defaults).
pub fn api_client(user_agent: &str) -> Result<reqwest::Client> {
    init_tls();
    reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .map_err(Error::from)
}

/// Builds a client for large transfers: no read deadline beyond stalls, longer pool life.
pub fn transfer_client(user_agent: &str) -> Result<reqwest::Client> {
    init_tls();
    reqwest::Client::builder()
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
pub async fn send_json<T: DeserializeOwned>(req: RequestBuilder) -> Result<T> {
    let res = send(req).await?;
    let bytes = res.bytes().await?;
    serde_json::from_slice(&bytes).map_err(|e| Error::decode(e.to_string()))
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
        return Err(Error::invalid("empty server address"));
    }
    let with_scheme = if s.contains("://") {
        s.to_owned()
    } else {
        format!("https://{s}")
    };
    let url = reqwest::Url::parse(&with_scheme).map_err(|e| Error::invalid(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::invalid("server address must be an http(s) URL"));
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
    fn status_mapping() {
        assert_eq!(status_error(StatusCode::UNAUTHORIZED), Error::Unauthorized);
        assert_eq!(status_error(StatusCode::NOT_FOUND), Error::NotFound);
        assert!(status_error(StatusCode::BAD_GATEWAY).is_transient());
        assert!(!Error::Unauthorized.is_transient());
    }
}
