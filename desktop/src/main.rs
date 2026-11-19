// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch desktop (Linux, Windows): Slint UI over the Rust core, libmpv for playback.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod headless;
mod mpv;
mod platform;
mod player;

use std::time::Instant;

use alwywatch_core::{AlwyWatch, ClientInfo, CoreConfig};

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let t0 = Instant::now();
    if let Some(out) = std::env::var_os("ALWYWATCH_SCREENSHOT") {
        let after = std::env::var("ALWYWATCH_SCREENSHOT_AFTER_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000);
        headless::Headless::install(
            out.to_string_lossy().into_owned(),
            std::time::Duration::from_millis(after),
        );
    }
    let paths = platform::paths();
    let tokens = platform::Tokens::new(&paths.data);
    let device_id = device_id(&paths.data);
    let core = AlwyWatch::new(CoreConfig {
        data_dir: paths.data.to_string_lossy().into_owned(),
        cache_dir: paths.cache.to_string_lossy().into_owned(),
        client: ClientInfo {
            product: "AlwyWatch".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            device_name: hostname(),
            device_id,
            platform: if cfg!(windows) { "Windows" } else { "Linux" }.into(),
        },
        image_cache_mb: 300,
        downloads_dir: paths.downloads.to_string_lossy().into_owned(),
    })?;
    for a in core.accounts()? {
        if let Some(t) = tokens.get(&a.id) {
            core.activate_account(a.id.clone(), t)?;
        }
    }
    if core.seerr_connection().is_some()
        && let Some(cookie) = tokens.get("seerr")
    {
        let _ = core.seerr_activate(cookie);
    }
    let ui = AppWindow::new()?;
    let app = app::App::new(core.clone(), &ui, tokens);
    // Test hook (headless screenshots): start the Jellyfin sign-in with this address.
    if let Ok(url) = std::env::var("ALWYWATCH_TEST_SEERR_URL") {
        app.test_seerr(url);
    }
    if let Ok(url) = std::env::var("ALWYWATCH_TEST_JELLYFIN_URL") {
        app.test_jellyfin(url);
    }
    if std::env::var_os("ALWYWATCH_TIMING").is_some() {
        // Startup measurement used by docs/perf.md: time to the first rendered frame.
        let weak = ui.as_weak();
        let _ = ui.window().set_rendering_notifier(move |state, _| {
            if let slint::RenderingState::AfterRendering = state {
                eprintln!("first frame after {} ms", t0.elapsed().as_millis());
                if std::env::var_os("ALWYWATCH_EXIT_AFTER_FIRST_FRAME").is_some()
                    && let Some(w) = weak.upgrade()
                {
                    let _ = w.hide();
                }
            }
        });
    }
    ui.run()?;
    Ok(())
}

fn device_id(data: &std::path::Path) -> String {
    let file = data.join("device-id");
    if let Ok(id) = std::fs::read_to_string(&file)
        && !id.trim().is_empty()
    {
        return id.trim().to_owned();
    }
    // Not security sensitive: identifies this install to servers (sessions list, Plex devices).
    let seed = format!(
        "{:?}{}{}",
        std::time::SystemTime::now(),
        std::process::id(),
        hostname()
    );
    let id = format!(
        "{:016x}{:016x}",
        store_hash(&seed),
        store_hash(&seed.chars().rev().collect::<String>())
    );
    let _ = std::fs::create_dir_all(data);
    let _ = std::fs::write(&file, &id);
    id
}

fn store_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Desktop".into())
}
