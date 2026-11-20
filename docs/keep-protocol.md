# Keep-service protocol (`/v1`)

Spoken by `alwywatch-server` and the apps (`core/keep-service` holds the shared types and the
client). JSON, `snake_case`, enums as snake_case strings, times as unix seconds.

## Authentication

Every call except `/healthz`, `/v1/servers` and webhooks sends:

```
Authorization: Bearer <token>
X-AlwyWatch-Server: <allow-listed server id>
```
* Jellyfin: the user's own Jellyfin access token. The server calls `GET /Users/Me` on that
  Jellyfin; Jellyfin administrators are AlwyWatch admins.
* Plex: the user's **plex.tv account token**. The server calls plex.tv `/api/v2/user` and
  `/api/v2/resources`; the user must have access to the allow-listed Plex server; its owner is admin.

Results are cached 10 minutes under a keyed hash; tokens are never stored or logged. The user key
is `jellyfin:<server>:<user id>` or `plex:<server>:<plex user id>`.

Errors: `{"error": "<message>"}` with 400 (invalid vote), 401 (bad token), 403 (not allowed /
server not served / rights), 404, 502 (media server or Seerr unreachable).

## Endpoints

| Method | Path | Who | Body → Response |
|---|---|---|---|
| GET | `/healthz` | anyone | `ok` |
| GET | `/v1/servers` | anyone | → `[ServerEntry]` (allow-list: `id`, `kind`, `name`, `url`) |
| GET | `/v1/me` | user | → `Me` (`user_key`, `name`, `server_id`, `is_admin`, `rights`, `forever_count`) |
| GET | `/v1/leaving-soon` | user | → `[LeavingItem]`, most urgent first |
| GET | `/v1/triage?limit=N` | user | → `[LeavingItem]` not voted on yet and not protected (swipe mode; default 20, max 200) |
| PUT | `/v1/votes/{key}` | user | `VoteRequest` → `LeavingItem` (replaces the caller's vote) |
| DELETE | `/v1/votes/{key}` | user | → `LeavingItem` |
| GET | `/v1/admin/users` | admin | → `[AdminUser]` |
| PUT | `/v1/admin/users/{user_key}/rights` | admin | `Rights` → `AdminUser` |
| POST | `/v1/admin/collections/{id}/countdown` | admin | `{"stagger_days": N}` → `{"reset": n, "skipped": n}` |
| GET | `/v1/seerr` | user | → `SeerrBridgeInfo` (links or creates the caller's Seerr user) |
| any | `/v1/seerr/api/v1/{path}` | user | Seerr API, allow-listed (see below) |
| POST | `/v1/hooks/{tautulli\|jellyfin\|plex}/{secret}` | webhook | wakes the "watched" check → 204 |

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
| GET | `auth/me`, `search`, `movie/{id}`, `tv/{id}`, `tv/{id}/season/{n}`, `request`, `request/{id}`, `settings/public`, `discover/{movies,tv,trending}` |
| POST | `request` |
| DELETE | `request/{id}` |

Everything else gets 403. `GET /v1/seerr` links the caller to an existing Seerr user with the
same Plex id / Jellyfin user id, or creates a local one (`create_users = true`), and returns
`{"seerr_user_id", "user_name", "public_url", "cache_images"}`. 404 when the bridge is off.

## Watch together

People on different servers (Plex or Jellyfin) watch the same title in sync. Rooms live in the
server's memory only. Types: `keep_service::together`.

| Method | Path | Body → answer |
|---|---|---|
| POST | `/v1/together` | `{media, position_ms, playing}` → `{member_id, state}`: a new room, six-digit `state.code` |
| POST | `/v1/together/{code}/join` | → `{member_id, state}` |
| PUT | `/v1/together/{code}` | `{member_id, playing, position_ms, media?}` → `state` |
| GET | `/v1/together/{code}/wait?after={seq}&member={id}` | → `state` once `seq > after`, else after ~25 s |
| DELETE | `/v1/together/{code}/members/{id}` | → 204 |

All calls are signed in like the others; a member id only works for the user who got it (403).
`media` names the title by `tmdb`/`imdb`/`tvdb` (episodes: the show's ids, `season`, `episode`)
so each member plays their own server's copy. `state` = `{code, seq, media, playing,
position_ms, at_ms, now_ms, by, members}`: the position at `at_ms` on the server clock; clients
add the time elapsed since (`now_ms` tells the server's clock) and seek when more than 1.5 s off.
`wait` doubles as the heartbeat: a member silent for a minute leaves, an empty room closes after
five minutes, any room after twelve hours. Long polls: a reverse proxy must allow ~40 s requests
(Caddy's defaults do).
