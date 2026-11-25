# Core API (`alwywatch-core`)

The one object UIs talk to: `AlwyWatch` (Kotlin through UniFFI, Rust directly on desktop). The
Rust doc comments in `core/alwywatch-core/src/app.rs` are the reference; this page is the map.

## Crates

| Crate | Role | Used by |
|---|---|---|
| `net` | Shared `reqwest` client (rustls + OS trust store on native; the browser's `fetch` on wasm32), URL helpers, `Error` | everything |
| `media-api` | `MediaServer` trait, `JellyfinClient`, `PlexClient`, sign-in flows (`JellyfinAuth`, `PlexAuth`), models | core, server, web |
| `seerr` | Seerr client, direct (cookie) or bridged through the AlwyWatch server | core, web |
| `maintainerr` | Maintainerr read/write client | server only |
| `keep-service` | `/v1` protocol types and `KeepClient`; watch together and transfer crypto | core, server, web |
| `downloads` | Resumable download manager (`Range`/`If-Range`, `.part` files, offsets in SQLite) | core |
| `store` | SQLite cache and settings, disk image cache | core |
| `alwywatch-core` | The `AlwyWatch` facade below, exported through UniFFI | Android (Kotlin), desktop (Rust) |

The web app links `media-api`, `seerr`, `keep-service` and `net` directly, without
`alwywatch-core`, `store` or `downloads` (see [web.md](web.md)).

## Conventions

* **Cache-first pairs.** `cached_x(...)` is synchronous: it reads SQLite and returns
  `None` when nothing is cached. Render it immediately. `refresh_x(..., force)` is async: it fetches
  when the cached copy is older than its TTL (or `force`), stores the result and returns it.
  UIs call both, never show a spinner when `cached_x` returned data.
* **Async functions** run on the core's own tokio runtime (Kotlin: `suspend`), so callers never
  need a runtime.
* **Errors**: `CoreError`: `Unauthorized`, `Forbidden`, `NotFound`, `Server{status}`, `Network{detail}`,
  `Pending` (a poll is not done yet), `Invalid{detail}`, `NotSignedIn{account_id}`, `Storage{detail}`, `Internal{detail}`.
  `Unauthorized` is a lost sign-in ("sign in again"). `Forbidden` is a Jellyfin/Plex server's 403:
  signed in, but this account may not do it (downloads or playback turned off for the user).
  Seerr and the AlwyWatch server keep 403 as `Unauthorized`: they use it for an expired session
  or a server they don't serve (`net::send` vs `net::send_strict`).
* **Secrets never live in the core's storage.** Sign-in returns tokens; the platform keeps them
  (Android Keystore, Windows DPAPI, Linux 0600 file) and hands them back with `activate_*` at
  start, which is offline.

## Setup and accounts

| Function | Notes |
|---|---|
| `AlwyWatch::new(CoreConfig)` | `data_dir`, `cache_dir`, `client` (device info), `image_cache_mb`, `downloads_dir`, `profile` (device profile this core runs for; None = the original one), `downloads_db` (shared by a device's profiles). No network. |
| `accounts()`, `activate_account(id, token)`, `is_active(id)`, `remove_account(id)` | Several Jellyfin/Plex accounts side by side |
| `jellyfin_server_info(url)`, `jellyfin_sign_in(url, user, pass)`, `jellyfin_quick_connect_start/poll` | Returns `SignedIn{account, token}` |
| `plex_create_pin(strong)`, `plex_check_pin(id)`, `plex_servers(account_token)`, `plex_connect(account_token, server, preferred_url)` | plex.tv PIN flow; `SignedIn.plex_account_token` is what Seerr and the keep service accept |
| `plex_home_users(account_token)`, `plex_switch_user(account_token, uuid, pin)` | Plex Home "who's watching"; the switch returns the token to use instead of the account token |
| `set_plex_account_token(account, token)` | The plex.tv token of a Plex account, for its watchlist |
| `ping(account)` | Light authenticated call with a short timeout; sends queued progress on success |
| `normalize_server_url(input)`, `core_version()` | Free functions |

## Library and playback

| Function | Notes |
|---|---|
| `cached_home` / `refresh_home(account, force)` | Rails: continue watching, next up, latest per library. Partial failures keep the other rails. A shows library's "recently added" is one card per show (newest arrival first, ≤ 20) with a `RailNote` in `LibraryRail.notes`: `label` ("S1 · E6 · Edenia", "S2 · E3–E5", "S2 · 3 new episodes", "Season 2", "2 new seasons", "5 new episodes", "New show · 8 episodes") and what opening the card shows (`open_id`: the episode when only one arrived, else the show; `open_season_id`: the season concerned). Grouping is `media_api::recently_added`, shared with the web |
| `cached_items` / `refresh_items(account, ItemQuery)` | Paged library grids |
| `cached_item` / `refresh_item`, `cached_seasons` / `refresh_seasons`, `cached_episodes` / `refresh_episodes` | Details |
| `cached_genres` / `refresh_genres`, `cached_collections` / `refresh_collections` | Library "Categories" and "Collections" tabs |
| `cached_watchlist` / `refresh_watchlist`, `is_watchlisted`, `set_watchlisted` | Jellyfin favorites, Plex's plex.tv watchlist |
| `people`, `cached_people`, `trailers`, `theme_song` | Title page extras |
| `adjacent_episodes(account, item)` | Previous and next episode across seasons; works offline from cache |
| `search(account, query, limit)`, `set_played(...)`, `remove_from_continue_watching(...)` | |
| `merge_homes(homes)`, `same_as(item)` | Free functions: several servers' homes as one; the cross-server identity of a title |
| `cached_image` / `image(...)`, `cached_remote_image` / `remote_image(url)` | Disk-cached files (size-capped LRU), by width bucket; return a local path |
| `playback_info(account, item, DeviceProfile)`, `prefetch_playback_info(...)` | Sources, original first; kept two minutes so a prefetch makes play instant. See [Playback](#playback-direct-play-and-conversion) |
| `segments(account, item)`, `playback_prompt(segments, position, duration, has_next)` | Skippable spans; what to offer now ("Skip intro", "Next episode", credits countdown). Pure, so every player behaves the same |
| `report_progress(account, ProgressReport)`, `flush_pending_progress()` | Offline reports are queued and sent when the network returns |

## `MediaServer` trait (`core/media-api`)

What every server client implements; `AnyServer` dispatches statically to `JellyfinClient` or
`PlexClient` (`AnyServer::from_session(session, client, http)` rebuilds one from a stored
session). Sign-in is not on the trait: `JellyfinAuth` (password, Quick Connect) and `PlexAuth`
(PIN, resources, Plex Home). Futures are `Send` on native and not in the browser (`MaybeSend`).

| Method | Returns |
|---|---|
| `session()` | The signed-in `Session` (kind, base URL, server and user ids, token) |
| `libraries()` | Libraries (`Movies`, `Shows`, `Other`) |
| `items(ItemQuery)` | A page of a library: sort, genre, collection, unwatched filter |
| `item(id)` | One item with user data, images and provider ids |
| `seasons(series)`, `episodes(series, season)` | Children of a show |
| `resume(limit)` | Partly watched items, most recent first |
| `next_up(limit)` | Next unwatched episode of shows in progress |
| `latest(library, limit)` | Recently added, episode by episode (Jellyfin `groupItems=false`, like Plex); `recently_added(items, max)` groups them per show with a label |
| `search(query, limit)` | Movies, shows and episodes |
| `shows_last_watched(series_ids)` | When each show last had an episode finished (orders "continue watching") |
| `remove_from_continue_watching(id)` | `false` when the server has no such thing (the caller hides it locally) |
| `genres(library)`, `collections(library)` | Library tabs |
| `playback_info(id, DeviceProfile)` | Sources, original file first; each with a direct request and an optional conversion |
| `segments(id)` | Skippable spans (see below) |
| `report_progress(ProgressReport)` | Start, progress, pause, stop |
| `trailers(id)`, `theme_song(id)`, `people(id)` | Title page extras |
| `watchlist(plex_tv_token)`, `set_watchlisted(id, on, plex_tv_token)` | Jellyfin favorites; Plex's plex.tv watchlist (needs the plex.tv token; without it the list is empty and adding fails with `Unauthorized`) |
| `set_played(id, played)` | Watched toggle |
| `watch_history()`, `set_watch_state(id, WatchState)` | History copy and sync |
| `download_source(id)` | The original file, never a transcode |
| `provider_index(kind)` | Every movie or show with external ids (maps Maintainerr items) |
| `image_request(id, ImageRef, max_width)` | A sized image request; no network |

Models that matter (`media_api::model`): `Item` (kind, ids, `ProviderIds` tmdb/imdb/tvdb,
`UserData`, `ImageRef`s with BlurHash), `MediaSource` (`streams: [MediaStream]`, `direct` and
`transcode: Option` as `HttpRequest`, `supports_direct_play`), `MediaStream` (codec, language,
bit depth, size, `HdrKind`, Dolby Vision profile, forced/default/external), `DeviceProfile`,
`Segment`, `ProgressReport`, `WatchState`, `DownloadSource`. An `HttpRequest` carries auth in
headers, never in the URL, so URLs are safe to log (the web app is the exception, see
[web.md](web.md)).

## Playback: direct play and conversion

Each platform builds a `DeviceProfile` (containers, video codecs, `video_10bit_codecs`, audio
and subtitle codecs, max width/height, HDR kinds) and passes it to `playback_info`.

* **Android** (`player/DeviceProfiles.kt`): video codecs from `MediaCodecList`; HEVC, VP9 and AV1
  count only when a **hardware** decoder exists (the software ones are too slow on TV sticks), so
  without the chip the server converts. 10-bit support from the decoder's profile levels; HDR
  kinds from the display (Dolby Vision only with a DV decoder). Audio includes AC3, E-AC3, DTS and
  TrueHD (bundled FFmpeg decoder or passthrough). The player decides with `canDirectPlay`
  (container, video codec, bit depth, size, one playable audio track), plays `direct` when it
  passes, `transcode` otherwise, and switches to `transcode` at the same position when the
  decoder fails on the original.
* **Desktop**: a fixed broad profile (mpv/FFmpeg decode nearly everything, 10-bit included) and
  the original file unless a quality is picked (below).
* **Web**: the profile is asked to the browser; see [web.md](web.md#playback).

Per server:

* **Jellyfin**: the profile becomes a Jellyfin `DeviceProfile` (direct play for what the device
  decodes; 8-bit limits and max size as codec conditions; HLS/TS transcoding profile, HEVC first
  only if the device decodes 10-bit HEVC; text subtitles also as external files). Jellyfin
  decides: `supports_direct_play` and the transcoding URL come from its answer, direct-playable
  sources first.
* **Plex**: Plex does not decide for the client. `supports_direct_play` is always `true` and the
  platform checks the codecs itself. Every source also gets a conversion URL (`transcode_url` in
  `plex.rs`): `/video/:/transcode/universal/start.m3u8` with `directStream=1`. Plex refuses
  unknown clients with 400 unless the request describes the client, so it names the `Generic`
  profile and adds `X-Plex-Client-Profile-Extra`: an HLS/MPEG-TS target with H.264 (plus HEVC
  when the profile lists `ts` and `hevc`) and AAC (plus AC3/E-AC3/MP3 when listed), the max
  width/height, and an 8-bit limit for each codec not in `video_10bit_codecs`. Streams the device
  plays are copied, the others converted. The same session name is used in progress reports, so
  Plex ties them together and stops converting when playback stops.

### Quality menu

Every player's track panel ("Audio, subtitles & quality") shows what plays now and offers the
original plus smaller sizes (`media_api::quality`: `quality_label`, `quality_choices`, exported
by the core for Kotlin). Sizes are presets (4K 40 Mbps, 1080p 20/8, 720p 4, 480p 1.5, 360p 0.7)
smaller than the file, named from the picture's width (a 1920×800 film is "1080p"). A pick sets
`TrackSelection.quality = Quality { max_bitrate, max_height }` and playback restarts at the
same position through `playback_info_with`, which then always converts:

* **Jellyfin**: `EnableDirectPlay`/`EnableDirectStream` false, `MaxStreamingBitrate` and the
  profile's max size capped, `MaxHeight` added to the transcoding URL when missing.
* **Plex**: the profile's max size capped and `maxVideoBitrate` (kbit/s) on the conversion URL;
  `supports_direct_play` false. No stream selection is saved on the part for a quality alone.

"Original" goes back to the file (or the server's own conversion when the device can't play
it). The pick holds for the next titles of the session. Desktop doesn't offer it in a watch
together room (everyone plays the same file).

### Burned-in subtitles

A conversion burns the chosen subtitle into the picture. Jellyfin first extracts a file's text
subtitles (one read of the whole file, 30 s to over a minute for a film, then cached), and the
conversion waits for it. `prepare_subtitle(account, item, source, index)` asks for that
subtitle (`GET /Videos/{item}/{source}/Subtitles/{index}/0/Stream.srt`) and returns once it is
ready; it returns at once for picture subtitles, separate files and Plex (which burns in from the
file). Jellyfin finishes the extraction even when the request times out, and asking again waits
for the same work. Android waits 3 s for it; past that the conversion starts without the
subtitle and restarts with it, at the same position, once it is ready.

## Skippable segments

`segments(id)` returns `Segment { kind: Intro|Credits|Recap|Preview|Commercial, start_ms, end_ms }`.

* **Jellyfin**: `GET /MediaSegments/{id}` (Jellyfin 10.10+, filled by a segment provider such as
  the Intro Skipper plugin). When that is empty or fails: the file's chapters, when named like an
  intro, credits, recap or preview (English and French names, whole words; implausibly long
  spans dropped).
* **Plex**: the item's markers (`includeMarkers=1`): `intro`, `credits`, `commercial`.

`playback_prompt` turns segments and the position into what the player shows; "Next episode"
replaces "Skip credits" when there is a next episode, and is offered 40 s before the end when no
credits are marked.

## UniFFI layer

`alwywatch-core` uses UniFFI proc-macros only (no UDL): records and enums of the other crates
derive `uniffi::Record`/`Enum` behind their `uniffi` feature, and `AlwyWatch` is a
`uniffi::Object`. Kotlin bindings are generated by `tools/uniffi-bindgen` in library mode from
the host debug build (release builds strip the metadata); Gradle runs it and `cargo ndk` (see
[build.md](build.md)). Async methods become Kotlin `suspend` functions running on the core's own
tokio runtime. On Android, `NativeTls.init(context)` (JNI, `android.rs`) must run once before any
network call so rustls can use Android's trust store. The desktop app links the crate as plain
Rust.

## Downloads (phone/tablet, desktop)

`download_item(account, item)`, `download_season(...)`, `download_series(...)`, `downloads()`,
`pause_download(id)`, `resume_download(id)`, `remove_download(id)`, `downloaded_file(account, item)`,
`has_pending_downloads()`, `run_downloads()` (called by the platform's background worker; resumes
with HTTP `Range` / `If-Range`, survives network loss, process death and full disks).

Housekeeping: each download has a `DeleteRule` (`Never`, `AfterDays`, `WhenWatched`,
`DaysAfterWatched`; `set_download_rule`, `default_delete_rule` / `set_default_delete_rule`).
`set_auto_download(account, series, title, count, rule)` keeps the next `count` unwatched episodes of a
show downloaded; `sync_downloads()` applies the rules and tops those shows up. Downloads always
fetch the original file (`MediaServer::download_source`), never a transcode. With device profiles,
`profile()` names the current one and `release_profile_downloads(profile)` hands a removed
profile's downloads back.

## Seerr

Direct: `seerr_info(url)`, `seerr_sign_in_plex/jellyfin/local`, `seerr_quick_connect_start/poll`
(platform stores the returned cookie), `seerr_activate(cookie)`.
Through the AlwyWatch server (any media server type): `seerr_connect_bridge()` (the keep
account), or `seerr_connect_bridge_as(account, token)` / `seerr_activate_bridge(token)` for
another account.
Then: `seerr_connection()`, `seerr_active()`, `seerr_sign_out()`, `seerr_search(query)`,
`seerr_title(type, tmdb)` (details, cast, saga), `seerr_collection(id)`, `seerr_seasons(tmdb)`,
`seerr_request(type, tmdb, seasons)`, `seerr_cancel(id)`,
`cached_seerr_requests` / `refresh_seerr_requests(force)`.

Requesting more seasons of a show: `seerr_season_info(tmdb)` returns every season (specials
included) with its availability, whether an open request covers it (pending, approved or
failed; not declined, completed or 4K) and whether Sonarr's queue holds episodes of it. The
pure `season_plan(info, have)` (also `seerr::season_plan` for the web app) turns that into one
state per season (`OnServer`, `PartlyOnServer`, `Requested`, `Downloading`, `Requestable`,
`Blocked`) with its label (`season_state_label`), the requestable numbers (`missing`) and
`has_some` (button reads "Request the missing seasons"). `have`: the season numbers the browsed
media server shows (Seerr follows one media server only). Specials are listed only when they
are all that can be requested. `seasons_requested_message(numbers)` is the shared confirmation.

## Keep service (AlwyWatch server)

Protocol: [keep-protocol.md](keep-protocol.md).

| Function | Notes |
|---|---|
| `keep_servers(url)` | Public allow-list, to offer servers without typing URLs |
| `keep_set_preset(url)`, `keep_clear_preset()`, `keep_onboard(account, token)` | Add-server flow: the picked AlwyWatch server is connected right after sign-in, with Seerr through it |
| `keep_connect(url, account, token)`, `keep_activate(token)`, `keep_connection()`, `keep_active()`, `keep_disconnect()` | `token`: Jellyfin token or plex.tv account token |
| `keep_me()` | Rights, admin flag, display name, avatar, `profile_set` |
| `cached_leaving_soon` / `refresh_leaving_soon(force)`, `keep_triage(limit)` | Most urgent first; triage = not voted yet |
| `keep_vote(key, VoteRequest)`, `keep_unvote(key)` | Patch the cached list in place |
| `keep_status_of(Item)`, `keep_entry(Item)` | A title page's keep entry: from the cache (instant, `None` when in no deletion list), or asked from the server for a title in no list, to keep it ahead |
| `keep_due_reminders(now)` | "Keep until watched" reminders due now (local notifications) |
| `keep_admin_users()`, `keep_admin_set_rights(user, Rights)`, `keep_admin_countdown(collection, stagger_days)` | Admins only |
| `keep_admin_forever()` | Who keeps what forever, shared and alone |
| `keep_admin_seerr_users()`, `keep_admin_link_seerr(user, seerr_user_id)` | Link a user's requests to an existing Seerr user (None: automatic) |

## Watch together

Private rooms that start as a lobby, with invitations ([protocol](keep-protocol.md#watch-together),
app flows: [together-apps.md](together-apps.md)):

| Function | Notes |
|---|---|
| `together_create(account, item) -> TogetherJoined` | A lobby with this title, the caller its host (nothing plays) |
| `together_rooms() -> TogetherRooms`, `together_rooms_wait(after_version) -> TogetherRooms` | My rooms and invitations; the wait is the invitation channel (~25 s long poll) |
| `together_people() -> [TogetherPerson]` | Who can be invited: recent partners first, then by name |
| `together_invite(code, member, user_keys) -> TogetherState`, `together_uninvite(code, member, user_key)` | Host only |
| `together_join(code) -> TogetherJoined`, `together_decline(code)` | Invited user (or my other device for join) |
| `together_start(code, member, position_ms) -> TogetherState` | Host: starts the lobby, playing |
| `together_update(code, member, playing, position_ms, account?, item?)` | Play/pause/seek/new title (lobby: host only, title) |
| `together_wait(code, member, after_seq)`, `together_leave(code, member)` | Long poll + heartbeat; leave for good |
| `together_find(account_ids, media) -> TogetherMatch?` | The room's title on the first account that has it (TMDB/IMDb/TVDB, season, episode) |
| `together_position(state, received_at_ms, now_ms)`, `together_is_host(state, member)` | Pure helpers |

## AlwyWatch profile

| Function | Notes |
|---|---|
| `keep_set_display_name(name)`, `keep_reset_display_name()` | → `Me`; check first with `display_name_problem(name)` |
| `keep_set_avatar_preset(id)`, `keep_upload_avatar(bytes, content_type)`, `keep_remove_avatar()` | → `Me` |
| `keep_avatar_request(user_key, version) -> HttpRequest` | URL + sign-in headers of a custom avatar; cache by (user_key, version) |
| `keep_mark_profile_set()` | → `Me`; ends the one-time welcome step for the account |
| `avatar_presets()`, `avatar_default_color(name)`, `avatar_initial(name)` | Draw built-in and default avatars ([table](keep-protocol.md#built-in-avatars)) |

## Device transfer and setup copy

| Function | Notes |
|---|---|
| `transfer_start(url, device)` | New device: registers a one-time key; returns `TransferCode { code, check, handle, ... }`. No sign-in |
| `transfer_receive(url, handle)` | New device: `TransferReceived { bundle, account_token }` once approved, `None` while waiting. `bundle`: another device's setup (as before); `account_token`: an AlwyWatch account session (store it, then `account_use_token`) |
| `transfer_peer(code)`, `transfer_send(code, bundle)` | Signed-in device: the asking device and its check number, then send the sealed bundle |
| `export_config(setting_keys)`, `import_config(json)` | The bundle: accounts, AlwyWatch server and Seerr connections, chosen settings. Tokens are added and stored by the platform |

Protocol and crypto: [keep-protocol.md](keep-protocol.md#device-transfer).

## AlwyWatch accounts and Quick Connect

An account signs a device in to the AlwyWatch server without a media server (profile, synced
settings, the user's server list); the platform keeps its token in secure storage under
`alwywatch_account`. Server side: [keep-protocol.md](keep-protocol.md#alwywatch-accounts).

| Function | Notes |
|---|---|
| `account_login(url, username, password, device)` | → `AccountSignedIn { account, token }` |
| `account_use_token(url, token)` | After Quick Connect brought an `account_token` |
| `account_activate(token)`, `account()`, `account_sign_out()` | Start-up (no network), the saved account, sign out |
| `account_link_media(account_id, token)` | After a Jellyfin/Plex sign-in: it joins the account (false: not a served server) |
| `account_approve(code)` | Quick Connect from this device without sending its setup |
| `known_servers()` | Servers in the synced list (`account.servers`, filled from every device's accounts) not signed in here |
| `qr_code(text)`, `quick_connect_url(url, code)` | The QR code of `<url>/q/<code>` shown next to the code |

Settings sync goes through the account session when no media sign-in is linked to the
AlwyWatch server.

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

## Settings across devices

Settings about the viewer follow them to their other devices: on the AlwyWatch server when one
is connected and reachable (`GET`/`PUT /v1/me/settings`, any server type), else with their
Jellyfin user in Jellyfin's per-app `DisplayPreferences` (`CustomPrefs`, client "AlwyWatch").
Each synced setting carries its local change time; per setting the newer side wins, ties take the
shared value (a device that never synced adopts it). Other apps' entries there are kept.

Synced: languages, subtitle mode/colour/background, skip and next-episode choices, per-title
tracks and versions, which servers the home shows and the one in use, theme music, confirm quit,
menu libraries. Per device: refresh rate, tunneling, passthrough, subtitle size and raise, clock,
scroll positions, downloads.

Per-title track choices follow the title, not the sign-in: `track_title_key` is the item's id
then, after `|`, the title as every server names it (`tmdb:`/`imdb:` id for a movie, else its
name and year; `show:` and the show's name for a show and its episodes). `set_track_pref` saves
both `tracks.<account>.<id>` and `tracks.title.<shared>`; `track_pref` takes whichever changed
last, so subtitles chosen on Jellyfin apply on Plex and on every device of the person.

`sync_settings()`: at start and when the app comes back (> 0: reload). Changing a synced setting
with `set_setting` shares it a few seconds later.

## Settings

`get_setting(key)`, `set_setting(key, value)`: small UI state (current account, last tab).
