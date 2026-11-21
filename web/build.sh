#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Builds the web app into web/dist (served by alwywatch-server at /). Needs the wasm32 target and
# the wasm-bindgen CLI at the exact version pinned in web/Cargo.toml.
set -eu
cd "$(dirname "$0")/.."
cargo build --profile web --locked -p alwywatch-web --target wasm32-unknown-unknown
rm -rf web/dist && mkdir -p web/dist
wasm-bindgen --target web --no-typescript --out-name alwywatch_web --out-dir web/dist \
  target/wasm32-unknown-unknown/web/alwywatch-web.wasm
# Optional: smaller and faster wasm when Binaryen is installed.
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int \
    -o web/dist/alwywatch_web_bg.wasm web/dist/alwywatch_web_bg.wasm
fi
cp -r web/static/. web/dist/
# Precompressed copies, sent when the browser accepts gzip.
find web/dist -type f \( -name '*.wasm' -o -name '*.js' -o -name '*.css' -o -name '*.html' -o -name '*.svg' \) \
  -exec gzip -9 -k -f {} \;
du -sh web/dist
ls -la web/dist/alwywatch_web_bg.wasm web/dist/alwywatch_web_bg.wasm.gz
