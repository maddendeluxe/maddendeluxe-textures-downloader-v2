#!/usr/bin/env bash
# Build the Linux bundles (AppImage/deb/rpm) inside a container that mirrors
# the ubuntu-22.04 GitHub Actions job. Run from anywhere on a host with
# rootless podman (or docker via CONTAINER_RUNTIME=docker).
#
#   tools/linux-build/build.sh                       # every game in games/
#   tools/linux-build/build.sh --game madden09
#   tools/linux-build/build.sh --game all --bundles "appimage deb"
#   tools/linux-build/build.sh --self-test
#
# Each game is an independent build: the bundles, the rendered download_textures.sh
# and a SHA256SUMS land in build-output/<game>/. Nothing in the source tree is
# edited between games -- the game comes from games/<id>/ (see tools/games.py).
# A host directory (CARGO_CACHE_DIR, default ~/.cache/tauri-linux-build) caches the cargo registry between runs.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
RUNTIME="${CONTAINER_RUNTIME:-podman}"
IMAGE="${IMAGE:-tauri-linux-build:jammy}"
CARGO_CACHE_DIR="${CARGO_CACHE_DIR:-$HOME/.cache/tauri-linux-build/cargo-registry}"

self_test() {
    local fail=0
    check() { if "$@" >/dev/null 2>&1; then echo "ok   ${*: -1}"; else echo "FAIL ${*: -1}"; fail=1; fi; }
    check test -f "$HERE/Containerfile"
    check test -f "$ROOT/src-tauri/tauri.conf.json"
    check command -v "$RUNTIME"
    # CI parity: every apt package the workflow's Linux step installs must be in the Containerfile.
    local wf="$ROOT/.github/workflows/build.yml" pkg
    for pkg in $(grep -o 'apt-get install -y [^&]*' "$wf" | sed 's/apt-get install -y //'); do
        if grep -q -- "$pkg" "$HERE/Containerfile"; then echo "ok   Containerfile has $pkg"
        else echo "FAIL Containerfile missing CI package $pkg"; fail=1; fi
    done
    if "$HERE/strip-bundled-wayland.sh" --self-test >/dev/null 2>&1; then echo "ok   strip-bundled-wayland self-test"
    else echo "FAIL strip-bundled-wayland --self-test"; fail=1; fi
    # Every game must be buildable: a manifest, a Tauri override, and the two agreeing.
    if python3 "$ROOT/tools/games.py" validate >/dev/null 2>&1; then echo "ok   games.py validate"
    else echo "FAIL games.py validate (run it for the reason)"; fail=1; fi
    # CI must build the same set of games this script does, or a game ships untested.
    local game
    for game in $(python3 "$ROOT/tools/games.py" list); do
        if grep -q "\\b$game\\b" "$wf"; then echo "ok   workflow builds $game"
        else echo "FAIL workflow build.yml never mentions $game"; fail=1; fi
    done
    return $fail
}

if [ "${1:-}" = "--self-test" ]; then self_test; exit $?; fi

GAMES=""
BUNDLES="appimage deb rpm"
while [ $# -gt 0 ]; do
    case "$1" in
        --game) GAMES="$2"; shift 2 ;;
        --bundles) BUNDLES="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
cd "$ROOT"
if [ -z "$GAMES" ] || [ "$GAMES" = "all" ]; then GAMES="$(python3 tools/games.py list | tr '\n' ' ')"; fi

python3 tools/games.py validate

if ! "$RUNTIME" image exists "$IMAGE" 2>/dev/null; then
    "$RUNTIME" build -t "$IMAGE" -f "$HERE/Containerfile" "$HERE"
fi

# Rootless podman maps container root to the invoking user, so output files come
# out owned by you. (--userns=keep-id is deliberately NOT used: crun refuses the
# bind-mounted workdir with it on SELinux hosts.)
mkdir -p "$CARGO_CACHE_DIR"
for GAME in $GAMES; do
    echo "=============================================================="
    echo "== building $GAME"
    echo "=============================================================="
    # One shared cargo target dir: changing GAME only rebuilds this crate (build.rs
    # declares rerun-if-env-changed=GAME), not the ~400 dependencies, and four
    # separate dirs would want ~16 GB. Every game's bundles therefore land in the
    # same bundle/ directory, so the copy below takes only files newer than this
    # marker rather than everything matching the glob. (The AppImage bundler
    # re-downloads linuxdeploy per run either way: the container is --rm and its
    # cache does not survive.)
    MARKER="$(mktemp)"
    "$RUNTIME" run --rm \
        --tmpfs /tmp:rw,exec,mode=1777,size=4g \
        -v "$ROOT":/work:z \
        -v "$CARGO_CACHE_DIR":/opt/cargo/registry:z \
        -w /work \
        -e GAME="$GAME" \
        -e CARGO_TARGET_DIR=/work/src-tauri/target \
        "$IMAGE" bash -c "
            set -euo pipefail
            npm install --no-audit --no-fund
            npx tauri build --config games/$GAME/tauri.conf.json --bundles $BUNDLES
            ls -la src-tauri/target/release/bundle/*/
        "
    OUT="$ROOT/build-output/$GAME"
    rm -rf "$OUT"; mkdir -p "$OUT"
    find src-tauri/target/release/bundle \
        -maxdepth 2 -type f -newer "$MARKER" \
        \( -name '*.AppImage' -o -name '*.deb' -o -name '*.rpm' \) \
        ! -name 'linuxdeploy*' ! -name 'AppRun*' ! -name 'appimagetool*' \
        -exec cp {} "$OUT/" \;
    rm -f "$MARKER"
    # A build that produced nothing must not pass quietly as an empty output folder.
    if [ -z "$(ls -A "$OUT")" ]; then
        echo "FAIL $GAME: no bundles were produced" >&2
        exit 1
    fi
    # Tauri names the bundles after productName, spaces and all
    # ("Madden 04 Deluxe Downloader_2.0.0_amd64.AppImage"). Ship the space-free name
    # instead: it is what the README tells testers to type and what the releases use.
    shopt -s nullglob
    for f in "$OUT"/*\ *; do mv -- "$f" "${f// /}"; done
    shopt -u nullglob
    # Tauri's AppImage ships the build image's libwayland, which makes the webview fail
    # to start on a Wayland desktop -- a window that never paints. Fix the copy we ship.
    find "$OUT" -maxdepth 1 -name '*.AppImage' -print0 | xargs -0 -r "$HERE/strip-bundled-wayland.sh"
    find "$OUT" -maxdepth 1 -name '*.AppImage' -print0 | xargs -0 -r "$HERE/strip-bundled-wayland.sh" --check
    python3 tools/games.py render "$GAME" src/download_textures.sh.in "$OUT/download_textures.sh"
    ( cd "$OUT" && sha256sum ./* > SHA256SUMS )
    echo "-- $GAME bundles in build-output/$GAME:"
    ls -la "$OUT"
done
