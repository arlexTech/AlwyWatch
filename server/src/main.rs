// SPDX-License-Identifier: AGPL-3.0-or-later
//! alwywatch-server: the keep service.
//!
//! Users see what Maintainerr will delete and ask to keep it (N days, until watched, forever).
//! Requests are stored per user, resolved (forever > until watched > longest extension) and
//! applied to Maintainerr (global exclusions, postponed deletion dates). Apps never write to
//! Maintainerr. Configuration: see `config.rs` and docs/deploy.md.

mod account_api;
mod accounts;
mod api;
mod auth;
mod config;
mod db;
mod keep;
mod limit;
mod oidc;
mod passkey;
mod seerr;
mod together;
mod transfer;
mod web;

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
    // Dry run: its simulated votes and exclusions never mix with the real ones.
    let db_file = if config.dry_run {
        eprintln!("[keep] DRY RUN: nothing is written to Maintainerr or Seerr");
        "keep-dryrun.sqlite"
    } else {
        "keep.sqlite"
    };
    let db_path = config.data_dir.join(db_file);
    let db = db::Db::open(&db_path).map_err(|e| {
        format!(
            "cannot open {}: {e}. The data directory must exist and be writable by the \
             container's user (uid 65534): chown -R 65534:65534 data",
            db_path.display()
        )
    })?;
    eprintln!("[keep] {}", config.summary());
    let http = net::api_client(concat!("AlwyWatch-Server/", env!("CARGO_PKG_VERSION")))
        .map_err(|e| e.to_string())?;
    let listen = config.listen;
    let state = Arc::new(keep::State::new(config, db, http, "https://plex.tv"));
    if state.config.keep_on() {
        match state.maintainerr.version().await {
            Ok(v) if !v.starts_with("3.") => {
                return Err(format!(
                    "Maintainerr {v} is not supported: this build works with Maintainerr 3.x and \
                     will not write to another major version. Use Maintainerr 3.x, or a newer \
                     alwywatch-server"
                ));
            }
            Ok(v) => eprintln!("[keep] Maintainerr {v}"),
            Err(e) => eprintln!("[keep] Maintainerr not reachable yet ({e}); will retry"),
        }
    }
    keep::spawn_jobs(state.clone());
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("[keep] listening on {listen}");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(stop_signal())
        .await
        .map_err(|e| e.to_string())
}

/// Ctrl-C, or the SIGTERM of `docker stop` (unhandled, the container was killed after its
/// 10 s grace period at every restart).
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests;
