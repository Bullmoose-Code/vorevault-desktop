// Prevents an extra console window from showing up on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod config;
mod db;
mod deeplink;
mod folders_api;
mod keychain;
mod notifier;
mod path;
mod pipeline;
mod rules;
mod settings_window;
mod tray;
mod updater;
mod uploader;
mod watcher;

use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // Second-launch fired with a URL — forward to running instance.
            // The first argument is the executable path; skip it and look for
            // any vorevault:// URL in the remainder.
            for arg in argv.iter().skip(1) {
                if arg.starts_with("vorevault://") {
                    crate::deeplink::dispatch(app, arg);
                }
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            settings_window::get_state,
            settings_window::get_autostart,
            settings_window::set_autostart,
            settings_window::sign_out,
            settings_window::sign_in,
            settings_window::list_rules,
            settings_window::save_rule,
            settings_window::delete_rule,
            settings_window::fetch_folders,
            settings_window::fetch_tags,
            updater::updater_get_state,
            updater::updater_check_now,
            updater::updater_install_and_restart,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tray::install(&handle)?;
            crate::settings_window::install_close_handler(&handle);
            crate::updater::spawn_startup_check(handle.clone());
            try_enable_autostart_on_first_launch(&handle);

            #[cfg(debug_assertions)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(e) = app.deep_link().register_all() {
                    log::warn!("deep-link: dev-mode register_all failed: {}", e);
                }
            }

            // Deep-link listener: fires when a vorevault:// URL is delivered
            // by the OS (either at launch or later, while the app is running).
            // The single-instance plugin handles the second-launch path
            // separately; this listener handles the first-launch URL and any
            // subsequent same-process URL events (mostly relevant on macOS).
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let deeplink_handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        crate::deeplink::dispatch(&deeplink_handle, url.as_str());
                    }
                });
            }

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
    if cfg.rules.is_empty() {
        log::info!("no watch rules configured; pipeline not started");
        return Ok(());
    }

    let dir = config::config_dir()?;
    let db = Arc::new(db::Db::open(&dir)?);

    let roots: Vec<PathBuf> = cfg
        .rules
        .iter()
        .filter_map(|r| {
            let p = PathBuf::from(&r.path);
            if p.is_dir() {
                Some(p)
            } else {
                log::warn!("watch rule path does not exist: {}", r.path);
                None
            }
        })
        .collect();

    if roots.is_empty() {
        log::warn!("all configured watch rule paths are missing; pipeline not started");
        return Ok(());
    }

    let (watcher_rx, watcher_handle) = watcher::start(&roots, cfg.debounce_ms)?;
    let watcher_handle = Arc::new(watcher_handle);

    let token_getter: Arc<dyn Fn() -> Option<String> + Send + Sync> =
        Arc::new(|| keychain::load().ok().flatten());

    // NOTE: rules contains the full configured set (including any whose
    // paths don't exist on disk — those are filtered out of `roots` above
    // and not registered with the watcher). PipelineState.watching_paths
    // therefore reflects "configured" rather than "actually watched".
    // The settings UI surfaces missing paths via a per-rule warning badge
    // (spec section 10); the tray menu's "Watching: N folders" count
    // matches the configured set for symmetry. Revisit if the tray ever
    // gets per-rule status indicators.
    let rules = Arc::new(std::sync::RwLock::new(cfg.rules.clone()));

    let pipeline = pipeline::start(
        watcher_rx,
        db.clone(),
        vault_url.to_string(),
        token_getter,
        rules,
        watcher_handle,
        handle.clone(),
    );

    if cfg.scan_existing_on_pick {
        for r in &cfg.rules {
            let p = PathBuf::from(&r.path);
            if p.is_dir() {
                scan_and_enqueue(&p, &pipeline);
            }
        }
    }

    *tray::PIPELINE.write().unwrap() = Some(pipeline);

    Ok(())
}

/// On the very first launch after install (config has `first_launch_done: false`),
/// enable autostart. Records `first_launch_done = true` so subsequent launches
/// respect the user's later choices in settings. Failures are logged and
/// non-fatal — the app continues to launch normally.
fn try_enable_autostart_on_first_launch(handle: &tauri::AppHandle) {
    let mut cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("first-launch autostart: could not load config: {}", e);
            return;
        }
    };

    if cfg.first_launch_done {
        return;
    }

    use tauri_plugin_autostart::ManagerExt;
    if let Err(e) = handle.autolaunch().enable() {
        log::warn!("first-launch autostart: enable failed: {}", e);
    } else {
        log::info!("first-launch autostart: enabled");
    }

    cfg.first_launch_done = true;
    if let Err(e) = config::save(&cfg) {
        log::warn!("first-launch autostart: could not save flag: {}", e);
    }
}

fn scan_and_enqueue(root: &std::path::Path, pipeline: &pipeline::Pipeline) {
    fn walk(p: &std::path::Path, pipeline: &pipeline::Pipeline) {
        if let Ok(entries) = std::fs::read_dir(p) {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_file() {
                    if !crate::path::is_hidden(&path) {
                        pipeline.enqueue(path);
                    }
                } else if path.is_dir() && !crate::path::is_hidden(&path) {
                    walk(&path, pipeline);
                }
            }
        }
    }
    walk(root, pipeline);
}
