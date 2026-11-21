# Web app

AlwyWatch in a browser, served by `alwywatch-server` at the root of its site
(`https://watch.example.org/`). Source: `web/` (crate `alwywatch-web`).

## Architecture

* **Same Rust clients as the apps, compiled to `wasm32-unknown-unknown`**: `media-api`
  (Jellyfin, Plex), `seerr`, `keep-service` and `net`. On wasm32, `net` uses the browser's
  `fetch` (the browser does TLS; there are no request timeouts) and `MediaServer` futures are not
  `Send` (`MaybeSend`).
* **UI**: Leptos 0.8, client-side rendering only (no server functions), `leptos_router` for real
  URLs, so links and the back button work.
* **Not used**: `alwywatch-core`, `store` and `downloads`. There is no SQLite and no
  stale-while-revalidate layer. The app keeps its state in `localStorage` (keys `aw.*`): the
  signed-in accounts (including their tokens), a device id, the AlwyWatch server link, player
  preferences, and copies of the home rails, "Leaving soon" and some list choices so the next
  start can show them at once.
* **No address to type for the AlwyWatch server**: it is the page's own origin. The sign-in page
  lists `/v1/servers`; "Another Jellyfin server" accepts any Jellyfin URL.

### Who the browser talks to

| Service | How | Why |
|---|---|---|
| AlwyWatch server | Same origin (`/v1/...`) | No CORS needed; the server sets no CORS headers |
| Jellyfin | Directly, cross-origin | Jellyfin answers any origin |
| Plex, plex.tv | Directly, cross-origin | Plex echoes the requesting origin |
| Seerr | Only through the AlwyWatch server's [Seerr bridge](keep-protocol.md#seerr-bridge) | Seerr sends no CORS headers |

The CORS behaviour of Jellyfin, Plex and Seerr belongs to those servers, not to AlwyWatch.
Consequence: Seerr search and requests on the web need an account linked to the AlwyWatch
server, and the server needs the bridge configured (`[seerr]` and `SEERR_API_KEY`).

An account is linked to the AlwyWatch server automatically after sign-in when its server is in
the allow-list (the first such account). Keep, requests and watch together appear only then.

## Pages

Routes in `web/src/main.rs`:

| Path | Page |
|---|---|
| `/` | Home rails |
| `/signin`, `/settings` | Sign in (Jellyfin password or Quick Connect, Plex PIN in a new tab); accounts, AlwyWatch server link, playback settings |
| `/library/:acc/:id` | Library grid: sort (remembered), genre, "Not watched" |
| `/item/:acc/:id` | Title page: seasons and episodes, cast, watched and My list toggles |
| `/search` | Search, with Seerr results when the bridge is available |
| `/list` | My list (Jellyfin favorites, Plex watchlist) |
| `/requests`, `/request/:kind/:tmdb` | Seerr requests (mine / everyone) and a requestable title's page |
| `/together` | Watch together rooms |
| `/leaving`, `/leaving/sort` | Leaving soon with keep votes; one-at-a-time sorting |
| `/admin`, `/admin/forever` | Admins: rights, Seerr user links, countdown; who keeps what forever |
| `/play/:acc/:id` | Player |

`:acc` is `jf|px.<server id>.<user id>`. Without any account, every page shows the sign-in.

## Playback

The player is the browser's `<video>` element (its own controls, keyboard and fullscreen), with
skip intro/credits, next episode with countdown, watch together sync and progress reports on top
(`web/src/player.rs`).

1. The device profile is asked to the browser (`canPlayType`): H.264 always; HEVC, VP9 and AV1
   when the browser says so (10-bit HEVC and AV1 too); AAC and MP3, plus Opus, Vorbis, FLAC, AC3
   and E-AC3 when supported; MP4/MOV/WebM, and Matroska on Chromium and Firefox. No HDR kinds,
   no size or bitrate limit.
2. If the server marks the first source as direct-playable, the original file plays. Jellyfin
   decides this from the profile. Plex always says yes (see
   [core-api.md](core-api.md#playback-direct-play-and-conversion)), so on Plex the original is
   always tried first.
3. If the original fails (an audio codec or container the browser only claimed), the server's
   HLS conversion takes over at the same position. HLS plays natively where the browser supports
   it (Safari), else through **hls.js** (`web/static/vendor/hls.light.min.js`, Apache-2.0,
   vendored), loaded only at that moment from the same site.

Subtitles: only text ones. On Jellyfin any text track (SRT, VTT, ASS/SSA, mov_text) is fetched
as WebVTT from the server; on Plex only external SRT/VTT files. The text is fetched, converted
to WebVTT if needed and attached as a `<track>` from the page. Styling of ASS/SSA is lost.
Image subtitles (PGS, DVD, DVB) are not shown.

## Limitations (true in the code today)

* **Tokens live in `localStorage`**, readable by any script on the page. The settings page says
  to sign out on shared computers.
* **Tokens go in media URLs** (`api_key=` for Jellyfin, `X-Plex-Token=` for Plex), because
  `<img>` and `<video>` cannot send headers. They can end up in browser history and server logs.
* **No audio track choice**: the browser plays the file's default audio track (or the
  conversion's). Progress reports carry no audio or subtitle index.
* **Only the first media source** is played; there is no version picker.
* **Not synced**: player preferences stay in the browser (no `/v1/me/settings`, no Jellyfin
  `DisplayPreferences`); per-title track choices are not shared with the apps.
* **Not available on the web**: downloads, watch history copy and two-server sync, device
  transfer, device profiles, trailers, theme music, Seerr sagas, Seerr without the bridge.
* No HDR handling, no passthrough, no offline mode.

## Build and serve

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-bindgen-cli --version 0.2.130   # must equal the `wasm-bindgen` pin in web/Cargo.toml
web/build.sh                                                # -> web/dist
```

`web/build.sh` builds with the `web` cargo profile (opt-level "z", `panic = "abort"`, symbols
kept for wasm-bindgen), runs `wasm-bindgen --target web`, runs `wasm-opt -O3` if Binaryen is
installed, copies `web/static/` and writes `.gz` copies of the text and wasm files.

`alwywatch-server` serves the folder named by `ALWYWATCH_WEB` (default `/web`;
`server/src/web.rs`):

* every path that is not `/v1` or `/v1/...` is a file, or `index.html` when the path has no
  extension, so app links (`/item/...`, `/play/...`) open directly;
* the `.gz` copy is sent when the browser accepts gzip;
* `Cache-Control: no-cache` with an ETag (size + mtime), so a new build is picked up at once;
* `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`, `X-Frame-Options: DENY`;
* `..`, hidden files and backslashes in paths get 404.

`server/Dockerfile` installs the same wasm-bindgen version, runs `web/build.sh` and copies
`web/dist` to `/web` in the image. Locally, run the server with `ALWYWATCH_WEB=web/dist`.

Checks: `cargo clippy -p alwywatch-web --target wasm32-unknown-unknown -- -D warnings` and
`cargo test -p alwywatch-web` (the tests run on the host). Both run in CI (`scripts/ci.sh web`).
