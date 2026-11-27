---
title: Privacy policy
---

<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# AlwyWatch privacy policy

*Last updated: 27 November 2026*

AlwyWatch is a free, open source app for watching what is on Jellyfin and Plex media servers.
It has no ads, no analytics, no crash reporting and no tracking, and nothing about you is sold
or shared for advertising. The code is public: <https://github.com/arlexTech/alwywatch>.

AlwyWatch does not come with a server of its own. It only talks to the servers **you** add,
and to the services those servers need. What each of them keeps is described below.

## On your device

The app keeps on the device: the addresses of the servers you added, your sign-ins to them
(stored encrypted with the Android Keystore), a cache of their catalogues and artwork so
screens open at once, your settings and profiles, and the videos you download. Uninstalling
the app, or clearing its storage, removes all of it.

## Media servers you add (Jellyfin, Plex)

To browse and play, the app sends your sign-in and what you watch (playback progress, watched
marks, favourites) to the Jellyfin or Plex server you added, as their own apps do. That server
is run by you or by whoever invited you, and its owner's privacy practices apply to it.

Signing in with Plex, and the Plex watchlist, go through plex.tv, which is run by Plex, Inc.
under [its own privacy policy](https://www.plex.tv/about/privacy-legal/).

## Seerr

If you add a Seerr (Overseerr or Jellyseerr) server, the titles you search for and request are
sent to it. It is run by the owner of your media server.

## AlwyWatch server (optional)

An AlwyWatch server is a small, self-hosted companion run by the owner of a media server. It
adds "leaving soon" and keep requests, watch together and AlwyWatch accounts. It is used only
if you add one. It keeps:

- your account, if you create one: user name, display name and picture, email address (only
  if you give one, for invitations and password resets), your password as an Argon2id hash
  (never the password itself), the public keys of your passkeys, and which Jellyfin or Plex
  sign-ins you linked to it (their identifiers, not your passwords);
- your settings and server list, so other devices can sign in with them;
- your keep requests and watch together rooms.

It never stores your Jellyfin or Plex password, and it only reads your media server sign-in to
check who you are. You can delete your account and its data at any time (Settings → Account →
Delete account): see [Delete your AlwyWatch account](delete-account) for what goes and
what stays.

## Microphone and camera

- **Microphone**: only when you press the voice search button. The audio goes to your
  device's speech recognition service (on most devices, Google's), not to AlwyWatch or to any
  of your servers.
- **Camera**: only to scan the QR code another of your devices shows to sign it in. The image
  is decoded on the device and is neither stored nor sent anywhere.

Both are asked for the first time you use them, and the app works without them.

## Notifications

Used for download progress and for history copies. Nothing is sent anywhere to show them.

## Children

AlwyWatch is not aimed at children. Profiles can be limited by the media server's own
parental controls.

## Changes and contact

Changes to this policy are published on this page and in the project's history. Questions:
open an issue at <https://github.com/arlexTech/alwywatch/issues>.
