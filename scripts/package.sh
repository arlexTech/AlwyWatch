#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Desktop packages. Output in dist/.
#   scripts/package.sh linux     -> dist/AlwyWatch-<version>-x86_64.AppImage
#   scripts/package.sh windows   -> dist/AlwyWatch-<version>-setup.exe   (needs cargo-xwin, makensis)
# Windows: bundles libmpv-2.dll from a pinned shinchiro "mpv-dev-x86_64" build (checksum
# verified), or MPV_DLL=/path/to/libmpv-2.dll to use another one.
# Linux: the AppImage uses the system libmpv (Debian/Ubuntu: "libmpv2"; there the mpv player
# does not bring it; elsewhere usually "mpv"), which also keeps the
# distribution's VA-API/VDPAU drivers in charge of hardware decoding.
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
mkdir -p dist
cache="${XDG_CACHE_HOME:-$HOME/.cache}/alwywatch-build"
mkdir -p "$cache"

# AppImage type 2 runtime, pinned by tag and checksum (instead of appimagetool "continuous").
RUNTIME_TAG=20251108
RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
# libmpv for Windows (https://github.com/shinchiro/mpv-winbuild-cmake), pinned.
MPV_TAG=20261111
MPV_FILE=mpv-dev-x86_64-20261111-git-bcde6b93cc.7z
MPV_SHA256=273c079b11ea5575822f972756f4c9a71149a5c10130353abeb8586738938452

fetch_libmpv() {
    local archive="$cache/$MPV_FILE" dll="$cache/libmpv-2-$MPV_TAG.dll" tmp
    if [[ ! -f $dll ]]; then
        if [[ ! -f $archive ]]; then
            curl -fsSL -o "$archive.tmp" "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/$MPV_TAG/$MPV_FILE"
            echo "$MPV_SHA256  $archive.tmp" | sha256sum -c --quiet
            mv "$archive.tmp" "$archive"
        fi
        tmp=$(mktemp -d)
        7z e -y -o"$tmp" "$archive" libmpv-2.dll >/dev/null
        mv "$tmp/libmpv-2.dll" "$dll"
        rm -rf "$tmp"
    fi
    echo "$dll"
}

linux() {
    cargo build --release --locked -p alwywatch-desktop
    local appdir runtime="$cache/runtime-x86_64-$RUNTIME_TAG"
    if [[ ! -f $runtime ]]; then
        curl -fsSL -o "$runtime.tmp" "https://github.com/AppImage/type2-runtime/releases/download/$RUNTIME_TAG/runtime-x86_64"
        echo "$RUNTIME_SHA256  $runtime.tmp" | sha256sum -c --quiet
        mv "$runtime.tmp" "$runtime"
    fi
    appdir=$(mktemp -d)
    trap 'rm -rf "$appdir"' RETURN
    install -Dm755 target/release/alwywatch "$appdir/usr/bin/alwywatch"
    install -Dm644 packaging/linux/alwywatch.desktop "$appdir/alwywatch.desktop"
    install -Dm644 packaging/linux/alwywatch.png "$appdir/alwywatch.png"
    install -Dm644 packaging/linux/alwywatch.png "$appdir/usr/share/icons/hicolor/256x256/apps/alwywatch.png"
    install -Dm644 LICENSE "$appdir/usr/share/licenses/alwywatch/LICENSE"
    ln -s alwywatch.png "$appdir/.DirIcon"
    cat > "$appdir/AppRun" <<'RUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/alwywatch" "$@"
RUN
    chmod 755 "$appdir/AppRun"
    local fs out="dist/AlwyWatch-$version-x86_64.AppImage"
    fs=$(mktemp)
    mksquashfs "$appdir" "$fs" -root-owned -noappend -comp zstd -quiet >/dev/null
    cat "$runtime" "$fs" > "$out"
    rm -f "$fs"
    chmod 755 "$out"
    ls -l "$out"
}

windows() {
    local target=x86_64-pc-windows-msvc stage
    cargo xwin build --release --locked -p alwywatch-desktop --target $target
    stage=$(mktemp -d)
    trap 'rm -rf "$stage"' RETURN
    cp "target/$target/release/alwywatch.exe" "$stage/"
    cp LICENSE "$stage/LICENSE.txt"
    cp "${MPV_DLL:-$(fetch_libmpv)}" "$stage/libmpv-2.dll"
    (cd packaging/windows && makensis -V2 -DVERSION="$version" -DSRC="$stage" -DOUT="$PWD/../../dist/AlwyWatch-$version-setup.exe" alwywatch.nsi)
    ls -l "dist/AlwyWatch-$version-setup.exe"
}

case "${1:-}" in
    linux) linux ;;
    windows) windows ;;
    *) echo "usage: $0 linux|windows" >&2; exit 2 ;;
esac
