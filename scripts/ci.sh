#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Runs the same checks as .forgejo/workflows/ci.yml locally.
# Usage: scripts/ci.sh [rust|android|windows|server|bench|all]   (default: all; bench is opt-in)
set -euo pipefail
cd "$(dirname "$0")/.."
what="${1:-all}"

rust() {
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
}

android() {
    (cd android && ./gradlew --no-daemon ktlintCheck assembleRelease)
    ls -l android/app/build/outputs/apk/release/
}

windows() {
    cargo xwin build --release --locked -p alwywatch-desktop --target x86_64-pc-windows-msvc
}

# Startup Macrobenchmark on a connected device/emulator (ANDROID_SERIAL, ABI in BENCH_ABI).
bench() {
    : "${ANDROID_SERIAL:?set ANDROID_SERIAL to the device to benchmark on}"
    (cd android && ./gradlew --no-daemon :baselineprofile:connectedBenchmarkReleaseAndroidTest -Palwywatch.abis="${BENCH_ABI:-x86_64}")
    python3 - <<'PY'
import glob, json
for f in glob.glob("android/baselineprofile/build/outputs/connected_android_test_additional_output/benchmarkRelease/connected/*/*benchmarkData.json"):
    for b in json.load(open(f))["benchmarks"]:
        for k, v in b["metrics"].items():
            print(f"{b['name']:28} {k:24} median {v['median']:.0f} ms (min {v['minimum']:.0f}, max {v['maximum']:.0f})")
PY
}

server() {
    docker build -f server/Dockerfile -t alwywatch-server:ci .
}

case "$what" in
    rust) rust ;;
    android) android ;;
    windows) windows ;;
    server) server ;;
    bench) bench ;;
    all) rust; android; windows; server ;;
    *) echo "unknown target: $what" >&2; exit 2 ;;
esac
