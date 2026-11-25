<img src="packaging/linux/alwywatch.png" width="96" alt="AlwyWatch logo" align="right" />

# AlwyWatch

[![License: AGPL v3+](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)

A lightweight, open source media client for **Jellyfin** and **Plex**, with **Seerr** requests
and a **"keep"** system on top of **Maintainerr**: see what is about to be deleted from the
server and ask to keep it for N days, until you have watched it, or forever.

* **Without `alwywatch-server`**: a plain Jellyfin/Plex client with downloads, and Seerr
  requests if you give it your Seerr address.
* **With it** (one Docker container next to Maintainerr, [set up in a few steps](docs/deploy.md)):
  Leaving soon and keep choices, watch together, the web app, settings that follow you across
  devices, signing a new device in with a code, optional AlwyWatch accounts, and Seerr for
  both Jellyfin and Plex users.

> AlwyWatch is in beta. The first stable release isn't out yet, so take a **Beta** file for now.

## Which file do I download?

Find your device in the table and click the file next to it.

| Your device | Stable | Beta |
|---|---|---|
| 📱 **Android** phone or tablet (Android 8 or newer) | Not yet | [.apk](https://github.com/arlexTech/alwywatch/releases/download/beta/AlwyWatch-android.apk) |
| 📺 **Android TV**, Google TV, Fire TV | Not yet | [.apk](https://github.com/arlexTech/alwywatch/releases/download/beta/AlwyWatch-android.apk) |
| 🐧 **Linux**, any distribution | Not yet | [.AppImage](https://github.com/arlexTech/alwywatch/releases/download/beta/AlwyWatch-linux-x86_64.AppImage) ¹ |
| 🪟 **Windows** 10 / 11 | Not yet | [.exe](https://github.com/arlexTech/alwywatch/releases/download/beta/AlwyWatch-windows-x64-setup.exe) |
| 🍏 **macOS** | Not yet | Not yet |
| 🍎 **iPhone / iPad** | Not yet | [.ipa](https://github.com/arlexTech/alwywatch/releases/download/ios/AlwyWatch-ios-unsigned.ipa) ² |

🌐 **Web browser**: nothing to install, the web app is served by `alwywatch-server` at its
address. 🖥️ **The server** itself: [set it up](docs/deploy.md).

**Which column?** **Stable:** tested releases. **Beta:** every change as soon as it's made, so
it may have bugs. There is no automatic update yet: download new versions yourself.

Smaller Android files, for one kind of device only: [64-bit phones and recent TVs](https://github.com/arlexTech/alwywatch/releases/download/beta/AlwyWatch-android-arm64-v8a.apk),
[older 32-bit devices and many TVs](https://github.com/arlexTech/alwywatch/releases/download/beta/AlwyWatch-android-armeabi-v7a.apk),
[x86_64 emulators and PCs](https://github.com/arlexTech/alwywatch/releases/download/beta/AlwyWatch-android-x86_64.apk). Not sure? Take the one in the table.

¹ **Linux:** needs libmpv from your distribution (Debian/Ubuntu: `libmpv2`, elsewhere usually
`mpv`).

² **iPhone (early):** not on the App Store. Install the .ipa with
[Plume Impactor](https://github.com/claration/impactor), SideStore or AltStore and a free Apple
ID (with a free Apple ID it must be refreshed every 7 days).

Older versions are on the [releases page](https://github.com/arlexTech/alwywatch/releases).

### Installing

**📱 Android**
1. Download the `.apk` file on your phone.
2. Open it. If Android asks, allow your browser or file manager to install apps.
3. Tap **Install**, then open AlwyWatch.

**📺 Android TV**
1. On the TV, install a downloader app (for example *Downloader* by AFTVnews) and allow it to
   install apps.
2. Open the `.apk` link above in it, then **Install**.
3. Or from a computer: turn on developer options on the TV, then `adb connect <TV address>` and
   `adb install AlwyWatch-android.apk`.

**🐧 Linux**
1. Download the `.AppImage` file.
2. Right-click it, open **Properties**, and tick **Allow executing file as program**.
3. Double-click it to start AlwyWatch.

Or in a terminal:
```sh
chmod +x AlwyWatch-linux-x86_64.AppImage
./AlwyWatch-linux-x86_64.AppImage
```

**🪟 Windows**
1. Download the `.exe` file and open it.
2. Windows may say it protected your PC (the installer isn't signed by a paid certificate yet):
   click **More info**, then **Run anyway**.

Building it yourself instead: [docs/build.md](docs/build.md).

First launch: "Sign in to AlwyWatch" with the address the person who runs your server gave
you, "Create an AlwyWatch account" with an invitation, or "Jellyfin or Plex only". A TV or
second device can be signed in with a code from a device that already is.

The server (`alwywatch-server`, one small container) is set up in its web app, like the *arr
apps: [docs/deploy.md](docs/deploy.md).

## Features

| | Android phone/tablet | Android TV | Desktop (Linux, Windows) | Web |
|---|---|---|---|---|
| Jellyfin (password, Quick Connect) and Plex (plex.tv sign-in) accounts, several at once | yes | yes | yes | yes |
| Plex Home user switch | yes | yes | no | no |
| Home rails, libraries (genres, collections), title pages, search | yes | yes | yes | yes |
| Renders from a local cache first | SQLite | SQLite | SQLite | `localStorage` copies of a few pages |
| Player | Media3/ExoPlayer: HW decoding, HDR, passthrough, FFmpeg audio, libass | same | libmpv (own window) | browser `<video>`, HLS fallback |
| Direct play, server conversion as fallback | yes | yes | always direct | yes |
| Audio and subtitle choice, remembered per title | yes | yes | yes | text subtitles only, no audio choice |
| Skip intro/credits, next episode | yes | yes | yes | yes |
| Resumable downloads | yes | no | yes | no |
| My list (Jellyfin favorites, Plex watchlist), trailers, cast | yes | yes | yes | My list and cast |
| Seerr search and requests (direct or through the server) | yes | yes | yes | through the server only |
| Leaving soon, keep choices, one-by-one sorting, reminders | yes | yes | yes | yes, no reminders |
| Admin: rights, countdown, kept-forever stats | yes | yes | yes | yes |
| Admin: link users to existing Seerr users | yes | yes | no | yes |
| Watch together | yes | yes | yes | yes |
| Device profiles with optional PIN | yes | yes | yes | no |
| Copy watch history between servers | yes | yes | yes | no |
| Keep two servers' watch state in sync | yes | yes | no | no |
| Settings follow the user across devices | yes | yes | yes | no |
| Sign a new device in with a code (end-to-end encrypted) | yes | yes | yes | no |
| Voice search | yes | yes | no | no |

TV is D-pad first (leanback launcher, focus restored where it was). Phones use a bottom bar,
tablets and TV a side rail, desktop a sidebar. Web limitations are listed in
[docs/web.md](docs/web.md#limitations-true-in-the-code-today).

## Layout

| Path | What |
|---|---|
| `core/` | Rust core shared by every app: media servers, Seerr, keep service, downloads, cache |
| `android/` | Android phone/tablet/TV app (Kotlin, Jetpack Compose, Media3), core through UniFFI |
| `desktop/` | Linux and Windows app (Rust, Slint, libmpv) |
| `web/` | Web app (Rust to wasm, Leptos), served by the server |
| `ios/` | iPhone/iPad app (SwiftUI, AVPlayer), core through UniFFI, Xcode project from XcodeGen |
| `server/` | `alwywatch-server`: keep votes, Seerr bridge, watch together, device transfer, web app |
| `tools/uniffi-bindgen` | Kotlin and Swift bindings generator (Gradle, `scripts/ios.sh`) |
| `scripts/`, `packaging/` | CI, dev loop and packaging scripts |
| `docs/` | Server setup, build, decisions, APIs, performance |

## Build

Details and requirements: [docs/build.md](docs/build.md).

```sh
scripts/ci.sh rust                      # fmt, clippy -D warnings, tests (whole workspace)
cd android && ./gradlew assembleDebug   # Android APKs (builds the Rust core with cargo-ndk)
cargo run -p alwywatch-desktop          # desktop app
desktop/packaging/install.sh            # per-user install after `cargo build --release -p alwywatch-desktop`
scripts/package.sh linux|windows        # AppImage / Windows installer in dist/
web/build.sh                            # web app into web/dist (wasm32 target + wasm-bindgen-cli)
scripts/ios.sh                          # unsigned iPhone/iPad .ipa in dist/ (macOS, Xcode, xcodegen)
docker build -f server/Dockerfile -t alwywatch-server .   # server image, web app included
```

## Docs

* [Building](docs/build.md) and packaging
* [Languages](docs/i18n.md): adding a language is adding one file
* [Setting up the server](docs/deploy.md): quick start, options, administration, backup
* [Core API](docs/core-api.md), [keep-service protocol](docs/keep-protocol.md), [web app](docs/web.md)
* [Decisions and dependencies](docs/decisions.md)
* [Performance](docs/perf.md)

## License

[GNU AGPL v3 or later](LICENSE).
