// Per-game constants for the PS2 Textures Downloader.
//
// They are NOT edited here. vite.config.ts reads games/<GAME>/game.json — the same
// manifest src-tauri/build.rs compiles into the Rust side — and bakes it in as
// __GAME_CONFIG__. GAME picks the loader (default madden09); adding a game means
// adding a folder under games/, never touching this file.

/// Application title displayed in the header
export const APP_TITLE = __GAME_CONFIG__.title;

/// Repository owner (GitHub username or organization)
export const REPO_OWNER = __GAME_CONFIG__.repoOwner;

/// Repository name
export const REPO_NAME = __GAME_CONFIG__.repoName;

/// Full URL to the repository (for linking)
export const REPO_URL = __GAME_CONFIG__.repoUrl;

/// The target folder name (typically the PS2 game identifier like SLUS-XXXXX)
export const TARGET_FOLDER = __GAME_CONFIG__.slusFolder;

/// Path within the repo to sparse checkout (e.g., "textures/SLUS-21214")
export const SPARSE_PATH = __GAME_CONFIG__.sparsePath;

/// Example textures path shown under the directory picker
export const EXAMPLE_PATH = __GAME_CONFIG__.examplePath;

/// The game id this build was made for (the folder name under games/)
export const GAME_ID = __GAME_CONFIG__.id;
