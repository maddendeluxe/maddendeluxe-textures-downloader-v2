mod commands;
mod config;

use commands::{
    backup_existing_folder, check_existing_folder, check_git_installed, cleanup_processes,
    delete_existing_folder, get_git_error, start_installation, validate_directory,
    // State management
    load_state, save_state, set_textures_path, mark_setup_complete,
    update_last_sync_commit, set_initial_setup_done, set_github_token,
    set_sync_disclaimer_acknowledged,
    // Sync
    get_latest_commit, run_sync, check_sync_status,
    run_verification_scan, apply_verification_fixes, run_quick_count_check,
    analyze_full_sync, execute_analyzed_sync,
    // App info
    get_app_version, fetch_installer_data, compare_versions,
    // Opening links in the system browser
    open_external,
    // Debug log (~/textures-downloader-debug.log)
    frontend_log, get_debug_log_path,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Everything below is recorded in ~/textures-downloader-debug.log, so a bug report
    // from a machine we cannot reach comes with its own evidence.
    commands::debug_log::start_session(env!("CARGO_PKG_VERSION"));

    // `--diagnose-open [url]`: try every way of opening a link, one after another,
    // without starting the window. Run from a terminal; see open_external.rs.
    #[cfg(target_os = "linux")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(at) = args.iter().position(|a| a == "--diagnose-open") {
            let url = args.get(at + 1).cloned().unwrap_or_else(|| "https://github.com/".to_string());
            commands::diagnose_all(&url);
            println!("Log written to {}", commands::debug_log::log_path().display());
            return;
        }
        // What this desktop can open links with, recorded before anyone clicks one.
        std::thread::spawn(commands::survey);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            check_existing_folder,
            backup_existing_folder,
            delete_existing_folder,
            validate_directory,
            check_git_installed,
            get_git_error,
            start_installation,
            // State management
            load_state,
            save_state,
            set_textures_path,
            mark_setup_complete,
            update_last_sync_commit,
            set_initial_setup_done,
            set_github_token,
            set_sync_disclaimer_acknowledged,
            // Sync
            get_latest_commit,
            run_sync,
            check_sync_status,
            run_verification_scan,
            apply_verification_fixes,
            run_quick_count_check,
            analyze_full_sync,
            execute_analyzed_sync,
            // App info
            get_app_version,
            fetch_installer_data,
            compare_versions,
            // Opening links in the system browser
            open_external,
            // Debug log
            frontend_log,
            get_debug_log_path,
        ])
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Kill any running git processes when window is closed
                cleanup_processes();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
