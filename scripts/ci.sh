#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Runs the same checks as .forgejo/workflows/ci.yml locally.
# Usage: scripts/ci.sh [rust|android|windows|server|all]   (default: all)
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

server() {
    docker build -f server/Dockerfile -t alwywatch-server:ci .
}

case "$what" in
    rust) rust ;;
    android) android ;;
    windows) windows ;;
    server) server ;;
    all) rust; android; windows; server ;;
    *) echo "unknown target: $what" >&2; exit 2 ;;
esac
