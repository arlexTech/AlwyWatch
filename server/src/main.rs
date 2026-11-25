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
mod dashboard;
mod db;
mod invites;
mod jellyfin_admin;
mod keep;
mod limit;
mod mail;
mod oidc;
mod passkey;
mod remote;
mod seerr;
mod server_settings;
mod settings;
mod together;
mod transfer;
mod web;

use std::future::IntoFuture;
use std::sync::Arc;

fn main() -> std::process::ExitCode {
    let (listen, data_dir) = match config::Config::env_basics() {
        Ok(b) => b,
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
    match rt.block_on(serve(listen, data_dir)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fatal: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// One generation: the server with one set of settings, and its background jobs.
struct Generation {
    state: keep::Shared,
    jobs: Vec<tokio::task::JoinHandle<()>>,
}

/// Serves until stopped. Saving the settings in the app makes a new generation and switches
/// to it without closing the site: requests already running finish on the old one.
async fn serve(listen: std::net::SocketAddr, data_dir: std::path::PathBuf) -> Result<(), String> {
    use tower::ServiceExt;
    let lasting = keep::Lasting::default();
    let mut current = start(listen, &data_dir, &lasting).await?;
    let router = Arc::new(std::sync::RwLock::new(api::router(current.state.clone())));
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("[keep] listening on {listen}");
    let r = router.clone();
    let app = axum::Router::new().fallback_service(tower::service_fn(
        move |req: axum::extract::Request| {
            let now = r.read().unwrap_or_else(|e| e.into_inner()).clone();
            now.oneshot(req)
        },
    ));
    let ending = Arc::new(tokio::sync::Notify::new());
    let ended = ending.clone();
    let server = tokio::spawn(
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                stop_signal().await;
                ended.notify_one();
            })
            .into_future(),
    );
    let reload = lasting.reload.clone();
    loop {
        tokio::select! {
            _ = reload.notified() => {}
            _ = ending.notified() => break,
        }
        match start(listen, &data_dir, &lasting).await {
            Ok(next) => {
                *router.write().unwrap_or_else(|e| e.into_inner()) =
                    api::router(next.state.clone());
                let old = std::mem::replace(&mut current, next);
                old.state.stop_jobs(old.jobs).await;
                eprintln!("[settings] saved: now running with them");
            }
            Err(e) => eprintln!("[settings] not applied, still running as before: {e}"),
        }
    }
    // Requests still open (long polls, slow clients) get 5 s, then are cut.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    current.state.stop_jobs(current.jobs).await;
    Ok(())
}

async fn start(
    listen: std::net::SocketAddr,
    data_dir: &std::path::Path,
    lasting: &keep::Lasting,
) -> Result<Generation, String> {
    let stored = config::Config::load_stored(data_dir)?;
    let mut config = config::Config::from_stored(&stored, listen, data_dir.to_owned())?;
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
    let http = net::server_client(concat!("AlwyWatch-Server/", env!("CARGO_PKG_VERSION")))
        .map_err(|e| e.to_string())?;
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
        let code = server_settings::new_setup_code();
        eprintln!(
            "[setup] No media server yet. Open this site and enter the setup code {code} \
             (a new code at every start until setup is done)"
        );
        code
    });
    *lasting.stored.lock().await = stored;
    let mut state = keep::State::new(config, db, http, "https://plex.tv").with_lasting(lasting);
    state.setup_code = setup_code;
    *state
        .maintainerr_problem
        .get_mut()
        .unwrap_or_else(|e| e.into_inner()) = problem;
    let state = Arc::new(state);
    let jobs = keep::spawn_jobs(state.clone());
    Ok(Generation { state, jobs })
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
