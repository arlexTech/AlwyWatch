# AlwyWatch

[![License: AGPL v3+](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)

A lightweight, fast, open source media client for **Jellyfin** and **Plex**, with **Seerr** requests and a **"keep"** system on top of **Maintainerr**: see what is about to be deleted from the server and ask to keep it for a while, until you have watched it, or forever.

Runs on Android phones and tablets, Android TV, Linux and Windows.

> Status: early development. See [the plan](docs/plan.md).

## Goals

1. No perceived loading time: every screen renders from a local cache first and refreshes in the background.
2. Great on Android TV (D-pad first) and on phones and tablets.
3. Low RAM, small download.
4. Offline downloads that resume after network loss.

## Layout

| Path | What |
|---|---|
| `core/` | Rust core shared by every app (media servers, Seerr, downloads, cache) |
| `android/` | Android phone/tablet/TV app (Kotlin, Jetpack Compose, Media3) using the core through UniFFI |
| `desktop/` | Linux and Windows app (Rust, Slint, libmpv) |
| `server/` | `alwywatch-server`, the optional self-hosted keep service |
| `docs/` | Plan, build instructions, decisions, performance results |

Building: see [`docs/build.md`](docs/build.md) (coming with the first release). Quick checks: `scripts/ci.sh`.

## License

[GNU AGPL v3 or later](LICENSE).
