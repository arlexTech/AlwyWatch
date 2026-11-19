# Performance

Measured on 2026-11-19, version 0.1.0. Priorities: no perceived loading, then RAM, then size.

## What is done for speed

* **Cache-first everywhere.** Every list/detail screen renders from SQLite synchronously
  (`cached_*`), then refreshes (`refresh_*`) with per-screen TTLs. No spinner for data we have.
* **Nothing on the startup path.** Android: the Rust core loads on a background thread
  (`Services.core` is a lazy `Deferred`); codec enumeration for the device profile happens after
  it. No network at startup. Desktop: the core and the window start in parallel.
* **Baseline Profile** (`android/baselineprofile`, 16.8k rules, startup profile too) shipped in the
  APK and installed by `profileinstaller` on sideloads.
* **Images**: one disk cache in the Rust core (size-capped LRU), BlurHash placeholders, posters
  requested at the size they are shown (width buckets), prefetch of the next page.
* **Release builds**: R8 (minify + shrink resources), Rust with fat LTO, 1 codegen unit,
  opt-level 3, stripped.

## Android

### Cold start: OnePlus Ace 2 Pro (PJA110, Snapdragon 8 Gen 2, Android 16)

`am start -W` TotalTime (first frame, "Add a server" screen), 8 runs, release APK:

| Compilation | Median | Range |
|---|---|---|
| No profile (`compile -m verify`) | 147 ms | 142–230 ms |
| Baseline Profile (`speed-profile`, what users get) | **132 ms** | 127–140 ms |

Macrobenchmark could not run on this phone: ColorOS freezes backgrounded apps, which blocks the
benchmark's profile broadcasts. Measured with `am start -W` instead.

### Cold start: emulator (Android TV 16, x86_64; CI reference)

Macrobenchmark `StartupBenchmark`, 10 iterations, median:

| | No profile | Baseline Profile |
|---|---|---|
| timeToInitialDisplay | 186 ms | 174 ms |
| timeToFullDisplay (`ReportDrawn`) | 323 ms | 287 ms |

Run with `ANDROID_SERIAL=<device> scripts/ci.sh bench`.

### Memory (phone, idle on the first screen, 5 s after start)

`dumpsys meminfo`: **90 MB PSS** total, of which Graphics 59 MB (window buffers for a
1240×2772 screen), Java heap 7.4 MB, native heap 6.2 MB.

### APK size (release, R8, signed)

| APK | Native libs uncompressed (AGP default) | **Compressed (shipped)** |
|---|---|---|
| arm64-v8a | 16.2 MB | **8.5 MB** |
| armeabi-v7a | 12.8 MB | **7.9 MB** |
| x86_64 | 17.1 MB | **8.8 MB** |
| universal | 47.1 MB | **22.4 MB** |

arm64 native libraries (uncompressed): Rust core 7.5 MB, libass 3.0 MB, FFmpeg audio 1.5 MB,
libc++ 1.3 MB, JNA 0.17 MB. Dex: 4.6 MB (2.2 MB compressed). A size-optimised Rust core
(`-Palwywatch.rustProfile=release-size`, opt-level "s") saves 0.6 MB more; not used because
speed ranks above size.

## Desktop (Linux, release build)

| | Value |
|---|---|
| Binary | 27 MB (`target/release/alwywatch`), AppImage 12.8 MB |
| Windows installer | 43 MB (8 MB without the bundled libmpv) |
| First frame | 21–31 ms (headless software renderer, 3 runs) |
| Idle RSS, first screen | **25 MB** (5.2 MB anonymous), headless software renderer |

The headless renderer is the same UI and core without GPU drivers; a real window adds the GL
driver's own memory. **Playback RAM was not measured**: mpv needs a display and this machine's
build shell has none. To measure: start a video, then
`grep VmRSS /proc/$(pidof alwywatch)/status` (mpv runs inside the process through libmpv).

## Not yet measured / next steps

* Signed-in home screen start (needs a test account in CI); screen transitions benchmark.
* Desktop playback RAM and Android playback start time on the real TV device.
* Android TV (Chromecast with Google TV 4K) cold start, once connected over adb.
