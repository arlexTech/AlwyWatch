// SPDX-License-Identifier: AGPL-3.0-or-later
//! alwywatch-server: the keep service.
//!
//! Users see what Maintainerr will delete and ask to keep it (N days, until watched, forever).
//! Requests are stored per user, resolved (forever > until watched > longest extension) and
//! applied to Maintainerr (global exclusions, postponed deletion dates). Apps never write to
//! Maintainerr. Configuration: see `config.rs` and docs/deploy.md.

mod api;
mod auth;
mod config;
mod db;
mod keep;
mod seerr;

use std::sync::Arc;

fn main() -> std::process::ExitCode {
    let config = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    match rt.block_on(run(config)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fatal: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(config: config::Config) -> Result<(), String> {
    let db = db::Db::open(&config.data_dir.join("keep.sqlite")).map_err(|e| e.to_string())?;
    let http = net::api_client(concat!("AlwyWatch-Server/", env!("CARGO_PKG_VERSION")))
        .map_err(|e| e.to_string())?;
    let listen = config.listen;
    let state = Arc::new(keep::State::new(config, db, http, "https://plex.tv"));
    match state.maintainerr.version().await {
        Ok(v) if !v.starts_with("3.") => {
            return Err(format!(
                "Maintainerr {v} is not supported (built against 3.x); refusing to write to it"
            ));
        }
        Ok(v) => eprintln!("[keep] Maintainerr {v}"),
        Err(e) => eprintln!("[keep] Maintainerr not reachable yet ({e}); will retry"),
    }
    keep::spawn_jobs(state.clone());
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("[keep] listening on {listen}");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
