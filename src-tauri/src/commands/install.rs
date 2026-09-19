use crate::config::{REPO_URL, SLUS_FOLDER, SPARSE_PATH, TEMP_DIR_NAME};
use regex::Regex;
use serde::Serialize;
use std::io::{BufReader, Read as IoRead};
use std::path::PathBuf;
#[cfg(not(target_os = "windows"))]
use std::process::{Command, Stdio};
#[cfg(target_os = "windows")]
use std::process::Command;
use std::fs;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Window};

// Track running process PIDs so we can kill them on app exit
static RUNNING_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Kill all tracked processes (called on app exit)
pub fn cleanup_processes() {
    if let Ok(pids) = RUNNING_PIDS.lock() {
        for pid in pids.iter() {
            #[cfg(target_os = "windows")]
            {
                // Use taskkill to kill the process tree
                let _ = Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .output();
            }
            #[cfg(not(target_os = "windows"))]
            {
                // On Linux the runner gives the wrapper its own process group
                // (see run_git_with_pty), so kill the group first; then the pid
                // itself, which is all macOS's caffeinate needs.
                let mut group = Command::new("kill");
                use_system_libraries(&mut group);
                let _ = group.args(["-9", "--", &format!("-{}", pid)]).output();

                let mut single = Command::new("kill");
                use_system_libraries(&mut single);
                let _ = single.args(["-9", &pid.to_string()]).output();
            }
        }
    }
}

#[derive(Clone, Serialize)]
pub struct ProgressPayload {
    pub stage: String,
    pub message: String,
    pub percent: Option<u32>,
}

