# Settings: the same sections on every platform

Settings are split into the same sections, with the same names and in the same order, on
Android (phone, tablet, TV), desktop and web. What differs is only how a section is reached:

* **Android**: a list of sections (each row says what is set there), each opening its page
  (touch and D-pad).
* **Desktop and web**: the sections in a column on the left, the chosen one beside it (web,
  narrow screens: a row of chips above it). Web addresses: `/settings/<id>`.

## Words

The same words everywhere, so a new user never wonders whether two names are one thing:

| Word | Means |
|---|---|
| Profile | One person on this device |
| AlwyWatch account | Your account on an AlwyWatch server (name, picture, settings, server list) |
| Login | A Jellyfin or Plex login (never "account" or "sign-in") |
| Server | A Jellyfin or Plex server only |
| Request site | Seerr (Overseerr, Jellyseerr) |
| Sign in with a code / Sign in another device | The new device / the device already signed in |
| Quick Connect | Jellyfin's own code sign-in only |
| Sign out | Removing a login, or the AlwyWatch account |
| Showing now | The login whose library is shown (and the one AlwyWatch uses) |
| Picture | The avatar |

## Sections

| Id | Title | What it holds | Not on |
|---|---|---|---|
| `account` | Account | See below | |
| `servers` | Servers | One row per login, "*user* on *server*" with a Plex/Jellyfin badge, **Showing now** (or **Preferred** when shown together), **Switch Plex user**, **Sign out**; **Add a server** (the AlwyWatch server's servers first); **Show all servers together**; **Keep in sync**; **Copy watch history** | Keep in sync: desktop, web |
| `requests` | Requests | See below | |
| `playback` | Playback | Intros and credits, skipping, resuming, picture, watch-together rights; desktop also sound and passthrough, marked "This device only" | |
| `audio` | Audio & subtitles | Audio language, subtitles mode and language, subtitle look | look: Android only |
| `interface` | Menu & pages | Libraries and pages in the menu, theme music, clock, confirm before quitting | |
| `downloads` | Downloads | When to delete, manage downloads | TV, desktop, web |
| `device` | This device | **Profiles** (list, switch, rename, PIN, remove, **Add a profile**, skip "Who's watching?"), **Sign in another device** (profiles with AlwyWatch only), **Language**, **Storage** (image cache), **About** (version, licence) | PIN: web |
| `admin` | Admin | Only for admins of the AlwyWatch server (`Me.is_admin`). See below | |

A platform leaves a section out only when it has nothing to put there. New settings go in the
matching section on every platform that supports them, under the same wording.

Older ids still open the right page: `services` and `alwywatch` → `account`, `about` and
`profiles` → `device` (Android back stacks saved by an earlier version; web
`/settings/alwywatch` redirects to `/settings/account`).

### Account

With AlwyWatch (connected to an AlwyWatch server, or signed in to an AlwyWatch account):

1. **How others see you**: picture and name, **Change name** (and **Reset to** the login's
   name when changed), **Change picture** ([profile-apps.md](profile-apps.md)).
2. **AlwyWatch server**: its address and state ("Connected", "Can't reach it", "Sign-in
   expired: sign in again"); selecting it changes the address (the "Sign in to AlwyWatch"
   screens, prefilled).
3. **Password & passkeys** (AlwyWatch accounts), deleting the account included: inline on the
   web; Android phone and desktop open the server's `/account` page in the browser; TV shows
   its QR code and address.
4. **Linked logins** ([identity-apps.md](identity-apps.md)).
5. One **Sign out of AlwyWatch**. Confirm: "This profile keeps its Jellyfin and Plex logins
   and becomes a profile without an AlwyWatch account. Leaving soon and watch together
   disappear from it. Your keep choices stay on the server." It signs the account out and
   disconnects the AlwyWatch server for this profile.

The login used for AlwyWatch is always the login **showing now**: when it changes (Servers,
Switch Plex user) and the AlwyWatch server serves it, the app switches to it by itself. There
is no separate choice.

Without AlwyWatch ("Jellyfin or Plex only" profiles): the profile's own name on this device
(rename), and a **Connect to AlwyWatch** card with **Sign in** and **Create an account** (the
same screens as "Add a profile").

### Requests

* Connected to an AlwyWatch server that takes requests: a **Requests** switch, on: "Through
  your AlwyWatch server, as *Seerr user*".
* Connected to one that doesn't: "Your AlwyWatch server doesn't take requests. Ask its owner,
  or use a request site address below."
* **Request site**: its address and login (**Sign out**), or **Use a request site address**,
  with the hint "It only accepts the logins it's set up for (Plex or Jellyfin)". The web app
  has the AlwyWatch rows only.

The list of requests is a page of the menu (phone: the bottom bar), not part of Settings.

### Admin

* **People**: per person what they may keep, and **Requests as** (their Seerr user).
* **Leaving soon countdown** (restart a collection's countdown) and **Kept forever**. The
  Admin button on Leaving soon stays as a shortcut to both.
* **Invitations**: a new invitation (how many days, how many people) shows its code and link
  (`account_invite(days, uses)`; web: `KeepClient`). Phones can share it; TV shows the code and
  the link to type elsewhere.
* **Server settings** (web app; native apps open it in the browser, TV shows its address):
  media servers, keys, Maintainerr, Seerr, Tautulli, email, sign-up, Pocket ID, webhooks, test
  mode; a Test button per section (docs/deploy.md).
* **Server status** (read only, `GET /v1/admin/status`: core `keep_admin_status()`, web
  `KeepClient::admin_status()`), each line with what to change:

| Line | Shown |
|---|---|
| Test mode (`dry_run`) | "Test mode: nothing is changed in Maintainerr or Seerr. Turn it off in Server settings." |
| Maintainerr `off` | "Off. Add Maintainerr in Server settings." |
| Maintainerr `starting` | "Reading Maintainerr for the first time…" |
| Maintainerr `unreachable` | "Can't reach Maintainerr. Check its address and API key in Server settings (Test)." |
| Maintainerr `ok` | "Maintainerr *version* · read *time* ago · *n* titles leaving" |
| Requests on / off | "On: every app requests as each person's Seerr user." / "Off: only direct request-site logins work. Add Seerr in Server settings." |
| No Jellyfin key | "Jellyfin keeps don't end by themselves. Add the Jellyfin server again in Server settings (it makes its key)." |
| No Tautulli | "Plex keeps don't end by themselves. Add Tautulli in Server settings." |
| No webhooks | "Optional: watched titles are noticed within 15 min. For instant, paste the webhook addresses from Server settings." |
| Passkeys off | "Off: set this site's address in Server settings" |
| Pocket ID, new accounts | On/off; who may create an account (an invitation, anyone, closed) |

## Add a profile (and first launch)

A profile is one person. A new profile (and the app's very first start, titled "Welcome to
AlwyWatch") opens on three cards:

1. **Sign in to AlwyWatch**: "Your name, settings and servers come with you".
2. **Create an AlwyWatch account**: "You have an invitation".
3. **Jellyfin or Plex only**: "No account: nothing synced, no Leaving soon or watch together".
   On the web too: "No AlwyWatch account: your logins and settings stay in this browser" (the
   profile is never linked to the AlwyWatch server by itself; "Connect to AlwyWatch" does it).

The web doesn't sync settings, so its first card reads "Your name and picture, Leaving soon and
watch together".

Then:

* Sign in: the AlwyWatch address (prefilled from the device's other profiles; the web skips
  it), then **Sign in with a code** (first on TV), a **Jellyfin or Plex login** on one of its
  servers ("Use your login" when the profile already has one there), **Username and password**,
  passkey or Pocket ID (native apps: with a code approved on the web).
* Create: the address, then (from `account_options(url)`: invitation needed or not, closed)
  the invitation code, username and password (`account_register`), then a login.
* Jellyfin or Plex only: Jellyfin (Quick Connect or password) or Plex (plex.tv); a Plex Home
  asks "Which Plex user?".

Then **Your servers** (the account's servers to sign in to), the one-time welcome step (name
and picture), and Home.

A profile without AlwyWatch hides everything that needs it: Leaving soon, watch together, the
welcome step, "Are these both you?", linked logins, requests through AlwyWatch, Sign in another
device and the keep reminders. Whether a profile has AlwyWatch is read from what it stores (an
AlwyWatch server connection or account session); nothing else is kept.

## Switching profile

With several profiles, the current one is in the navigation on every platform: one click or tap
opens "Who's watching?" and picking a profile switches to it (its PIN is asked there).

* Android TV and tablets: the first item of the side menu. Phones: the avatar at the top of
  Home (it also leads to Settings).
* Desktop: the first item of the side menu ("Who's watching?" over the app; Escape closes).
* Web: the profile item under the name in the menu. Web profiles are namespaces of the
  browser's storage (`web/src/profiles.rs`), without PINs.
