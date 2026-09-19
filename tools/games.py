#!/usr/bin/env python3
"""The per-game manifest reader for this repo: one loader codebase, one folder per game.

Every game-specific string lives in `games/<id>/game.json` (plus a `tauri.conf.json`
override the Tauri bundler merges). Nothing in the shared source tree names a game, so
all loaders build from one checkout with no file edits between them:

    tools/games.py list                       # every game id, one per line
    tools/games.py show madden09              # resolved manifest as JSON
    tools/games.py show madden09 --shell      # the same as sh variable assignments
    tools/games.py validate [--remote]        # every manifest, and the fields that must agree
    tools/games.py render madden09 src/download_textures.sh.in out.sh
    tools/games.py --self-test

`validate` is the guard that replaces the old hand-edit ritual: it fails when a manifest
and its Tauri override disagree, when two games claim the same bundle identifier, or
(with --remote) when a texture repo has no installer-data.json, which is what made the
Madden 05 build 404 at startup on 2026-09-18.

Consumers: src-tauri/build.rs and vite.config.ts read games/<id>/game.json directly, so
this tool is for humans, CI and the build scripts, never a build-time dependency.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
REQUIRED = (
    "id", "title", "shortName", "description", "identifier", "repoOwner", "repoName",
    "repoUrl", "slusFolder", "sparsePath", "tempDirName", "examplePath",
)


def games_dir(root: Path | None = None) -> Path:
    return (root or REPO_ROOT) / "games"


def list_games(root: Path | None = None) -> list[str]:
    d = games_dir(root)
    return sorted(p.name for p in d.iterdir() if (p / "game.json").is_file()) if d.is_dir() else []


def load(game: str, root: Path | None = None) -> dict:
    path = games_dir(root) / game / "game.json"
    if not path.is_file():
        raise SystemExit(f"no such game: {game} (expected {path})")
    return json.loads(path.read_text())


def shell_name(key: str) -> str:
    """camelCase -> UPPER_SNAKE, the name used in shell output and templates."""
    return re.sub(r"(?<!^)(?=[A-Z])", "_", key).upper()


def check(game: str, root: Path | None = None) -> list[str]:
    """Everything that must be true of one game. Returns human-readable failures."""
    bad: list[str] = []
    m = load(game, root)
    for key in REQUIRED:
        if not isinstance(m.get(key), str) or not m[key].strip():
            bad.append(f"{game}: missing or empty field {key!r}")
    if bad:
        return bad
    if m["id"] != game:
        bad.append(f"{game}: id is {m['id']!r}, must match the folder name")
    # Fields that are spelled out rather than derived, so they are checked instead.
    expect = {
        "repoUrl": f"https://github.com/{m['repoOwner']}/{m['repoName']}.git",
        "sparsePath": f"textures/{m['slusFolder']}",
        "tempDirName": f"_temp_{m['repoName']}_repo",
    }
    for key, want in expect.items():
        if m[key] != want:
            bad.append(f"{game}: {key} is {m[key]!r}, expected {want!r}")
    if not re.fullmatch(r"SLUS-\d{5}|SLES-\d{5}|SCUS-\d{5}", m["slusFolder"]):
        bad.append(f"{game}: slusFolder {m['slusFolder']!r} is not a PS2 serial")
    if not re.fullmatch(r"[a-z0-9.-]+", m["identifier"]) or m["identifier"].count(".") < 2:
        bad.append(f"{game}: identifier {m['identifier']!r} is not a reverse-domain id")
    # The Tauri override must say the same thing as the manifest, or the bundle is
    # named for one game and downloads another.
    conf_path = games_dir(root) / game / "tauri.conf.json"
    if not conf_path.is_file():
        bad.append(f"{game}: missing tauri.conf.json override")
        return bad
    try:
        conf = json.loads(conf_path.read_text())
    except json.JSONDecodeError as e:
        bad.append(f"{game}: tauri.conf.json is not valid JSON: {e}")
        return bad
    if conf.get("productName") != m["title"]:
        bad.append(f"{game}: tauri productName {conf.get('productName')!r} != title {m['title']!r}")
    if conf.get("identifier") != m["identifier"]:
        bad.append(f"{game}: tauri identifier {conf.get('identifier')!r} != {m['identifier']!r}")
    # Without an explicit mainBinaryName the bundles are named "Madden 04 Deluxe
    # Downloader_2.0.0_amd64.AppImage", spaces and all.
    if conf.get("mainBinaryName") != m["title"].replace(" ", ""):
        bad.append(f"{game}: tauri mainBinaryName {conf.get('mainBinaryName')!r} != "
                   f"{m['title'].replace(' ', '')!r}")
    windows = conf.get("app", {}).get("windows") or []
    if not windows or windows[0].get("title") != m["title"]:
        bad.append(f"{game}: tauri window title != title {m['title']!r}")
    return bad


def check_remote(game: str, root: Path | None = None) -> list[str]:
    """The two URLs a released build depends on: installer-data.json and the sparse path."""
    from urllib.error import HTTPError, URLError
    from urllib.request import Request, urlopen

    m = load(game, root)
    bad = []
    urls = [
        f"https://raw.githubusercontent.com/{m['repoOwner']}/{m['repoName']}/main/installer-data.json",
        f"https://github.com/{m['repoOwner']}/{m['repoName']}/tree/main/{m['sparsePath']}",
    ]
    for url in urls:
        try:
            with urlopen(Request(url, headers={"User-Agent": "games.py"}), timeout=20) as r:
                code = r.status
        except HTTPError as e:
            code = e.code
        except URLError as e:
            bad.append(f"{game}: {url} unreachable ({e.reason})")
            continue
        if code != 200:
            bad.append(f"{game}: {url} -> HTTP {code}")
    return bad


def cmd_validate(args) -> int:
    root = Path(args.root) if args.root else None
    games = list_games(root)
    if not games:
        print("FAIL no games found in", games_dir(root))
        return 1
    failures = []
    seen: dict[str, str] = {}
    for game in games:
        bad = check(game, root)
        if args.remote and not bad:
            bad += check_remote(game, root)
        m = load(game, root)
        for key in ("identifier", "repoName"):
            other = seen.get(f"{key}={m.get(key)}")
            if other:
                bad.append(f"{game}: {key} {m.get(key)!r} is already used by {other}")
            seen[f"{key}={m.get(key)}"] = game
        for line in bad:
            print("FAIL", line)
        failures += bad
        if not bad:
            print(f"ok   {game}: {m['title']} -> {m['repoName']}/{m['sparsePath']}")
    print("VALIDATE", "RED" if failures else "GREEN", f"({len(games)} games)")
    return 1 if failures else 0


def cmd_list(args) -> int:
    for game in list_games(Path(args.root) if args.root else None):
        print(game)
    return 0


def cmd_show(args) -> int:
    m = load(args.game, Path(args.root) if args.root else None)
    if args.shell:
        for key, value in m.items():
            print(f"{shell_name(key)}={json.dumps(value)}")
    else:
        print(json.dumps(m, indent=2))
    return 0


def render(text: str, manifest: dict) -> str:
    """Replace @UPPER_SNAKE@ placeholders with the manifest's values."""
    for key, value in manifest.items():
        text = text.replace(f"@{shell_name(key)}@", value)
    left = re.findall(r"@[A-Z][A-Z0-9_]*@", text)
    if left:
        raise SystemExit(f"unresolved placeholders: {' '.join(sorted(set(left)))}")
    return text


