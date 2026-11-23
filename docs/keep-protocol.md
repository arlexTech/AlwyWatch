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

The server id must be in the allow-list (the media servers in Server settings, the same list
`/v1/servers` returns). An [AlwyWatch account](#alwywatch-accounts) session is the other way
in (`Bearer aws_…`, no `X-AlwyWatch-Server`), for the account itself, the profile, settings and
Quick Connect only. Media-server results are cached 10
minutes under a keyed hash of (server, token); tokens are never stored or logged. The user key is
`jellyfin:<server>:<user id>` or `plex:<server>:<plex user id>`. Too many rejected tokens from
one address (20 in 10 minutes) get 429 before the media server is asked.

Right after validation the signed-in user key is resolved to its **person**: the canonical user
key of the AlwyWatch account it belongs to (see [Linked identities](#linked-identities)). Every
`user_key` the server returns or stores (`Me`, votes, rights, rooms, people, settings, Seerr
link) is a canonical key; an alias key sent by a client (invitations, admin routes, avatar URLs)
is resolved the same way. A user who never linked anything is a one-identity person whose
canonical key is their own user key, so nothing changes for them.

## Errors

JSON `{"error": "<message>"}` (`keep_service::ApiError`) with:

| Status | When |
|---|---|
| 400 | Invalid vote, setting too long or too many, malformed transfer, invalid display name, bad avatar (type, bytes, unknown preset), 0 or more than 20 invitations at once |
| 401 | Missing bearer token or `X-AlwyWatch-Server`; token refused by the media server (also the second token of a link) |
| 403 | Server not served here; not an admin; rights do not allow the vote; Seerr path not bridged; no Seerr account; read-only test mode (`dry_run`); in a room you are in: someone else's seat or a host-only action |
| 404 | Unknown item, user, room or transfer code; a room you are neither in nor invited to; no custom avatar; Seerr bridge or webhooks off; wrong webhook secret |
| 409 | Room full; transfer code already used; linking a second, different sign-in on a server the account already has one on; unlinking the canonical sign-in |
| 413 | Avatar picture over 512 KiB |
| 429 | Too many failed sign-ins, room creations, invitations, profile changes, transfer starts or link attempts |
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
| GET | `/v1/me` | user | → `Me` (`user_key`, `name`, `media_name`, `name_is_custom`, `avatar`, `profile_set`, `server_id`, `is_admin`, `rights`, `forever_count`, `services`, `identities`) |

`services` = `{leaving_soon, requests}`: `leaving_soon` is false once a Maintainerr sync has
failed (true until the first one runs), `requests` is false when the server has no Seerr link.
Apps hide Leaving soon (and the keep panel) and the Requests page they reach through this
server accordingly. Missing (older servers): both true.
| GET | `/v1/me/links` | user | → `[LinkedIdentity]` ([Linked identities](#linked-identities)) |
| POST | `/v1/me/links` | user | `LinkRequest` → `[LinkedIdentity]` |
| DELETE | `/v1/me/links/{user_key}` | user | → `[LinkedIdentity]` |
| … | `/v1/me/name`, `/v1/me/avatar`, `/v1/me/profile-done`, `/v1/users/{user_key}/avatar` | user | [Profile](#profile-display-name-avatar-welcome-step) |
| GET | `/v1/me/settings` | user | → `{key: value}` (see [Settings across devices](#settings-across-devices)) |
| PUT | `/v1/me/settings` | user | `{key: value}` → 204 |
| GET | `/v1/leaving-soon` | user | → `[LeavingItem]`, most urgent first |
| GET | `/v1/triage?limit=N` | user | → `[LeavingItem]` not voted on yet and not protected (swipe mode; default 20, max 200) |
| PUT | `/v1/votes/{key}` | user | `VoteRequest` → `LeavingItem` (replaces the caller's vote) |
| DELETE | `/v1/votes/{key}` | user | → `LeavingItem` |
| POST | `/v1/keep-entry` | user | `TitleLookup` → `LeavingItem` of a movie or show that is in no deletion list, to vote on it ahead (see below); 404 when the library Maintainerr manages doesn't have it |
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
(`none`/`extended`/`until_watched`/`forever`), `my_vote`, `counts` and `listed` (in a
deletion list right now).

#### Keeping a title before it is in a list

A title's page offers the same choices when the title is in no deletion list. The client sends
`TitleLookup` (`item_id` on the caller's server, `kind` `movie`/`show`, `title`, `tmdb_id`,
`tvdb_id`); the server finds the title on the server Maintainerr manages: by `item_id` when the
caller uses that server (checked with Maintainerr's `GET /api/media-server/meta/{id}`), else by
TMDB/TVDB id in that server's libraries (`GET /api/media-server/libraries`, then
`library/{id}/content` page by page, kept an hour; not by name: the same film is "Sœurs…" on
one server and "Soeurs…" on the other). It remembers the item and answers its
`LeavingItem` (`listed: false`, no date), whose `key` takes votes like any other.

- *Forever* and *until watched* add the usual global exclusion, so the title never enters a list.
- *Keep N days* has nothing to postpone yet: `protection` stays `none`; if the title gets into a
  list before the N days are over, its date is pushed to then.
- *I don't care* is recorded.

`/v1/leaving-soon` lists such a title once someone keeps it, and to its voter whatever the
choice (for the title's page). The apps' Leaving soon lists show only what is `listed` or kept
(forever / until watched).

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
invitation is the approval) or decline; the host starts playback when ready. When the host
leaves the player the room goes back to the lobby and everyone stays in it: the host picks
another title and starts again. Each start waits until every player has loaded the title,
then plays at the same server instant everywhere. Nobody else can
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
| POST | `/v1/together/{code}/ready` | member | `TogetherReady` → `TogetherState`: this seat has loaded the round's title |
| GET | `/v1/together/clock` | anyone (no sign-in) | → `TogetherClock`: the server clock, for the clock sync |
| DELETE | `/v1/together/{code}/members/{member}` | member | → 204 |

"Host" = any seat of the host's **user** (the host on their phone and TV both have the host's
rights). The old knock routes (`/knock…`, `/requests/{id}`) are gone (404).

### Bodies

```
TogetherMedia   {kind: "movie"|"episode", title, year?, tmdb?, imdb?, tvdb?,
                 series_title?, series_tmdb?, series_tvdb?, season?, episode?}
TogetherCreate  {media, rights = all true}
TogetherRights  {change_title, play_pause, seek}
TogetherInvite  {member_id, user_keys: [user_key]}            // 1 to 20 at once
TogetherUpdate  {member_id, playing, position_ms, media?, started = false, lobby = false}
TogetherReady   {member_id, round}
TogetherClock   {now_ms}
TogetherJoined  {member_id, state}
TogetherState   {code, seq, media, started, playing, position_ms, at_ms, now_ms,
                 round, loading: [member id], ready_by_ms, by,
                 members: [TogetherMember], host, invited: [TogetherInvitee], rights}
TogetherMember  {id, name, user_key, avatar}
TogetherInvitee {user_key, name, avatar}
TogetherRooms   {version, rooms: [TogetherRoom]}
TogetherRoom    {code, host, host_avatar, media, members: [name], started, playing,
                 position_ms, at_ms, now_ms, mine, hosting, invited}
TogetherPerson  {user_key, name, media_name, avatar, server_name, last_together_at?}
```

* `rights`: what members other than the host's user may do once the room plays, set by the
  creator (their Settings → Watch together, synced as `together.others_title|play|seek`).
  A refused update answers **423** (not 403, which clients read as a lost sign-in): another
  title when `change_title` is false, playing ≠ the room's when `play_pause` is false, a
  position more than 3 s from the room's when `seek` is false (`SEEK_TOLERANCE_MS`). Apps then
  say "Only the host can …" and go back in step with the room.
* `media` names the title by `tmdb`/`imdb`/`tvdb` (episodes: the show's ids, `season`,
  `episode`) so each member plays their own server's copy. Text is cut to 200 characters, ids
  to 40, control characters removed.
* `TogetherState`: `seq` grows on every change (playback, title, start, joins, leaves,
  invitations, declines, host handover, loading). `started` is false in a lobby (at first, and
  again after the host's `lobby: true`). `playing` is always false in a lobby and while
  `loading` is not empty. `position_ms` is the position at `at_ms` (server clock, Unix ms);
  `at_ms` may be up to a second ahead (a start every seat hears before it plays: hold the
  position until then, `TogetherState::starts_in`). Clients read the server's clock through
  `ClockSync` (a few `GET /v1/together/clock` round trips, the quickest one wins, again every
  two minutes; within a few ms) and follow `TogetherState::position_at(server_now)`
  continuously: slightly faster or slower from 60 ms apart until under 20 ms, a seek past 2 s
  (`together_correction`). `round` grows at each start; `loading` lists the seats still
  loading it (see Lifecycle 5).
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
* `TogetherPerson`: every person (once, by canonical key, however many sign-ins they linked)
  who has signed in to this AlwyWatch server through a server still in the allow-list, except
  the caller. `name` = display name, `media_name` = media-server name,
  `server_name` = allow-list name of their server (names are not unique: show them when two
  names match). Order: people the caller was in a **started** room with, most recent
  `last_together_at` (unix seconds) first; then everyone else by name, case-insensitive.

### Lifecycle

A room may be created with an empty title (`media.title == ""`, the other fields empty: a room opened before choosing what to watch). It behaves like any lobby, except that starting it answers 409 until an update carries a `media`.

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
   (`started: false` never changes anything). That begins a **round**: `round` + 1, `loading`
   = every seat (and anyone joining meanwhile), `playing: false`, `ready_by_ms` = now + 20 s.
   Each player loads the title paused at `position_ms` and, once `READY_BUFFER_MS` (5 s) of it
   is buffered (or the rest of it), sends `POST …/ready {member_id, round}`. When the last
   seat is ready, or at `ready_by_ms`, the room plays from `at_ms` = now + 1 s for everyone at
   once (each app presses play at that instant on the server's clock). Another title while
   the room plays (`media` in a `PUT`) is a new round the same way. Any other `PUT` during the
   wait ends it (`playing` as sent). From then on every member may play, pause, seek or change
   the title, within the host's `rights`. Starting with two or more distinct users in the
   room, and every later join of a started room, records the pairs of users in the history
   (`last_together_at`).
6. **Back to the lobby.** The host closing their player sends `PUT {lobby: true}`: `started`
   false, nothing plays, everyone stays (the others' players close and show the Room screen).
   The host may then change the title (`media`, `playing: false`) and start again: a new
   round, which opens everyone's player wherever they are in the app. Non-hosts sending
   `lobby` → 403. Members closing their own player stay in the room.
7. **Leave.** `DELETE …/members/{member}`. When a user's last seat leaves on purpose, their
   right to come back goes too (a new invitation is needed). When the host leaves, the
   longest-present member becomes host (lobby included: they can then invite and start);
   invitations stay valid.
8. **Heartbeat.** The room's `wait` doubles as the seat's heartbeat: a seat that does not poll
   for 60 s is **dropped** (not "left": that user may `join` again, even while the room is
   empty, for five minutes). On a 404 from `wait`/`PUT` with a seat that used to work, call
   `join` once; if that is 404 too, the room is gone.
9. **Closing.** A room with nobody in it is hidden from every list at once and cannot be
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
is_admin, rights, forever_count, identities}` (`user_key` is the canonical key, `server_id` the
server this request signed in to, `identities` = `[LinkedIdentity]`, see
[Linked identities](#linked-identities)). `name` is the chosen display name if set, else the
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
| GET | `/v1/transfer/poll/{handle}` | the new device | 202 while waiting, then `{sealed, account_token}` once; 404 when unknown or expired |
| GET | `/v1/transfer/{code}` | user or account session | → `{public_key, device}`; 409 once answered |
| PUT | `/v1/transfer/{code}` | user (media sign-in) | `{sealed}` → 204; 409 if already answered |
| PUT | `/v1/transfer/{code}/approve` | user or account session | → `{device}`: Quick Connect without sign-ins to send (the web) |

Quick Connect: either answer also gives the new device a session of the approver's AlwyWatch
account (`account_token`; an account without a way to sign in is made for a person who had
none). `sealed` is empty after an approval. The apps show the code with a QR code of
`<public URL>/q/<code>`, which opens the approval page of the web app.

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

## AlwyWatch accounts

A way for a person to sign in without a media server: username + password, passkeys or Pocket
ID. It belongs to a person like a linked sign-in does; a person with only an account is
`account:<id>`, and linking a Jellyfin/Plex sign-in merges as in
[Linked identities](#linked-identities) (the account follows the canonical person). An account
alone reaches its profile, synced settings (the apps keep the user's server list there) and
Quick Connect; votes, rooms and Seerr need a media sign-in. Code: `server/src/accounts.rs`,
`account_api.rs`, `passkey.rs`, `oidc.rs`.

Server environment: `ALWYWATCH_PUBLIC_URL` (needed for passkeys and Pocket ID),
`ALWYWATCH_REGISTRATION` = `invite` (default) | `open` | `closed`,
`ALWYWATCH_POCKETID_ISSUER`, `_CLIENT_ID`, `_CLIENT_SECRET` (callback:
`<public URL>/v1/account/oidc/callback`).

| Method | Path | Who | Body → answer |
|---|---|---|---|
| GET | `/v1/account/options` | anyone | → `{registration, passkeys, pocketid}` |
| POST | `/v1/account/register` | anyone | `{username, password, invite?, device}` → `{token, account}` |
| POST | `/v1/account/login` | anyone | `{username, password, device}` → `{token, account}`; 401 |
| POST | `/v1/account/logout` | session | 204 |
| POST | `/v1/account/delete` | session | `{password?}` (required when the account has one, 403 otherwise) → 204. Deletes the account, its sessions, passkeys and Pocket ID link. A person who only existed through the account (no Jellyfin/Plex sign-in) is deleted with it: profile, avatar, synced settings. A person with media sign-ins stays as before the account; their keep requests and profile belong to those sign-ins |
| GET | `/v1/account` | session | → account (username, name, user_key, has_password, passkeys, pocketid, sessions, linked) |
| PUT | `/v1/account/password` | session | `{current?, new}` → 204; other sessions end |
| PUT | `/v1/account/username` | session | `{username}` → 204; 409 taken |
| DELETE | `/v1/account/sessions/{id}` | session | signs a device out |
| POST | `/v1/account/link` | session | `{server_id, token}` → account: merges that media sign-in |
| POST | `/v1/account/claim` | user (media) | `{username, password}` → account: the person can sign in without the media server |
| POST | `/v1/account/invites` | admin | `{days?, uses?}` → `{code, url, expires_in_days, uses}` |
| POST | `/v1/account/passkeys/register/start` · `/finish` | session | → `{flow_id, options}` · `{flow_id, name?, credential}` |
| POST | `/v1/account/passkeys/login/start` · `/finish` | anyone | → `{flow_id, options}` · `{flow_id, device, credential}` → `{token, account}` |
| PUT · DELETE | `/v1/account/passkeys/{id}` | session | rename `{name}` · remove |
| GET | `/v1/account/oidc/start?next=` | browser | → Pocket ID → `/v1/account/oidc/callback` → `/account/signed-in#token=…&next=…`. `start` sets an `aw_oidc` cookie (HttpOnly, SameSite=Lax) and the callback refuses to go on without it: a flow only ends in the browser that started it |
| POST | `/v1/account/oidc/ticket` | session | → `{ticket}`; `start?ticket=` links Pocket ID to that account |
| POST | `/v1/account/oidc/link` | session | `{id}` from `/account?link=<id>` (where the callback lands when linking): the account confirms, only then is Pocket ID linked. 403 from another account, 400 expired |
| DELETE | `/v1/account/oidc` | session | unlinks Pocket ID |
| PUT | `/v1/account/media-sign-in` | session | `{allowed}`: whether a Jellyfin/Plex sign-in of the person may act on the account. Off: `claim` and Quick Connect approval by a media sign-in answer 403, and a device transfer moves the media sign-ins without an account session. Everything else a media sign-in does is unchanged. Turning it off needs a password, passkey or Pocket ID (409). Shown as `media_sign_in` in the account view; the web app proposes it once the account has its own way in, never forces it |

* Usernames: 3–32 of `A-Z a-z 0-9 . _ -`, unique ignoring case. Passwords: 8–256
  characters, Argon2id. Session tokens (`aws_` + 64 hex) and invitation codes are stored as
  SHA-256 only; a session unused for 180 days ends; 50 sessions per account at most.
* Sign-up needs an invitation (single use by default, 7 days) unless registration is `open`.
  A first Pocket ID sign-in creates the account without one (Pocket ID users are the
  owner's own).
* Passkeys: discoverable credentials, attestation `none`, ES256 / EdDSA / RS256, checked with
  `ring` (challenge, origin = public URL, RP id hash, user present, signature, counter).
  `credential` fields are base64url as in the browser's `PublicKeyCredential`.
* The last way into an account with no media sign-in cannot be removed (409).
* Limits: 10 failed sign-ins per address and per username in 10 minutes; 5 sign-ups per
  address per hour.

## Settings across devices

`GET /v1/me/settings` returns the caller's settings as a JSON object of strings; `PUT
/v1/me/settings` upserts some of them (204). Values are opaque to the server (the apps store
"change time ␟ value" and merge per key, newer wins). Limits: 20 000 keys per user, keys up to
256 bytes, values up to 4 KiB (400 otherwise).

## Linked identities

One person, one AlwyWatch account, whatever they sign in with: a person's Plex user and Jellyfin
user (or two Jellyfin users on two servers) can be **linked**. A person is a set of identities
(media-server users); one is **canonical**: all of the account's data is stored under its user
key, the others are aliases. Every request is resolved to the canonical key right after its
token is validated (see [Authentication](#authentication)), so votes, profile, rights, settings,
Seerr link, rooms and the people list are the person's on every device. Types:
`keep_service::identity`. App flows: [identity-apps.md](identity-apps.md).

### Routes

| Method | Path | Body → answer |
|---|---|---|
| GET | `/v1/me/links` | → `[LinkedIdentity]` |
| POST | `/v1/me/links` | `LinkRequest` → `[LinkedIdentity]` |
| DELETE | `/v1/me/links/{user_key}` (percent-encoded) | → `[LinkedIdentity]` |

```
LinkRequest    {server_id, token}
LinkedIdentity {user_key, server_id, server_name, kind: "jellyfin"|"plex", media_name,
                canonical, current}
```

* `LinkedIdentity`: `server_name` is the allow-list name (the id when the server is no longer
  served); `canonical`: the account's data is stored under this key (it is `Me.user_key`);
  `current`: the sign-in this request was made with. Canonical first, then oldest link first.
  `Me.identities` carries the same list (one entry: nothing linked).
* **Link** (`POST`): signed in as identity A (the usual headers), the body names a second
  allow-listed server and **that server's token for identity B** (Jellyfin access token, or
  plex.tv account token), so the caller proves they hold both. The token is validated exactly
  like a sign-in (same cache, a refusal counts towards the per-address failed sign-in limit) and
  is never stored nor logged. Answers: 200 with the merged list; 401 the second token is refused
  (nothing changes); 403 the server is not served here; 409 the two accounts each have a
  different sign-in on the same server (unlink one first: two users of one server are never one
  person); 429 more than **10 link attempts per person per ten minutes** (counted before the
  media server is asked); 502 media server unreachable. **Idempotent**: when A and B already
  are one person (or are the same user) nothing changes and the answer is 200. B may belong to
  a person with other sign-ins: both persons merge. After a link, read `Me` again: `user_key`
  may have changed.
* **Unlink** (`DELETE`): the named sign-in (one of the caller's) leaves the account and becomes
  its own account again, **with fresh, empty data** (no votes, name, avatar, settings, rights,
  Seerr link or history). The canonical sign-in cannot be unlinked (409 with an explanation:
  unlink the others instead); a key that is not one of the caller's: 404. The caller may unlink
  the sign-in they are using.

### Canonical rule

When two persons merge, the canonical key is the first of: A's canonical key, then B's person's
canonical key, then any other identity of the two, **that is a Jellyfin user on a server still
served**; when none is, A's canonical key. Jellyfin (open source, the server people move to)
holds the data whenever it can, so stopping the Plex server never strands an account. Data is
moved to the new canonical key in the same transaction. Canonical-ness does not depend on the
server still being served: a person whose canonical key is on a server removed from the
allow-list keeps working through any alias still served (they keep their key, show in the
people list under a served sign-in's server name, and can be invited).

### Merge rules

Everything happens in **one SQLite transaction** (all or nothing). A = the caller's person,
B = the person of the proven sign-in.

| Data | Result |
|---|---|
| Display name | A's chosen name, else B's |
| Avatar | A's (built-in or custom picture) if A chose one, else B's; the custom picture file moves to the canonical key; `custom_version` becomes higher than both (caches by (user_key, version) never show a stale picture) |
| Welcome step (`profile_set`) | done if done on either side |
| Media-server name (`media_name`), `server_id` of admin lists | those of the canonical identity |
| Rights | both set by an admin: the more permissive (`can_keep_forever` if either, the larger `max_*`, `null` = unlimited wins); only one set: that one (an admin's explicit restriction is never lifted by linking an untouched account); none: default |
| Votes | one per item per person: the stronger — active keep forever > until watched > keep N days (the latest date) > "don't care" > a keep that is over (fulfilled, expired); A's on a tie. Votes over a `max_forever` stay (no new ones until under it) |
| Seerr link | a link to a Seerr user that already existed beats one the bridge created (the person's real account and requests); then A's |
| Settings across devices | union; per key the newer "time ␟ value" wins, A's on a tie |
| Watch together history | re-pointed to the canonical key, latest `last_at` per pair, pairs with oneself dropped |
| In-memory rooms | seats and invitations of B's keys become the canonical key (a room joined with one sign-in is "mine" with the other: join without invitation, host rights on every seat) |
| Audit log | unchanged (old keys stay as they were) |

**AlwyWatch admin** (never a media-server admin: nothing is written to Jellyfin or Plex) is
decided per request: the signed-in identity's own rule (Jellyfin administrator / Plex server
owner) OR the last known rule of a linked identity, counted only while that identity's server is
still served and it signed in during the last 30 days. Linking needs a valid token for both
users, so the person legitimately holds both admin rights already.

Rate limits use the canonical key (one budget per person). "Keep until I have watched it" is
fulfilled when **any** sign-in of the person has watched the item (Tautulli for Plex users, the
Jellyfin played flag for Jellyfin users). The Seerr bridge links a person to an existing Seerr
user matching any of their sign-ins (signed-in one first) before creating one.

### Stopping the Plex server

1. Each Plex user signs in once with their new Jellyfin user on a device where they are
   still signed in with Plex, and accepts "Are these both you?" (or links from Settings): the
   Jellyfin identity becomes canonical and their votes, name, avatar, rights, admin flag, Seerr
   link and history move to it.
2. Seerr and Maintainerr switch to Jellyfin mode (Maintainerr item keys change: votes on old
   Plex ratingKeys need a mapping step then; out of scope here).
3. Plex is removed from `[[servers]]`. Linked people keep everything; someone who never linked
   keeps their old data under their Plex key, unreachable until an admin helps (a later admin
   tool can link on their behalf).

### Storage and migration

Table `identities(user_key PK, person, server_id, kind, user_id, media_name, is_admin,
last_seen, linked_at)`, indexed by `person`. Opening an older database creates it and adds
every existing user as a one-identity person (idempotent). After a merge, aliases have no
`users` row: every per-user table holds canonical keys only.
