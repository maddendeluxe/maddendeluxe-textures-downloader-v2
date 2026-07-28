// Configurable constants for the PS2 Textures Downloader
// Modify these values to adapt this app for other PS2 texture mod projects
// Note: Also update frontend/config.ts to match these values

/// Application title (also update in tauri.conf.json and frontend/config.ts)
#[allow(dead_code)]
pub const APP_TITLE: &str = "Madden 09 Deluxe Downloader";

/// Repository owner (GitHub username or organization)
pub const REPO_OWNER: &str = "maddendeluxe";

/// Name of the texture mod repository
pub const REPO_NAME: &str = "madden09deluxe";

/// Full URL to the git repository
pub const REPO_URL: &str = "https://github.com/maddendeluxe/madden09deluxe.git";

/// The target folder name (typically the PS2 game identifier like SLUS-XXXXX)
pub const SLUS_FOLDER: &str = "SLUS-21770";

/// Path within the repo to sparse checkout
pub const SPARSE_PATH: &str = "textures/SLUS-21770";

/// Temporary directory name used during clone
pub const TEMP_DIR_NAME: &str = "_temp_madden09deluxe_repo";