/// Get the path to git executable
/// On Windows x64, use bundled MinGit if available
/// On Windows ARM, require system git
/// On macOS and Linux, use system git
fn get_git_path() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let is_arm = cfg!(target_arch = "aarch64");

        // On x64, check for bundled MinGit first
        if !is_arm {
            if let Ok(exe_path) = std::env::current_exe() {
                if let Some(exe_dir) = exe_path.parent() {
                    // Try multiple possible resource paths
                    let paths_to_try = [
                        // Full nested path
                        exe_dir.join("resources").join("mingit").join("x64").join("cmd").join("git.exe"),
                        // Flattened cmd folder
                        exe_dir.join("resources").join("cmd").join("git.exe"),
                        // Direct in resources
                        exe_dir.join("resources").join("git.exe"),
                    ];

                    for mingit_path in &paths_to_try {
                        if mingit_path.exists() {
                            return Ok(mingit_path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        // Fall back to system git
        if Command::new("git").arg("--version").output().is_ok() {
            return Ok("git".to_string());
        }

        // Build error message based on architecture
        if is_arm {
            Err("Git not found. On Windows ARM, please install Git manually from https://git-scm.com/download/win".to_string())
        } else {
            let mut err_msg = String::from("Git not found. Searched locations:\n");
            if let Ok(exe_path) = std::env::current_exe() {
                if let Some(exe_dir) = exe_path.parent() {
                    err_msg.push_str(&format!("  - {}\\resources\\mingit\\x64\\cmd\\git.exe\n", exe_dir.display()));
                    err_msg.push_str(&format!("  - {}\\resources\\cmd\\git.exe\n", exe_dir.display()));
                    err_msg.push_str(&format!("  - {}\\resources\\git.exe\n", exe_dir.display()));
                }
            }
            err_msg.push_str("  - System PATH\n");
            err_msg.push_str("\nPlease reinstall the app or install Git from https://git-scm.com/download/win");
            Err(err_msg)
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // On macOS/Linux, check for system git. Probe it the same way it will be run:
        // inside an AppImage the bundled libraries would otherwise be on its path, and a
        // git that cannot start looks exactly like a git that is not installed.
        let mut probe = Command::new("git");
        use_system_libraries(&mut probe);
        if probe.arg("--version").output().is_ok() {
            return Ok("git".to_string());
        }

        #[cfg(target_os = "macos")]
        let hint = "Please install Xcode Command Line Tools by running: xcode-select --install";
        #[cfg(not(target_os = "macos"))]
        let hint = "Please install it with your package manager (for example: sudo apt install git, \
                    sudo dnf install git, or sudo pacman -S git) and then restart this app.";

        Err(format!("Git not found. {}", hint))
    }
}

/// Check if git is available
#[tauri::command]
pub fn check_git_installed() -> Result<bool, String> {
    match get_git_path() {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Get the git installation error message (for display to user)
#[tauri::command]
pub fn get_git_error() -> String {
    match get_git_path() {
        Ok(_) => String::new(),
        Err(e) => e,
    }
}

/// Strip ANSI escape codes from a string
fn strip_ansi_codes(s: &str) -> String {
    let ansi_re = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    ansi_re.replace_all(s, "").to_string()
}

/// Detect the stage and percentage from git output
fn detect_git_stage(line: &str) -> (Option<&'static str>, Option<u32>) {
    let percent_re = Regex::new(r"(\d+)%").ok();
    let percent = percent_re
        .as_ref()
        .and_then(|re| re.captures(line))
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse().ok());

    if line.contains("Receiving objects:") {
        return (Some("downloading"), percent);
    }
    if line.contains("Updating files:") {
        return (Some("extracting"), percent);
    }
    if line.contains("Resolving deltas:") {
        return (Some("downloading"), percent);
    }
    if line.contains("Compressing objects:") {
        return (Some("compressing"), percent);
    }
    if line.contains("Enumerating objects:") || line.contains("Counting objects:") {
        return (Some("compressing"), percent);
    }
    if line.contains("remote:") {
        return (Some("compressing"), percent);
    }

    (None, percent)
}

/// Read output handling both \r and \n as line terminators
/// Git uses \r to update progress on the same line
/// When detect_stages is false, always uses default_stage
/// Returns the last few lines of output for error reporting
fn read_output_with_progress<R: IoRead>(
    reader: R,
    window: &Window,
    default_stage: &str,
    detect_stages: bool,
    recent_lines: Option<Arc<Mutex<Vec<String>>>>
) {
    let mut buf_reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        match buf_reader.read(&mut byte) {
            Ok(0) => break, // EOF
            Ok(_) => {
                if byte[0] == b'\r' || byte[0] == b'\n' {
                    if !buffer.is_empty() {
                        if let Ok(line) = String::from_utf8(buffer.clone()) {
                            let line = strip_ansi_codes(line.trim());
                            if !line.is_empty() {
                                // Store recent lines for error reporting
                                if let Some(ref lines) = recent_lines {
                                    if let Ok(mut lines) = lines.lock() {
                                        lines.push(line.clone());
                                        // Keep only the last 10 lines
                                        if lines.len() > 10 {
                                            lines.remove(0);
                                        }
                                    }
                                }

                                let (detected_stage, percent) = detect_git_stage(&line);
                                let stage = if detect_stages {
                                    detected_stage.unwrap_or(default_stage)
                                } else {
                                    default_stage
                                };

                                let _ = window.emit(
                                    "install-progress",
                                    ProgressPayload {
                                        stage: stage.to_string(),
                                        message: line,
                                        percent,
                                    },
                                );
                            }
                        }
                        buffer.clear();
                    }
                } else {
                    buffer.push(byte[0]);
                }
            }
            Err(_) => break,
        }
    }

    // Handle any remaining data in buffer
    if !buffer.is_empty() {
        if let Ok(line) = String::from_utf8(buffer) {
            let line = strip_ansi_codes(line.trim());
            if !line.is_empty() {
                // Store recent lines for error reporting
                if let Some(ref lines) = recent_lines {
                    if let Ok(mut lines) = lines.lock() {
                        lines.push(line.clone());
                        if lines.len() > 10 {
                            lines.remove(0);
                        }
                    }
                }

                let (detected_stage, percent) = detect_git_stage(&line);
                let stage = if detect_stages {
                    detected_stage.unwrap_or(default_stage)
                } else {
                    default_stage
                };

                let _ = window.emit(
                    "install-progress",
                    ProgressPayload {
                        stage: stage.to_string(),
                        message: line,
                        percent,
                    },
                );
            }
        }
    }
}

/// Run a git command with PTY support (using the BSD `script` command on macOS)
/// This ensures git outputs progress even when not connected to a real terminal
/// Uses caffeinate to prevent system sleep during long operations
/// When detect_stages is false, always uses default_stage instead of detecting from output
/// Returns Ok(true) on success, Ok(false) on failure with error details, or Err on spawn failure
#[cfg(target_os = "macos")]
fn run_git_with_pty(
    git_path: &str,
    args: &[&str],
    working_dir: &PathBuf,
    window: &Window,
    default_stage: &str,
    detect_stages: bool,
) -> Result<(bool, String), String> {
    // Use 'caffeinate' to prevent sleep, 'script' to create a PTY for git
    // caffeinate -d: prevent display sleep (also prevents screensaver)
    // script -q /dev/null: create PTY without saving typescript
    let mut cmd_args: Vec<&str> = vec!["-d", "script", "-q", "/dev/null", git_path];
    cmd_args.extend(args);

    let mut caffeinate = Command::new("caffeinate");
    use_system_libraries(&mut caffeinate);
    let mut cmd = caffeinate
        .args(&cmd_args)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start command: {}", e))?;

    // Collect recent output for error reporting
    let recent_lines = Arc::new(Mutex::new(Vec::<String>::new()));

    // script command outputs everything to stdout (including what would normally be stderr)
    if let Some(stdout) = cmd.stdout.take() {
        read_output_with_progress(stdout, window, default_stage, detect_stages, Some(recent_lines.clone()));
    }

    let status = cmd
        .wait()
        .map_err(|e| format!("Command failed: {}", e))?;

    // Get recent output for error message
    let error_context = recent_lines.lock()
        .map(|lines| lines.join("\n"))
        .unwrap_or_default();

    Ok((status.success(), error_context))
}

/// Let a system binary see the system's libraries, not the AppImage's.
///
/// An AppImage exports `LD_LIBRARY_PATH` pointing at its own bundled libraries, and every
/// process it starts inherits it. `git` is a system binary: on a distribution whose
/// libcurl is newer than the bundled OpenSSL, `git-remote-https` dies with
/// "version `OPENSSL_3.2.0' not found (required by /usr/lib/libcurl.so.4)" and the clone
/// fails before a byte is transferred. Confirmed on a Steam Deck (SteamOS 3.8.16,
/// git 2.50.1) on 2026-09-18, where `git --version` still worked -- only a real clone
/// reaches the HTTPS helper. The same leak made `systemd-inhibit` fail there.
///
/// Harmless outside an AppImage, where these variables are normally unset, so it is
/// applied on macOS too rather than kept as a Linux special case.
#[cfg(not(target_os = "windows"))]
fn use_system_libraries(cmd: &mut Command) {
    cmd.env_remove("LD_LIBRARY_PATH");
    cmd.env_remove("LD_PRELOAD");
    cmd.env_remove("PERLLIB"); // git's perl subcommands
    cmd.env_remove("PYTHONPATH");
}

/// Quote a string for use inside a POSIX shell command line
#[cfg(target_os = "linux")]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Run a git command on Linux with PTY support.
///
/// util-linux `script -c <cmd>` gives git a pseudo-terminal so it prints live
/// progress even though our stdout is a pipe (`-q` no banner, `-e` propagate
/// git's exit status, `-f` flush as output arrives). When `systemd-inhibit` is
/// usable it also stops the machine from sleeping mid-download. If `script`
/// is not installed, git runs directly: it still works, just without live
/// progress percentages.
/// Returns Ok(true) on success, Ok(false) on failure with error details, or Err on spawn failure
#[cfg(target_os = "linux")]
fn run_git_with_pty(
    git_path: &str,
    args: &[&str],
    working_dir: &PathBuf,
    window: &Window,
    default_stage: &str,
    detect_stages: bool,
) -> Result<(bool, String), String> {
    let have_script = {
        let mut c = Command::new("script");
        use_system_libraries(&mut c);
        c.arg("--version").output().is_ok()
    };
    // Probe by actually taking a lock: `--list` can succeed on systems where
    // taking one is denied (containers, WSL, no logind session).
    let have_inhibit = {
        let mut c = Command::new("systemd-inhibit");
        use_system_libraries(&mut c);
        c.args(["--what=idle:sleep", "--who=Textures Downloader", "--why=probe", "true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };

    let git_cmdline = std::iter::once(git_path)
        .chain(args.iter().copied())
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    let script_args = ["-qefc", git_cmdline.as_str(), "/dev/null"];

    let mut cmd = if have_script && have_inhibit {
        let mut c = Command::new("systemd-inhibit");
        c.args([
            "--what=idle:sleep",
            "--who=Textures Downloader",
            "--why=Downloading textures",
            "script",
        ])
        .args(script_args);
        c
    } else if have_script {
        let mut c = Command::new("script");
        c.args(script_args);
        c
    } else {
        let mut c = Command::new(git_path);
        c.args(args);
        c
    };

    // git, script and systemd-inhibit are all system binaries: none of them must load
    // the libraries this AppImage carries.
    use_system_libraries(&mut cmd);

    // Put the wrapper in its own process group so closing the app mid-download can
    // kill the whole group at once. This fully cleans up the no-`script` fallback
    // (git and its index-pack child). Note that `script` calls setsid() for the
    // command it hosts, so in the PTY path the git it started lives in a separate
    // session and can briefly outlive the app; the leftover temp directory is
    // removed at the start of the next install either way.
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start command: {}", e))?;

    // Track the PID so the process can be killed if the app exits mid-download
    if let Ok(mut pids) = RUNNING_PIDS.lock() {
        pids.push(child.id());
    }

    // Collect recent output for error reporting
    let recent_lines = Arc::new(Mutex::new(Vec::<String>::new()));

    // `script` merges git's stderr (where progress goes) into the PTY, which we read
    // from stdout; anything left on stderr is then the wrapper's own complaint, so
    // keep it for the error report. Without `script`, git's progress and errors
    // are on stderr.
    if have_script {
        let stderr_lines = recent_lines.clone();
        let stderr_thread = child.stderr.take().map(|stderr| {
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if let Ok(mut lines) = stderr_lines.lock() {
                        lines.push(line);
                    }
                }
            })
        });
        if let Some(stdout) = child.stdout.take() {
            read_output_with_progress(stdout, window, default_stage, detect_stages, Some(recent_lines.clone()));
        }
        if let Some(t) = stderr_thread {
            let _ = t.join();
        }
    } else if let Some(stderr) = child.stderr.take() {
        read_output_with_progress(stderr, window, default_stage, detect_stages, Some(recent_lines.clone()));
    }

    let status = child
        .wait()
        .map_err(|e| format!("Command failed: {}", e))?;

    if let Ok(mut pids) = RUNNING_PIDS.lock() {
        pids.retain(|&p| p != child.id());
    }

    // Get recent output for error message
    let error_context = recent_lines.lock()
        .map(|lines| lines.join("\n"))
        .unwrap_or_default();

    Ok((status.success(), error_context))
}

/// Run a git command on Windows using ConPTY for proper progress output
/// Uses SetThreadExecutionState to prevent system sleep during long operations
/// When detect_stages is false, always uses default_stage instead of detecting from output
/// Returns Ok((true, _)) on success, Ok((false, error_context)) on failure, or Err on spawn failure
#[cfg(target_os = "windows")]
fn run_git_with_pty(
    git_path: &str,
    args: &[&str],
    working_dir: &PathBuf,
    window: &Window,
    default_stage: &str,
    detect_stages: bool,
) -> Result<(bool, String), String> {
    use conpty::spawn;
    use std::io::Read as _;
    use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED, ES_DISPLAY_REQUIRED};

    // Prevent system sleep during the operation
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED);
    }

    let working_dir_str = working_dir.to_string_lossy().to_string();

    // Build command arguments
    // For clone command, replace "." destination with full path
    // For other commands, use -C flag to set working directory
    let is_clone = args.first() == Some(&"clone");

    let full_args: Vec<String> = if is_clone {
        // For clone, replace "." with the full path
        args.iter().map(|arg| {
            if *arg == "." {
                format!("\"{}\"", working_dir_str)
            } else if arg.contains(' ') {
                format!("\"{}\"", arg)
            } else {
                arg.to_string()
            }
        }).collect()
    } else {
        // For other commands, use -C flag
        let mut v: Vec<String> = vec![
            "-C".to_string(),
            format!("\"{}\"", working_dir_str),
        ];
        for arg in args {
            if arg.contains(' ') {
                v.push(format!("\"{}\"", arg));
            } else {
                v.push(arg.to_string());
            }
        }
        v
    };

    // Build command line - use cmd.exe /c wrapper when path has spaces
    // ConPTY doesn't handle quoted executable paths correctly
    let command_line = if git_path.contains(' ') {
        format!("cmd.exe /c \"\"{}\" {}\"", git_path, full_args.join(" "))
    } else {
        format!("{} {}", git_path, full_args.join(" "))
    };

    // Spawn process using ConPTY (Windows Pseudo Console)
    // This makes git think it's connected to a real terminal
    let mut proc = spawn(&command_line)
        .map_err(|e| {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS); }
            format!("Failed to spawn process with ConPTY: {}", e)
        })?;

    // Track the PID so we can kill it if the app closes
    let pid = proc.pid();
    if let Ok(mut pids) = RUNNING_PIDS.lock() {
        pids.push(pid);
    }

    // Read output from the PTY in a separate thread
    // This prevents blocking if the PTY doesn't send EOF properly
    let output = proc.output().map_err(|e| {
        unsafe { SetThreadExecutionState(ES_CONTINUOUS); }
        format!("Failed to get process output: {}", e)
    })?;

    let window_clone = window.clone();
    let default_stage_owned = default_stage.to_string();

    // Collect recent output for error reporting
    let recent_lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let recent_lines_clone = recent_lines.clone();

    let reader_handle = std::thread::spawn(move || {
        let mut output = output;
        let mut buffer = [0u8; 1];
        let mut line_buffer = Vec::new();

        loop {
            match output.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let byte = buffer[0];
                    if byte == b'\r' || byte == b'\n' {
                        if !line_buffer.is_empty() {
                            if let Ok(line) = String::from_utf8(line_buffer.clone()) {
                                let line = line.trim().to_string();
                                if !line.is_empty() {
                                    // Store recent lines for error reporting
                                    if let Ok(mut lines) = recent_lines_clone.lock() {
                                        lines.push(line.clone());
                                        if lines.len() > 10 {
                                            lines.remove(0);
                                        }
                                    }

                                    let (detected_stage, percent) = detect_git_stage(&line);
                                    let stage = if detect_stages {
                                        detected_stage.unwrap_or(&default_stage_owned)
                                    } else {
                                        &default_stage_owned
                                    };

                                    let _ = window_clone.emit(
                                        "install-progress",
                                        ProgressPayload {
                                            stage: stage.to_string(),
                                            message: line,
                                            percent,
                                        },
                                    );
                                }
                            }
                            line_buffer.clear();
                        }
                    } else {
                        line_buffer.push(byte);
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait for process to exit (this returns even if reader is still running)
    let exit_code = proc.wait(None).map_err(|e| {
        unsafe { SetThreadExecutionState(ES_CONTINUOUS); }
        format!("Failed to wait for process: {}", e)
    })?;

    // Remove PID from tracking list
    if let Ok(mut pids) = RUNNING_PIDS.lock() {
        pids.retain(|&p| p != pid);
    }

    // Drop proc to close the PTY, which should cause the reader to get EOF
    drop(proc);

    // Give the reader thread a short time to finish reading any buffered output
    // Don't block forever - if it's stuck, just move on
    let join_timeout = std::time::Duration::from_secs(2);
    let start = std::time::Instant::now();
    while !reader_handle.is_finished() && start.elapsed() < join_timeout {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Don't call join() - if thread is stuck, let it be orphaned

    // Restore normal sleep behavior
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS);
    }

    // Get recent output for error message
    let mut error_context = recent_lines.lock()
        .map(|lines| lines.join("\n"))
        .unwrap_or_default();

    // If command failed, include the command line for debugging
    if exit_code != 0 {
        error_context.push_str(&format!("\n\n[Debug] Command: {}", command_line));
    }

    // Exit code 0 means success
    Ok((exit_code == 0, error_context))
}

/// Run the git sparse checkout installation
#[tauri::command]
pub async fn start_installation(textures_dir: String, window: Window) -> Result<(), String> {
    let git_path = get_git_path()?;
    let textures_path = PathBuf::from(&textures_dir);
    let temp_path = textures_path.join(TEMP_DIR_NAME);
    let final_path = textures_path.join(SLUS_FOLDER);

    // Emit initial progress
    let _ = window.emit(
        "install-progress",
        ProgressPayload {
            stage: "preparing".to_string(),
            message: "Preparing installation...".to_string(),
            percent: Some(0),
        },
    );

    // Clean up any existing temp directory
    if temp_path.exists() {
        fs::remove_dir_all(&temp_path)
            .map_err(|e| format!("Failed to clean temp directory: {}", e))?;
    }

    // Create temp directory (only on macOS/Linux - on Windows, git clone will create it)
    #[cfg(not(target_os = "windows"))]
    fs::create_dir_all(&temp_path)
        .map_err(|e| format!("Failed to create temp directory: {}", e))?;

    // Stage 1: Clone with sparse checkout (this is quick - just metadata)
    let _ = window.emit(
        "install-progress",
        ProgressPayload {
            stage: "cloning".to_string(),
            message: "Initializing repository...".to_string(),
            percent: Some(0),
        },
    );

    let (clone_success, clone_output) = run_git_with_pty(
        &git_path,
        &[
            "clone",
            "--depth=1",
            "--filter=blob:none",
            "--sparse",
            "--progress",
            REPO_URL,
            ".",
        ],
        &temp_path,
        &window,
        "cloning",
        false, // Don't detect stages - keep showing "Initializing repository..."
    )?;

    if !clone_success {
        let _ = fs::remove_dir_all(&temp_path);
        let error_msg = if clone_output.is_empty() {
            "Git clone has failed. Please check your internet connection.".to_string()
        } else {
            format!("Git clone has failed:\n{}", clone_output)
        };
        return Err(error_msg);
    }

    // Stage 2: Set sparse checkout path - THIS IS THE MAIN DOWNLOAD
    let _ = window.emit(
        "install-progress",
        ProgressPayload {
            stage: "downloading".to_string(),
            message: format!("Starting download of {}...", SPARSE_PATH),
            percent: Some(0),
        },
    );

    let (checkout_success, checkout_output) = run_git_with_pty(
        &git_path,
        &["sparse-checkout", "set", SPARSE_PATH],
        &temp_path,
        &window,
        "downloading",
        true, // Detect stages - show compressing/downloading/extracting
    )?;

    if !checkout_success {
        let _ = fs::remove_dir_all(&temp_path);
        let error_msg = if checkout_output.is_empty() {
            "Sparse checkout failed.".to_string()
        } else {
            format!("Sparse checkout failed:\n{}", checkout_output)
        };
        return Err(error_msg);
    }

    // Stage 3: Move folder to final location
    let _ = window.emit(
        "install-progress",
        ProgressPayload {
            stage: "moving".to_string(),
            message: format!("Moving {} to final location...", SLUS_FOLDER),
            percent: Some(0),
        },
    );

    let source_path = temp_path.join("textures").join(SLUS_FOLDER);

    if !source_path.exists() {
        let _ = fs::remove_dir_all(&temp_path);
        return Err(format!(
            "Expected folder {} not found in repository",
            SPARSE_PATH
        ));
    }

    // Move the folder
    fs::rename(&source_path, &final_path)
        .map_err(|e| format!("Failed to move folder to final location: {}", e))?;

    // Stage 4: Cleanup
    let _ = window.emit(
        "install-progress",
        ProgressPayload {
            stage: "cleanup".to_string(),
            message: "Cleaning up temporary files...".to_string(),
            percent: Some(0),
        },
    );

    fs::remove_dir_all(&temp_path)
        .map_err(|e| format!("Failed to clean up temp directory: {}", e))?;

    // Done!
    let _ = window.emit(
        "install-progress",
        ProgressPayload {
            stage: "complete".to_string(),
            message: format!(
                "Installation complete! Textures installed to: {}",
                final_path.display()
            ),
            percent: Some(100),
        },
    );

    Ok(())
}
