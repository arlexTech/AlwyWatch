# AlwyWatch profile: what every app offers

Display name, avatar and the one-time welcome step, for the Android (phone/tablet and TV),
desktop and web apps. Protocol: [keep-protocol.md § Profile](keep-protocol.md#profile-display-name-avatar-welcome-step).
Android and desktop call `alwywatch-core` (named first); the web app calls
`keep_service::KeepClient` and `keep_service::profile` (named second). All of it needs a
connected AlwyWatch server.

## Drawing an avatar

Everywhere a person is shown, draw a round avatar from their `Avatar` and `name`:

1. `custom_version = Some(v)`: the picture from `keep_avatar_request(user_key, v)` (an
   `HttpRequest`: URL + headers; it is not public) / web: `KeepClient::avatar_url(user_key, v)`
   fetched with `KeepClient::auth_headers()` (e.g. `fetch` then a blob URL: an `<img src>`
   cannot send the headers). Cache on disk by (user_key, v); a new version is a new picture, so
   never revalidate. While loading or on error, draw the default (3).
2. `preset = Some(id)`: the built-in avatar from `avatar_presets()` /
   `keep_service::profile::avatar_presets()`: disc of `background`, `shape` in `foreground`,
   geometry in the [table](keep-protocol.md#built-in-avatars). Unknown id (newer server): the
   default.
3. Otherwise the default: `avatar_initial(name)` in white on `avatar_default_color(name)` /
   `profile::avatar_initial`, `profile::default_avatar_color`.

Where: Settings account row ("Signed in as …"), the welcome step, the watch-together Room
screen (members, invited), the invite people list, invitation banners and the Together page
(host), the player's "n watching" list, admin lists (users, Seerr-link picker, kept-forever
people). Admin lists and the people list also show `media_name` (or the server name) next to
`name` when they differ or when two names are equal.

## Settings → AlwyWatch account

Shows the current avatar and display name (`keep_me()` / `KeepClient::me()`), with:

* **Name**: an edit action opening a text field prefilled with `name` (TV: the on-screen
  keyboard). Validate as the user types with `display_name_problem(name)` /
  `profile::clean_display_name(name)` and show the message; **Save** →
  `keep_set_display_name(name)` / `set_display_name(name)`. When `name_is_custom`, offer
  **Reset to <media_name>** → `keep_reset_display_name()` / `reset_display_name()`.
* **Avatar → Change**: a grid of the 12 built-in avatars (→ `keep_set_avatar_preset(id)` /
  `set_avatar_preset(id)`), **Upload a picture** (not on TV, see below) and **Remove** (→
  `keep_remove_avatar()` / `remove_avatar()`, back to the default).

Every call returns the updated `Me`: redraw from it. 429 means "too many changes, try again in a
few minutes".

### Upload a picture (phone, tablet, desktop, web)

1. The platform file/photo picker (images only).
2. A **cropper**: the picture under a round frame; the user moves (drag / arrows) and zooms
   (pinch / wheel / slider) it; **Save** / **Cancel**.
3. On Save, draw the square under the frame at min(512, its side in source pixels) × same
   (never upscale a smaller picture), encode **JPEG quality ~0.88**, and upload:
   `keep_upload_avatar(bytes, "image/jpeg")` / `upload_avatar(bytes, "image/jpeg")`. The result
   must stay under 512 KiB (413 otherwise; lower the quality and retry once).

### TV

No upload (no file picker): only the built-in avatars and Remove, with the line "You can set a
custom picture from your phone, computer or the web app." Name editing uses the on-screen
keyboard. Everything is D-pad reachable with visible focus.

## Welcome step (once per account)

Shown right after an account is linked to the AlwyWatch server, and at app start while
`Me.profile_set` is false (it is per account: done on one device, gone on all):

* a single screen: "Welcome! This is how others see you", the name field prefilled with
  `Me.name`, the avatar grid with the current avatar selected (default first), **Upload a
  picture** where the platform has a file picker (same cropper; not on TV);
* **Continue**: save what changed (`keep_set_display_name` if the name changed,
  `keep_set_avatar_preset` / `keep_upload_avatar` if the avatar changed), then
  `keep_mark_profile_set()` / `KeepClient::mark_profile_set()`;
* **Skip**: `keep_mark_profile_set()` only.

Both end the step, even with nothing changed. Never block the app on it: if a call fails, close
the step anyway and show it again at the next start (`profile_set` still false).
