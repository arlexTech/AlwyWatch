---
title: AlwyWatch
---

<img src="icon.png" width="96" alt="AlwyWatch logo" align="right" />

# AlwyWatch

A lightweight media app for **Jellyfin** and **Plex**: phones, tablets, Android TV, Linux,
Windows, the web and (early) iPhone. Request new titles through **Seerr**, and see what is
about to leave the server and ask to keep it: for a few days, until you have watched it, or
forever. Free and open source.

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
address. 🖥️ **The server** itself: [set it up](deploy).

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

- [Source code](https://github.com/arlexTech/alwywatch)
- [Set up the server](deploy)
- [Build it yourself](build)
- [Privacy policy](privacy)
- [Delete your AlwyWatch account](delete-account)
