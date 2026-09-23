#!/usr/bin/env python3
"""Prove a built AppImage opens links through the SYSTEM opener, with the desktop's own
environment, rather than through the copy of xdg-open it ships.

This is a regression test for two faults that shipped to users and were found on SteamOS
3.8.16 (Plasma 6) on 2026-09-19. Both were invisible: the click did nothing and every
exit code said success.

  1. AppRun puts $APPDIR/usr/bin first on PATH, so the AppImage's own xdg-open ran. It is
     xdg-utils 1.1.3 from the jammy build image, whose open_kde knows KDE 4 and 5 only.
     On Plasma 6 it matched nothing, ran nothing, and exited 0.
  2. AppRun prepends "$APPDIR/usr/share/:/usr/share:" to XDG_DATA_DIRS. Removing only the
     AppDir entries left /usr/share ahead of the Flatpak export dirs, so a stub
     "Install Firefox" .desktop in /usr/share won over the real browser and a click
     opened the software centre.

Neither is visible in source review or a unit test: they only appear when the packaged
binary spawns a real child. So this runs the real AppImage, replaces the system opener
with a recorder, and asserts on what the child actually received.

  check-appimage-opener.py <AppImage|directory> [...]   # the runtime test
  check-appimage-opener.py --source                     # the source-level guards
  check-appimage-opener.py --self-test

A directory is searched for *.AppImage and it is an error to find none: a step that
checks nothing must not report success.

The test arms its own trap and then proves the trap was armed: it fails if the AppImage
no longer ships an xdg-open of its own, because from then on a pass would prove nothing.
"""

import json
import os
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile

URL = "https://github.com/settings/personal-access-tokens/new?name=Textures+Downloader&x=1"
TIMEOUT = 180
MARKER = "appimage_extracted"
FLATPAK_DIR = "flatpak-exports/share"

RECORDER = """#!/bin/sh
# Stands in for the system xdg-open. Appends what it was handed, then reports success,
# exactly as the real one does once it has passed the URL to a browser.
#
# One record per call, because the app also probes this program with --version during its
# startup survey; a single-record file let that probe overwrite the real open.
#
# $0 is captured by the shell, not by python: python reads its script from stdin, so its
# own argv[0] is "-". Recording that left the "not from inside the AppImage" guard
# unfalsifiable in real runs while the self-test still passed.
OPENER_SELF="$0"; export OPENER_SELF
python3 -c '
import json, os, sys
keys = ("PATH", "XDG_DATA_DIRS", "LD_LIBRARY_PATH", "LD_PRELOAD", "APPDIR",
        "APPIMAGE", "PERLLIB", "PYTHONPATH", "GTK_PATH", "GIO_EXTRA_MODULES")
with open(os.environ["OPENER_RECORD"], "a") as fh:
    json.dump({"self": os.environ["OPENER_SELF"],
               "argv": sys.argv[1:],
               "env": {k: os.environ.get(k) for k in keys}}, fh)
    fh.write("\\n")
' "$@"
exit 0
"""


class Failure(Exception):
    pass


# The runtime test drives --diagnose-open, which never starts the webview. That leaves
# the UI's own path uncovered, so these guard it statically.
PLUGIN_IMPORT = "@tauri-apps/plugin-opener"


def check_source(files):
    """Problems with the source, as a list. `files` maps repo path -> contents."""
    problems = []

    for path, text in sorted(files.items()):
        if not path.startswith("frontend/"):
            continue
        if PLUGIN_IMPORT in text:
            problems.append(
                f"{path} imports {PLUGIN_IMPORT}. That plugin spawns the opener through "
                "the `open` crate, which does no environment sanitising and finds "
                "xdg-open on PATH, where the AppImage's own copy sits first. Use "
                "openExternal() from frontend/openExternal.ts."
            )
        if re.search(r"\bopenUrl\s*\(", text):
            problems.append(f"{path} calls openUrl(); links must go through openExternal().")

    helper = files.get("frontend/openExternal.ts")
    if helper is None:
        problems.append("frontend/openExternal.ts is gone; every link went through it.")
    elif 'invoke("open_external"' not in helper:
        problems.append(
            "frontend/openExternal.ts no longer invokes the open_external command."
        )

    lib = files.get("src-tauri/src/lib.rs", "")
    handler = re.search(r"generate_handler!\s*\[(.*?)\]", lib, re.S)
    if handler is None:
        problems.append("src-tauri/src/lib.rs has no generate_handler! list to read.")
    elif not re.search(r"\bopen_external\b", handler.group(1)):
        problems.append(
            "open_external is not registered in generate_handler!, so every link would "
            "fail at runtime with an unknown-command error."
        )
    return problems


