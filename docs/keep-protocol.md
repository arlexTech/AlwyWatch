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
| 400 | Invalid vote, setting too long or too many, malformed transfer, invalid display name, bad avatar (type, bytes, unknown preset), 0 or more than 20 invitations at once |
| 401 | Missing bearer token or `X-AlwyWatch-Server`; token refused by the media server |
| 403 | Server not served here; not an admin; rights do not allow the vote; Seerr path not bridged; no Seerr account; read-only test mode (`dry_run`); in a room you are in: someone else's seat or a host-only action |
| 404 | Unknown item, user, room or transfer code; a room you are neither in nor invited to; no custom avatar; Seerr bridge or webhooks off; wrong webhook secret |
| 409 | Room full; transfer code already used |
| 413 | Avatar picture over 512 KiB |
| 429 | Too many failed sign-ins, room creations, invitations, profile changes or transfer starts |
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
| GET | `/v1/me` | user | → `Me` (`user_key`, `name`, `media_name`, `name_is_custom`, `avatar`, `profile_set`, `server_id`, `is_admin`, `rights`, `forever_count`) |
| … | `/v1/me/name`, `/v1/me/avatar`, `/v1/me/profile-done`, `/v1/users/{user_key}/avatar` | user | [Profile](#profile-display-name-avatar-welcome-step) |
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

`AdminUser` = `user_key`, `name`, `media_name`, `avatar`, `server_id`, `is_admin`, `rights`,
`forever_count`, plus `last_seen` (unix seconds) and `seerr_user_id` (null: linked
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
 "people": [{"user_key": "...", "name": "...", "media_name": "...", "avatar": {"preset": null, "custom_version": 3},
             "count": 12, "shared": 3, "alone": 9,
             "overlaps": [{"user_key": "...", "name": "...", "both": 2}]}],
 "items": [{"key": "...", "title": "...", "year": 1999, "poster_url": null, "keepers": ["<user_key>"]}]}
```
`titles`: distinct titles someone keeps forever; `shared`: of those, kept by two people or more.
`people` is sorted by most titles, `overlaps` by most titles in common, `items` by most keepers.

## Watch together

People on different servers (Plex or Jellyfin) watch the same title in sync, in a **private
room** on the AlwyWatch server. A room starts as a **lobby** (title chosen, nothing playing);
its host invites people from the server's user list; invited people join directly (the
invitation is the approval) or decline; the host starts playback when ready. Nobody else can
learn that a room exists. Rooms live in the server's memory only; the "who watched with whom"
history used to order the people list is kept in SQLite. Types: `keep_service::together`. App
flows: [together-apps.md](together-apps.md).

### Routes

All signed in like every other call. `{code}` is the room's six digits; `{member}` a member id
(one seat = one device); `{user_key}` is percent-encoded (`:` → `%3A`).

| Method | Path | Who | Body → answer |
|---|---|---|---|
| GET | `/v1/together` | user | → `TogetherRooms` |
| GET | `/v1/together/wait?after={version}` | user | → `TogetherRooms` as soon as its `version` ≠ `after`, else after ~25 s |
| GET | `/v1/together/people` | user | → `[TogetherPerson]` |
| POST | `/v1/together` | user | `TogetherCreate` → `TogetherJoined`: a new lobby, the caller its host |
| POST | `/v1/together/{code}/invites` | host | `TogetherInvite` → `TogetherState` |
| DELETE | `/v1/together/{code}/invites/{user_key}?member={member}` | host | → `TogetherState`: invitation withdrawn |
| DELETE | `/v1/together/{code}/invites/me` | invited user | → 204: declined |
| POST | `/v1/together/{code}/join` | invited user, or a member's other device | → `TogetherJoined` |
| PUT | `/v1/together/{code}` | member (lobby: host) | `TogetherUpdate` → `TogetherState` |
| GET | `/v1/together/{code}/wait?after={seq}&member={member}` | member | → `TogetherState` as soon as `seq` > `after`, else after ~25 s |
| DELETE | `/v1/together/{code}/members/{member}` | member | → 204 |

"Host" = any seat of the host's **user** (the host on their phone and TV both have the host's
rights). The old knock routes (`/knock…`, `/requests/{id}`) are gone (404).

### Bodies

```
TogetherMedia   {kind: "movie"|"episode", title, year?, tmdb?, imdb?, tvdb?,
                 series_title?, series_tmdb?, series_tvdb?, season?, episode?}
TogetherCreate  {media}
TogetherInvite  {member_id, user_keys: [user_key]}            // 1 to 20 at once
TogetherUpdate  {member_id, playing, position_ms, media?, started = false}
TogetherJoined  {member_id, state}
TogetherState   {code, seq, media, started, playing, position_ms, at_ms, now_ms, by,
                 members: [TogetherMember], host, invited: [TogetherInvitee]}
TogetherMember  {id, name, user_key, avatar}
TogetherInvitee {user_key, name, avatar}
TogetherRooms   {version, rooms: [TogetherRoom]}
TogetherRoom    {code, host, host_avatar, media, members: [name], started, playing,
                 position_ms, at_ms, now_ms, mine, hosting, invited}
TogetherPerson  {user_key, name, media_name, avatar, server_name, last_together_at?}
```

* `media` names the title by `tmdb`/`imdb`/`tvdb` (episodes: the show's ids, `season`,
  `episode`) so each member plays their own server's copy. Text is cut to 200 characters, ids
  to 40, control characters removed.
* `TogetherState`: `seq` grows on every change (playback, title, start, joins, leaves,
  invitations, declines, host handover). `started` is false in a lobby and, once true, never
  goes back. `playing` is always false in a lobby. `position_ms` is the position at `at_ms`
  (server clock, Unix ms); clients add the time elapsed since (`now_ms` is the server clock when
  the answer was made) and seek when more than 1.5 s off (`TogetherState::position_at`).
  `by` = member id that made the change, empty for the server's changes (joins, leaves,
  invitations): a player ignores its own echo by comparing `by` with its member id. `members`
  is sorted by name (a user on two devices appears twice, same `user_key`); `host` is the
  host's member id (`TogetherState::is_host(member)` tells whether a seat has the host's
  rights); `invited` lists people invited who are not in the room, sorted by name.
* `TogetherRoom` (list entry): `host` = host's display name, `members` = names, `mine` = the
  caller is in it on some device, `hosting` = the caller is the host's user, `invited` = the
  caller is invited and not in it (exactly `!mine`).
* `TogetherRooms.version`: opaque number in [1, 2^53), changes whenever the caller's list
  changes in a way they can see (invited, uninvited, a room started, closed or emptied, title,
  host or members changed); playback alone does not change it. Order: invitations first, then
  the caller's rooms; newest first within each.
* `TogetherPerson`: everyone who has signed in to this AlwyWatch server through a server still
  in the allow-list, except the caller. `name` = display name, `media_name` = media-server name,
  `server_name` = allow-list name of their server (names are not unique: show them when two
  names match). Order: people the caller was in a **started** room with, most recent
  `last_together_at` (unix seconds) first; then everyone else by name, case-insensitive.

### Lifecycle

1. **Create.** `POST /v1/together {media}` → a lobby (`started: false`, `playing: false`,
   `position_ms: 0`), the caller's seat as host. It appears only in the caller's list.
2. **Invite.** The host posts user keys. Unknown users (or users of a server no longer served)
   → 404 `unknown user` and nobody is invited. Already invited or present users and the host
   themselves are skipped (idempotent). Each new invitation bumps `seq` (members' `wait`
   returns) and wakes the invitee's list long poll (`invited: true` entry). Withdrawing
   (host) removes the invitation; it does not remove someone who already joined (they just
   cannot come back after leaving).
3. **Join / decline.** An invited user joins with `POST …/join`: a seat at once, no approval.
   The same user as an existing member (another device) joins the same way without an
   invitation. Joining a lobby → Room screen; joining a started room → the player in sync.
   `DELETE …/invites/me` declines (404 when there is no pending invitation); the host sees the
   name leave `invited`.
4. **Lobby rules.** Before the start only the host's user may `PUT`: to change the title
   (`media`) and the start position; `playing` is stored as false. A `PUT` from any other
   member → 403.
5. **Start.** The host sends `PUT {member_id, playing: true, position_ms, started: true}`
   (`started: false` never changes anything). From then on every member may play, pause,
   seek or change the title (`media`), as in a normal synced session. Starting with two or more
   distinct users in the room, and every later join of a started room, records the pairs of
   users in the history (`last_together_at`).
6. **Leave.** `DELETE …/members/{member}`. When a user's last seat leaves on purpose, their
   right to come back goes too (a new invitation is needed). When the host leaves, the
   longest-present member becomes host (lobby included: they can then invite and start);
   invitations stay valid.
7. **Heartbeat.** The room's `wait` doubles as the seat's heartbeat: a seat that does not poll
   for 60 s is **dropped** (not "left": that user may `join` again, even while the room is
   empty, for five minutes). On a 404 from `wait`/`PUT` with a seat that used to work, call
   `join` once; if that is 404 too, the room is gone.
8. **Closing.** A room with nobody in it is hidden from every list at once and cannot be
   joined by invitees. It is removed as soon as nobody may come back (everyone left on
   purpose: e.g. a lobby whose host leaves alone closes immediately, its invitations vanish
   from the invitees' lists), else after five minutes empty. Any room closes after twelve
   hours.

### Privacy and errors

A caller who is neither in the room (on any device) nor invited gets **404** on every room
route, exactly as for a code that does not exist (also with a correct code and member id).
403 is only for members: someone else's seat, or a host-only action (invite, withdraw, start,
lobby change) from a non-host. 409: room full. 400: `user_keys` empty or more than 20.

### Limits

| What | Limit |
|---|---|
| Rooms opened | 20 per user per hour (429) |
| Invitations | 60 per user per ten minutes, each user key in a request counts (429) |
| People per room | 20 seats + pending invitations (409); an invitee's seat replaces their invitation |
| Rooms on the server | 200 (503) |
| Long polls | ~25 s (`WAIT_SECS`); clients use a ~40 s request timeout, a reverse proxy must allow it (Caddy's defaults do) |

Avatars and names in rooms are captured when the person joins or is invited (a change made
during a room shows from the next room on).

## Profile: display name, avatar, welcome step

Every AlwyWatch user (`user_key`) may choose a display name and an avatar, shown to others
instead of the media-server name: room members, invited people and host, the people list,
admin users, kept-forever stats, Seerr-link pickers and "Signed in as …". Types:
`keep_service::profile`. App flows: [profile-apps.md](profile-apps.md).

| Method | Path | Body → answer |
|---|---|---|
| PUT | `/v1/me/name` | `{"name": "…"}` → `Me` |
| DELETE | `/v1/me/name` | → `Me` (back to the media-server name) |
| PUT | `/v1/me/avatar` | JSON `{"preset": "p01"}`, or a raw picture with `Content-Type: image/jpeg`, `image/png` or `image/webp` → `Me` |
| DELETE | `/v1/me/avatar` | → `Me` (back to the default avatar) |
| GET | `/v1/users/{user_key}/avatar?v={custom_version}` | → the custom picture (any signed-in user); 404 when none |
| POST | `/v1/me/profile-done` | → `Me` with `profile_set: true` (idempotent) |

`Me` = `{user_key, name, media_name, name_is_custom, avatar, profile_set, server_id,
is_admin, rights, forever_count}`. `name` is the chosen display name if set, else the
media-server name (`media_name`); every response that names a person follows the same rule.
The media-server name is refreshed at each sign-in and never overwrites a chosen name.
`Avatar` = `{preset: string?, custom_version: u32?}`, both null = default; it appears next to
`name` in `Me`, `AdminUser`, `ForeverPerson`, `TogetherMember`, `TogetherInvitee`,
`TogetherPerson` and as `TogetherRoom.host_avatar`. `AdminUser`, `ForeverPerson` and
`TogetherPerson` also carry `media_name` (names are not unique).

**Display name.** Trimmed, inner whitespace collapsed to one space, 1 to 32 characters, no
control characters or line breaks (`profile::clean_display_name`); otherwise 400 with a
plain-words message ("the name cannot be empty", "the name is too long (32 characters at
most)", "the name cannot contain line breaks or control characters").

**Custom avatar.** The app crops a square and uploads at most 512×512 pixels. The server reads
at most 512 KiB of the body (413 beyond), accepts only the three content types (400 otherwise)
and checks the bytes really are that type (JPEG `FF D8 FF`, PNG `89 50 4E 47 0D 0A 1A 0A`,
WebP `RIFF` + 4 bytes + `WEBP`; 400 otherwise). It is stored as a file under the data dir
(`avatars/<sha256 of user_key>`) with its type in the database. `GET` answers with that type
only, plus `Cache-Control: private, max-age=31536000, immutable`, `X-Content-Type-Options:
nosniff` and `Content-Security-Policy: default-src 'none'`. Every avatar change (preset,
picture, removal) bumps the user's version, so `custom_version` in a URL never serves stale
bytes: cache by (user_key, version). An unknown preset id → 400.

**Limits.** Name and avatar changes together: 20 per user per ten minutes (429).

**Welcome step.** `profile_set` is false for every user until `POST /v1/me/profile-done`;
setting a name or avatar does not set it. Once set it holds for that account on every device.

### Built-in avatars

Drawn by the apps (the server never serves them): a filled disc of `background`, and the shape
in `foreground` (`#FFFFFF` for all), clipped to the disc. Coordinates are in a unit box
[-1, 1]², centred, scaled so 1 unit = 25 % of the avatar's diameter (the shape spans the middle
half); y grows downwards.

