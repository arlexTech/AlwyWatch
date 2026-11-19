#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Fast test loop: build, install and start the app.
#   scripts/dev.sh android [serial]   debug APK (no R8, Rust core without LTO, only the device's ABI)
#                                     shown as "AlwyWatch Dev". Signed with the release key when
#                                     present, so it replaces the release app and keeps its
#                                     sign-in. Device: serial, $ANDROID_SERIAL or the first one.
#   scripts/dev.sh desktop            cargo run (dev profile, dependencies optimised)
set -euo pipefail
cd "$(dirname "$0")/.."

android() {
    local serial="${1:-${ANDROID_SERIAL:-}}" abi apk
    if [[ -z $serial ]]; then
        serial=$(adb devices | awk 'NR > 1 && $2 == "device" { print $1; exit }')
    fi
    [[ -n $serial ]] || { echo "no device: connect one or start an emulator" >&2; exit 1; }
    abi=$(adb -s "$serial" shell getprop ro.product.cpu.abi | tr -d '\r')
    echo "device $serial ($abi)"
    (cd android && ./gradlew -q :app:assembleDebug -Palwywatch.abis="$abi")
    apk="android/app/build/outputs/apk/debug/app-$abi-debug.apk"
    adb -s "$serial" install -r -t "$apk" | tail -1
    adb -s "$serial" shell am start -n be.alwyapps.watch/be.alwyapps.watch.MainActivity >/dev/null
}

desktop() {
    cargo run -p alwywatch-desktop
}

case "${1:-}" in
    android) shift; android "$@" ;;
    desktop) desktop ;;
    *) echo "usage: $0 android [serial] | desktop" >&2; exit 2 ;;
esac
