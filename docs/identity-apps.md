# One AlwyWatch account for several sign-ins: what every app offers

For the Android (phone/tablet and TV), desktop and web apps. Protocol:
[keep-protocol.md § Linked identities](keep-protocol.md#linked-identities). The web app calls
`keep_service::KeepClient` (`links()`, `link(server_id, token)`, `unlink(user_key)`,
`me_with_identities()`); Android and desktop call the same through `alwywatch-core` (wrappers
to add there, e.g. `keep_links()`, `keep_link(account_id)`, `keep_unlink(user_key)`).

Background: the AlwyWatch server knows people by their media-server user. Someone signed in with
their Plex user on one device and their Jellyfin user on another had two AlwyWatch accounts (two
names and avatars, split keep votes, rooms of one invisible to the other). Linking makes them one
account; it is also how people keep their votes, name and history when the Plex server stops.

## "Are these both you?"

When a device holds **two signed-in accounts** (any two of its Plex/Jellyfin accounts) that are
**both on the AlwyWatch server** (both get a `Me`) and **are not the same person** (their
`Me.user_key` differ, and neither appears in the other's `Me.identities`), offer once:

> **Are these both you?** Use one AlwyWatch account for both *&lt;name A&gt;* (&lt;server A&gt;)
> and *&lt;name B&gt;* (&lt;server B&gt;): same keep votes, name, avatar and watch-together rooms.
> **[Link]** **[Not now]**

* Phone/tablet: a dialog or a card at the top of Settings → AlwyWatch account. TV: a full-screen
  dialog, **Link** focused first, both buttons D-pad reachable. Web/desktop: a dialog.
* Show both names, servers and avatars so a shared device (a family member's account) is not
  linked by mistake. Never offer it for two accounts on the **same** server (the server refuses:
  two users of one server are never one person).
* **Link** → `link(server_id_of_B, token_of_B)` sent signed in as A (the account currently used
  for AlwyWatch, i.e. the synced account). Then read `Me` again (its `user_key` may have changed:
  replace every cached `user_key`, e.g. the "is this seat mine" check in rooms) and refresh the
  leaving-soon list, the people list and the rooms list.
* Errors: 401 "Your <server B> sign-in has expired: sign in again, then link"; 409 (the server's
  message: an account already linked to another user of that server); 429 "Too many attempts,
  try again in a few minutes"; 502 "The media server is unreachable".
* **Not now** → remember the pair (both user keys) on the device and never ask again for it;
  the Settings entry below stays available. Ask again only if one of the two accounts is
  signed out and back in.
* Tokens: B's token is the one the app already stores for that account (Keystore / encrypted
  store). Never log it, never show it.

## Settings → AlwyWatch account → Linked sign-ins

Under the name and avatar (see [profile-apps.md](profile-apps.md)), a **Linked sign-ins**
row, shown when the account has more than one identity (`Me.identities.len() > 1`) or when the
device holds another account on the AlwyWatch server that is not linked:

* The list from `links()` (or `Me.identities`): avatar of the account, `media_name`,
  `server_name`, a "Main" tag on `canonical` and "This device" on `current`.
* Each non-canonical entry has **Unlink** → confirm "*&lt;media_name&gt;* on *&lt;server&gt;*
  becomes a separate, new AlwyWatch account with no votes, name or history. Continue?" →
  `unlink(user_key)` → show the returned list. The canonical entry has no Unlink (the server
  answers 409: show its message if it ever happens). If the device's current sign-in was
  unlinked, read `Me` again: it is now a fresh account (the welcome step will show).
* **Link another sign-in**: lists the device's other accounts on the AlwyWatch server that are
  not linked yet; picking one does the same as **Link** above. Accounts not on this device: the
  user signs in to them first (normal sign-in flow), then links.
* "Also signed in as …": wherever the app shows "Signed in as *name*", add the other
  identities' `media_name (server_name)` in a second line when there are any.

## After linking

* The app's **synced account** choice (which account talks to AlwyWatch) no longer matters for
  identity: every linked sign-in reaches the same account. It still decides which server's ids
  `LeavingItem.server_item_id` refers to (the server of the sign-in used), so keep using the
  account whose library the user is browsing.
* Watch together: a room joined from one sign-in shows as "mine" on devices signed in with the
  other, and can be joined there without an invitation (host rights included). The people
  list shows each person once.
* "Keep until I have watched it" is fulfilled by watching on any linked sign-in.