def cmd_render(args) -> int:
    m = load(args.game, Path(args.root) if args.root else None)
    src = Path(args.template)
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(render(src.read_text(), m))
    out.chmod(0o755 if os.access(src, os.X_OK) or src.suffix in (".sh", ".bat") else 0o644)
    print(f"RENDERED {src} -> {out} ({args.game})")
    return 0


def self_test() -> int:
    """Builds a throwaway games/ tree and proves validate says RED for each way a
    manifest can be wrong. GAMES_SELF_TEST_SABOTAGE=1 must turn this RED."""
    failures = []
    sabotage = os.environ.get("GAMES_SELF_TEST_SABOTAGE") == "1"

    def good() -> dict:
        return {
            "id": "testgame", "title": "Test Deluxe Downloader", "shortName": "Test Deluxe",
            "description": "d", "identifier": "com.testdeluxe.textures-downloader",
            "repoOwner": "owner", "repoName": "testdeluxe",
            "repoUrl": "https://github.com/owner/testdeluxe.git",
            "slusFolder": "SLUS-12345", "sparsePath": "textures/SLUS-12345",
            "tempDirName": "_temp_testdeluxe_repo", "examplePath": r"C:\x",
        }

    def write(root: Path, manifest: dict, conf: dict | None = None) -> None:
        d = root / "games" / manifest["id"]
        d.mkdir(parents=True, exist_ok=True)
        (d / "game.json").write_text(json.dumps(manifest))
        conf = conf if conf is not None else {
            "productName": manifest["title"],
            "mainBinaryName": manifest["title"].replace(" ", ""),
            "identifier": manifest["identifier"],
            "app": {"windows": [{"title": manifest["title"]}]},
        }
        (d / "tauri.conf.json").write_text(json.dumps(conf))

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write(root, good())
        if check("testgame", root):
            failures.append(f"a correct manifest was rejected: {check('testgame', root)}")
        if list_games(root) != ["testgame"]:
            failures.append("list_games did not find the manifest")

        # Each mutation must be caught. This is the whole point of the tool.
        mutations = {
            "wrong repoUrl": lambda m: m.update(repoUrl="https://github.com/owner/other.git"),
            "wrong sparsePath": lambda m: m.update(sparsePath="textures/SLUS-99999"),
            "wrong tempDirName": lambda m: m.update(tempDirName="_temp_other_repo"),
            "missing field": lambda m: m.pop("shortName"),
            "empty field": lambda m: m.update(title=" "),
            "bad serial": lambda m: m.update(slusFolder="MADDEN09", sparsePath="textures/MADDEN09"),
            "bad identifier": lambda m: m.update(identifier="testdeluxe"),
        }
        for label, mutate in mutations.items():
            m = good()
            if not (sabotage and label == "wrong repoUrl"):
                mutate(m)
            write(root, m)
            if not check("testgame", root):
                failures.append(f"validate accepted a manifest with: {label}")
        # A Tauri override that drifted from the manifest is the real-world failure.
        m = good()
        write(root, m, conf={"productName": "Other Downloader", "identifier": m["identifier"],
                             "mainBinaryName": m["title"].replace(" ", ""),
                             "app": {"windows": [{"title": m["title"]}]}})
        if not check("testgame", root):
            failures.append("validate accepted a tauri override with the wrong productName")
        write(root, m, conf={"productName": m["title"], "identifier": m["identifier"],
                             "mainBinaryName": m["title"],  # spaces left in
                             "app": {"windows": [{"title": m["title"]}]}})
        if not check("testgame", root):
            failures.append("validate accepted a mainBinaryName with spaces")

        # render substitutes and refuses to leave a placeholder behind
        m = good()
        if render("url=@REPO_URL@ slus=@SLUS_FOLDER@", m) != "url=https://github.com/owner/testdeluxe.git slus=SLUS-12345":
            failures.append("render did not substitute correctly")
        try:
            render("@NOT_A_FIELD@", m)
            failures.append("render accepted an unresolved placeholder")
        except SystemExit:
            pass

    if shell_name("repoOwner") != "REPO_OWNER":
        failures.append("shell_name is wrong")
    for line in failures:
        print("FAIL", line)
    print("SELF-TEST", "RED" if failures else "GREEN",
          f"({len(REQUIRED)} required fields, 10 negative controls)")
    return 1 if failures else 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--self-test", action="store_true")
    p.add_argument("--root", help="repo root to read games/ from (default: this repo)")
    sub = p.add_subparsers(dest="command")
    sub.add_parser("list")
    s = sub.add_parser("show"); s.add_argument("game"); s.add_argument("--shell", action="store_true")
    s = sub.add_parser("validate"); s.add_argument("--remote", action="store_true")
    s = sub.add_parser("render"); s.add_argument("game"); s.add_argument("template"); s.add_argument("output")
    return p


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    args = build_parser().parse_args()
    if args.command == "list":
        return cmd_list(args)
    if args.command == "show":
        return cmd_show(args)
    if args.command == "validate":
        return cmd_validate(args)
    if args.command == "render":
        return cmd_render(args)
    print(__doc__)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
