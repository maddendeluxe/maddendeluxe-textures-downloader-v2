// Per-game constants for the PS2 Textures Downloader.
//
// They are NOT edited here. `build.rs` generates them from `games/<GAME>/game.json`,
// where GAME is the environment variable that picks the loader (default `madden09`):
//
//     GAME=madden12 npx tauri build --config games/madden12/tauri.conf.json
//
// Adding a game means adding a folder under games/, never touching this file.
// The frontend reads the same manifest through vite.config.ts.
//
// Generated: GAME_ID, APP_TITLE, REPO_OWNER, REPO_NAME, REPO_URL, SLUS_FOLDER,
// SPARSE_PATH, TEMP_DIR_NAME.
include!(concat!(env!("OUT_DIR"), "/game_config.rs"));
