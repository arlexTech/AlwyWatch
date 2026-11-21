# AlwyWatch

[![License: AGPL v3+](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)

A lightweight, open source media client for **Jellyfin** and **Plex**, with **Seerr** requests
and a **"keep"** system on top of **Maintainerr**: see what is about to be deleted from the
server and ask to keep it for N days, until you have watched it, or forever.

Platforms: Android phones and tablets, Android TV, Linux and Windows desktop, and a web app
served by the optional AlwyWatch server. Every connection is optional: without the server it is
a plain Jellyfin/Plex client.

> Status: 0.1.0, not yet released. See [the plan](docs/plan.md).

## Features

| | Android phone/tablet | Android TV | Desktop (Linux, Windows) | Web |
|---|---|---|---|---|
| Jellyfin (password, Quick Connect) and Plex (PIN) accounts, several at once | yes | yes | yes | yes |
| Plex Home user switch | yes | yes | no | no |
| Home rails, libraries (genres, collections), title pages, search | yes | yes | yes | yes |
| Renders from a local cache first | SQLite | SQLite | SQLite | `localStorage` copies of a few pages |
| Player | Media3/ExoPlayer: HW decoding, HDR, passthrough, FFmpeg audio, libass | same | libmpv (own window) | browser `<video>`, HLS fallback |
| Direct play, server conversion as fallback | yes | yes | always direct | yes |
| Audio and subtitle choice, remembered per title | yes | yes | yes | text subtitles only, no audio choice |
| Skip intro/credits, next episode | yes | yes | yes | yes |
| Resumable downloads | yes | no | yes | no |
| My list (Jellyfin favorites, Plex watchlist), trailers, cast | yes | yes | yes | My list and cast |
| Seerr search and requests (direct or through the server) | yes | yes | yes | through the server only |
| Leaving soon, keep votes, swipe sorting, reminders | yes | yes | yes | yes, no reminders |
| Admin: rights, countdown, kept-forever stats | yes | yes | yes | yes |
| Admin: link users to existing Seerr users | yes | yes | no | yes |
| Watch together | yes | yes | yes | yes |
| Device profiles with optional PIN | yes | yes | yes | no |
| Copy watch history between servers | yes | yes | yes | no |
| Keep two servers' watch state in sync | yes | yes | no | no |
| Settings follow the user across devices | yes | yes | yes | no |
| Move sign-ins to a new device (end-to-end encrypted) | yes | yes | yes | no |
| Voice search | yes | yes | no | no |

TV is D-pad first (leanback launcher, focus restored where it was). Phones use a bottom bar,
tablets and TV a side rail, desktop a sidebar. Web limitations are listed in
[docs/web.md](docs/web.md#limitations-true-in-the-code-today).

## Layout

| Path | What |
|---|---|
| `core/` | Rust core shared by every app: media servers, Seerr, keep service, downloads, cache |
| `android/` | Android phone/tablet/TV app (Kotlin, Jetpack Compose, Media3), core through UniFFI |
| `desktop/` | Linux and Windows app (Rust, Slint, libmpv) |
| `web/` | Web app (Rust to wasm, Leptos), served by the server |
| `server/` | `alwywatch-server`: keep votes, Seerr bridge, watch together, device transfer, web app |
| `tools/uniffi-bindgen` | Kotlin bindings generator used by Gradle |
| `scripts/`, `packaging/` | CI, dev loop and packaging scripts |
| `docs/` | Plan, build, decisions, APIs, performance |

## Build

Details and requirements: [docs/build.md](docs/build.md).

```sh
scripts/ci.sh rust                      # fmt, clippy -D warnings, tests (whole workspace)
cd android && ./gradlew assembleDebug   # Android APKs (builds the Rust core with cargo-ndk)
cargo run -p alwywatch-desktop          # desktop app
desktop/packaging/install.sh            # per-user install after `cargo build --release -p alwywatch-desktop`
scripts/package.sh linux|windows        # AppImage / Windows installer in dist/
web/build.sh                            # web app into web/dist (wasm32 target + wasm-bindgen-cli)
docker build -f server/Dockerfile -t alwywatch-server .   # server image, web app included
```

## Docs

* [Building](docs/build.md) and packaging
* [Deploying the server](docs/deploy.md)
* [Core API](docs/core-api.md), [keep-service protocol](docs/keep-protocol.md), [web app](docs/web.md)
* [Decisions and dependencies](docs/decisions.md)
* [Performance](docs/perf.md)

## License

[GNU AGPL v3 or later](LICENSE).
