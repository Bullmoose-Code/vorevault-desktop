// Prevents an extra console window from showing up on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod config;
mod db;
mod dialogs;
mod keychain;
mod notifier;
mod pipeline;
mod settings_window;
mod tray;
mod uploader;
mod watcher;

use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            settings_window::get_state,
            settings_window::get_autostart,
            settings_window::set_autostart,
            settings_window::change_watch_folder,
            settings_window::sign_out,
            settings_window::sign_in,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tray::install(&handle)?;

            std::thread::spawn(move || {
                let vault_url = auth::vault_url_from_env();
                tray::refresh_menu(&handle, &vault_url);

                if let Err(e) = start_pipeline_if_configured(&handle, &vault_url) {
                    log::warn!("could not start pipeline: {}", e);
                }

                tray::refresh_menu(&handle, &vault_url);
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub(crate) fn start_pipeline_if_configured(
    handle: &tauri::AppHandle,
    vault_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = config::load()?;
    let Some(watch_folder) = cfg.watch_folder.as_deref() else {
        log::info!("no watch folder configured; pipeline not started");
        return Ok(());
    };

    let watch_path = PathBuf::from(watch_folder);
    if !watch_path.is_dir() {
        log::warn!("configured watch folder does not exist: {}", watch_folder);
        return Ok(());
    }

    let dir = config::config_dir()?;
    let db = Arc::new(db::Db::open(&dir)?);

    let watcher_rx = watcher::start(watch_path.clone(), cfg.debounce_ms)?;

    let token_getter: Arc<dyn Fn() -> Option<String> + Send + Sync> =
        Arc::new(|| keychain::load().ok().flatten());

    let pipeline = pipeline::start(
        watcher_rx,
        db.clone(),
        vault_url.to_string(),
        token_getter,
        watch_folder.to_string(),
        handle.clone(),
    );

    if cfg.scan_existing_on_pick {
        scan_and_enqueue(&watch_path, &pipeline);
    }

    *tray::PIPELINE.write().unwrap() = Some(pipeline);

    Ok(())
}

fn scan_and_enqueue(root: &std::path::Path, pipeline: &pipeline::Pipeline) {
    fn walk(p: &std::path::Path, pipeline: &pipeline::Pipeline) {
        if let Ok(entries) = std::fs::read_dir(p) {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_file() {
                    pipeline.enqueue(path);
                } else if path.is_dir() {
                    walk(&path, pipeline);
                }
            }
        }
    }
    walk(root, pipeline);
}
