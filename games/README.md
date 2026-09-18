# `games/` — one folder per loader

Every Deluxe loader is the same app pointed at a different texture repository. The
strings that differ live here, and **nowhere else**, so all of them build from one
checkout with no file edits in between:

```
games/
  madden04/  game.json  tauri.conf.json
  madden05/  game.json  tauri.conf.json
  madden09/  game.json  tauri.conf.json
  madden12/  game.json  tauri.conf.json
```

| File | Read by | Supplies |
|---|---|---|
| `game.json` | `src-tauri/build.rs` (Rust constants) and `vite.config.ts` (frontend) | repo, SLUS folder, sparse path, temp dir, title, example path |
| `tauri.conf.json` | the Tauri bundler, merged over `src-tauri/tauri.conf.json` with `--config` | product name, bundle identifier, window title |

`src-tauri/tauri.conf.json` is deliberately game-neutral ("PS2 Textures Downloader",
`com.ps2textures.downloader`). A build that forgets `--config` is therefore obviously
unbranded rather than quietly branded as the wrong game, and each loader keeps its own
settings directory because the identifier comes from its own override.

`GAME` picks the loader and defaults to `madden09`:

```bash
tools/games.py list                       # the ids that exist
tools/games.py validate                   # every manifest, and the fields that must agree
tools/games.py validate --remote          # also: does each texture repo have installer-data.json?
tools/linux-build/build.sh                # build every game into build-output/<id>/
tools/linux-build/build.sh --game madden12
GAME=madden12 npm run tauri dev           # run one in dev mode
```

## Adding a game

1. `cp -r games/madden09 games/madden<year>` and edit both files. Every field is
   spelled out rather than derived, so nothing is hidden; `validate` enforces that
   `repoUrl`, `sparsePath` and `tempDirName` agree with the repo name and serial,
   and that the Tauri override names the same product as `game.json`.
2. Add the id to the `game:` matrix in `.github/workflows/build.yml`.
   `tools/linux-build/build.sh --self-test` fails until you do, so a new game cannot
   quietly ship without a pipeline.
3. `tools/games.py validate --remote` before announcing it. A texture repo with no
   `installer-data.json` produces an app that shows "HTTP 404" at startup and cannot
   install anything — it is the one failure that looks like an app bug but is not.
   **`maddendeluxe/madden05deluxe` is in that state as of 2026-09-18**: the loader
   builds and is branded correctly, but it cannot install until the file is added to
   that repo (measured contents: 6,166 files, 2.79 GB under `textures/SLUS-21000`).
   Do not ship the madden05 bundles until `validate --remote` is green for it.

Optional per-game icons: drop a `games/<id>/icons/` folder in and point
`bundle.icon` at it from that game's `tauri.conf.json` (paths are relative to
`src-tauri/`, so `../games/<id>/icons/icon.png`). Without it every loader uses the
shared icons in `src-tauri/icons/`.
