//! Open a URL in the user's browser from inside an AppImage.
//!
//! Clicking a link did nothing on SteamOS while working on Ubuntu. Two separate faults,
//! both observed on a Valve Steam Machine running SteamOS 3.8.16, and both about search
//! order rather than the bundled libraries first suspected:
//!
//! 1. **The bundle's own `xdg-open` ran.** `AppRun` puts `$APPDIR/usr/bin` first on
//!    `PATH`, and Tauri bundles the build image's `xdg-open`. Ours is xdg-utils 1.1.3
//!    from jammy, whose `open_kde` has cases for KDE 4 and 5 only. On Plasma 6, which
//!    is what SteamOS runs, it matches nothing, runs nothing, then `exit_success`: a
//!    silent no-op reported as success. GNOME takes another branch, which is why Ubuntu
//!    was fine.
//! 2. **The wrong `.desktop` file won.** `AppRun` prepends `$APPDIR/usr/share/` and a
//!    literal `/usr/share` to `XDG_DATA_DIRS`. Dropping only the AppDir entries leaves
//!    `/usr/share` ahead of the Flatpak export directories, and SteamOS ships a stub
//!    `/usr/share/applications/org.mozilla.firefox.desktop` ("Install Firefox", which
//!    opens Discover) that the real Flatpak entry is meant to shadow. So a click opened
//!    the app store. See `scrub_list`.
//!
//! `tauri-plugin-opener` and the `open` crate underneath it guard against neither: there
//! is not one `env_remove` in that crate, and it looks its openers up on `PATH`. So
//! links go through this module, which resolves every program to an absolute path proven
//! to be outside the AppImage and hands it the desktop's own environment. The same leak
//! broke `git-remote-https` and `systemd-inhibit` here before; see `use_system_libraries`
//! in `install.rs`, which this deliberately does not reuse, because that helper is
//! private to that file and covers only a fixed list of variables.
//!
//! Every step is written to the debug log (`debug_log.rs`), because the original failure
//! was completely silent and cost several builds to find.

use super::debug_log::log_line;

#[cfg(target_os = "linux")]
mod linux {
    use super::super::debug_log::{log_line, open_for_append};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const GRACE: Duration = Duration::from_secs(3);
    const POLL: Duration = Duration::from_millis(50);

    pub struct Method {
        pub label: String,
        pub program: String,
        pub args: Vec<String>,
    }

