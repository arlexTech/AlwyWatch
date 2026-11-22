# Settings: the same sections on every platform

Settings are split into the same sections, with the same names and in the same order, on
Android (phone, tablet, TV), desktop and web. What differs is only how a section is reached:

* **Android**: a list of sections (each row says what is set there), each opening its page
  (touch and D-pad).
* **Desktop and web**: the sections in a column on the left, the chosen one beside it (web,
  narrow screens: a row of chips above it). Web addresses: `/settings/<id>`.

| Id | Title | What it holds | Not on |
|---|---|---|---|
| `servers` | Servers | Jellyfin and Plex sign-ins, "Add a server", several servers shown together, copying watch history | |
| `alwywatch` (Android: `services`) | AlwyWatch & requests | **You on AlwyWatch** (name, avatar, linked sign-ins), **Connection and devices** (the server's address, which account syncs, "Add a device", the AlwyWatch account, disconnect), **Requests** (Seerr) | |
| `profiles` | Profiles | Who's watching on this device: switch, add, rename, PIN, remove | PIN: not on web |
| `playback` | Playback | Intros and credits, skipping, resuming, picture, watch-together rights | |
| `audio` | Audio & subtitles | Audio language, subtitles mode and language, subtitle look | look: Android only |
| `interface` | Menu & pages | Libraries in the menu, theme music, clock, confirm before quitting | desktop, web |
| `downloads` | Downloads | When to delete, manage downloads | TV, desktop, web |
| `about` | About & storage | Version, licence, image cache | |

A platform leaves a section out only when it has nothing to put there. New settings go in the
matching section on every platform that supports them, under the same wording.

## Switching profile

With several profiles, the current one is in the navigation on every platform: one click or tap
opens "Who's watching?" and picking a profile switches to it (its PIN is asked there).

* Android TV and tablets: the first item of the side menu. Phones: the avatar at the top of
  Home (it also leads to Settings).
* Desktop: the first item of the side menu ("Who's watching?" over the app; Escape closes).
* Web: the profile item under the name in the menu. Web profiles are namespaces of the
  browser's storage (`web/src/profiles.rs`), without PINs.
