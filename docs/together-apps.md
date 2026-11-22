# Watch together: what every app offers

For the Android (phone/tablet and TV), desktop and web apps. Protocol details:
[keep-protocol.md § Watch together](keep-protocol.md#watch-together). Android and desktop call
`alwywatch-core` (`AlwyWatch` methods, named first below); the web app calls
`keep_service::KeepClient` directly (named second). Everything needs a connected AlwyWatch
server; without one, "Watch together" is hidden.

Rooms are private and start as a lobby: there is **no room code to type or show anywhere**.

## 1. "Watch together" on a title

On a movie or an episode (detail screen, and the player's menu when the title is playing
alone), "Watch together":

1. creates the lobby: `together_create(account_id, item_id)` / web: build the
   `TogetherMedia` (as today) then `KeepClient::together_create(&TogetherCreate { media })`;
2. opens the **Room screen** with the returned `TogetherJoined` (`member_id` + `state`).
   **Never the player.**

## 2. Room screen

Shows, from the latest `TogetherState`:

* the title (`state.media`: movie title, or show + SxxEyy), its poster from the user's own
  server when found (`together_find` / web: its own lookup);
* **Members**: avatar + name for each `state.members` entry, the host marked; a user on two
  devices may appear twice (same `user_key`): show them once with a device count;
* **Invited**: avatar + name for each `state.invited` entry, each with a remove action for the
  host (`together_uninvite(code, member_id, user_key)` / `KeepClient::together_uninvite(code,
  member, user_key)`);
* host only (`together_is_host(state, member_id)` / `state.is_host(member)`):
  * **Invite**: opens the people list (section 3);
  * **Start watching**: `together_start(code, member_id, position_ms)` /
    `KeepClient::together_start(code, member, position_ms)` (position 0, or the host's resume
    point if the app offers "resume"), then opens the player in sync;
  * while not started, the host may change the title: `together_update(code, member_id, false,
    0, account_id, item_id)` / `together_update` with `media: Some(..)`;
* everyone else: "Waiting for <host name> to start" (host name = the member whose `id ==
  state.host`);
* **Leave**: `together_leave(code, member_id)` / `KeepClient::together_leave`. Leaving is for
  good (a new invitation is needed to come back).

While the screen is open, loop on `together_wait(code, member_id, state.seq)` /
`KeepClient::together_wait(code, member, seq)` (it returns at once on any change, or after
~25 s) and redraw from each answer. **This loop is the seat's heartbeat**: a seat that does not
poll for 60 s is dropped, so keep it running in the lobby and in the player alike. When an
answer has `started == true`, open the player in sync (everyone, host included).

Errors in the loop: on a network error, retry with backoff. On 404 with a seat that worked
before, call `together_join(code)` once (a dropped seat may come back); 404 again means the
room is gone: tell the user and go back.

## 3. Inviting (people list)

"Invite" opens a list from `together_people()` / `KeepClient::together_people()`, already in the
right order: people the user last watched with first (`last_together_at` set; you may show a
"Recent" header over them), then everyone by name. Each row: avatar, `name`, and
`server_name` (or `media_name`) when two rows share a name. Hide people already in
`state.members` or `state.invited` (match `user_key`). Multi-select, then one call:
`together_invite(code, member_id, user_keys)` / `KeepClient::together_invite(code,
&TogetherInvite { member_id, user_keys })` (at most 20 at once; 409 = room full, 429 = slow
down). The answer is the room: redraw the Invited list. On TV: a D-pad list with check marks
and a "Send invitations" button; a search field filtering by name is welcome on long lists.

## 4. Invitations

Each app keeps a **list long poll** running while it is in the foreground and an AlwyWatch
server is connected: `together_rooms_wait(version)` / `KeepClient::together_rooms_wait(version)`,
starting with `version = 0` (answers at once), then passing the last `version`. Every answer
is the whole list (`TogetherRooms`). When an entry with `invited == true` appears that was not
there before (compare codes), show a prompt/banner **anywhere in the app** (including the
player, as a small non-blocking banner): "<host> invites you to watch <title>", with:

* **Join**: `together_join(code)` / `KeepClient::together_join(code)` → `TogetherJoined`;
  open the Room screen if `state.started` is false, else the player in sync (section 5);
* **Not now**: `together_decline(code)` / `KeepClient::together_decline(code)`.

Dismissing the banner without choosing leaves the invitation in the Together page. Stop the
poll in the background (Android: when the app is not visible) and restart it on resume; on
errors, retry with backoff. Do not show a banner for a room the user just left or declined.

## 5. Joining a started room

`together_join` on a started room returns `state.started == true`: find the title on the
user's servers (`together_find(account_ids, state.media)`), open the player at
`together_position(state, received_at_ms, now_ms)` / `state.position_at(..)`, playing if
`state.playing`, and run the room's `wait` loop. In the player, as today: send every play,
pause, seek and title change with `together_update`; on each `wait` answer whose `by` is not
this seat, follow it: every half second the player compares itself with the room and plays up to 6 % faster or slower for small drifts, seeking only past 2 s (`together_correction`; seeks only past 1 s when the speed can't change, as with passthrough audio). Not for 2 s after its own change, which the room hasn't echoed yet. The player shows "n watching" with the
members' avatars and names.

## 6. Together page

Lists `together_rooms()` / `KeepClient::together_rooms()` (cache it and render instantly, then
refresh; the list long poll keeps it current):

* invitations first (`invited == true`): host avatar + "<host> invites you", the title,
  started or not, Join / Not now;
* then my rooms (`mine == true`): title, host, members, "Lobby" or "Watching"; opening one goes
  to the Room screen or the player. A room of mine without a live seat on this device (joined
  on another device) is opened with `together_join(code)`;
* empty state: "No rooms yet. Open a movie or an episode and choose Watch together, then invite
  people." No code entry anywhere.

## Function map

| Step | alwywatch-core (`AlwyWatch`) | `keep_service::KeepClient` (web) |
|---|---|---|
| Create lobby | `together_create(account_id, item_id)` | `together_create(&TogetherCreate { media })` |
| My rooms / invitations | `together_rooms()`, `together_rooms_wait(after)` | `together_rooms()`, `together_rooms_wait(after)` |
| People to invite | `together_people()` | `together_people()` |
| Invite / withdraw | `together_invite(code, member_id, user_keys)`, `together_uninvite(code, member_id, user_key)` | `together_invite(code, &TogetherInvite)`, `together_uninvite(code, member, user_key)` |
| Join / decline | `together_join(code)`, `together_decline(code)` | `together_join(code)`, `together_decline(code)` |
| Start | `together_start(code, member_id, position_ms)` | `together_start(code, member, position_ms)` |
| Play/pause/seek/title | `together_update(code, member_id, playing, position_ms, account_id?, item_id?)` | `together_update(code, &TogetherUpdate)` (`started: false`) |
| Room changes + heartbeat | `together_wait(code, member_id, after)` | `together_wait(code, member, after)` |
| Leave | `together_leave(code, member_id)` | `together_leave(code, member)` |
| Am I host | `together_is_host(state, member_id)` | `state.is_host(member)` |
| Where to play | `together_position(state, received_at_ms, now_ms)`, `together_find(account_ids, media)` | `state.position_at(server_now_ms)` |
| Avatars | `keep_avatar_request(user_key, version)` ([profile-apps.md](profile-apps.md)) | `avatar_url(user_key, version)` + `auth_headers()` |
