# AlwyWatch

Lightweight, fast, open source media client for **Jellyfin and Plex**, with **Seerr** (requests) and **Maintainerr** (deletion visibility + "keep" requests) built in. Part of the "Alwy*" family of projects.

## Goals (in priority order)
1. **No perceived loading time, ever** (top priority), then **low RAM**, then **small APK**, in that order when they conflict. Every dependency must justify itself.
   - Cache-first, stale-while-revalidate: every screen renders instantly from the local SQLite cache, then refreshes in the background. Never show a blocking spinner for data we already have.
   - Persist and restore last UI state and scroll positions; ship Baseline Profiles and keep app startup free of network and heavy init (lazy-init everything).
   - Prefetch aggressively but cheaply: next page of lists, detail of focused/hovered items, artwork (use BlurHash/ThumbHash placeholders), next episode playback info and ExoPlayer preload.
   - Measure it: add startup and screen-transition benchmarks (Macrobenchmark) and track them in CI.
2. **Great on Android TV** (D-pad first, 10-foot UI) and Android phones/tablets.
3. One shared Rust core, thin native UIs.
4. Offline downloads that **resume after network loss**.

## Architecture
- **Rust core** (`/core`, cargo workspace) exposed to native UIs through **UniFFI** (Kotlin now, Swift later). All business logic lives here, UIs stay dumb.
  - `media-api`: `MediaServer` trait with `JellyfinClient` and `PlexClient` implementations. Common models: Library, Item, Season, Episode, PlaybackInfo, Stream/Subtitle tracks.
  - `seerr`: client for Seerr (Overseerr/Jellyseerr API): search, request, request status.
  - `maintainerr`: client for the Maintainerr API: collections, items scheduled for deletion, deletion dates.
  - `keep-service`: client for the AlwyWatch backend (see below).
  - `downloads`: resumable download manager (HTTP `Range`, `If-Range`/ETag, `.part` file, persisted offsets, retry with backoff).
  - `store`: local cache/settings in SQLite (`rusqlite`), image cache.
  - Async runtime: `tokio`; HTTP: `reqwest` with `rustls` (no OpenSSL); serde for models.
- **Android app** (`/android`): Kotlin + **Jetpack Compose** (TV: `androidx.tv` Material; phone: Material 3), one codebase with adaptive layouts. Player: **Media3/ExoPlayer** (hardware decoding, HDR, audio passthrough). Downloads run in a WorkManager/foreground service that calls the Rust download manager.
- **Desktop app** (`/desktop`, Linux + Windows): pure Rust, **Slint** UI calling the core directly (no UniFFI), player **libmpv** (hardware decoding, passthrough). Chosen over Compose Desktop for lower RAM and faster startup.
- Reliability of hardware decoding and audio passthrough on Android/TV is non-negotiable, hence Media3/ExoPlayer there rather than a shared Rust UI.
- **iOS**: out of scope for v1. Keep the core platform-agnostic so SwiftUI (AVPlayer) can be added later.
- **Backend `alwywatch-server`** (`/server`, Rust, axum, SQLite, single small Docker image): needed for the "keep" feature because Maintainerr has no per-user votes. It is **self-hosted on my own server** (reachable with `ssh serv`).

