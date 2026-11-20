# AlwyWatch

[![License: AGPL v3+](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)

A lightweight, fast, open source media client for **Jellyfin** and **Plex**, with **Seerr** requests and a **"keep"** system on top of **Maintainerr**: see what is about to be deleted from the server and ask to keep it for a while, until you have watched it, or forever.

Runs on Android phones and tablets, Android TV, Linux and Windows.

> Status: 0.1.0, first complete version, not yet released. See [the plan](docs/plan.md).

## Features

* Jellyfin (password or Quick Connect) and Plex (plex.tv PIN) accounts, several at once.
* Home rails, libraries, details, search; everything renders instantly from a local cache.
* Direct play with hardware decoding, HDR and audio passthrough (Media3 on Android, mpv on
  desktop); missing audio codecs are decoded on the device instead of transcoded on the server.
  Subtitles (including styled ASS) and audio track selection; resume position sync.
* Downloads on phone, tablet and desktop that resume after network loss.
* Seerr search and requests, for Plex and Jellyfin users alike (directly or through the
  AlwyWatch server).
* "Leaving soon" from Maintainerr, sorted by urgency: keep N days, until I watched it (with
  reminders), forever, or "I don't care"; swipe mode; admin rights and countdown.
* A modern, artwork-led design: heroes that follow what you point at, ambient colors from the
  artwork, logo art, landscape "continue watching" cards, custom player controls.
* Android TV: D-pad first (focus stays put, rows pin under the hero), leanback launcher.
  Phones: bottom bar; tablets and foldables: side rail. Desktop: sidebar.

Every connection is optional: the app works as a plain Jellyfin/Plex client.

## Goals

1. No perceived loading time: every screen renders from a local cache first and refreshes in the background.
2. Great on Android TV (D-pad first) and on phones and tablets.
3. Low RAM, small download.
4. Offline downloads that resume after network loss.

## Layout

| Path | What |
|---|---|
| `core/` | Rust core shared by every app (media servers, Seerr, keep service, downloads, cache) |
| `android/` | Android phone/tablet/TV app (Kotlin, Jetpack Compose, Media3) using the core through UniFFI |
| `desktop/` | Linux and Windows app (Rust, Slint, libmpv) |
| `server/` | `alwywatch-server`, the optional self-hosted keep service and Seerr bridge |
| `packaging/`, `scripts/` | Desktop packages, CI script |
| `docs/` | Plan, build instructions, decisions, performance results |

## Docs

* [Building](docs/build.md) and packaging (APK, AppImage, Windows installer)
* [Deploying the server](docs/deploy.md)
* [Core API](docs/core-api.md) and [keep-service protocol](docs/keep-protocol.md)
* [Decisions and dependencies](docs/decisions.md)
* [Performance](docs/perf.md)

Quick checks: `scripts/ci.sh`.

## License

[GNU AGPL v3 or later](LICENSE).
