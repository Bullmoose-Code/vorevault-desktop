//! Settings window: lifecycle, commands, and state-snapshot helpers.
//! Sub-project D of Theme 1.1. See
//! docs/superpowers/specs/2026-04-26-desktop-watcher-subproject-d-design.md.

use serde::Serialize;

/// Single source of truth pushed to the settings window's JS layer on every
/// state change. JS re-renders the whole DOM on each update.
#[derive(Clone, Debug, Serialize)]
pub struct SettingsState {
    /// Discord username when signed in, `None` when signed out.
    pub username: Option<String>,
    /// Configured watch rules (each routes one folder to one VV destination).
    pub rules: Vec<crate::rules::WatchRule>,
    /// Whether the upload pipeline is currently soft-paused.
    pub paused: bool,
    /// CARGO_PKG_VERSION at build time.
    pub version: &'static str,
}

/// Snapshot the current settings state from production sources:
/// auth (for signed-in username — performs a /api/auth/me check on each call),
/// config (for the rules vec), and pipeline (for paused).
pub fn current_state(_app: &tauri::AppHandle) -> SettingsState {
    let vault_url = crate::auth::vault_url_from_env();
    let auth_state = crate::auth::current_state(&vault_url);
    let username = auth_state.username;

    let rules = crate::config::load()
        .map(|c| c.rules)
        .unwrap_or_default();

    let paused = crate::tray::PIPELINE
        .read()
        .unwrap()
        .as_ref()
        .map(|p| p.is_paused())
        .unwrap_or(false);

    SettingsState {
        username,
        rules,
        paused,
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// Emit "settings:state-changed" with the current snapshot. Always performs
/// the same I/O as `current_state` (including a `/api/auth/me` HTTP request);
/// only the emit itself is a no-op when the window is closed (no listeners).
/// Callers on the main/event-dispatcher thread should consider wrapping in
/// `tauri::async_runtime::spawn_blocking` to avoid UI hazards.
pub fn emit_state_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let state = current_state(app);
    if let Err(e) = app.emit("settings:state-changed", &state) {
        log::warn!("failed to emit settings:state-changed: {}", e);
    }
}

use tauri::Manager;

/// Open the settings window (or focus it if already open). Idempotent.
pub fn show(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        if let Err(e) = window.show() {
            log::warn!("settings window show() failed: {}", e);
            return;
        }
        if let Err(e) = window.set_focus() {
            log::warn!("settings window set_focus() failed: {}", e);
        }
        return;
    }
    log::error!("settings window not found in app config");
}

/// First-run variant: same as show() today. Kept as a separate entry point so
/// we can later distinguish onboarding telemetry / behaviors if needed.
pub fn show_first_run(app: &tauri::AppHandle) {
    show(app);
}

/// Register the CloseRequested handler that hides instead of closing,
/// keeping the window object alive and listeners registered. Call this once
/// at app setup (from main.rs).
pub fn install_close_handler(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let w = window.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = w.hide();
            }
        });
    } else {
        log::warn!("settings window not yet created at install_close_handler time");
    }
}

#[tauri::command]
pub fn get_state(app: tauri::AppHandle) -> SettingsState {
    current_state(&app)
}

#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("autostart read failed: {}", e))
}

#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    let res = if enabled { mgr.enable() } else { mgr.disable() };
    res.map_err(|e| format!("autostart write failed: {}", e))
}

#[tauri::command]
pub fn sign_out(app: tauri::AppHandle) {
    let vault_url = crate::auth::vault_url_from_env();
    crate::auth::sign_out(&vault_url);
    // Stop pipeline by clearing it.
    {
        let mut guard = crate::tray::PIPELINE.write().unwrap();
        *guard = None;
    }
    crate::tray::refresh_menu(&app, &vault_url);
    emit_state_changed(&app);
}

#[tauri::command]
pub fn sign_in(app: tauri::AppHandle) {
    crate::tray::spawn_sign_in_command(app);
}

#[tauri::command]
pub fn list_rules() -> Vec<crate::rules::WatchRule> {
    crate::config::load().map(|c| c.rules).unwrap_or_default()
}

#[tauri::command]
pub fn save_rule(app: tauri::AppHandle, rule: crate::rules::WatchRule) -> Result<(), String> {
    let path_buf = std::path::PathBuf::from(&rule.path);
    if !path_buf.is_dir() {
        return Err("watch folder doesn't exist on disk".to_string());
    }

    let mut cfg = crate::config::load().map_err(|e| format!("config load: {}", e))?;

    // Validate overlap against the OTHER rules (exclude self-by-id).
    let other_paths: Vec<&std::path::Path> = cfg
        .rules
        .iter()
        .filter(|r| r.id != rule.id)
        .map(|r| std::path::Path::new(&r.path))
        .collect();
    if crate::rules::is_path_overlap(&other_paths, std::path::Path::new(&rule.path)) {
        return Err("that path overlaps with another rule".to_string());
    }

    // Validate tags (defense-in-depth — UI also pre-validates).
    for t in &rule.tags {
        crate::rules::normalize_tag(t).map_err(|e| format!("invalid tag {:?}: {}", t, e))?;
    }

    // Replace if id exists, else append.
    if let Some(existing) = cfg.rules.iter_mut().find(|r| r.id == rule.id) {
        *existing = rule.clone();
    } else {
        cfg.rules.push(rule.clone());
    }
    crate::config::save(&cfg).map_err(|e| format!("config save: {}", e))?;

    // Push the new rule set into the running pipeline (or start one if
    // this is the first rule).
    let vault_url = crate::auth::vault_url_from_env();
    {
        let guard = crate::tray::PIPELINE.read().unwrap();
        if let Some(p) = guard.as_ref() {
            p.replace_rules(cfg.rules.clone());
        }
    }
    let pipeline_running = crate::tray::PIPELINE.read().unwrap().is_some();
    if !pipeline_running {
        if let Err(e) = crate::start_pipeline_if_configured(&app, &vault_url) {
            log::warn!("pipeline start after save_rule failed: {}", e);
        }
    }

    crate::tray::refresh_menu(&app, &vault_url);
    emit_state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn delete_rule(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let mut cfg = crate::config::load().map_err(|e| format!("config load: {}", e))?;
    cfg.rules.retain(|r| r.id != id);
    crate::config::save(&cfg).map_err(|e| format!("config save: {}", e))?;

    let vault_url = crate::auth::vault_url_from_env();
    {
        let guard = crate::tray::PIPELINE.read().unwrap();
        if let Some(p) = guard.as_ref() {
            p.replace_rules(cfg.rules.clone());
        }
    }
    if cfg.rules.is_empty() {
        let mut guard = crate::tray::PIPELINE.write().unwrap();
        *guard = None;
    }
    crate::tray::refresh_menu(&app, &vault_url);
    emit_state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn fetch_folders() -> Result<Vec<crate::folders_api::FolderNode>, String> {
    let vault_url = crate::auth::vault_url_from_env();
    let token = crate::keychain::load()
        .ok()
        .flatten()
        .ok_or_else(|| "not signed in".to_string())?;
    crate::folders_api::fetch_folders(&vault_url, &token).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn fetch_tags() -> Result<Vec<crate::folders_api::TagSuggestion>, String> {
    let vault_url = crate::auth::vault_url_from_env();
    let token = crate::keychain::load()
        .ok()
        .flatten()
        .ok_or_else(|| "not signed in".to_string())?;
    crate::folders_api::fetch_tags(&vault_url, &token).map_err(|e| e.to_string())
}
