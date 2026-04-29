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
