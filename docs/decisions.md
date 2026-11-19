# Decisions and dependencies

Every dependency has to pay for itself: priorities are **no perceived loading**, then **low RAM**,
then **small APK** (see CLAUDE.md). This file states why each one is here and what was rejected.

## Architecture decisions

| Decision | Why | Rejected |
|---|---|---|
| One Rust core, thin native UIs | Business logic (APIs, cache, downloads, keep) written and tested once | Kotlin Multiplatform (no desktop RAM win, no Rust server sharing) |
| Android: Compose + Media3/ExoPlayer | Reliable HW decoding, HDR, passthrough on TV boxes is non-negotiable | Rust UI + mpv on Android (codec/HDR/passthrough quirks per SoC) |
| Desktop: Slint + libmpv (pure Rust) | ~25 MB idle RSS, first frame in ~25 ms, one binary | Compose Desktop (JVM: 200+ MB, slow start), Tauri/Electron (web engine RAM) |
| mpv in its own window on desktop | Embedding needs per-platform GL interop; a separate window keeps HW decoding and passthrough exactly as mpv does them | Rendering mpv into a Slint texture (copies frames, breaks HDR passthrough) |
| Cache-first SWR everywhere | Every screen renders from SQLite immediately and refreshes in the background | Network-first with spinners |
| Direct play only, no server transcode | Owner's choice; quality and server load. Missing codecs are decoded on device (FFmpeg extension) | Transcode fallback by default (still possible: HLS module kept for the rare case) |
| Backend optional; every connection optional | The app is a general Plex/Jellyfin/Seerr client; the backend only adds "leaving soon"/keep and the Seerr bridge | Mandatory AlwyWatch accounts |
| Keep resolution: forever > until watched > longest extension | Several people can keep the same item; one person removing theirs never deletes it for others | Last write wins |
| Seerr bridge in the backend | Seerr serves one media server type; the bridge acts as each user via Seerr's own `X-API-User`, so Jellyfin users get requests without a second Seerr | Second Seerr instance (two queues, two Radarr/Sonarr configs) |
| "Watched" detection by polling | No webhook source exists today (no Jellyfin webhook plugin, Plex webhooks need Plex Pass); only "until watched" items are polled | Requiring plugins/Plex Pass |
| No downloads UI on TV | TVs stream; saves storage and UI | — |
| tv-material dropped | `androidx.tv:tv-material` duplicated Material 3 (+~1 MB dex) for a few components; focus handling is done with plain Compose (`FocusCard`, `dpadEscape`) | Using it for cards/rails |
| Release key: new dedicated key | Not reused from other apps; stored outside the repo (`~/.config/alwywatch`) | Debug key / Play App Signing only |
| License AGPL-3.0-or-later | Same as the other licensed Alwy* project | — |

## Rust crates

| Crate | Used by | Why | Size/notes |
|---|---|---|---|
| `tokio` | core, server | Async runtime required by reqwest/axum; features trimmed per crate | Core runs its own small runtime |
| `reqwest` (rustls, no default features) | net | HTTP/1.1+2, JSON, streaming bodies for downloads; mature | No OpenSSL; `rustls-no-provider` + `ring` |
| `rustls` (ring) | net | TLS without OpenSSL (cross-compiles to Android/Windows trivially) | `ring` is smaller than aws-lc-rs and needs no cmake |
| `rustls-platform-verifier` | core | OS trust store (user-installed CAs, Android's store via JNI) | Kotlin AAR counterpart on Android |
| `jni` | core (Android only) | One JNI entry point to initialise the platform verifier | Android target only |
| `serde`, `serde_json` | all | API models | — |
| `thiserror` | core crates | Error enums without boilerplate | Compile-time only |
| `uniffi` 0.32 | core | Kotlin bindings (Swift later) from proc-macros | Bindings generated from the host debug build (release strips metadata) |
| `rusqlite` (bundled) | store, downloads, server | Cache, settings, download offsets, keep votes. Bundled SQLite = same version everywhere | ~1 MB per ABI |
| `axum` (http1, json, query only) | server | Small, tokio-native HTTP server | — |
| `toml` (parse + serde only) | server | Config file with the server allow-list | — |
| `slint` 1.19 (winit, femtovg + software renderer) | desktop | Native, low-RAM UI with declarative markup; software renderer powers headless screenshots/benchmarks | fluent-dark style |
| `image` (jpeg/png/webp only) | desktop | Decode posters into Slint images, downscaled to the card size | AVIF/TIFF/etc. disabled |
| `libloading` | desktop | Load libmpv at runtime: the binary starts without mpv installed and says so instead of failing to launch | No link-time mpv dependency |
| `windows-sys` (Cryptography) | desktop (Windows) | DPAPI for token storage | Windows only. `keyring` rejected: pulls D-Bus/Secret Service on Linux; Linux uses a 0600 file in the user's data dir |
| `wiremock`, `tempfile`, `tower` | tests only | Mock HTTP servers, temp dirs, router `oneshot` in server tests | Dev-dependencies |

## Android libraries

| Library | Why | Notes |
|---|---|---|
| Compose BOM (ui, foundation, material3) | UI for phone, tablet and TV in one codebase | No `material-icons-extended` (huge); text buttons |
| `activity-compose` | Compose host, `ReportDrawn` for startup metrics | — |
| JNA (`@aar`) | Required by UniFFI's Kotlin bindings | ~0.5 MB per ABI of native stub |
| `kotlinx-coroutines-android` | Rust async functions become `suspend` functions | — |
| WorkManager | Downloads survive process death and resume when the network returns; keep reminders | Foreground `dataSync` service |
| Media3 ExoPlayer + UI (pinned 1.8.0) | Player: HW decoding, HDR, tunneling, passthrough | Pinned because the two extensions below are built against exactly 1.8.0 |
| Media3 HLS | Only for the rare server-transcode fallback | Small |
| `org.jellyfin.media3:media3-ffmpeg-decoder` | Software DTS/DTS-HD/TrueHD/E-AC3 when the device lacks a decoder, so audio never forces a transcode | **GPLv3** (compatible with AGPL); ~3 MB per ABI |
| `io.github.peerless2012:ass-media` | libass rendering of ASS/SSA subtitles (styles, positions) | Overlay mode (OpenGL) |
| `org.rustls:rustls-platform-verifier` | Kotlin half of the Rust TLS verifier | Version read from Cargo.lock at build time |
| `androidx.profileinstaller` | Installs the Baseline Profile on sideloaded installs | Tiny |
| Benchmark (macro), UIAutomator, androidx.test | `:baselineprofile` module only (profile generation, startup benchmark) | Never shipped |

Rejected on Android: **Coil/Glide** (own disk-backed loader in the Rust core + a ~100-line
Compose loader with BlurHash placeholders: no second HTTP stack and cache), **Hilt/Koin** (a lazy
`Services` object is enough), **Retrofit/OkHttp** (all HTTP is in Rust), **Room** (SQLite is in
Rust), **tv-material** (see above), **Navigation-Compose** (a 60-line back-stack navigator with
saved scroll state).

## Build tooling

| Tool | Why |
|---|---|
| `cargo-ndk` | Builds the core for arm64-v8a/armeabi-v7a/x86_64 with the pinned NDK |
| `cargo-xwin` | Cross-builds the Windows desktop binary from Linux |
| ktlint Gradle plugin | Kotlin style (CLAUDE.md) |
| `androidx.baselineprofile` plugin | Generates and packages the Baseline Profile |
| NSIS (Windows), appimagetool (Linux) | Installer formats the owner asked for (no Flatpak) |
