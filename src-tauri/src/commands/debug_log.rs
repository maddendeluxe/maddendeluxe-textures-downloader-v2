//! A debug log the user can send back.
//!
//! The "open a link" failure on SteamOS took four builds to pin down because the app
//! said nothing at all: no console, no file, no dialog. Everything that could explain a
//! failure is now appended to one plain-text file in the home directory, and echoed to
//! stderr for anyone who starts the app from a terminal.
//!
//! The file is `~/textures-downloader-debug.log`. It is truncated once it passes
//! `MAX_BYTES`, at startup only, so one session is never cut in half.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const FILE_NAME: &str = "textures-downloader-debug.log";
const MAX_BYTES: u64 = 2 * 1024 * 1024;

static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Where the log lives: the home directory, because that is where a user will look.
pub fn log_path() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(FILE_NAME)
}

/// The log opened for appending, for handing to a child process as its stdout/stderr.
pub fn open_for_append() -> Option<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .ok()
}

/// Append one line. Never fails loudly: a log that cannot be written must not take
/// the app down with it.
pub fn log_line(source: &str, message: &str) {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = format!("{stamp} [{source}] {message}\n");
    eprint!("{line}");
    let _guard = WRITE_LOCK.lock();
    if let Some(mut file) = open_for_append() {
        let _ = file.write_all(line.as_bytes());
    }
}

/// True for a variable whose value must not reach a file the user will share.
pub fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL", "API_KEY", "APIKEY", "COOKIE"]
        .iter()
        .any(|needle| upper.contains(needle))
}

/// Start a session: rotate if needed, then record everything about this machine that
/// has ever mattered to a bug report.
pub fn start_session(version: &str) {
    let path = log_path();
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }

    log_line("session", "==================== new session ====================");
    log_line("session", &format!("app version {version}, game {}", crate::config::GAME_ID));
    log_line("session", &format!("os {} / {}", std::env::consts::OS, std::env::consts::ARCH));
    log_line("session", &format!("exe {:?}", std::env::current_exe()));
    log_line("session", &format!("cwd {:?}", std::env::current_dir()));
    log_line("session", &format!("args {:?}", std::env::args().collect::<Vec<_>>()));

    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
            for line in text.lines().filter(|l| {
                l.starts_with("PRETTY_NAME=") || l.starts_with("VERSION_ID=") || l.starts_with("BUILD_ID=") || l.starts_with("ID=")
            }) {
                log_line("session", &format!("os-release {line}"));
            }
        }
        if let Ok(text) = std::fs::read_to_string("/proc/version") {
            log_line("session", &format!("kernel {}", text.trim()));
        }
    }

    let mut vars: Vec<(String, String)> = std::env::vars().collect();
    vars.sort();
    for (key, value) in vars {
        let shown = if is_secret_name(&key) { "<redacted>".to_string() } else { value };
        log_line("env", &format!("{key}={shown}"));
    }
}

/// Where the debug log is, for the UI to show.
#[tauri::command]
pub fn get_debug_log_path() -> String {
    log_path().to_string_lossy().into_owned()
}

/// The frontend's console, errors and link clicks end up in the same file.
#[tauri::command]
pub fn frontend_log(level: String, message: String) {
    // Bound what a runaway console.log loop can write.
    let mut text: String = message.chars().take(4000).collect();
    if text.len() < message.len() {
        text.push_str(" ...[truncated]");
    }
    log_line(&format!("js:{level}"), &text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_names_are_recognised() {
        assert!(is_secret_name("GITHUB_TOKEN"));
        assert!(is_secret_name("my_api_key"));
        assert!(!is_secret_name("XAUTHORITY"));
        assert!(!is_secret_name("PATH"));
        assert!(!is_secret_name("LD_LIBRARY_PATH"));
    }
}