def gather_source(root):
    files = {}
    for folder, _, names in os.walk(os.path.join(root, "frontend")):
        for name in names:
            if name.endswith((".ts", ".tsx")):
                full = os.path.join(folder, name)
                files[os.path.relpath(full, root).replace(os.sep, "/")] = \
                    open(full, errors="replace").read()
    lib = os.path.join(root, "src-tauri/src/lib.rs")
    if os.path.exists(lib):
        files["src-tauri/src/lib.rs"] = open(lib, errors="replace").read()
    return files


def check(record, log, session_data_dirs, expected_self, appdir_marker=MARKER):
    """Every assertion, as one pure function so --self-test can make each one fail."""
    if record is None:
        raise Failure(
            "the stand-in system opener was never run: the app either used its own "
            "bundled copy, or failed to spawn an opener at all"
        )

    armed = "an unguarded PATH lookup of xdg-open would have run:"
    line = next((l for l in log.splitlines() if armed in l), None)
    if line is None:
        raise Failure(
            "the app did not report what an unguarded PATH lookup would have run, so "
            "this test cannot tell a real pass from a missing check"
        )
    if appdir_marker not in line:
        raise Failure(
            "the AppImage no longer shadows xdg-open on PATH, so this test would pass "
            "even with the guard removed. Re-point it at whatever it does shadow.\n"
            f"  {line.strip()}"
        )

    if appdir_marker in record["self"]:
        raise Failure(f"ran an opener from inside the AppImage: {record['self']}")

    # The recording must name the program we planted. Without this the check above is
    # unfalsifiable whenever the recorder fails to report its own path.
    if record["self"] != expected_self:
        raise Failure(
            "the opener did not report the path this test planted, so its origin cannot "
            f"be judged.\n  expected: {expected_self}\n  recorded: {record['self']!r}"
        )

    # --diagnose-open tags each attempt (#method-N) so a human can tell the tabs apart,
    # so the URL must be a prefix rather than the whole argument. A prefix match still
    # catches a truncated, re-encoded or substituted URL.
    if not any(a.startswith(URL) for a in record["argv"]):
        raise Failure(f"the opener was not handed the URL intact: {record['argv']}")

    env = record["env"]
    for name in ("APPDIR", "APPIMAGE", "LD_LIBRARY_PATH", "LD_PRELOAD", "PERLLIB", "PYTHONPATH"):
        if env.get(name):
            raise Failure(f"{name} leaked into the opener: {env[name]}")

    for name in ("PATH", "XDG_DATA_DIRS", "GTK_PATH", "GIO_EXTRA_MODULES"):
        value = env.get(name) or ""
        if appdir_marker in value:
            raise Failure(f"{name} still points into the AppImage: {value}")

    # The ordering fault. Equality, not "does not contain": the child must get the
    # desktop's list back untouched, with the Flatpak dir still ahead of /usr/share.
    if env.get("XDG_DATA_DIRS") != session_data_dirs:
        raise Failure(
            "XDG_DATA_DIRS reached the opener in the wrong order or with entries "
            "added or lost. A system dir promoted ahead of the user's own is how a "
            "stub .desktop wins over the real browser.\n"
            f"  expected: {session_data_dirs}\n"
            f"  received: {env.get('XDG_DATA_DIRS')}"
        )

    if "[session] app version" not in log:
        raise Failure("the app wrote no debug log; a silent failure would be invisible again")


