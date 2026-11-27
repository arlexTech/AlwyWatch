---
title: Delete your AlwyWatch account
---

<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Delete your AlwyWatch account

AlwyWatch accounts live on the AlwyWatch server you signed up on (each one is run by the owner
of a media server; AlwyWatch itself has no central server). To delete yours:

1. **In the app**: Settings → Account → **Delete account**. On a TV, it shows a QR code to
   open the page on your phone.
2. **Or in a browser**: open your AlwyWatch server's address followed by `/account#delete`
   (for example `https://watch.example.org/account#delete`), sign in, and press
   **Delete my account**. If your account has a password, it is asked again.

## What is deleted

At once and for good: your account (user name, email address, password hash), your passkeys
and Pocket ID link, and your sessions on every device. If you only ever used the AlwyWatch
account, everything else goes with it too: profile name and picture, synced settings and
server list, keep requests and watch together contacts.

If you also signed in to the AlwyWatch server with a Jellyfin or Plex login, that login's
profile, settings and keep requests stay, as they were before you had an account: they belong
to that login. Your Jellyfin or Plex account itself is not touched either: ask the owner of
that server to remove it.

Nothing is kept afterwards, except in the server's own backups if its owner makes any.

Can't sign in any more? Ask the person who invited you (the owner of the server) to delete it,
or open an issue at <https://github.com/arlexTech/alwywatch/issues> and we will help you reach
them.
