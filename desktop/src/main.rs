// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch desktop (Linux, Windows): Slint UI over the Rust core, libmpv for playback.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod avatars;
mod headless;
mod instance;
mod lang;
mod localtime;
mod mpv;
mod platform;
mod player;
mod profiles;
mod video;

use std::time::Instant;

use alwywatch_core::{AlwyWatch, ClientInfo, CoreConfig};

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let t0 = Instant::now();
    let driven = std::env::var_os("ALWYWATCH_DRIVE").is_some();
    if let Some(out) = std::env::var_os("ALWYWATCH_SCREENSHOT").or(driven.then(Default::default)) {
        let after = std::env::var("ALWYWATCH_SCREENSHOT_AFTER_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000);
        headless::Headless::install(
            out.to_string_lossy().into_owned(),
            std::time::Duration::from_millis(after),
            driven,
        );
    } else {
        // Desktop OpenGL rather than Slint's default OpenGL ES: mpv draws the video into this
        // context, and NVIDIA's GLES drew 10-bit video (P010: every 10-bit file once the GPU
        // decodes it) as lines of noise. `ALWYWATCH_GLES` keeps GLES (comparison).
        if std::env::var_os("ALWYWATCH_GLES").is_none() {
            let _ = slint::BackendSelector::new()
                .backend_name("winit".into())
                .require_opengl()
                .select();
        }
    }
    let paths = platform::paths();
    // One AlwyWatch at a time (not for headless test runs): a second launch brings the running
    // one forward instead.
    let instance = if std::env::var_os("ALWYWATCH_SCREENSHOT").is_some() || driven {
        None
    } else {
        match instance::acquire(&paths.data) {
            Ok(i) => Some(i),
            Err(instance::AlreadyRunning) => {
                instance::ask_to_raise(&paths.data);
                return Ok(());
            }
        }
    };
    // Several people on one computer: each profile its own database and tokens (the original
    // profile's stay where they were); downloads are shared.
    // The language first: everything after may build text.
    let language = lang::resolve(&lang::choice(&paths.data));
    lang::apply(&language, false);
    let profiles = profiles::ProfileStore::new(&paths.data);
    let profile = profiles.current();
    let gate = !profiles.opens_directly();
    let data = profiles.data_dir(&profile);
    let _ = std::fs::create_dir_all(&data);
    let tokens = platform::Tokens::new(&data);
    let base_id = device_id(&paths.data);
    // Per profile: Jellyfin signs a device's previous user out at sign-in (same as Android).
    let device_id = if profile == profiles::DEFAULT {
        base_id
    } else {
        format!("{base_id}-{profile}")
    };
    let core = AlwyWatch::new(CoreConfig {
        data_dir: data.to_string_lossy().into_owned(),
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
        profile: Some(profile),
        downloads_db: Some(
            paths
                .data
                .join("downloads.sqlite")
                .to_string_lossy()
                .into_owned(),
        ),
    })?;
    app::activate_saved(&core, &tokens);
    // Test hook: print the skippable segments of an item as each account sees it, then exit
    // (checks intro/credits data end to end against the real servers without playing anything).
    if let Ok(item) = std::env::var("ALWYWATCH_PROBE_SEGMENTS") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        for a in core.accounts()? {
            let found = rt.block_on(core.segments(a.id.clone(), item.clone()));
            eprintln!("{} ({:?}): {:?}", a.server_name, a.kind, found);
        }
        return Ok(());
    }
    // Test hook: print what Seerr says about a show's seasons (the "More seasons" button).
    if let Ok(tmdb) = std::env::var("ALWYWATCH_PROBE_SEASONS") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        eprintln!("seerr active: {}", core.seerr_active());
        let found = rt.block_on(core.seerr_season_info(tmdb.parse().unwrap_or_default()));
        eprintln!("{found:?}");
        return Ok(());
    }
    // Test hook: print each account's "recently added" rows as the home builds them.
    if std::env::var_os("ALWYWATCH_PROBE_HOME").is_some() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        for a in core.accounts()? {
            match rt.block_on(core.refresh_home(a.id.clone(), true)) {
                Ok(h) => {
                    for rail in h.latest {
                        let titles: Vec<String> = rail
                            .items
                            .iter()
                            .take(12)
                            .map(|i| i.title.clone())
                            .collect();
                        eprintln!(
                            "{} · {}: {}",
                            a.server_name,
                            rail.library.title,
                            titles.join(" | ")
                        );
                        // Each show card: what is new, and what opening it shows.
                        for i in &rail.items {
                            if let Some(n) = rail.notes.iter().find(|n| n.item_id == i.id) {
                                let target = if n.open_id == n.item_id {
                                    format!(
                                        "show page{}",
                                        n.open_season_id
                                            .as_ref()
                                            .map(|_| ", on its season")
                                            .unwrap_or_default()
                                    )
                                } else {
                                    "episode page".to_owned()
                                };
                                eprintln!("    {} — {} → {target}", i.title, n.label);
                            }
                        }
                        // The library page's "Recently added" order, for comparison.
                        let mut q =
                            alwywatch_core::ItemQuery::library(rail.library.id.clone(), 0, 8);
                        q.sort_by = alwywatch_core::SortBy::DateAdded;
                        q.descending = true;
                        if let Ok(p) = rt.block_on(core.refresh_items(a.id.clone(), q, true)) {
                            let t: Vec<String> =
                                p.page.items.iter().map(|i| i.title.clone()).collect();
                            eprintln!("    library, recently added: {}", t.join(" | "));
                        }
                    }
                }
                Err(e) => eprintln!("{}: {e}", a.server_name),
            }
        }
        return Ok(());
    }
    // Test hook: ask each account's server to start converting an item for a TV-stick-like
    // device and print the HTTP status (checks the transcode request end to end without
    // playing). The request goes through curl's stdin config: no token in a command line.
    if let Ok(item) = std::env::var("ALWYWATCH_PROBE_TRANSCODE") {
        use std::io::Write;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let list = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let profile = alwywatch_core::DeviceProfile {
            max_bitrate: None,
            containers: list(&["mkv", "mp4", "ts", "mpegts"]),
            video_codecs: list(&["h264", "hevc"]),
            video_10bit_codecs: list(&["hevc"]),
            audio_codecs: list(&["aac", "ac3", "eac3", "mp3"]),
            subtitle_codecs: list(&["srt"]),
            max_width: Some(3840),
            max_height: Some(2160),
            hdr: vec![],
        };
        for a in core.accounts()? {
            let info = rt.block_on(core.playback_info(a.id.clone(), item.clone(), profile.clone()));
            let Some(t) = info
                .ok()
                .and_then(|i| i.sources.into_iter().next())
                .and_then(|s| s.transcode)
            else {
                eprintln!(
                    "{} ({:?}): no such item or no conversion offered",
                    a.server_name, a.kind
                );
                continue;
            };
            let mut config = format!("url = \"{}\"\n", t.url);
            for h in &t.headers {
                config.push_str(&format!("header = \"{}: {}\"\n", h.name, h.value));
            }
            let mut curl = std::process::Command::new("curl")
                .args([
                    "-s",
                    "-o",
                    "/dev/null",
                    "-w",
                    "%{http_code} %{content_type} %{size_download}B",
                    "-K",
                    "-",
                ])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()?;
            curl.stdin
                .take()
                .ok_or("curl stdin")?
                .write_all(config.as_bytes())?;
            let out = curl.wait_with_output()?;
            eprintln!(
                "{} ({:?}): {}",
                a.server_name,
                a.kind,
                String::from_utf8_lossy(&out.stdout)
            );
        }
        return Ok(());
    }
    let ui = AppWindow::new()?;
    lang::apply(&language, true);
    let app = app::App::new(core.clone(), &ui, tokens, profiles, gate);
    // Test hook (headless screenshots): start the Jellyfin sign-in with this address.
    match std::env::var("ALWYWATCH_TEST_PAGE").as_deref() {
        Ok("settings") => app.test_settings(),
        Ok("home") => app.test_home(),
        Ok("seerrtitle") => app.test_seerr_title(),
        Ok(step @ ("mylist" | "mylist-first")) => app.test_mylist(step == "mylist-first"),
        Ok("detail") => app.test_detail(),
        Ok(step @ ("room" | "room-guest" | "room-picker" | "player-room")) => app.test_room(step),
        Ok("invitation") => app.test_invitation(),
        Ok(step @ ("welcome" | "avatars" | "account" | "crop")) => app.test_account(step),
        Ok(step @ ("links" | "links-ask" | "links-unlink")) => app.test_links(step),
        Ok(step @ ("receive" | "send")) => app.test_transfer(step),
        Ok(tab @ ("recommended" | "browse" | "collections" | "categories")) => {
            app.test_home();
            app.test_library(tab);
        }
        _ => {}
    }
    if let Ok(file) = std::env::var("ALWYWATCH_TEST_PLAY") {
        app.test_play(file);
    }
    if let Ok(url) = std::env::var("ALWYWATCH_TEST_SEERR_URL") {
        app.test_seerr(url);
    }
    if let Ok(url) = std::env::var("ALWYWATCH_TEST_JELLYFIN_URL") {
        app.test_jellyfin(url);
    }
    // Video inside the window (and the startup measurement used by docs/perf.md).
    video::install(
        &ui,
        std::env::var_os("ALWYWATCH_TIMING").map(|_| {
            (
                t0,
                std::env::var_os("ALWYWATCH_EXIT_AFTER_FIRST_FRAME").is_some(),
            )
        }),
    );
    app.app_fullscreen_restore();
    let raise = slint::Timer::default();
    if let Some(instance) = instance {
        let w = ui.as_weak();
        raise.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(1),
            move || {
                if instance.raise_requested()
                    && let Some(ui) = w.upgrade()
                {
                    use slint::winit_030::WinitWindowAccessor;
                    ui.window().set_minimized(false);
                    ui.window().with_winit_window(|w| w.focus_window());
                }
            },
        );
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

pub(crate) fn hostname() -> String {
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