def run_one(appimage):
    name = os.path.basename(appimage)
    with tempfile.TemporaryDirectory(prefix="opener-check-") as tmp:
        home = os.path.join(tmp, "home")
        fakebin = os.path.join(tmp, "system-bin")
        flatpak = os.path.join(tmp, FLATPAK_DIR)
        for d in (home, fakebin, flatpak):
            os.makedirs(d, exist_ok=True)

        recorder = os.path.join(fakebin, "xdg-open")
        with open(recorder, "w") as fh:
            fh.write(RECORDER)
        os.chmod(recorder, os.stat(recorder).st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)

        record_path = os.path.join(tmp, "record.json")
        session_data_dirs = f"{flatpak}:/usr/local/share:/usr/share"

        env = dict(os.environ)
        env.update({
            "HOME": home,
            "OPENER_RECORD": record_path,
            "PATH": f"{fakebin}:/usr/local/bin:/usr/bin:/bin",
            "XDG_DATA_DIRS": session_data_dirs,
            # The desktop the faults were found on.
            "XDG_CURRENT_DESKTOP": "KDE",
            "KDE_SESSION_VERSION": "6",
        })
        for stale in ("APPDIR", "APPIMAGE", "LD_LIBRARY_PATH", "OWD", "ARGV0"):
            env.pop(stale, None)

        cmd = [os.path.abspath(appimage), "--appimage-extract-and-run", "--diagnose-open", URL]
        try:
            proc = subprocess.run(cmd, env=env, cwd=tmp, timeout=TIMEOUT,
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        except subprocess.TimeoutExpired:
            print(f"FAIL {name}\n  the app did not finish within {TIMEOUT}s. An opener "
                  "that hangs is as broken as one that does nothing.")
            return False
        except PermissionError:
            # A downloaded CI artifact arrives without its executable bit.
            print(f"FAIL {name}\n  not executable. If this is a downloaded artifact, "
                  f"run: chmod +x {shlex.quote(appimage)}")
            return False

        log_path = os.path.join(home, "textures-downloader-debug.log")
        log = open(log_path, errors="replace").read() if os.path.exists(log_path) else ""
        # One record per call. Pick the one that carries the URL: the startup survey
        # also probes this program with --version, and that probe must not be mistaken
        # for the link being opened.
        records = []
        if os.path.exists(record_path):
            records = [json.loads(line) for line in open(record_path) if line.strip()]
        record = next((r for r in records if any(a.startswith("http") for a in r["argv"])), None)
        if record is None and records:
            probes = sorted({" ".join(r["argv"]) for r in records})
            print(f"FAIL {name}\n  the system opener was run only for {probes}, never to "
                  "open the URL. The link was handed to some other program, which is what "
                  "happens when the AppImage's own bundled xdg-open wins on PATH.")
            return False

        try:
            check(record, log, session_data_dirs, recorder)
        except Failure as exc:
            print(f"FAIL {name}\n  {exc}")
            tail = proc.stdout.strip().splitlines()[-15:] if proc.stdout else []
            if tail:
                print(f"  (app exited {proc.returncode}, last output:)")
                print("  " + "\n  ".join(tail))
            return False

        if proc.returncode != 0:
            print(f"FAIL {name}\n  the opener was fine but the app exited "
                  f"{proc.returncode}; something else is broken.")
            return False

        print(f"ok   {name}: ran {record['self']} with the desktop's own environment")
        return True


BAD_CASES = {
    "used the bundled opener": lambda r, l, d: (
        {**r, "self": "/tmp/appimage_extracted_abc/usr/bin/xdg-open"}, l),
    "LD_LIBRARY_PATH leaked": lambda r, l, d: (
        {**r, "env": {**r["env"], "LD_LIBRARY_PATH": "/tmp/appimage_extracted_abc/usr/lib"}}, l),
    "APPDIR leaked": lambda r, l, d: (
        {**r, "env": {**r["env"], "APPDIR": "/tmp/appimage_extracted_abc"}}, l),
    "AppImage left on PATH": lambda r, l, d: (
        {**r, "env": {**r["env"], "PATH": "/tmp/appimage_extracted_abc/usr/bin:/usr/bin"}}, l),
    "data dirs reordered (the SteamOS fault)": lambda r, l, d: (
        {**r, "env": {**r["env"], "XDG_DATA_DIRS": "/usr/share:" + d}}, l),
    "a data dir dropped": lambda r, l, d: (
        {**r, "env": {**r["env"], "XDG_DATA_DIRS": "/usr/share"}}, l),
    "URL replaced": lambda r, l, d: ({**r, "argv": ["https://example.com/"]}, l),
    "URL truncated at the query string": lambda r, l, d: (
        {**r, "argv": [URL.split("?")[0]]}, l),
    "URL re-encoded (+ became a space)": lambda r, l, d: (
        {**r, "argv": [URL.replace("+", " ")]}, l),
    "no opener ran at all": lambda r, l, d: (None, l),
    "the opener could not report its own path": lambda r, l, d: ({**r, "self": "-"}, l),
    "a different program than the one planted": lambda r, l, d: (
        {**r, "self": "/usr/bin/xdg-open"}, l),
    "no debug log written": lambda r, l, d: (r, ""),
    "trap not armed (nothing shadowed)": lambda r, l, d: (
        r, l.replace("/tmp/appimage_extracted_abc/usr/bin/xdg-open", "/usr/bin/xdg-open")),
}


GOOD_SOURCE = {
    "frontend/openExternal.ts": (
        'import { invoke } from "@tauri-apps/api/core";\n'
        'export async function openExternal(url) { await invoke("open_external", { url }); }\n'
    ),
    "frontend/components/Header.tsx": 'import { openExternal } from "../openExternal";\n',
    "src-tauri/src/lib.rs": "tauri::generate_handler![load_state, open_external, save_state,]\n",
}

BAD_SOURCE = {
    "the UI went back to the opener plugin": lambda s: {
        **s, "frontend/components/Header.tsx":
        'import { openUrl } from "@tauri-apps/plugin-opener";\n'},
    "a component calls openUrl() directly": lambda s: {
        **s, "frontend/components/Header.tsx": "await openUrl(REPO_URL);\n"},
    "the shared helper was deleted": lambda s: {
        k: v for k, v in s.items() if k != "frontend/openExternal.ts"},
    "the helper stopped invoking the command": lambda s: {
        **s, "frontend/openExternal.ts": "export async function openExternal(url) {}\n"},
    "the command was unregistered": lambda s: {
        **s, "src-tauri/src/lib.rs": "tauri::generate_handler![load_state, save_state,]\n"},
    "the handler list cannot be found": lambda s: {**s, "src-tauri/src/lib.rs": "fn run() {}\n"},
}


SELF = "/t/system-bin/xdg-open"


def self_test():
    dirs = "/t/flatpak-exports/share:/usr/local/share:/usr/share"
    good_record = {
        "self": SELF,
        "argv": [URL],
        "env": {"PATH": "/t/system-bin:/usr/bin", "XDG_DATA_DIRS": dirs,
                "LD_LIBRARY_PATH": None, "APPDIR": None, "APPIMAGE": None},
    }
    good_log = (
        "2026-09-20 07:00:00.000 [session] app version 2.0.6, game madden09\n"
        "2026-09-20 07:00:00.001 [survey] an unguarded PATH lookup of xdg-open would have "
        "run: Some(\"/tmp/appimage_extracted_abc/usr/bin/xdg-open\")\n"
    )
    failed = 0

    for label, argv in (("a clean recording", [URL]),
                        ("the #method-N tag diagnose mode adds", [URL + "#method-0"])):
        try:
            check({**good_record, "argv": argv}, good_log, dirs, SELF)
            print(f"ok   {label} passes")
        except Failure as exc:
            print(f"FAIL {label} should pass: {exc}")
            failed += 1

    for label, mutate in BAD_CASES.items():
        record, log = mutate(good_record, good_log, dirs)
        try:
            check(record, log, dirs, SELF)
        except Failure:
            print(f"ok   caught: {label}")
        else:
            print(f"FAIL not caught: {label}")
            failed += 1

    problems = check_source(GOOD_SOURCE)
    if problems:
        print(f"FAIL clean source should pass: {problems}")
        failed += 1
    else:
        print("ok   clean source passes")

    for label, mutate in BAD_SOURCE.items():
        if check_source(mutate(GOOD_SOURCE)):
            print(f"ok   caught: {label}")
        else:
            print(f"FAIL not caught: {label}")
            failed += 1

    if shutil.which("python3") is None:
        print("FAIL the recorder needs python3 on PATH")
        failed += 1

    print(f"---- {len(BAD_CASES) + len(BAD_SOURCE)} guards, each shown failing above")
    return failed


def main(argv):
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

    if "--self-test" in argv:
        failures = self_test()
        print("SELF-TEST GREEN" if not failures else f"SELF-TEST RED ({failures} guard(s) dead)")
        return 1 if failures else 0

    if "--source" in argv:
        problems = check_source(gather_source(root))
        for problem in problems:
            print(f"FAIL {problem}")
        print("SOURCE CHECK GREEN" if not problems else "SOURCE CHECK RED")
        return 1 if problems else 0

    targets = []
    for arg in (a for a in argv if not a.startswith("-")):
        if os.path.isdir(arg):
            found = sorted(
                os.path.join(arg, n) for n in os.listdir(arg) if n.endswith(".AppImage")
            )
            if not found:
                print(f"FAIL {arg}: no .AppImage in this directory. Refusing to report "
                      "success without testing anything.")
                return 1
            targets.extend(found)
        else:
            targets.append(arg)

    if not targets:
        print("usage: check-appimage-opener.py <AppImage|directory> [...] | --source | "
              "--self-test", file=sys.stderr)
        return 2

    ok = True
    for appimage in targets:
        if not os.path.exists(appimage):
            print(f"FAIL {appimage}: no such file")
            ok = False
            continue
        ok = run_one(appimage) and ok
    print(f"OPENER CHECK GREEN ({len(targets)} checked)" if ok else "OPENER CHECK RED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
