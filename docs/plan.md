# AlwyWatch v1 implementation plan

Status: **draft, waiting for approval** (2026-11-19).
Source of truth for goals: `CLAUDE.md`. This document records what the real environment looks like, the design decisions that follow from it, and the order of work.

---

## 0. Environment findings (read-only inspection of `serv`, 2026-11-19)

Host: Debian 13, everything in Docker, **Caddy 2.11.9** as the TLS edge (ports 80/443), each app on its own Docker network that Caddy joins.

| Service | Version | Public hostname | Exposure | Notes |
|---|---|---|---|---|
| Plex Media Server | 1.43.5.11079 (linuxserver image) | `plex.example.org` | **Public** | Caddy → `https://plex:32400` (self-signed backend). Remote access via custom URL `https://plex.example.org:443` (Plex relay/publish off). Hardware transcoding enabled. Plex Home server. |
| Jellyfin | **12.4.0** (`jellyfin/jellyfin:latest`) | `jellyfin.example.org` | **LAN/VPN only** (`localonly`) | Marked "Plex alternative under test". Same media, mounted read-only. **QuickConnect enabled.** No plugins installed (so no webhook plugin). Every API endpoint we need exists in its OpenAPI (AuthenticateByName, QuickConnect, UserViews, Items, Shows/Seasons/Episodes, NextUp, Resume, PlaybackInfo, Sessions/Playing*, UserPlayedItems, Items/{id}/Download, Videos/{id}/stream). |
| Seerr | 3.5.2 (`ghcr.io/seerr-team/seerr`) | `seerr.example.org`, `vod.example.org` | Public | **Plex mode** (`mediaServerType=1`), sign-in with media server only (local login off). Jellyfin users cannot sign in to it as configured. |
| Maintainerr | **3.32.0** | `maintainerr.example.org` | LAN/VPN only | **Configured for Plex** (`GET /api/media-server/type → plex`). Maintainerr 3.x supports Plex, Jellyfin and Emby, **but only one media server at a time** (it has a `media-server/switch` flow). API answers without auth from inside the Docker network (`vod_default`, shared with Seerr). |
| Tautulli | latest | `tautulli.example.org` | LAN/VPN only | Plex watch history; can be the "watched" signal for Plex without Plex Pass. |

