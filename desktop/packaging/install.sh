#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Installs the desktop app for the current user only (no root): ~/.local/bin + app menu entry.
# Needs libmpv from the system. Run from the repository root after `cargo build --release`.
set -eu
here=$(dirname "$0")
bin="$here/../../target/release/alwywatch"
[ -x "$bin" ] || { echo "Build first: cargo build --release -p alwywatch-desktop" >&2; exit 1; }
data=${XDG_DATA_HOME:-$HOME/.local/share}
install -Dm755 "$bin" "$HOME/.local/bin/alwywatch"
install -Dm644 "$here/alwywatch.svg" "$data/icons/hicolor/scalable/apps/alwywatch.svg"
install -Dm644 "$here/be.alwyapps.watch.desktop" "$data/applications/be.alwyapps.watch.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
echo "Installed: run 'alwywatch' or open AlwyWatch from your app menu."