| id | background | shape | geometry (unit box) |
|---|---|---|---|
| p01 | `#C62828` | circle | filled disc, centre (0,0), radius 1 |
| p02 | `#AD1457` | ring | circle centre (0,0), radius 0.75, stroke width 0.5 |
| p03 | `#6A1B9A` | square | filled square (-0.85,-0.85)–(0.85,0.85) |
| p04 | `#4527A0` | diamond | polygon (0,-1) (1,0) (0,1) (-1,0) |
| p05 | `#1565C0` | triangle | polygon (0,-1) (0.95,0.7) (-0.95,0.7) |
| p06 | `#00838F` | plus | rectangles (-1,-0.3)–(1,0.3) and (-0.3,-1)–(0.3,1) |
| p07 | `#2E7D32` | cross | the plus rotated 45° about (0,0) |
| p08 | `#9E6A00` | star | 5-pointed star, outer radius 1, inner radius 0.45, first point straight up |
| p09 | `#D84315` | hexagon | regular hexagon, radius 1, vertices at angles -90° + k·60° (point up) |
| p10 | `#4E342E` | bars | rectangles (-1,-0.8)–(1,-0.4), (-1,-0.2)–(1,0.2), (-1,0.4)–(1,0.8) |
| p11 | `#37474F` | half | upper half of the disc of radius 1 centred (0,0.25) (flat side down) |
| p12 | `#00695C` | dots | discs of radius 0.35 centred (±0.5, ±0.5) |

`keep_service::profile::avatar_presets()` (`avatar_presets()` in alwywatch-core) returns this
table.

**Default avatar** (no preset, no picture): the first letter or digit of the display name,
uppercased (`?` if none), white, bold, about 45 % of the diameter, centred on a disc of
`DEFAULT_COLORS[fnv1a32(utf8(name)) % 8]` with `DEFAULT_COLORS` = `#C62828 #AD1457 #6A1B9A
#1565C0 #00838F #2E7D32 #D84315 #4E342E` and FNV-1a 32 bit (offset 0x811C9DC5, prime
0x01000193). Use `default_avatar_color(name)` / `avatar_initial(name)` rather than
reimplementing.

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
