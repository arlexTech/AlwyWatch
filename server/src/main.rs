// SPDX-License-Identifier: AGPL-3.0-or-later
//! alwywatch-server entry point.

use std::net::SocketAddr;

use axum::{Router, routing::get};

fn app() -> Router {
    Router::new().route("/healthz", get(|| async { "ok" }))
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr: SocketAddr = std::env::var("ALWYWATCH_LISTEN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 8080)));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_is_ok() {
        let res = app()
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }
}
