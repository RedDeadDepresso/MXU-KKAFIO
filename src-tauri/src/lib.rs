pub mod commands;
mod tray;
mod web_server;
pub mod ws_broadcast;

use commands::{AppConfigState, AppState, KkafioState};
use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};
use ws_broadcast::WsBroadcast;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let logs_dir = commands::utils::get_logs_dir();
    let _ = std::fs::create_dir_all(&logs_dir);

    // Auto-migrate legacy registry autostart → task scheduler
    #[cfg(windows)]
    commands::system::migrate_legacy_autostart();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart".into()]),
        ))
        .plugin(
            tauri_plugin_log::Builder::new()
                .max_file_size(1 * 1024 * 1024)
                .rotation_strategy(RotationStrategy::KeepSome(8))
                .targets({
                    #[allow(unused_mut)]
                    let mut targets = vec![Target::new(TargetKind::Folder {
                        path: logs_dir,
                        file_name: Some("mxu-tauri".into()),
                    })];
                    #[cfg(debug_assertions)]
                    targets.push(Target::new(TargetKind::Stdout));
                    targets
                })
                .timezone_strategy(TimezoneStrategy::UseLocal)
                .level(log::LevelFilter::Debug)
                .build(),
        )
        .setup(|app| {
            // Create shared state objects
            let app_state = Arc::new(AppState::default());
            let app_config = Arc::new(AppConfigState::default());
            let kkafio_state = Arc::new(KkafioState::default());
            let ws_broadcast = Arc::new(WsBroadcast::new(256));

            // Load interface.json from the exe directory
            match commands::utils::get_exe_directory() {
                Ok(exe_dir) => {
                    app_config.load_interface(&exe_dir);
                }
                Err(e) => {
                    log::warn!("AppConfigState: could not get exe dir: {}", e);
                }
            }

            // Load config file
            match commands::utils::get_app_data_dir() {
                Ok(data_dir) => {
                    app_config.load_config(&data_dir);
                }
                Err(e) => {
                    log::warn!("AppConfigState: could not get data dir: {}", e);
                }
            }

            // Register shared state
            app.manage(ws_broadcast.clone());
            app.manage(app_state.clone());
            app.manage(app_config.clone());
            app.manage(kkafio_state);

            // Start the HTTP web server (background tokio task)
            {
                let cfg_clone = app_config.clone();
                let ws_clone = ws_broadcast.clone();
                let app_handle = app.handle().clone();

                let settings = app_config.config.lock().unwrap();
                let settings_obj = settings.get("settings");

                let allow_lan_access = settings_obj
                    .and_then(|s| s.get("allowLanAccess"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let web_port = settings_obj
                    .and_then(|s| s.get("webServerPort"))
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u16::try_from(v).ok())
                    .filter(|&p| p > 0)
                    .unwrap_or(web_server::DEFAULT_PORT);

                drop(settings);

                tauri::async_runtime::spawn(async move {
                    web_server::start_web_server(
                        cfg_clone,
                        app_handle,
                        ws_clone,
                        web_port,
                        allow_lan_access,
                    )
                    .await;
                });
            }

            // Remove system title bar on Windows (custom title bar in use)
            #[cfg(target_os = "windows")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_decorations(false);
                }
            }

            // Clean up leftover cache/old directory on startup
            if let Ok(data_dir) = commands::get_data_dir() {
                let old_dir = std::path::Path::new(&data_dir).join("cache").join("old");
                if old_dir.exists() {
                    std::thread::spawn(move || {
                        let (deleted, failed) = commands::cleanup_dir_contents(&old_dir);
                        if deleted > 0 || failed > 0 {
                            if failed == 0 {
                                log::info!("Cleaned up cache/old: {} items deleted", deleted);
                            } else {
                                log::warn!(
                                    "Cleaned up cache/old: {} deleted, {} failed",
                                    deleted,
                                    failed
                                );
                            }
                        }
                    });
                }
            }

            // Initialize system tray
            if let Err(e) = tray::init_tray(app.handle()) {
                log::error!("Failed to initialize system tray: {}", e);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // KKAFIO process commands
            commands::kkafio::kkafio_start,
            commands::kkafio::kkafio_stop,
            commands::kkafio::kkafio_is_running,
            commands::kkafio::kkafio_group_chara_export,
            commands::kkafio::kkafio_rename_chara_export,
            commands::kkafio::kkafio_trash,
            commands::kkafio::kkafio_run_game,
            // File operation commands
            commands::file_ops::read_local_file,
            commands::file_ops::read_local_file_base64,
            commands::file_ops::local_file_exists,
            commands::file_ops::get_exe_dir,
            commands::file_ops::get_data_dir,
            commands::file_ops::clear_log_files,
            commands::file_ops::get_cwd,
            commands::file_ops::check_exe_path,
            commands::file_ops::set_executable,
            commands::file_ops::export_logs,
            // Log / state commands
            commands::state::log_to_stdout,
            commands::state::push_log,
            commands::state::get_all_logs,
            commands::state::clear_instance_logs,
            // Update / install commands
            commands::update::extract_zip,
            commands::update::check_changes_json,
            commands::update::apply_incremental_update,
            commands::update::apply_full_update,
            commands::update::cleanup_extract_dir,
            commands::update::fallback_update,
            commands::update::move_file_to_old,
            commands::update::cleanup_update_artifacts,
            // Download commands
            commands::download::get_github_release_by_version,
            commands::download::download_file,
            commands::download::cancel_download,
            // System commands
            commands::system::is_elevated,
            commands::system::is_autostart,
            commands::system::get_start_instance,
            commands::system::has_quit_after_run_flag,
            commands::system::restart_as_admin,
            commands::system::open_file,
            commands::system::run_and_wait,
            commands::system::is_process_running,
            commands::system::get_process_path_from_hwnd,
            commands::system::check_vcredist_missing,
            commands::system::autostart_enable,
            commands::system::autostart_disable,
            commands::system::autostart_is_enabled,
            commands::system::get_arch,
            commands::system::get_os,
            commands::system::get_system_info,
            commands::system::get_web_server_port,
            commands::system::get_local_lan_ip,
            commands::system::get_webview2_dir,
            // Tray commands
            commands::tray::set_minimize_to_tray,
            commands::tray::get_minimize_to_tray,
            commands::tray::update_tray_icon,
            commands::tray::update_tray_tooltip,
            // Config sync commands
            commands::app_config::notify_config_changed,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if tray::handle_close_requested(window.app_handle()) {
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}