#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Installs the desktop app for the current user only (no root): ~/.local/bin + app menu entry.
# Needs libmpv from the system. Builds the release first (an older build was installed silently
# when this step was left to the caller).
set -eu
here=$(dirname "$0")
cargo build --release -p alwywatch-desktop --manifest-path "$here/../../Cargo.toml"
bin="$here/../../target/release/alwywatch"
data=${XDG_DATA_HOME:-$HOME/.local/share}
install -Dm755 "$bin" "$HOME/.local/bin/alwywatch"
install -Dm644 "$here/alwywatch.svg" "$data/icons/hicolor/scalable/apps/alwywatch.svg"
install -Dm644 "$here/be.alwyapps.watch.desktop" "$data/applications/be.alwyapps.watch.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
echo "Installed: run 'alwywatch' or open AlwyWatch from your app menu."
