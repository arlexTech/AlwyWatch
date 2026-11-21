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

## Watch history copy

Moves a viewer's history between their own servers (Plex → Jellyfin when switching). Titles are
matched by TMDB/IMDb/TVDB ids, then title and year; episodes by show and season/episode number.
Only adds: nothing is unmarked and a resume point never moves backwards.

| Function | Notes |
|---|---|
| `history_copy_plan(from, to)` | Reads both histories, writes nothing: `changes`, `up_to_date`, `not_found` (labels) |
| `history_copy_apply(to, changes)` | One request per change, continues past failures: `applied`, `failed` |

Server side: `MediaServer::watch_history()` (watched or started movies and episodes) and
`set_watch_state(item, WatchState)` (Jellyfin `POST /UserItems/{id}/UserData`, with the last played
date; Plex `/:/scrobble` or `/:/progress`).

## Keeping two servers in sync

Two accounts (a viewer's Plex and Jellyfin) kept in sync both ways. Each run reads both histories,
links titles both ways (same matching as the copy) and compares each with the state agreed at the
previous run (a snapshot per pair, in the cache): changed on one side only, that side wins, unwatching
and rewinding included; changed on both, or never synced, the more advanced state wins. A failed write
leaves the title out of the snapshot, so the next run settles it as never synced. Watching in
AlwyWatch is mirrored at once (playback stop and watched toggles), for titles a run already linked.

| Function | Notes |
|---|---|
| `sync_pairs()`, `set_sync_pair(a, b, on)` | Off forgets the snapshot |
| `sync_now()` | Every pair; progress through `history_copy_progress` |
| `sync_status(a, b)` | Last run time, changes, failures, error, a few updated titles |

Android runs it every 6 hours (WorkManager, foreground while running) and on demand.

## Audio and subtitles

One choice per movie or show (episodes follow), shared by every player. Besides the language, the
track's title and its position among same-language tracks are kept, so a pack with the same naming
and order across a season gets the same track in every episode (VFF over VFQ, full over SDH).

| Function | Notes |
|---|---|
| `track_title_key(item)` | Movie id, or the show's id for an episode |
| `track_pref(account, key)`, `set_track_pref(account, key, pref)` | Stored per account |
| `choose_tracks(pref, rules, tracks)` | Once the file's tracks are known: the choice, else the settings (`player.audio_lang`, `player.sub_mode` auto/forced/always/off, `player.sub_lang`). Returns the audio to select (None: keep the player's) and the subtitle (None: off) |
| `pick_track(pref, kind, tracks, picked)` | The new choice after the viewer picked a track (`picked` None: subtitles off) |
| `remembered_track(pref, kind, tracks)` | The track the choice points to in a file (title page) |
| `lang3(code)` | Any code ("fr", "fre", "fr-CA") → ISO 639-2/T |

`tracks` is the player's list in file order (`TrackInfo { id, kind, language, title, forced }`);
`id` is the player's own index, returned as is.

## Settings

`get_setting(key)`, `set_setting(key, value)`: small UI state (current account, last tab).
