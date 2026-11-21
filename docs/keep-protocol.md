# Keep-service protocol (`/v1`)

Spoken by `alwywatch-server` and the apps (`core/keep-service` holds the shared types and the
client; routes are in `server/src/api.rs`). JSON, `snake_case`, enums as snake_case strings,
times as unix seconds (watch together: milliseconds, fields ending in `_ms`).

## Authentication

Every call except `/healthz`, `/v1/servers`, the webhooks and the two calls of a new device in
a [transfer](#device-transfer) sends:

```
Authorization: Bearer <token>
X-AlwyWatch-Server: <allow-listed server id>
```
* Jellyfin: the user's own Jellyfin access token. The server calls `GET /Users/Me` on that
  Jellyfin; Jellyfin administrators are AlwyWatch admins.
* Plex: the user's **plex.tv account token**. The server calls plex.tv `/api/v2/user` and
  `/api/v2/resources`; the user must have access to the allow-listed Plex server; its owner is admin.

The server id must be in the allow-list (`[[servers]]` in `config.toml`, the same list
`/v1/servers` returns); there are no AlwyWatch accounts or passwords. Results are cached 10
minutes under a keyed hash of (server, token); tokens are never stored or logged. The user key is
`jellyfin:<server>:<user id>` or `plex:<server>:<plex user id>`. Too many rejected tokens from
one address (20 in 10 minutes) get 429 before the media server is asked.

## Errors

JSON `{"error": "<message>"}` (`keep_service::ApiError`) with:

| Status | When |
|---|---|
| 400 | Invalid vote, setting too long or too many, malformed transfer |
| 401 | Missing bearer token or `X-AlwyWatch-Server`; token refused by the media server |
| 403 | Server not served here; not an admin; rights do not allow the vote; Seerr path not bridged; no Seerr account; read-only test mode (`dry_run`); not your room member or request |
| 404 | Unknown item, user, room or transfer code; Seerr bridge or webhooks off; wrong webhook secret |
| 409 | Room full; transfer code already used |
| 429 | Too many failed sign-ins, room creations, join requests or transfer starts |
| 500 | Internal error. The message is always "server error, try again later"; details go to the server log |
| 502 | Media server or Seerr unreachable |
| 503 | Too many rooms or pending transfers on the server |

A `/v1/...` path with no route gets a plain-text `404 not found` (the web app fallback refuses
`/v1`). Any other unknown path serves the web app (see [web.md](web.md)).

## Endpoints

| Method | Path | Who | Body → Response |
|---|---|---|---|
| GET | `/healthz` | anyone | `ok` (text) |
| GET | `/v1/servers` | anyone | → `[ServerEntry]` (allow-list: `id`, `kind` `jellyfin`/`plex`, `name`, `url` = public URL) |
| GET | `/v1/me` | user | → `Me` (`user_key`, `name`, `server_id`, `is_admin`, `rights`, `forever_count`) |
| GET | `/v1/me/settings` | user | → `{key: value}` (see [Settings across devices](#settings-across-devices)) |
| PUT | `/v1/me/settings` | user | `{key: value}` → 204 |
| GET | `/v1/leaving-soon` | user | → `[LeavingItem]`, most urgent first |
| GET | `/v1/triage?limit=N` | user | → `[LeavingItem]` not voted on yet and not protected (swipe mode; default 20, max 200) |
| PUT | `/v1/votes/{key}` | user | `VoteRequest` → `LeavingItem` (replaces the caller's vote) |
| DELETE | `/v1/votes/{key}` | user | → `LeavingItem` |
| GET | `/v1/admin/users` | admin | → `[AdminUser]` |
| PUT | `/v1/admin/users/{user_key}/rights` | admin | `Rights` → `AdminUser` |
| PUT | `/v1/admin/users/{user_key}/seerr` | admin | `SeerrLinkRequest` → `AdminUser` |
| GET | `/v1/admin/seerr-users` | admin | → `[SeerrUser]` (`id`, `name`); 404 when the bridge is off |
| GET | `/v1/admin/forever` | admin | → `ForeverStats` |
| POST | `/v1/admin/collections/{id}/countdown` | admin | `{"stagger_days": N}` (capped at 365) → `{"reset": n, "skipped": n}` |
| GET | `/v1/seerr` | user | → `SeerrBridgeInfo` (links or creates the caller's Seerr user) |
| any | `/v1/seerr/api/v1/{path}` | user | Seerr API, allow-listed (see below) |
| POST | `/v1/hooks/{tautulli\|jellyfin\|plex}/{secret}` | webhook | wakes the "watched" check → 204 |
| … | `/v1/together…` | user | [Watch together](#watch-together) |
| … | `/v1/transfer…` | new device / user | [Device transfer](#device-transfer) |

`AdminUser` = `Me` fields plus `last_seen` (unix seconds) and `seerr_user_id` (null: linked
automatically or not yet).

### Votes

`VoteRequest`:
```json
{"kind": "keep_days", "days": 30, "reminder_days": null, "reason": null}
```
| `kind` | Meaning | Fields |
|---|---|---|
| `dont_care` | Signal for the admin; never deletes anything by itself | — |
| `keep_days` | Keep N more days | `days` ≥ 1, ≤ `rights.max_keep_days` and ≤ 3650 |
| `keep_until_watched` | Keep until *this user* has watched it (show: last episode) | `reminder_days`: the app reminds every N days |
| `keep_forever` | Keep forever | `reason` (free text); needs `rights.can_keep_forever` and stays under `rights.max_forever` |

Resolution per item over everyone's active votes: **forever > until watched > longest
extension**. Forever/until-watched become a Maintainerr global exclusion; extensions postpone the
deletion date. When the last protecting vote goes (removed, expired, watched) the exclusion is
removed. An until-watched vote is marked fulfilled when Tautulli (Plex) or Jellyfin reports the
item played by that user (polled every 15 minutes, webhooks wake it earlier).

`LeavingItem` carries `key` (Maintainerr id), `title`, `year`, `kind` (`movie`/`show`),
`poster_url`, `tmdb_id`, `tvdb_id`, `server_item_id` (the same item on the caller's server,
matched by provider ids for the other server type), `collection_*`, `deletion_date`,
`effective_deletion_date` (null when protected), `protection`
(`none`/`extended`/`until_watched`/`forever`), `my_vote` and `counts`.

### Rights

```json
{"can_keep_forever": true, "max_forever": null, "max_keep_days": null}
```
Default: unrestricted. `null` = unlimited.

### Countdown

Every item of an inactive collection may already be past its deletion date. Before activating a
collection in Maintainerr, the admin resets the countdown (`stagger_days` spreads new dates over
N days). The server never activates collections.

## Seerr bridge

Seerr serves one media server type. With `[seerr]` configured and `SEERR_API_KEY` set, the server
forwards these Seerr calls (same paths, query strings and JSON as Seerr's `/api/v1`) as the
caller's Seerr user (`X-API-User`), so Seerr enforces that user's permissions and quotas:

| Method | Seerr path |
|---|---|
| GET | `auth/me`, `search`, `movie/{id}`, `tv/{id}`, `collection/{id}`, `tv/{id}/season/{n}`, `request`, `request/{id}`, `settings/public`, `discover/{movies,tv,trending}` |
| POST | `request` |
| DELETE | `request/{id}` |

Everything else gets 403. `GET /v1/seerr` links the caller to an existing Seerr user with the
same Plex id / Jellyfin user id, or creates a local one (`create_users = true`), and returns
`{"seerr_user_id", "user_name", "public_url", "cache_images"}`. 404 when the bridge is off.
An admin can point a user at an existing Seerr user instead (`PUT /v1/admin/users/{user_key}/seerr`
with `{"seerr_user_id": 12}`, or `null` to go back to automatic linking); only the AlwyWatch
database changes. With `dry_run` on, bridged writes get 403.

## Kept forever (admins)

`GET /v1/admin/forever` → `ForeverStats`:

```json
{"titles": 40, "shared": 6,
 "people": [{"user_key": "...", "name": "...", "count": 12, "shared": 3, "alone": 9,
             "overlaps": [{"user_key": "...", "name": "...", "both": 2}]}],
 "items": [{"key": "...", "title": "...", "year": 1999, "poster_url": null, "keepers": ["<user_key>"]}]}
```
`titles`: distinct titles someone keeps forever; `shared`: of those, kept by two people or more.
`people` is sorted by most titles, `overlaps` by most titles in common, `items` by most keepers.

## Watch together

People on different servers (Plex or Jellyfin) watch the same title in sync. Everyone signed in
to the AlwyWatch server sees the open rooms; joining asks the room's host, who accepts or
declines (the same user on another device gets in at once). Rooms live in the server's memory
only. Types: `keep_service::together`.

| Method | Path | Body → answer |
|---|---|---|
| GET | `/v1/together` | → `[{code, host, media, members, playing, position_ms, at_ms, now_ms, mine}]`: open rooms, newest first |
| POST | `/v1/together` | `{media, position_ms, playing}` → `{member_id, state}`: a new room, the caller its host |
| POST | `/v1/together/{code}/knock` | → `{request_id, status, joined?}`: asks to join (`waiting`, or `accepted` at once for one's own devices) |
| GET | `/v1/together/{code}/knock/{id}/wait` | → the same, once the host answered (`accepted` with `joined = {member_id, state}`, or `declined`), else after ~25 s |
| DELETE | `/v1/together/{code}/knock/{id}` | → 204: no longer asking |
| POST | `/v1/together/{code}/requests/{id}` | `{member_id, accept}` (host only, else 403) → `state` |
| PUT | `/v1/together/{code}` | `{member_id, playing, position_ms, media?}` → `state` |
| GET | `/v1/together/{code}/wait?after={seq}&member={id}` | → `state` once `seq > after`, else after ~25 s |
| DELETE | `/v1/together/{code}/members/{id}` | → 204 |

All calls are signed in like the others; member and request ids only work for the user who got
them (403). `media` names the title by `tmdb`/`imdb`/`tvdb` (episodes: the show's ids, `season`,
`episode`) so each member plays their own server's copy. `state` = `{code, seq, media, playing,
position_ms, at_ms, now_ms, by, members, host, requests}`: the position at `at_ms` on the server
clock; clients add the time elapsed since (`now_ms` tells the server's clock) and seek when more
than 1.5 s off. `host` is the host's member id (when they leave, the longest member takes over);
`requests` lists who is asking, a new one wakes the host's `wait`. An answer is handed over once.
`wait` (and `knock/{id}/wait` for someone asking) doubles as the heartbeat: a member or request
silent for a minute goes, an empty room closes after five minutes, any room after twelve hours.
Limits: 20 rooms opened per user per hour, 30 join requests per user per ten minutes (429), 10
requests waiting per room, 20 members per room (409), 200 rooms on the server (503). Long polls: a reverse proxy must allow ~40 s requests (Caddy's
defaults do).

## Device transfer

Moves a user's sign-ins to a new device without the server being able to read them. Types:
`keep_service::transfer`. Everything lives in memory; a pending transfer lasts 10 minutes and the
bundle is dropped once picked up.

| Method | Path | Who | Body → answer |
|---|---|---|---|
| POST | `/v1/transfer` | anyone (10 per address per 10 min) | `{public_key, device}` → `{code, handle, expires_in}` |
| GET | `/v1/transfer/poll/{handle}` | the new device | 202 while waiting, then `{sealed}` once; 404 when unknown or expired |
| GET | `/v1/transfer/{code}` | user | → `{public_key, device}`; 409 once something was sent |
| PUT | `/v1/transfer/{code}` | user | `{sealed}` → 204; 409 if already sent |

1. The new device makes a one-time X25519 key pair and registers the public key (64 hex
   characters); `device` is a label (control characters removed, 80 characters max). It shows
   the six-digit `code` and polls with the secret `handle`.
2. On a signed-in device the user types the code, compares the four-digit check number both
   devices derive from the public key (`check_number`: SHA-256, first two bytes mod 10 000), and
   sends the bundle sealed to that key.
3. Sealing: ephemeral X25519 agreement, HKDF-SHA256 (salt = ephemeral key ‖ recipient key, info
   `alwywatch-transfer v1`), ChaCha20-Poly1305. `sealed` = hex of ephemeral public key ‖
   ciphertext, at most 256 KB of hex.

The server holds at most 200 pending transfers (503 beyond).

## Settings across devices

`GET /v1/me/settings` returns the caller's settings as a JSON object of strings; `PUT
/v1/me/settings` upserts some of them (204). Values are opaque to the server (the apps store
"change time ␟ value" and merge per key, newer wins). Limits: 20 000 keys per user, keys up to
256 bytes, values up to 4 KiB (400 otherwise).
