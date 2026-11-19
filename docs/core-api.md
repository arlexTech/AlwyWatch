# Core API (`alwywatch-core`)

The one object UIs talk to: `AlwyWatch` (Kotlin through UniFFI, Rust directly on desktop). The
Rust doc comments in `core/alwywatch-core/src/app.rs` are the reference; this page is the map.

## Conventions

* **Cache-first pairs.** `cached_x(...)` is synchronous: it reads SQLite and returns
  `None` when nothing is cached. Render it immediately. `refresh_x(..., force)` is async: it fetches
  when the cached copy is older than its TTL (or `force`), stores the result and returns it.
  UIs call both, never show a spinner when `cached_x` returned data.
* **Async functions** run on the core's own tokio runtime (Kotlin: `suspend`), so callers never
  need a runtime.
* **Errors**: `CoreError`: `Unauthorized`, `NotFound`, `Server{status}`, `Network{detail}`,
  `Pending` (a poll is not done yet), `Invalid{detail}`, `NotSignedIn{account_id}`, `Storage{detail}`, `Internal{detail}`.
* **Secrets never live in the core's storage.** Sign-in returns tokens; the platform keeps them
  (Android Keystore, Windows DPAPI, Linux 0600 file) and hands them back with `activate_*` at
  start, which is offline.

## Setup and accounts

| Function | Notes |
|---|---|
| `AlwyWatch::new(CoreConfig)` | `data_dir`, `cache_dir`, `client` (device info), `image_cache_mb`, `downloads_dir`. No network. |
| `accounts()`, `activate_account(id, token)`, `is_active(id)`, `remove_account(id)` | Several Jellyfin/Plex accounts side by side |
| `jellyfin_server_info(url)`, `jellyfin_sign_in(url, user, pass)`, `jellyfin_quick_connect_start/poll` | Returns `SignedIn{account, token}` |
| `plex_create_pin(strong)`, `plex_check_pin(id)`, `plex_servers(account_token)`, `plex_connect(account_token, server, preferred_url)` | plex.tv PIN flow; `SignedIn.plex_account_token` is what Seerr and the keep service accept |
| `normalize_server_url(input)`, `core_version()` | Free functions |

## Library and playback

| Function | Notes |
|---|---|
| `cached_home` / `refresh_home(account, force)` | Rails: continue watching, next up, latest per library. Partial failures keep the other rails |
| `cached_items` / `refresh_items(account, ItemQuery)` | Paged library grids |
| `cached_item` / `refresh_item`, `cached_seasons` / `refresh_seasons`, `cached_episodes` / `refresh_episodes` | Details |
| `search(account, query, limit)`, `set_played(...)` | |
| `cached_image` / `image(...)`, `cached_remote_image` / `remote_image(url)` | Disk-cached files (size-capped LRU), by width bucket; return a local path |
| `playback_info(account, item, DeviceProfile)`, `prefetch_playback_info(...)` | Direct play first; the device profile comes from the platform's decoders |
| `report_progress(account, ProgressReport)`, `flush_pending_progress()` | Offline reports are queued and sent when the network returns |

## Downloads (phone/tablet, desktop)

`download_item(account, item)`, `downloads()`, `pause_download(id)`, `resume_download(id)`,
`remove_download(id)`, `downloaded_file(account, item)`, `has_pending_downloads()`,
`run_downloads()` (called by the platform's background worker; resumes with HTTP `Range` /
`If-Range`, survives network loss, process death and full disks).

## Seerr

Direct: `seerr_info(url)`, `seerr_sign_in_plex/jellyfin/local`, `seerr_quick_connect_start/poll`
(platform stores the returned cookie), `seerr_activate(cookie)`.
Through the AlwyWatch server (any media server type): `seerr_connect_bridge()`; nothing to store.
Then: `seerr_connection()`, `seerr_active()`, `seerr_sign_out()`, `seerr_search(query)`,
`seerr_seasons(tmdb)`, `seerr_request(type, tmdb, seasons)`, `seerr_cancel(id)`,
`cached_seerr_requests` / `refresh_seerr_requests(force)`.

## Keep service (AlwyWatch server)

Protocol: [keep-protocol.md](keep-protocol.md).

| Function | Notes |
|---|---|
| `keep_servers(url)` | Public allow-list, to offer servers without typing URLs |
| `keep_set_preset(url)`, `keep_onboard(account, token)` | Add-server flow: the picked AlwyWatch server is connected right after sign-in, with Seerr through it |
| `keep_connect(url, account, token)`, `keep_activate(token)`, `keep_connection()`, `keep_active()`, `keep_disconnect()` | `token`: Jellyfin token or plex.tv account token |
| `keep_me()` | Rights, admin flag |
| `cached_leaving_soon` / `refresh_leaving_soon(force)`, `keep_triage(limit)` | Most urgent first; triage = not voted yet |
| `keep_vote(key, VoteRequest)`, `keep_unvote(key)` | Patch the cached list in place |
| `keep_due_reminders(now)` | "Keep until watched" reminders due now (local notifications) |
| `keep_admin_users()`, `keep_admin_set_rights(user, Rights)`, `keep_admin_countdown(collection, stagger_days)` | Admins only |

## Settings

`get_setting(key)`, `set_setting(key, value)`: small UI state (current account, last tab).
