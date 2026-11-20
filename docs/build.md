# Building AlwyWatch

## Requirements

| For | Needs |
|---|---|
| Everything | Rust stable (edition 2024, see `rust-version` in `Cargo.toml`) |
| Android | JDK 21, Android SDK (compileSdk 37), NDK `29.0.14206865`, `cargo install cargo-ndk`, Rust targets `aarch64-linux-android armv7-linux-androideabi x86_64-linux-android` |
| Desktop (Linux) | Wayland or X11 dev libs (`libxkbcommon`, `fontconfig`); **libmpv** at run time (package `mpv`/`libmpv2`) |
| Desktop (Windows, cross from Linux) | `cargo install cargo-xwin`, target `x86_64-pc-windows-msvc`, `makensis` (NSIS) and `7z` for the installer |
| Server image | Docker |

## Checks (same as CI)

```sh
scripts/ci.sh rust      # fmt --check, clippy -D warnings, tests
scripts/ci.sh android   # ktlint + release APKs
scripts/ci.sh windows   # cross-build alwywatch.exe
scripts/ci.sh server    # Docker image
```

## Fast test loop

```sh
scripts/dev.sh android [serial]   # ~6 s after a Kotlin change, a few seconds after a Rust change
scripts/dev.sh desktop
```
Android: debug APK (no R8), Rust core in the `android-dev` profile (no LTO, incremental), only
the device's ABI, shown as **AlwyWatch Dev**. With the release key present it is signed like
release and installs over the release app, keeping its sign-in and cache.
A release build after a Kotlin change takes minutes because of R8; use it only for size and
performance measurements.

## Android

```sh
cd android
./gradlew assembleDebug                         # all ABIs
./gradlew assembleDebug -Palwywatch.abis=arm64-v8a   # faster: one ABI
./gradlew assembleRelease                       # R8 + ABI splits (+ universal APK)
```
Gradle runs `cargo ndk` (Rust core → `libalwywatch_core.so` per ABI) and `uniffi-bindgen`
(Kotlin bindings, generated from the host debug build because release strips the metadata)
before compiling Kotlin. `-Palwywatch.rustProfile=release-size` builds the core for size.

**Signing.** Release APKs are signed when `~/.config/alwywatch/signing.properties` exists
(`storeFile`, `storePassword`, `keyAlias`, `keyPassword`); otherwise they are unsigned. The key
never enters the repository. Keep a backup of the keystore: losing it means users must uninstall
to update.

**Baseline Profile and startup benchmark** (device or emulator with API 33+ connected):
```sh
ANDROID_SERIAL=<device> ./gradlew :app:generateBaselineProfile -Palwywatch.abis=<its ABI>
ANDROID_SERIAL=<device> ./gradlew :baselineprofile:connectedBenchmarkReleaseAndroidTest -Palwywatch.abis=<its ABI>
```
The profile is written to `app/src/release/generated/baselineProfiles/` and committed.
Generating it **clears the app's data** on that device (sign-in included): use a test device.
Generate it with an account signed in on a copy you can lose, so the profile covers the home screen. Some
vendor ROMs (ColorOS/OxygenOS) freeze backgrounded apps, which blocks the profile-save broadcast:
use an emulator or a stock-Android device for generation.

## Desktop

```sh
cargo run -p alwywatch-desktop                 # debug
cargo build --release -p alwywatch-desktop     # target/release/alwywatch
scripts/package.sh linux                       # dist/AlwyWatch-<v>-x86_64.AppImage
scripts/package.sh windows                     # dist/AlwyWatch-<v>-setup.exe (bundles libmpv)
```
The AppImage contains only AlwyWatch and uses the system libmpv, so the distribution's
VA-API/VDPAU drivers do hardware decoding. The Windows installer is per-user (no admin) and
bundles a pinned, checksum-verified `libmpv-2.dll` (override with `MPV_DLL=...`).

Headless mode (CI, screenshots, timing): `ALWYWATCH_SCREENSHOT=out.png
[ALWYWATCH_SCREENSHOT_AFTER_MS=3000] alwywatch`; `ALWYWATCH_TIMING=1` prints the first-frame time.

## Server

See [deploy.md](deploy.md). `docker build -f server/Dockerfile -t alwywatch-server .` from the
repository root.
