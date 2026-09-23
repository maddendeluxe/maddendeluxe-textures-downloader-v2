#!/usr/bin/env python3
"""The app's version lives in five files. They must agree, or a user cannot tell us
which build they are running.

Written after three rounds of exactly that: every Linux bundle shipped so far called
itself 2.0.0, including the ones with the blank window and the ones that could not
download, because tauri.conf.json (which names the bundle) had drifted from Cargo.toml
and package.json. A bug report then names a version that three different binaries share.

  check-versions.py              # all five must agree
  check-versions.py --self-test

Cargo.lock is parsed for THIS crate's entry only: other packages legitimately carry
versions that look like ours (proc-macro-crate sat at 2.0.2 while the app did).

A game's Tauri override under games/ is also rejected if it carries a version of its own.
Tauri merges that file over the root config, so a version there would silently name that
game's bundle something the five files above never mention.
"""

import glob
import json
import os
import re
import sys

CRATE = "ps2-textures-downloader"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def from_json(path, *keys):
    with open(os.path.join(ROOT, path)) as fh:
        data = json.load(fh)
    for key in keys:
        data = data[key]
    return data


def cargo_toml_version(text):
    """The version in [package], not in any other table."""
    section = None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped[1:-1]
            continue
        if section == "package":
            match = re.match(r'version\s*=\s*"([^"]+)"', stripped)
            if match:
                return match.group(1)
    return None


def cargo_lock_version(text, crate=CRATE):
    """The version of one named package in a Cargo.lock."""
    current = None
    for line in text.splitlines():
        stripped = line.strip()
        match = re.match(r'name\s*=\s*"([^"]+)"', stripped)
        if match:
            current = match.group(1)
            continue
        match = re.match(r'version\s*=\s*"([^"]+)"', stripped)
        if match and current == crate:
            return match.group(1)
    return None


def game_override_versions(paths_and_texts):
    """{path: version} for any game override that sets one. It should always be empty."""
    found = {}
    for path, text in paths_and_texts.items():
        version = json.loads(text).get("version")
        if version is not None:
            found[path] = version
    return found


def collect():
    with open(os.path.join(ROOT, "src-tauri/Cargo.toml")) as fh:
        cargo_toml = fh.read()
    with open(os.path.join(ROOT, "src-tauri/Cargo.lock")) as fh:
        cargo_lock = fh.read()
    return {
        "package.json": from_json("package.json", "version"),
        "package-lock.json": from_json("package-lock.json", "version"),
        'package-lock.json packages[""]': from_json("package-lock.json", "packages", "", "version"),
        "src-tauri/tauri.conf.json": from_json("src-tauri/tauri.conf.json", "version"),
        "src-tauri/Cargo.toml": cargo_toml_version(cargo_toml),
        "src-tauri/Cargo.lock": cargo_lock_version(cargo_lock),
    }


def self_test():
    failed = 0

    def expect(label, got, want):
        nonlocal failed
        if got == want:
            print(f"ok   {label}")
        else:
            print(f"FAIL {label}: got {got!r}, wanted {want!r}")
            failed += 1

    # The trap this check exists for: another package at the version ours used to be.
    lock = '''
[[package]]
name = "proc-macro-crate"
version = "2.0.2"

[[package]]
name = "ps2-textures-downloader"
version = "2.0.6"
dependencies = ["tauri"]
'''
    expect("Cargo.lock reads this crate, not the first match",
           cargo_lock_version(lock), "2.0.6")
    expect("Cargo.lock returns None for an absent crate",
           cargo_lock_version(lock, "not-here"), None)

    toml = '''
[package]
name = "ps2-textures-downloader"
version = "2.0.6"

[dependencies]
tauri = { version = "2" }
'''
    expect("Cargo.toml reads [package], not [dependencies]", cargo_toml_version(toml), "2.0.6")
    expect("Cargo.toml returns None when [package] has no version",
           cargo_toml_version('[package]\nname = "x"\n'), None)

    # And the comparison itself must reject a drift.
    drifted = {"a": "2.0.6", "b": "2.0.6", "c": "2.0.0"}
    if len(set(drifted.values())) == 1:
        print("FAIL a drifted set was treated as agreeing")
        failed += 1
    else:
        print("ok   caught: one file left behind")

    expect("a game override with no version is fine",
           game_override_versions({"games/x/tauri.conf.json": '{"productName": "X"}'}), {})
    if not game_override_versions({"games/x/tauri.conf.json": '{"version": "9.9.9"}'}):
        print("FAIL a game override carrying its own version was not caught")
        failed += 1
    else:
        print("ok   caught: a game override carrying its own version")

    # Every file the check claims to read must still be there and parseable.
    try:
        found = collect()
    except Exception as exc:
        print(f"FAIL the repo's own version files could not be read: {exc}")
        return failed + 1
    for name, value in found.items():
        if not value:
            print(f"FAIL {name}: the check found no version, so it can no longer guard it")
            failed += 1
    return failed


def main(argv):
    if "--self-test" in argv:
        failures = self_test()
        print("SELF-TEST GREEN" if not failures else f"SELF-TEST RED ({failures})")
        return 1 if failures else 0

    overrides = {
        os.path.relpath(p, ROOT).replace(os.sep, "/"): open(p).read()
        for p in sorted(glob.glob(os.path.join(ROOT, "games", "*", "tauri.conf.json")))
    }
    if not overrides:
        print("FAIL no games/*/tauri.conf.json found; this check can no longer see them")
        return 1
    stray = game_override_versions(overrides)
    if stray:
        for path, version in stray.items():
            print(f"FAIL {path} sets its own version ({version}). Tauri merges it over "
                  "the root config, so that game's bundle would be named from a version "
                  "no other file carries. Remove it, or bump every file below to match.")
        return 1

    found = collect()
    missing = [name for name, value in found.items() if not value]
    if missing:
        for name in missing:
            print(f"FAIL {name}: no version found")
        return 1

    width = max(len(name) for name in found)
    for name, value in found.items():
        print(f"  {name:<{width}}  {value}")

    if len(set(found.values())) != 1:
        print("\nVERSIONS DISAGREE. Every file above must carry the same version: "
              "tauri.conf.json is the one that names the bundle, so a mismatch ships "
              "a binary whose filename contradicts its About box.")
        return 1

    print(f"  {len(overrides)} game override(s) set no version of their own")
    print(f"\nVERSIONS AGREE ({next(iter(found.values()))})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