    /// Directories that belong to the AppImage. Anything under these is never executed
    /// and never left in a child's environment.
    ///
    /// `$APPDIR` is what the AppImage runtime sets, but the app can also be started from
    /// an extracted tree or a wrapper that does not set it, so the root is derived from
    /// the executable as well: `<root>/usr/bin/<exe>` with an `AppRun` beside `usr`.
    /// The `AppRun` test matters -- a .deb install lives in `/usr/bin` too, and must not
    /// make `/` look like a bundle.
    pub fn bundle_roots() -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(dir) = std::env::var_os("APPDIR").filter(|d| !d.is_empty()) {
            roots.push(PathBuf::from(dir));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(root) = exe.parent().and_then(Path::parent).and_then(Path::parent) {
                if root.join("AppRun").exists() && root != Path::new("/") {
                    roots.push(root.to_path_buf());
                    if let Ok(real) = root.canonicalize() {
                        roots.push(real);
                    }
                }
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }

    pub fn inside_bundle(entry: &str, roots: &[PathBuf]) -> bool {
        roots.iter().any(|root| {
            let root = root.to_string_lossy();
            let root = root.trim_end_matches('/');
            !root.is_empty() && entry.contains(root)
        })
    }

    /// A colon-separated list put back the way the desktop had it. `None` when nothing
    /// is left: the variable must then be unset, not set empty -- an empty
    /// `XDG_DATA_DIRS` means "no data dirs", not "the default".
    ///
    /// Dropping the AppImage's entries is not enough. `AppRun` prepends
    /// `$APPDIR/usr/share/:/usr/share:` to `XDG_DATA_DIRS`, and that literal `/usr/share`
    /// survives the scrub *in front of* the user's own directories. Order is priority:
    /// SteamOS ships a stub `/usr/share/applications/org.mozilla.firefox.desktop`
    /// ("Install Firefox", opens Discover) that the real Flatpak entry, in
    /// `/var/lib/flatpak/exports/share`, is meant to shadow. With `/usr/share` first the
    /// stub won and a link opened the app store instead of the browser. Whatever AppRun
    /// prepends that the user already had shows up twice, so keep the LAST occurrence of
    /// each entry: that is the one in the user's original position.
    pub fn scrub_list(value: &str, roots: &[PathBuf]) -> Option<String> {
        let entries: Vec<&str> = value
            .split(':')
            .filter(|entry| !entry.is_empty() && !inside_bundle(entry, roots))
            .collect();
        let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
        let kept: Vec<&str> = entries
            .iter()
            .enumerate()
            .filter(|(i, entry)| !entries[i + 1..].iter().any(|later| same(entry, later)))
            .map(|(_, entry)| *entry)
            .collect();
        if kept.is_empty() {
            None
        } else {
            Some(kept.join(":"))
        }
    }

    /// The directories a system program may be found in: the user's PATH without the
    /// AppImage, then the standard locations in case PATH was reduced to nothing.
    fn search_dirs(roots: &[PathBuf]) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(path) = std::env::var("PATH") {
            if let Some(clean) = scrub_list(&path, roots) {
                dirs.extend(clean.split(':').map(PathBuf::from));
            }
        }
        for standard in ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            dirs.push(PathBuf::from(standard));
        }
        let mut seen = std::collections::HashSet::new();
        dirs.retain(|d| seen.insert(d.clone()));
        dirs
    }

    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    /// Absolute path of a system program, never one inside the AppImage.
    pub fn resolve(program: &str, roots: &[PathBuf]) -> Option<PathBuf> {
        search_dirs(roots)
            .into_iter()
            .map(|dir| dir.join(program))
            .find(|candidate| {
                is_executable(candidate)
                    && !inside_bundle(&candidate.to_string_lossy(), roots)
                    && !candidate
                        .canonicalize()
                        .map(|real| inside_bundle(&real.to_string_lossy(), roots))
                        .unwrap_or(false)
            })
    }

    /// Give a child the environment it would have had if it had been started from the
    /// desktop rather than from inside the AppImage. Returns what was changed, for the log.
    pub fn use_system_environment(cmd: &mut Command, roots: &[PathBuf]) -> Vec<String> {
        let mut changes: Vec<String> = Vec::new();

        // Loader and interpreter variables: a system program never needs ours.
        // GDK_BACKEND and WEBKIT_DISABLE_DMABUF_RENDERER are set for our own webview and
        // have no business in the user's browser.
        for var in [
            "LD_LIBRARY_PATH", "LD_PRELOAD", "PERLLIB", "PYTHONPATH", "PYTHONHOME",
            "GDK_BACKEND", "WEBKIT_DISABLE_DMABUF_RENDERER", "APPDIR", "APPIMAGE", "ARGV0", "OWD",
        ] {
            if std::env::var_os(var).is_some() {
                cmd.env_remove(var);
                changes.push(format!("unset {var}"));
            }
        }

        for (key, value) in std::env::vars() {
            if !inside_bundle(&value, roots) || changes.iter().any(|c| c == &format!("unset {key}")) {
                continue;
            }
            match scrub_list(&value, roots) {
                Some(clean) => {
                    changes.push(format!("{key}={clean}"));
                    cmd.env(&key, clean);
                }
                None => {
                    changes.push(format!("unset {key}"));
                    cmd.env_remove(&key);
                }
            }
        }

        // Rust looks the program up on the child's PATH once PATH has been touched; we
        // pass absolute paths anyway, so this is for whatever the child execs next.
        if let Ok(path) = std::env::var("PATH") {
            let clean = scrub_list(&path, roots).unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
            cmd.env("PATH", clean);
        }
        changes
    }

    /// The same openers `tauri-plugin-opener`'s `open` crate tries, in its order, plus
    /// the desktop portal. The fix is that these now resolve outside the AppImage with
    /// the desktop's own environment; the list itself is deliberately not longer than
    /// what the plugin already attempted.
    pub fn methods(url: &str) -> Vec<Method> {
        let m = |label: &str, program: &str, args: &[&str]| Method {
            label: label.to_string(),
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        };
        vec![
            m("xdg-open", "xdg-open", &[url]),
            m("gio open", "gio", &["open", url]),
            m("gnome-open", "gnome-open", &[url]),
            m("kde-open", "kde-open", &[url]),
            m("portal via gdbus", "gdbus", &[
                "call", "--session", "--dest", "org.freedesktop.portal.Desktop",
                "--object-path", "/org/freedesktop/portal/desktop",
                "--method", "org.freedesktop.portal.OpenURI.OpenURI", "", url, "{}",
            ]),
        ]
    }

    pub enum Outcome {
        /// Exited 0, or still running after the grace period.
        Accepted(String),
        NotInstalled,
        Failed(String),
    }

    /// Run one method and say what happened. The child's stdout and stderr go straight
    /// into the debug log, so nothing it prints is lost and no pipe can fill up.
    pub fn try_method(method: &Method, roots: &[PathBuf]) -> Outcome {
        let program = match resolve(&method.program, roots) {
            Some(p) => p,
            None => {
                log_line("open", &format!("[{}] not installed (no `{}` outside the AppImage)", method.label, method.program));
                return Outcome::NotInstalled;
            }
        };

        let mut cmd = Command::new(&program);
        cmd.args(&method.args).stdin(Stdio::null());
        match (open_for_append(), open_for_append()) {
            (Some(out), Some(err)) => {
                cmd.stdout(Stdio::from(out)).stderr(Stdio::from(err));
            }
            _ => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            // Our own cwd can be inside the AppImage's FUSE mount.
            cmd.current_dir(home);
        }
        let changes = use_system_environment(&mut cmd, roots);
        log_line("open", &format!("[{}] running {} {:?}", method.label, program.display(), method.args));
        log_line("open", &format!("[{}] environment changes: {}", method.label, changes.join(" | ")));

        let started = Instant::now();
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                let why = format!("could not start {}: {e}", program.display());
                log_line("open", &format!("[{}] {why}", method.label));
                return Outcome::Failed(why);
            }
        };
        log_line("open", &format!("[{}] pid {}", method.label, child.id()));

        let described = format!("{} ({})", method.label, program.display());
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    log_line("open", &format!("[{}] exited 0 after {:?} -- counted as success", method.label, started.elapsed()));
                    return Outcome::Accepted(described);
                }
                Ok(Some(status)) => {
                    log_line("open", &format!("[{}] FAILED: {status} after {:?} (its output is just above)", method.label, started.elapsed()));
                    return Outcome::Failed(format!("{}: {status}", method.label));
                }
                Ok(None) if started.elapsed() >= GRACE => {
                    log_line("open", &format!("[{}] still running after {GRACE:?} -- counted as success", method.label));
                    let label = method.label.clone();
                    std::thread::spawn(move || {
                        let status = child.wait();
                        log_line("open", &format!("[{label}] finally exited: {status:?} after {:?}", started.elapsed()));
                    });
                    return Outcome::Accepted(described);
                }
                Ok(None) => std::thread::sleep(POLL),
                Err(e) => {
                    log_line("open", &format!("[{}] wait failed: {e}", method.label));
                    return Outcome::Failed(format!("{}: {e}", method.label));
                }
            }
        }
    }

    /// Open `url` with the first method that works.
    pub fn open(url: &str) -> Result<(), String> {
        let roots = bundle_roots();
        let list = methods(url);
        log_line("open", &format!("request for {url}; bundle roots {roots:?}"));

        let mut failures: Vec<String> = Vec::new();
        for method in &list {
            match try_method(method, &roots) {
                Outcome::Accepted(described) => {
                    log_line("open", &format!("handed to {described}"));
                    return Ok(());
                }
                Outcome::NotInstalled => {}
                Outcome::Failed(why) => failures.push(why),
            }
        }

        let detail = if failures.is_empty() {
            "none of them is installed".to_string()
        } else {
            failures.join("; ")
        };
        log_line("open", &format!("every method failed: {detail}"));
        Err(format!("No method could open a browser ({detail}). Details are in the debug log."))
    }

    /// Run a short system command and put its output in the log. For the startup survey.
    fn survey_command(program: &str, args: &[&str], roots: &[PathBuf]) {
        let Some(path) = resolve(program, roots) else {
            log_line("survey", &format!("{program}: not installed"));
            return;
        };
        let mut cmd = Command::new(&path);
        cmd.args(args).stdin(Stdio::null());
        use_system_environment(&mut cmd, roots);
        match cmd.output() {
            Ok(out) => log_line("survey", &format!(
                "{} {:?} -> {} | stdout: {} | stderr: {}",
                path.display(), args, out.status,
                String::from_utf8_lossy(&out.stdout).trim(),
                String::from_utf8_lossy(&out.stderr).trim(),
            )),
            Err(e) => log_line("survey", &format!("{} {:?} -> could not run: {e}", path.display(), args)),
        }
    }

    /// Record, once per session, what this desktop has to open links with -- so the log
    /// explains a failure even if the user never clicks a link.
    pub fn survey() {
        let roots = bundle_roots();
        log_line("survey", &format!("bundle roots: {roots:?}"));
        log_line("survey", &format!("search dirs: {:?}", search_dirs(&roots)));
        for root in &roots {
            let bundled = root.join("usr/bin/xdg-open");
            if bundled.exists() {
                log_line("survey", &format!("the AppImage bundles its own xdg-open at {} (never used)", bundled.display()));
            }
        }
        if let Ok(path) = std::env::var("PATH") {
            let first = path.split(':').find_map(|dir| {
                let candidate = Path::new(dir).join("xdg-open");
                is_executable(&candidate).then_some(candidate)
            });
            log_line("survey", &format!("an unguarded PATH lookup of xdg-open would have run: {first:?}"));
        }
        let mut seen = std::collections::HashSet::new();
        for method in methods("https://example.invalid/") {
            if seen.insert(method.program.clone()) {
                log_line("survey", &format!("{} -> {:?}", method.program, resolve(&method.program, &roots)));
            }
        }
        survey_command("xdg-open", &["--version"], &roots);
        survey_command("xdg-settings", &["get", "default-web-browser"], &roots);
        survey_command("xdg-mime", &["query", "default", "x-scheme-handler/https"], &roots);
    }

    /// `--diagnose-open`: try EVERY installed method in turn, regardless of exit codes.
    /// Each one that works opens a tab; the log says which was which.
    pub fn diagnose_all(url: &str) {
        let roots = bundle_roots();
        survey();
        for (index, method) in methods(url).iter().enumerate() {
            let tagged = Method {
                label: method.label.clone(),
                program: method.program.clone(),
                args: method.args.iter().map(|a| if a == url { format!("{url}#method-{index}") } else { a.clone() }).collect(),
            };
            match try_method(&tagged, &roots) {
                Outcome::NotInstalled => {}
                Outcome::Accepted(_) | Outcome::Failed(_) => {
                    println!("method {index}: {} -- did a browser tab ending in #method-{index} open?", method.label);
                    std::thread::sleep(Duration::from_secs(4));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn roots() -> Vec<PathBuf> {
            vec![PathBuf::from("/tmp/.mount_MaddenAbC123")]
        }

        #[test]
        fn scrub_list_drops_only_bundle_entries() {
            let value = "/tmp/.mount_MaddenAbC123/usr/bin/:/tmp/.mount_MaddenAbC123/bin/:/usr/local/bin:/usr/bin";
            assert_eq!(scrub_list(value, &roots()).as_deref(), Some("/usr/local/bin:/usr/bin"));
        }

        #[test]
        fn scrub_list_handles_the_gtk_hooks_double_slash() {
            let value = "/tmp/.mount_MaddenAbC123//usr/lib/gtk-3.0:/usr/lib64/gtk-3.0";
            assert_eq!(scrub_list(value, &roots()).as_deref(), Some("/usr/lib64/gtk-3.0"));
        }

        #[test]
        fn scrub_list_restores_the_users_order() {
            // Verbatim from a Steam Machine (SteamOS 3.8.16), 2026-09-19. Before this, the
            // result began with /usr/share and links opened Discover's "Install Firefox".
            let roots = vec![PathBuf::from("/tmp/.mount_MaddenHaNMPB")];
            let value = "/tmp/.mount_MaddenHaNMPB/usr/share/:/tmp/.mount_MaddenHaNMPB/usr/share:/usr/share:/home/deck/.local/share/flatpak/exports/share:/var/lib/flatpak/exports/share:/usr/local/share:/usr/share";
            assert_eq!(
                scrub_list(value, &roots).as_deref(),
                Some("/home/deck/.local/share/flatpak/exports/share:/var/lib/flatpak/exports/share:/usr/local/share:/usr/share")
            );
        }

        #[test]
        fn scrub_list_unsets_rather_than_empties() {
            assert_eq!(scrub_list("/tmp/.mount_MaddenAbC123/usr/share", &roots()), None);
        }

        #[test]
        fn a_trailing_slash_on_the_root_still_matches() {
            let roots = vec![PathBuf::from("/tmp/.mount_MaddenAbC123/")];
            assert!(inside_bundle("/tmp/.mount_MaddenAbC123/usr/bin/xdg-open", &roots));
            assert!(!inside_bundle("/usr/bin/xdg-open", &roots));
        }

        #[test]
        fn resolve_never_returns_a_bundled_program() {
            // A fake AppImage with its own xdg-open, first on PATH: the bug itself.
            let dir = std::env::temp_dir().join(format!("opener-test-{}", std::process::id()));
            let bin = dir.join("usr/bin");
            std::fs::create_dir_all(&bin).unwrap();
            let fake = bin.join("xdg-open");
            std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

            let old = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{}:{old}", bin.display()));
            let found = resolve("xdg-open", &[dir.clone()]);
            // Without the guard the same lookup does find the fake: the test has power.
            let unguarded = resolve("xdg-open", &[]);
            std::env::set_var("PATH", old);
            let _ = std::fs::remove_dir_all(&dir);

            assert_eq!(unguarded.as_deref(), Some(fake.as_path()));
            assert!(found.map_or(true, |p| !p.starts_with(&dir)));
        }

        #[test]
        fn xdg_open_is_tried_first_and_the_url_is_passed_whole() {
            let url = "https://github.com/settings/personal-access-tokens/new?name=A+B&x=1";
            let list = methods(url);
            assert_eq!(list[0].program, "xdg-open");
            assert!(list.iter().all(|m| m.args.iter().any(|a| a == url)));
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{diagnose_all, survey};

/// Open an http(s) URL in the user's default browser.
#[tauri::command]
pub async fn open_external(#[allow(unused_variables)] app: tauri::AppHandle, url: String) -> Result<(), String> {
    // Only ever hand a browser URL to the desktop. Nothing here goes through a shell, so
    // this is not about quoting -- it keeps `file://` and arbitrary schemes away from
    // the system handler.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        log_line("open", &format!("refused non-http(s) URL: {url}"));
        return Err(format!("refusing to open non-http(s) URL: {url}"));
    }

    #[cfg(target_os = "linux")]
    {
        // The methods wait on a child process; keep that off the async runtime's workers.
        let target = url.clone();
        tauri::async_runtime::spawn_blocking(move || linux::open(&target))
            .await
            .map_err(|e| format!("opener task failed: {e}"))?
    }
    #[cfg(not(target_os = "linux"))]
    {
        use tauri_plugin_opener::OpenerExt;
        log_line("open", &format!("request for {url} (opener plugin)"));
        app.opener().open_url(url, None::<&str>).map_err(|e| {
            log_line("open", &format!("opener plugin failed: {e}"));
            e.to_string()
        })
    }
}
