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
mod invites;
mod jellyfin_admin;
mod keep;
mod limit;
mod mail;
mod oidc;
mod passkey;
mod seerr;
mod server_settings;
mod settings;
mod together;
mod transfer;
mod web;

use std::sync::Arc;

fn main() -> std::process::ExitCode {
    let (listen, data_dir) = match config::Config::env_basics() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Each generation runs on its own runtime: saving the settings in the app ends it (every
    // task with it) and starts the next one with the new settings, like a restart.
    loop {
        let stored = match config::Config::load_stored(&data_dir) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("settings error: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let config = match config::Config::from_stored(&stored, listen, data_dir.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("settings error: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let outcome = rt.block_on(run(config, stored));
        rt.shutdown_timeout(std::time::Duration::from_secs(3));
        match outcome {
            Ok(Ending::Stop) => return std::process::ExitCode::SUCCESS,
            Ok(Ending::Reload) => eprintln!("[settings] saved: starting again with them"),
            Err(e) => {
                eprintln!("fatal: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
}

enum Ending {
    Stop,
    Reload,
}

async fn run(mut config: config::Config, stored: settings::Stored) -> Result<Ending, String> {
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
    // Never write to a Maintainerr this build doesn't know: Leaving soon stays off and the
    // admin page says why (the server keeps running so it can be fixed from the app).
    let mut problem = None;
    if config.keep_on() {
        let m = maintainerr::Maintainerr::new(
            http.clone(),
            &config.maintainerr.url,
            config.secrets.maintainerr_api_key.clone(),
        );
        match m.version().await {
            Ok(v) if !v.starts_with("3.") => {
                let p = format!(
                    "Maintainerr {v} is not supported: this version of AlwyWatch works with \
                     Maintainerr 3.x and will not write to another major version"
                );
                eprintln!("[keep] {p}");
                problem = Some(p);
                config.maintainerr = Default::default();
            }
            Ok(v) => eprintln!("[keep] Maintainerr {v}"),
            Err(e) => eprintln!("[keep] Maintainerr not reachable yet ({e}); will retry"),
        }
    }
    let setup_code = config.servers.is_empty().then(|| {
        let code = format!(
            "{:06}",
            u32::from_str_radix(&accounts::random_hex(4), 16).unwrap_or(0) % 1_000_000
        );
        eprintln!(
            "[setup] No media server yet. Open this site and enter the setup code {code} \
             (a new code at every start until setup is done)"
        );
        code
    });
    let mut state = keep::State::new(config, db, http, "https://plex.tv");
    state.setup_code = setup_code;
    *state.stored.get_mut() = stored;
    *state
        .maintainerr_problem
        .get_mut()
        .unwrap_or_else(|e| e.into_inner()) = problem;
    let state = Arc::new(state);
    let reload = state.reload.clone();
    keep::spawn_jobs(state.clone());
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("[keep] listening on {listen}");
    let reloading = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = reloading.clone();
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = stop_signal() => {}
                _ = reload.notified() => flag.store(true, std::sync::atomic::Ordering::Relaxed),
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(if reloading.load(std::sync::atomic::Ordering::Relaxed) {
        Ending::Reload
    } else {
        Ending::Stop
    })
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
