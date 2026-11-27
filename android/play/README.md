<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Publishing on Google Play (phone, tablet and Android TV)

One listing and one bundle serve phones, tablets and Android TV: the manifest requires neither
a touch screen nor leanback, and has the TV banner and launcher entry.

## Build the bundle

```sh
cd android && ./gradlew bundleRelease
# → app/build/outputs/bundle/release/app-release.aab
```

- Signed with the release key from `~/.config/alwywatch/signing.properties`. On Play, enrol in
  **Play App Signing** and upload with this key: it becomes the *upload key* (Google keeps
  the app signing key; a lost upload key can be reset, a lost signing key could not).
- `versionCode` is the commit count (`-PversionCode=…` overrides it); each upload needs a
  higher one. `versionName` is in `app/build.gradle.kts`.
- Native code: arm64-v8a, armeabi-v7a, x86_64; 64-bit libraries are 16 KB aligned (Play's
  requirement for apps targeting Android 15+).

## Graphics (this folder)

| Play asks for | File |
|---|---|
| App icon, 512×512 32-bit PNG | `icon-512.png` |
| Feature graphic, 1024×500 | `feature-graphic-1024x500.png` |
| Android TV banner, 1280×720 | `tv-banner-1280x720.png` |
| Phone screenshots (2–8, 16:9 or 9:16) | to take: demo server below |
| Android TV screenshots (1920×1080) | to take: demo server below |

Screenshots: use Jellyfin's public demo server (below), not a private library: its films
are free to show.

## Store listing

**App name** (30): `AlwyWatch`

**Short description** (80):
- en: `A fast, open source player for your Jellyfin and Plex servers, on phone and TV.`
- fr: `Un lecteur rapide et libre pour vos serveurs Jellyfin et Plex, mobile et TV.`

**Full description** (en):

> AlwyWatch is a light, fast app to watch what is on your own media server: Jellyfin or Plex,
> or both at once.
>
> • Opens instantly: everything you have seen is kept on the device, then refreshed in the
>   background.
> • Made for the remote on Android TV, and for the hand on phones and tablets.
> • Plays files as they are (direct play, HDR, surround passthrough), and asks the server to
>   convert only what the device can't play.
> • Subtitles (including styled ASS), audio tracks, chapters, skip intro.
> • Downloads for offline viewing that pick up where they stopped after a lost connection.
> • Several servers in one home screen, profiles with an optional PIN.
> • Request new films and shows with Seerr (Overseerr, Jellyseerr).
> • With an AlwyWatch server: see what is leaving soon and ask to keep it, watch together,
>   and sign your devices in with one account.
>
> AlwyWatch does not provide any content: you need access to a Jellyfin or Plex server.
> Free and open source (AGPL-3.0), no ads, no tracking.
> Code: github.com/arlexTech/alwywatch

**Full description** (fr):

> AlwyWatch est une application légère et rapide pour regarder ce qui se trouve sur votre
> propre serveur multimédia : Jellyfin ou Plex, ou les deux à la fois.
>
> • S'ouvre instantanément : ce que vous avez déjà vu reste sur l'appareil, puis se met à jour
>   en arrière-plan.
> • Pensée pour la télécommande sur Android TV, et pour la main sur téléphone et tablette.
> • Lit les fichiers tels quels (lecture directe, HDR, son multicanal en passthrough) et ne
>   demande au serveur de convertir que ce que l'appareil ne sait pas lire.
> • Sous-titres (y compris ASS stylés), pistes audio, chapitres, passer le générique.
> • Téléchargements hors ligne qui reprennent là où ils s'étaient arrêtés après une coupure.
> • Plusieurs serveurs sur un seul accueil, profils avec code PIN facultatif.
> • Demandez de nouveaux films et séries avec Seerr (Overseerr, Jellyseerr).
> • Avec un serveur AlwyWatch : voyez ce qui va bientôt partir et demandez à le garder,
>   regardez ensemble, et connectez vos appareils avec un seul compte.
>
> AlwyWatch ne fournit aucun contenu : il faut un accès à un serveur Jellyfin ou Plex.
> Libre et open source (AGPL-3.0), sans publicité ni pistage.
> Code : github.com/arlexTech/alwywatch

**Category**: Video Players & Editors. **Tags**: media player, streaming.

**Contact**: an email address (shown publicly on the listing) and the website
`https://arlextech.github.io/AlwyWatch/`.

**Privacy policy**: `https://arlextech.github.io/AlwyWatch/privacy`

## App content (Policy → App content)

- **App access**: "All or some functionality is restricted". Instructions for reviewers:
  > The app needs a media server. On the first screen choose Jellyfin, enter the server
  > address `https://demo.jellyfin.org/stable`, user name `demo`, no password (Jellyfin's
  > public demo server). On Android TV the same screens work with the remote.
- **Ads**: no ads.
- **Content rating** (IARC questionnaire): category "Reference, News, or Educational" does
  not fit; choose "All other app types" and answer that the app streams content from the
  user's own server and has no user-to-user communication beyond watch together (people the
  server owner invited, no public chat). Expect a rating driven by "unrestricted media
  access", as for other Jellyfin/Plex clients.
- **Target audience**: 18+ (or 16+); not designed for children.
- **News app**: no. **Government app**: no. **Financial features**: none. **Health**: no.
- **Data safety** (answers that match the code and `docs/privacy.md`):
  - Data collected: yes. *Personal info → Name* (display name, optional), *Email address*
    (optional), *User IDs* (account id); *App activity → App interactions* (keep requests,
    watch together); *Photos* (profile picture, optional). Purpose: App functionality,
    Account management. Not shared with third parties. Not processed ephemerally.
    Collection is optional: only with an AlwyWatch server account.
  - Encrypted in transit: **no** (the app also accepts `http://` servers on a home network).
  - Users can request deletion: yes, `https://arlextech.github.io/AlwyWatch/delete-account`.
- **Account deletion URL**: `https://arlextech.github.io/AlwyWatch/delete-account`.
  In the app: Settings → Account → Delete account.
- **Foreground service** (`FOREGROUND_SERVICE_DATA_SYNC`): "Data sync / user-initiated
  download". Video to record: start a download of an episode and show its notification.
- **Permissions**: camera (scanning a sign-in QR code, on device), microphone (voice search,
  device speech service), notifications (download progress).

## Android TV

In *Release → Advanced settings → Form factors*, add **Android TV** and opt in; Google reviews
TV separately (usually a few days) against its TV quality checklist: D-pad navigation
everywhere, banner, no touch-only screens, leanback launcher entry. All are in place; add the
TV screenshots and the 1280×720 banner to the TV listing.

## First release with a new personal developer account

Google requires a **closed test with at least 12 testers opted in for 14 days in a row**
before production access. Create a closed testing track, upload the bundle, add the testers'
Google accounts (an email list or a Google Group), share the opt-in link, then apply for
production after the 14 days. An organisation account (needs a D-U-N-S number) skips this.
