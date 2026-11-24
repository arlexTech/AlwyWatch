#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Builds the iPhone/iPad app, unsigned, as dist/AlwyWatch-ios-unsigned.ipa (macOS + Xcode only).
# Sign and install it with Impactor, SideStore or AltStore and a free Apple ID.
#   1. the Rust core as a static library for iOS devices (aarch64-apple-ios),
#   2. its Swift bindings (UniFFI, from a host build: release builds strip the metadata),
#   3. the Xcode project from ios/project.yml (XcodeGen), then an unsigned archive.
# Needs: rustup target aarch64-apple-ios, xcodegen (brew install xcodegen).
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
gen=ios/Generated
rm -rf "$gen" && mkdir -p "$gen/lib" "$gen/include" "$gen/swift"

export IPHONEOS_DEPLOYMENT_TARGET=16.0
cargo rustc --locked --release -p alwywatch-core --lib --target aarch64-apple-ios --crate-type staticlib
cp target/aarch64-apple-ios/release/libalwywatch_core.a "$gen/lib/"

cargo build --locked -p alwywatch-core
cargo run --locked -q -p uniffi-bindgen -- generate --library target/debug/libalwywatch_core.dylib \
    --language swift --out-dir "$gen/bindings" --no-format
mv "$gen"/bindings/*.swift "$gen/swift/"
mv "$gen"/bindings/*.h "$gen/include/"
# One module map for every crate's C module (one Swift module holds all the bindings).
cat "$gen"/bindings/*.modulemap > "$gen/include/module.modulemap"
rm -rf "$gen/bindings"

cd ios
xcodegen generate --quiet
log="$root/target/ios-build.log"
if ! xcodebuild -project AlwyWatch.xcodeproj -scheme AlwyWatch -configuration Release \
    -destination 'generic/platform=iOS' -archivePath "$root/target/ios/AlwyWatch.xcarchive" \
    CODE_SIGNING_ALLOWED=NO archive > "$log" 2>&1; then
    # The compiler's errors first (the full log is long), then the end of the log.
    grep -E "(error|fatal error):" "$log" | sort -u | head -80 || true
    tail -n 30 "$log"
    exit 1
fi
grep -E "ARCHIVE SUCCEEDED" "$log"

app="$root/target/ios/AlwyWatch.xcarchive/Products/Applications/AlwyWatch.app"
rm -rf "$root/target/ios/ipa" && mkdir -p "$root/target/ios/ipa/Payload" "$root/dist"
cp -R "$app" "$root/target/ios/ipa/Payload/"
(cd "$root/target/ios/ipa" && zip -qr "$root/dist/AlwyWatch-ios-unsigned.ipa" Payload)
ls -l "$root/dist/AlwyWatch-ios-unsigned.ipa"
