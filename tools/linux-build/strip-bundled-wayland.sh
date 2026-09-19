#!/usr/bin/env bash
# Remove the libwayland libraries Tauri's AppImage bundler ships, and repack.
#
# WHY: linuxdeploy copies the BUILD machine's libwayland-client/server/egl/cursor into the
# AppImage. On a Wayland desktop the host's Mesa EGL driver uses the HOST libwayland, so
# the process ends up with two of them and eglGetPlatformDisplay refuses the display with
# EGL_BAD_PARAMETER. WebKit's web process aborts, the UI process survives, and the user
# gets a window with the right title that never paints.
#
# Confirmed on a Steam Deck (SteamOS 3.8.16, KDE on Wayland, 2026-09-18): blank white
# window in all three launch modes, with exactly that error on stderr. X11 sessions never
# load the bundled library, which is why the test rig (GNOME on Xorg) showed nothing wrong.
# Removing them makes the app use the host's own libwayland, which is the one Mesa uses.
#
#   strip-bundled-wayland.sh App.AppImage [More.AppImage ...]   # strip and repack in place
#   strip-bundled-wayland.sh --check App.AppImage               # fail if any are bundled
#   strip-bundled-wayland.sh --dir path/to/AppDir               # strip a directory only
#   strip-bundled-wayland.sh --self-test
#
# appimagetool is fetched once into APPIMAGETOOL_DIR (default ~/.cache/tauri-linux-build/tools).
set -uo pipefail

PATTERN='libwayland-*.so.*'
APPIMAGETOOL_DIR="${APPIMAGETOOL_DIR:-$HOME/.cache/tauri-linux-build/tools}"
APPIMAGETOOL_URL="${APPIMAGETOOL_URL:-https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage}"

# The one rule, in one place, so the self-test exercises what the build uses.
matches() {
    local sabotage="${STRIP_WAYLAND_SABOTAGE:-0}"
    if [ "$sabotage" = "1" ]; then
        find "$1" -type f -name 'nothing-matches-this' 2>/dev/null
    else
        find "$1" -type f -name "$PATTERN" 2>/dev/null
    fi
}

strip_dir() {
    local dir="$1" found=0 f
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        echo "   removing ${f#"$dir"/}"
        rm -f "$f"
        found=$((found + 1))
    done < <(matches "$dir")
    echo "$found"
}

appimagetool() {
    local tool="$APPIMAGETOOL_DIR/appimagetool-x86_64.AppImage"
    if [ ! -x "$tool" ]; then
        mkdir -p "$APPIMAGETOOL_DIR"
        echo "   fetching appimagetool" >&2
        curl -fsSL "$APPIMAGETOOL_URL" -o "$tool" || return 1
        chmod +x "$tool"
    fi
    APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tool" "$@"
}

process() {
    local img="$1" check_only="${2:-0}"
    if [ ! -f "$img" ]; then
        echo "FAIL no such AppImage: $img" >&2
        return 1
    fi
    local work
    work="$(mktemp -d)"
    chmod +x "$img" 2>/dev/null
    local abs
    abs="$(cd "$(dirname "$img")" && pwd)/$(basename "$img")"
    ( cd "$work" && APPIMAGE_EXTRACT_AND_RUN=1 "$abs" --appimage-extract >/dev/null 2>&1 )
    if [ ! -d "$work/squashfs-root" ]; then
        echo "FAIL could not extract $img" >&2
        rm -rf "$work"
        return 1
    fi

    if [ "$check_only" = "1" ]; then
        local hits
        hits="$(matches "$work/squashfs-root" | wc -l)"
        rm -rf "$work"
        if [ "$hits" -gt 0 ]; then
            echo "FAIL $(basename "$img") still bundles $hits wayland library file(s)"
            return 1
        fi
        echo "ok   $(basename "$img") bundles no wayland libraries"
        return 0
    fi

    echo "== $(basename "$img")"
    local removed
    removed="$(strip_dir "$work/squashfs-root" | tail -1)"
    if [ "$removed" = "0" ]; then
        echo "   nothing to remove, leaving it alone"
        rm -rf "$work"
        return 0
    fi
    if ! appimagetool --no-appstream "$work/squashfs-root" "$work/out.AppImage" >"$work/tool.log" 2>&1; then
        echo "FAIL appimagetool could not repack $img" >&2
        tail -5 "$work/tool.log" >&2
        rm -rf "$work"
        return 1
    fi
    mv -f "$work/out.AppImage" "$img"
    chmod +x "$img"
    echo "   repacked without $removed wayland file(s): $(du -h "$img" | cut -f1)"
    rm -rf "$work"
}

self_test() {
    local fail=0 tmp
    tmp="$(mktemp -d)"
    mkdir -p "$tmp/AppDir/usr/lib"
    : > "$tmp/AppDir/usr/lib/libwayland-client.so.0"
    : > "$tmp/AppDir/usr/lib/libwayland-egl.so.1"
    : > "$tmp/AppDir/usr/lib/libwebkit2gtk-4.1.so.0"
    : > "$tmp/AppDir/usr/lib/im-wayland.so"

    local removed
    removed="$(strip_dir "$tmp/AppDir" | tail -1)"
    if [ "$removed" = "2" ]; then echo "ok   removed both wayland libraries"
    else echo "FAIL removed '$removed' files, expected 2"; fail=1; fi
    # The libraries that must survive: webkit is the app, and the GTK input module is
    # not ABI-coupled to Mesa.
    if [ -f "$tmp/AppDir/usr/lib/libwebkit2gtk-4.1.so.0" ]; then echo "ok   kept libwebkit2gtk"
    else echo "FAIL deleted libwebkit2gtk"; fail=1; fi
    if [ -f "$tmp/AppDir/usr/lib/im-wayland.so" ]; then echo "ok   kept the GTK wayland input module"
    else echo "FAIL deleted im-wayland.so (it is a GTK module, not a wayland library)"; fail=1; fi
    if [ "$(matches "$tmp/AppDir" | wc -l)" = "0" ]; then echo "ok   a second pass finds nothing"
    else echo "FAIL libraries survived the strip"; fail=1; fi

    rm -rf "$tmp"
    if [ "${STRIP_WAYLAND_SABOTAGE:-0}" = "1" ] && [ "$fail" = "0" ]; then
        echo "FAIL sabotage did not trip a check"; fail=1
    fi
    echo "SELF-TEST $([ "$fail" = 0 ] && echo GREEN || echo RED) (4 checks)"
    return "$fail"
}

case "${1:-}" in
    --self-test) self_test; exit $? ;;
    --dir)
        [ -d "${2:-}" ] || { echo "usage: $0 --dir <AppDir>" >&2; exit 2; }
        strip_dir "$2" >/dev/null; exit 0 ;;
    --check)
        shift
        [ $# -gt 0 ] || { echo "usage: $0 --check <AppImage>..." >&2; exit 2; }
        rc=0
        for img in "$@"; do process "$img" 1 || rc=1; done
        exit $rc ;;
    "") echo "usage: $0 <AppImage>... | --check <AppImage>... | --dir <AppDir> | --self-test" >&2; exit 2 ;;
    *)
        rc=0
        for img in "$@"; do process "$img" 0 || rc=1; done
        exit $rc ;;
esac