## Auth and users
- Users sign in with their existing **Jellyfin or Plex** account on the servers the app is configured for. Since 2026-11-22 (owner's request) there are also **AlwyWatch accounts** on the backend: username + password, passkeys or Pocket ID, invitation-only sign-up; an account alone only holds profile settings and the user's server list, and Quick Connect (code + QR) signs devices in (docs/keep-protocol.md "AlwyWatch accounts").
- The backend only serves the **servers I have selected** (those already running on my server). It validates the user's Jellyfin/Plex token against the matching server and identifies the user from that; it never stores media-server passwords or tokens (AlwyWatch account passwords are stored as Argon2id hashes).
- Server list is a backend config (allow-list), and the app can fetch it so users don't type URLs.

## My server
- `ssh serv` gives access to the host where Jellyfin, Plex, Seerr, Maintainerr (and the future backend) run. You may use it **read-only to inspect** the setup (containers, versions, ports, API availability, reverse proxy) to design against the real environment. Ask before changing anything, deploying, restarting services or touching config.

## Environment reality (inspected 2026-11-19, details in `docs/plan.md` §0)
- Caddy is the TLS edge. Plex 1.43.x (`plex.example.org`, public), Seerr 3.5.x (`seerr.example.org`, public, **Plex mode**, media-server login only), Jellyfin **12.4** (`jellyfin.example.org`, **public since 2026-11-20**, QuickConnect enabled, no plugins), Maintainerr **3.32** (LAN/VPN only, **configured for Plex**; it supports Jellyfin too but only one media server at a time), Tautulli (LAN/VPN only).
- Maintainerr item IDs are Plex ratingKeys plus tmdb/tvdb IDs, so keep requests are mapped by provider IDs for Jellyfin users.
- No playback webhook source exists today (no Jellyfin webhook plugin, Plex webhooks need Plex Pass), so "watched" detection = polling Tautulli / Jellyfin for the small set of "until watched" items, with webhooks as optional accelerators.
- Seerr supports **one** media server type at a time (verified in code); the owner plans to move from Plex to Jellyfin (open source), at which point Seerr and Maintainerr switch to Jellyfin mode. Design everything server-agnostic.
- Every connection (Plex, Jellyfin, Seerr, AlwyWatch backend) is optional and can be combined freely; the backend's server list is only a convenience preset.
- All Maintainerr collections are inactive on purpose and all their items are already past their nominal deletion date: never activate a collection or reset dates without the owner.
- Repo is hosted on GitHub (`arlexTech/alwywatch`, public). CI: `.github/workflows/ci.yml` on GitHub's runners, `.forgejo/workflows/ci.yml` on the owner's runner; both call `scripts/ci.sh`.
- Do not commit server identifiers (machine IDs, usernames, internal subnets) to this public repo.

## "Keep" feature (core differentiator)
Users see what is scheduled for deletion (from Maintainerr) and can request:
- **Extend** by N days,
- **Keep until I have watched it** (backend polls Tautulli/Jellyfin watched state for these items; Tautulli/Plex/Jellyfin webhooks accepted as accelerators),
- **Keep forever** ("ad vitam", it's a banger).
The backend stores requests per user, resolves conflicts (forever > until-watched > extend; take the longest), and applies the result to Maintainerr (exclusions / collection changes) through its API. The client never talks to Maintainerr write endpoints directly. Verify the actual Maintainerr API and Jellyfin/Plex support before designing endpoints; do not assume.

## License
**AGPL-3.0-or-later**, same as `arlexTech/alwychat` (the only other Alwy* repo with a license; alwyfarm, alwyhouse and alwymail have none). Add `LICENSE`, `SPDX-License-Identifier: AGPL-3.0-or-later` headers and a README badge.

## Design principles
- Native-feeling, minimal, content-first. Dark theme default. Large poster grids, rails, skeleton loading, no heavy animations.
- TV: every screen fully usable with D-pad, visible focus states, no touch-only interactions, remote-friendly search.
- Prefer direct play; transcoding only as fallback. Downloads always fetch the original file (transcodes cannot be resumed).
- Offline-first caches for library metadata; images cached on disk with size cap.
- Accessibility: subtitles, audio track selection, font scaling.

## v1 scope
Jellyfin + Plex connection and auth, libraries, item details, direct-play playback with resume position sync, Seerr search/requests, Maintainerr "leaving soon" view + keep requests, resumable downloads on phone. Platforms: Android phone/tablet, Android TV, Linux, Windows.
Out of scope for v1: iOS, live TV, music, Chromecast. (Device profiles with an optional PIN were added on the owner's request, 2026-11-20.)

## Engineering rules
- Small, reviewable commits. Keep the workspace building and tests passing at each step.
- Unit-test the core (mock HTTP with `wiremock`); the download manager needs tests for interruption and resume, ETag change, and disk-full.
- `cargo clippy -- -D warnings` and `cargo fmt` must pass. Kotlin: ktlint.
- Secrets/tokens go in Android Keystore-backed storage, never in logs.
- Do not add a dependency without stating why; check size impact for the APK (R8 + ABI splits enabled for release).
- Document the public core API and the keep-service protocol in `/docs`.

## First milestones
1. Cargo workspace + UniFFI hello-world callable from a Compose app (CI builds Android `.so` for arm64/armv7/x86_64 with `cargo-ndk`).
2. `MediaServer` trait + Jellyfin client (login, libraries, items, playback info) + minimal Compose list/detail/player with Media3.
3. Plex client behind the same trait.
4. Android TV UI pass (focus, rails, leanback launcher).
5. Resumable downloads.
6. Seerr integration.
7. `alwywatch-server` + Maintainerr integration + keep UI.

Before starting, ask me about anything ambiguous instead of guessing, in particular the Maintainerr API surface and the auth flows.