Maintainerr state today: 2 rule collections, **both inactive on purpose** (the owner doesn't want to delete anything people still want): "Deletion" (movies, delete after 30 days, 776 items) and "Series out" (shows, delete after 30 days, 259 items). **All 1035 items are already past their nominal deletion date** (`addDate + 30 d` falls between 2026-05-06 and 2026-05-26). If a collection were activated as-is, its next run would delete everything in it. The backend therefore needs a "start countdown" admin action before activation (see §6.1).

Follow-up checks (2026-11-19):
- **Jellyfin public exposure readiness**: `EnableRemoteAccess=true`, `KnownProxies=caddy` (real client IPs are seen), HTTPS terminated by Caddy, legacy auth disabled. The only thing keeping it LAN-only is `import localonly` in its Caddy site block. Before opening it: check every user's login-lockout policy and consider a Caddy rate limit on `/Users/AuthenticateByName`.
- **Seerr is single media server**, verified in 3.5.2's code. `settings.main.mediaServerType` is one value (PLEX=1, JELLYFIN=2, EMBY=3), and `/auth/jellyfin` and the QuickConnect routes are rejected while it is in Plex mode. It cannot serve Plex and Jellyfin users at the same time. Jellyfin-mode Seerr signs users in with Jellyfin username/password or **Jellyfin QuickConnect** (`/auth/jellyfin/quickconnect/*`). Local accounts (`/auth/local`) exist but are disabled here.
- **Forgejo 15** (`git.example.org`) has **no Actions runner** registered and no `[actions]` section in `app.ini`.

### Maintainerr API surface we will use (verified in the 3.32.0 server code)

Read:
- `GET /api/collections`: collections with `id, title, isActive, type, deleteAfterDays, libraryId, mediaServerType, mediaCount…`
- `GET /api/collections/media/:id/content/:page`: paged items of a collection (`mediaServerId` = Plex ratingKey, `tmdbId`, `tvdbId`, `addDate`, `image_path`)
- `GET /api/rules/exclusion`: current exclusions
- `GET /api/media-server/meta/:id/maintainerr-status`: per-item status
- Deletion date = `addDate + deleteAfterDays` (same helper `getCollectionDeleteDate` as the UI)

Write (backend only, never the client):
- **Extend**: `POST /api/collections/media/postpone` `{collectionId, mediaId, days}` with `days` in 1..3650. The server sets `addDate = max(addDate, now - deleteAfterDays) + days` and returns the new `deletionDate`. It is **relative and not idempotent**, so the backend stores the target date and only posts the delta.
- **Forever / until watched**: `POST /api/rules/exclusion` `{mediaServerId, collectionId?, ruleGroupId?, action: 0}` (ADD); removal with `action: 1` or `DELETE /api/rules/exclusion/:id`. Exclusion is safer than rolling postpones: if the backend is down, the item stays protected.
- Writes take Maintainerr's execution lock and return **409** while a rule run is in progress → retry with backoff.

---

## 1. Decisions that follow from the environment

1. **Keep feature is keyed on provider IDs, not on one server's IDs.** Maintainerr knows Plex `ratingKey` + `tmdbId`/`tvdbId`. Plex users map directly. Jellyfin users map through `ProviderIds.Tmdb/Tvdb`. The backend never assumes which server Maintainerr is configured for (it reads `/api/media-server/type`), so switching Maintainerr to Jellyfin later only changes the ID mapping.
2. **"Watched" detection = poll + optional webhooks**, because no webhook source exists today (no Jellyfin webhook plugin; Plex native webhooks need Plex Pass):
   - Plex: Tautulli API `get_history` (per user, per rating key) polled for items under an "until watched" keep only (small set), plus an optional Tautulli webhook notifier (`/v1/hooks/tautulli`) for immediacy.
   - Jellyfin: admin API key, `GET /Items?userId=…&ids=…&Filters=IsPlayed` on the same small set, plus optional `/v1/hooks/jellyfin` if the webhook plugin is installed later.
   - Plex native webhook endpoint `/v1/hooks/plex` also accepted (only works if the owner has Plex Pass).
3. **Backend runs on the same host and talks to services over internal Docker networks** (`vod_default` for Maintainerr/Seerr, `plex_default`, `jellyfin_default`, `tautulli_default`). It needs a public hostname for remote Plex users (to be decided, see questions).
4. **Auth**
   - Plex: plex.tv **PIN flow** (`POST https://plex.tv/api/v2/pins`, user enters the code at plex.tv/link: ideal on TV; on phone, open the auth URL in a Custom Tab). Then `GET /api/v2/resources` to find the allow-listed server (by machine identifier) and its per-user `accessToken` (shared users get a server-specific token).
   - Jellyfin: **QuickConnect** (primary on TV, already enabled) and username/password (`/Users/AuthenticateByName`).
   - Backend validation: the client sends `Authorization: Bearer <server token>` + `X-AlwyWatch-Server: <id>`. Backend calls `GET /Users/Me` (Jellyfin) or `GET https://plex.tv/api/v2/user` + resource check (Plex), and caches the result for 10 min keyed by **SHA-256 of the token** (raw tokens are never stored or logged).
5. **Seerr**: the app reads `GET /api/v1/settings/public` and offers whichever sign-in the instance supports: Plex token (`/auth/plex`), Jellyfin QuickConnect / password (`/auth/jellyfin*`) or local account (`/auth/local`). The session cookie is kept by the core in secure storage. Requests are attributed to the real user. With the planned Plex → Jellyfin migration, the owner's Seerr switches to Jellyfin mode and Jellyfin users sign in with QuickConnect. Until then, only Plex accounts can sign in to it directly.
   **Seerr bridge (added 2026-11-19, on the owner's request that Jellyfin users get every feature):** when an AlwyWatch backend is connected, the app can use Seerr *through* it. The backend validates the user's own Jellyfin/Plex token, links them to their Seerr user (same Plex id / Jellyfin user id) or creates a local Seerr user once, and forwards an allow-listed subset of the Seerr API with Seerr's API key and `X-API-User` (Seerr's own "act as user" header, verified in 3.5.2's `middleware/auth.js`). Seerr keeps enforcing that user's permissions, quotas and auto-approve. One Seerr, one request queue, no second instance; after the switch to Jellyfin mode the same bridge serves Plex users.
8. **Every connection is optional.** The app is a general Plex/Jellyfin/Seerr client: users add any number of Plex servers, Jellyfin servers, Seerr instances and AlwyWatch backends, in any combination. The backend's server list is only a shortcut (presets); manual URLs always work. "Leaving soon" and keep features appear only when a backend is connected, and the backend identity comes from one of the user's connected Plex/Jellyfin accounts on an allow-listed server.
9. **No server transcoding unless nothing else can play it.** APK size is secondary to that: bundle the Media3 FFmpeg audio decoder (DTS, DTS-HD, TrueHD, E-AC3/AC3 for devices without decoders), libass rendering for ASS/SSA subtitles, and a software AV1 decoder (dav1d) for devices without hardware AV1. Server transcode remains the last resort for video codecs the device cannot decode at all (VC-1, MPEG-2 on some TV SoCs).
6. **TLS** everywhere through `rustls` with the `ring` provider (no OpenSSL, no aws-lc cross-compile pain), and **`rustls-platform-verifier`** so Android/Windows/Linux system trust stores (and user-installed CAs for self-hosters) work.
7. **Plex playback**: direct play via `/library/parts/{id}/…` with the token; resume sync via `/:/timeline` and `/:/scrobble`. Plex's 2025 rule that remote streaming of personal media requires Plex Pass/Remote Watch Pass may apply to remote users. I'll verify with a non-owner account during milestone 3 and document the result.

---

## 2. Repository layout

```
/core                    cargo workspace members (lib crates)
  media-api/             MediaServer trait, JellyfinClient, PlexClient, common models
  seerr/                 Seerr client
  maintainerr/           Maintainerr read client (used by server) + shared types
  keep-service/          client for alwywatch-server + protocol types (shared with server)
  downloads/             resumable download manager
  store/                 SQLite cache/settings (rusqlite, bundled), disk image cache
  alwywatch-core/        facade: app state, SWR repositories, prefetch, UniFFI exports (proc-macro)
/server                  alwywatch-server (axum, SQLite, Dockerfile, compose example)
/desktop                 Slint UI + libmpv player (calls alwywatch-core directly)
/android                 Gradle project: :app (phone+tablet+TV, one APK), :core-bindings,
                         :baselineprofile, :macrobenchmark
/docs                    plan, build, decisions, perf, core-api, keep-protocol
/.forgejo/workflows      ci.yml (fmt, clippy, tests, server image, android, linux, windows cross-build)
                         repo: git.example.org (Forgejo); needs a runner, see open questions
```

One Cargo workspace at the repo root with members `core/*`, `server`, `desktop`. `uniffi` is used **proc-macro only** (no UDL), and `uniffi-bindgen` runs as a workspace binary (library mode) from Gradle.

---

## 3. Core design

### 3.1 `MediaServer` trait (async, `Send + Sync`)
`authenticate(...)`, `quick_connect_*` / `plex_pin_*`, `libraries()`, `items(library, query, page)`, `item(id)`, `seasons(series)`, `episodes(season)`, `resume()`, `next_up()`, `latest(library)`, `search(q)`, `playback_info(item, device_profile)` (returns direct-play URL + streams; transcode URL only as fallback), `report_start/progress/stop(...)`, `set_played(item, bool)`, `download_source(item) -> {url, size, etag?, filename}` (always the original file), `image_url(item, kind, width)`.
Common models: `Library, Item{kind, ids{server, tmdb, tvdb, imdb}, …}, Season, Episode, PlaybackInfo, MediaStream{video|audio|subtitle}, ImageRef{url, blurhash?}`.

### 3.2 Cache-first / stale-while-revalidate
- Every read API returns the cached value **synchronously** (`Option<T>`, from SQLite, sub-millisecond) and the core starts the refresh in the background. UIs subscribe through a callback/Flow (`Repository.observe(key)`), which emits cached → fresh. No blocking spinner if a cache entry exists; skeletons only on first-ever load.
- Tables: `servers, accounts, libraries, items (json blob + indexed columns), children, home_rails, playback_state, ui_state (scroll positions, last screen), etag/refresh timestamps, pending_progress (offline queue), downloads, http_cache`.
- Token storage: the core never persists tokens. Android stores them in Keystore-backed `EncryptedFile`/AES-GCM key in AndroidKeyStore and hands them to the core at start. Desktop uses the OS keyring via the `keyring` crate (Secret Service / Windows Credential Manager).
- Prefetch: next page when 70% scrolled, detail + playback info for focused item after 300 ms dwell, next-episode playback info at 80% of an episode.

### 3.3 Image cache
- Core: disk cache (`store`), size cap (default 300 MB, LRU by atime in SQLite), fetches resized images from the server (Jellyfin `maxWidth`, Plex `/photo/:/transcode`).
- Placeholders: Jellyfin `ImageBlurHashes` (decoded in core, tiny); Plex has none → a 16 px thumbnail fetched with the list and cached as a placeholder.
- Android: **Coil 3 `coil-compose-core` only** (memory cache, sizing, lifecycle) with a custom `Fetcher` that reads from the core's disk cache. No OkHttp/Ktor network module.
- Desktop: `image` crate (jpeg/png/webp features only) → Slint `SharedPixelBuffer`, decoded off the UI thread, in-memory LRU.

### 3.4 Downloads (`downloads` crate)
- `.part` file + SQLite row `{id, url, etag, last_modified, total, offset, state, attempts}`; offset is fsync'd periodically (every 4 MiB) and on pause.
- Resume: `Range: bytes=offset-` + `If-Range: <etag or last-modified>`. A `200` instead of `206` means the file changed → truncate and restart (ETag-change test). Validates `Content-Range`.
- Retry with exponential backoff + jitter (1 s → 5 min cap), resumes on network regain (platform tells the core).
- Disk full: `ENOSPC`/`ERROR_DISK_FULL` → state `PausedNoSpace`, keep the `.part`, surface it to the UI. Pre-check free space vs `Content-Length`.
- Rename `.part` → final atomically, then mark completed. Original file only (Jellyfin `/Items/{id}/Download`, Plex part URL with `download=1`).
- Android: WorkManager `CoroutineWorker` promoted to foreground service (`dataSync` type) calling the core. Desktop: in-process tokio task. **No downloads UI on TV.**
- Tests (wiremock + temp dirs): interruption mid-body then resume, ETag change → restart, server without Range support, disk full (simulated via a writer trait with an injected `ENOSPC`, plus a real small tmpfs/loop test on Linux CI if available), retry/backoff timing with paused tokio clock.

### 3.5 Offline playback & progress
Downloaded files play locally; progress reports go to `pending_progress` and are flushed when the server is reachable (last-write-wins by timestamp).

---

## 4. Android (phone, tablet, TV: one APK)

- Kotlin, Compose BOM, Material 3 (phone/tablet), `androidx.tv:tv-material` (TV). Adaptive via `WindowSizeClass` + `PackageManager.FEATURE_LEANBACK` to select TV navigation shell. Leanback launcher intent + banner. `minSdk 26`, `targetSdk 36`.
- Screens: server picker (from backend allow-list), sign-in (QuickConnect / Plex PIN / password), home (rails: continue watching, next up, latest per library, leaving soon), library grid, detail (movie/series/season), player, search (media + Seerr), requests, leaving soon + keep sheet, downloads (phone/tablet only), settings.
- TV: every screen D-pad navigable, visible focus (scale + outline), focus restoration per screen, remote-friendly search (on-screen keyboard + voice input intent), no touch-only gestures.
- Player: **Media3 ExoPlayer**, `DefaultRenderersFactory` with MediaCodec hardware decoders first, decoder fallback on. Tunneling optional (setting, default off: it breaks on several TV SoCs). **Audio passthrough**: `AudioCapabilities` from the HDMI receiver; AC3/E-AC3/E-AC3-JOC/DTS/DTS-HD/TrueHD offloaded to the sink when supported; otherwise decode (see Q6) or request server audio-only transcode. **HDR**: HDR10/HLG/Dolby Vision through MediaCodec; DV profile 7/8 fallback to HDR10 base layer when the decoder lacks DV; display mode switching (refresh rate match via `Surface.setFrameRate`, resolution via `Display.Mode` on TV) as a setting.
- Track selection: audio/subtitle pickers from `PlaybackInfo` + ExoPlayer tracks; external subtitles (SRT/VTT/ASS) side-loaded; PGS/DVB supported natively. Font scaling for subtitles follows system captioning settings.
- Device profile sent to the server is built from `MediaCodecList` + audio capabilities, so the server only falls back to transcoding when really needed.
- Release: R8 full mode, resource shrinking, **ABI splits** (arm64-v8a, armeabi-v7a, x86_64) + universal. Baseline Profile module + Macrobenchmark module.

## 5. Desktop (Linux + Windows)

- Slint (femtovg/OpenGL renderer by default; software renderer fallback) with the same screens, full keyboard navigation (arrows/Enter/Esc/Backspace, focus rings) and mouse.
- Player: **libmpv** through a thin hand-written FFI (around 20 functions, loaded dynamically with `libloading` so the app starts and browses even if libmpv is missing). Settings: `hwdec=auto-safe` (VAAPI/NVDEC on Linux, D3D11VA on Windows), `vo=gpu-next`, `audio-spdif=ac3,eac3,dts,dts-hd,truehd` (user setting, off by default since it needs a receiver), `demuxer-max-bytes=64MiB` to bound RAM.
- Player window: see Q9 (recommended: mpv renders in its own native fullscreen window, Slint stays for browsing; best HDR/hwdec reliability and least code).
- Packaging: **Windows `.exe` installer** built with NSIS (`makensis` runs on Linux, so it is cross-built in CI with `cargo-xwin`), bundling `libmpv-2.dll`. Linux: no Flatpak; tarball with `.desktop` file + system libmpv, plus a format still to be chosen (see open questions).

## 6. `alwywatch-server`

- Rust, axum, rusqlite (bundled), tokio, reqwest+rustls. Single static binary, `FROM scratch`/distroless image (~10 MB).
- Config (TOML + env for secrets): allow-listed servers `{id, kind, name, public_url, internal_url, plex_machine_id?}`, Maintainerr internal URL (+ API key if enabled), Tautulli URL + API key, Jellyfin admin API key (for watched polling and ID mapping), webhook shared secret, policy limits.
- API (`/v1`, documented in `docs/keep-protocol.md`):
  - `GET /v1/servers`: public allow-list (name, kind, public URL) so users never type URLs.
  - `GET /v1/me`
  - `GET /v1/leaving-soon?cursor=`: Maintainerr items (with deletion dates) of **active** collections only (see Q2), mapped into the caller's server IDs, with the caller's keep and the effective state.
  - `PUT /v1/keeps/{item}` `{kind: extend{days} | until_watched | forever}`, `DELETE /v1/keeps/{item}`, `GET /v1/keeps`
  - `POST /v1/hooks/{tautulli|plex|jellyfin}` (shared secret in the URL path or header)
- Resolution per item: `forever` (any) > `until_watched` (any user who hasn't watched yet) > `extend` (latest target date across users). Expired/fulfilled keeps are retired automatically. Keeps are **per user**: removing mine never removes someone else's, and the item stays protected while at least one active keep exists.

### 6.1 Leaving-soon UX, admin, countdown
- **Leaving soon** is sorted by urgency (earliest deletion date first). The list includes inactive collections, labelled "not active yet", because that is the review phase the owner wants. For each item the user picks:
  - **I don't care**: recorded as a vote. Admins see it; it never deletes anything by itself.
  - **Keep N days**.
  - **Keep until I watch it**, with a reminder every X days/weeks. Reminders are local notifications scheduled by the app (WorkManager on Android, shown on launch on desktop). No Google push or FCM.
  - **Keep forever**, with an optional reason: "I love it" or "classic".
- **Swipe mode** ("triage"): a card stack of the most urgent items the user hasn't voted on yet.
  - Phone: swipe left = don't care, right = keep options, up = forever.
  - TV: D-pad left/right/up plus buttons.
  - Desktop: arrow keys.
- **Admin section** (visible to Jellyfin administrators and the Plex server owner):
  - Per-user rights: may keep forever yes/no, max active forever keeps, max extend days. **Default: unrestricted.**
  - Per-item vote overview (keeps / don't care / not voted).
  - **"Start countdown"** for a collection: resets every item's `addDate` to today via Maintainerr's postpone without `days`, optionally staggered. Items get a real deletion date before the owner activates the collection. Without this, activation deletes all 1035 overdue items at the next run.
  - The backend never activates collections itself.
- Apply loop (reconciler, every 5 min + on change): desired state vs last-applied state (stored) → minimal Maintainerr calls (exclusion add/remove, postpone delta), idempotent, retries on 409/5xx, audit log table. The client never touches Maintainerr.
- Deliverable: `server/deploy/docker-compose.yml` snippet + `docs/deploy.md` (networks to join, Caddy site block, keys to create). **Nothing is deployed or changed on `serv`.**

## 7. Performance targets (priority order) and how they're measured

| Metric | Target | How |
|---|---|---|
| Android cold start to first frame with cached home | < 500 ms mid-range phone, < 800 ms Android TV stick | Macrobenchmark `StartupTimingMetric` (cold), Baseline Profile on/off |
| Screen transition (home → detail, cached) | < 100 ms, no frame > 32 ms | Macrobenchmark `FrameTimingMetric` |
| Android PSS idle / playback | < 120 MB / < 250 MB | `dumpsys meminfo` script |
| APK per ABI (arm64) | < 12 MB (Rust `.so` < 5 MB: `opt-level="z"` for non-hot crates, LTO, `panic=abort`, strip) | CI size report |
| Desktop startup to first frame | < 300 ms | timestamp log, hyperfine |
| Desktop RSS idle / 4K playback | < 80 MB / < 300 MB | `/proc/self/status`, Windows `GetProcessMemoryInfo` |

Startup does no network and no heavy init: the core is lazy (`OnceLock` runtime, SQLite opened on first use from a background thread before first frame is requested, UI renders cached `ui_state` immediately).

## 8. Implementation order (after approval)

Each step = several small commits, workspace green, tests passing, clippy `-D warnings`, fmt, ktlint.

1. **Skeleton**: git init, workspace, crates, UniFFI proc-macro hello-world, Gradle task (`cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64` + `uniffi-bindgen generate --library`), Compose hello screen calling Rust, Slint hello window, server hello (`/healthz`), CI (fmt/clippy/test, Android assemble + `.so` per ABI, Linux, Windows MSVC, server Docker build), `LICENSE` (AGPL-3.0-or-later), SPDX headers, README badge.
2. **Core**: models + trait, Jellyfin client (password + QuickConnect, libraries, items, seasons/episodes, resume/next-up, playback info, progress/played), Plex client (PIN, resources, same surface), store + SWR repositories, image cache, wiremock tests for both clients.
3. **Android app** (phone/tablet/TV shells, all screens without Seerr/keep), Media3 player with device profile, passthrough, HDR, track selection, subtitles.
4. **Desktop app**: same screens, libmpv player.
5. **Downloads**: core + tests, Android worker/foreground service + phone UI, desktop in-process + UI.
6. **Seerr**: client + search/request/status screens on all platforms.
7. **alwywatch-server** + Maintainerr/Tautulli/Jellyfin integration + leaving-soon/keep screens in both apps, compose snippet + deploy notes.
8. **Performance pass**: Baseline Profiles, Macrobenchmark, measurements, R8/LTO, `docs/perf.md`, fixes.
9. **Docs**: `docs/build.md`, `docs/decisions.md` (every dependency justified, with size impact), `docs/core-api.md`, `docs/keep-protocol.md`, README.

## 9. Planned dependencies (each justified again in `docs/decisions.md`)

Rust core: `tokio` (rt-multi-thread, net, time, fs, sync), `reqwest` (rustls, json, stream; no default features), `rustls` (ring), `rustls-platform-verifier`, `serde`/`serde_json`, `quick-xml` + serde (Plex XML for a few endpoints; JSON elsewhere), `rusqlite` (bundled), `uniffi`, `thiserror`, `tracing` (with a redacting layer), `blurhash` (tiny), `sha2` (server token hashing). Tests: `wiremock`, `tempfile`.
Server: `axum`, `tower-http` (trace, timeout only), `toml`.
Desktop: `slint`, `libloading` (dynamic libmpv), `image` (jpeg/png/webp), `keyring`.
Android: Compose BOM, `androidx.tv:tv-material`, Media3 (exoplayer, ui-compose or a thin custom UI), WorkManager, Coil 3 core, `androidx.security`-free Keystore code (hand-written, ~80 lines), profileinstaller, benchmark/baselineprofile (test-only). Optional: Media3 FFmpeg audio extension (Q6).

## 10. Risks / honest limits

- **"Verified" HDR/DV/passthrough needs real hardware** (TV SoC + AVR). Emulators don't do passthrough or DV. I can implement and unit-test capability negotiation, but final verification needs your devices (Q7).
- Jellyfin 12.x is new; I'll code against the OpenAPI pulled from your server (12.4.0) rather than older docs.
- Maintainerr postpone is relative and its API is internal (not versioned/stable). The backend reads the version (`/api/settings/version`) and refuses to write on an unknown major.
- Plex remote-streaming entitlement: not an issue in this setup. Users connect to the direct address `plex.example.org` and non-Pass users already stream remotely today.
- Test devices: **Chromecast with Google TV 4K** (TV) and **OnePlus Ace 2 Pro** (phone, assumed) over `adb`, later in the project.
- Size of v1: this is a large amount of work. I'll commit milestone by milestone and stop to ask whenever something is ambiguous rather than guess.

## 11. Open questions (need answers before step 1)

See the list in the chat message accompanying this plan. Answers will be folded back into this file.

## 12. Post-v1: web companion (requested 2026-11-19, scope to confirm)

A full browser player would depend on server transcoding (browsers lack MKV, HEVC/DV, DTS/TrueHD,
styled ASS and passthrough), which contradicts the direct-play goal, and Jellyfin Web / Plex Web
already exist. Proposed instead: a small web companion served by `alwywatch-server` at the same
hostname: leaving soon + swipe triage, keep requests, Seerr search/requests, admin page, and a
"Play" link that opens Jellyfin Web or Plex Web. Same sign-in (Jellyfin/Plex account), a few
hundred KB, no extra hosting.
