//! Settings window: lifecycle, commands, and state-snapshot helpers.
//! Sub-project D of Theme 1.1. See
//! docs/superpowers/specs/2026-04-26-desktop-watcher-subproject-d-design.md.

use serde::Serialize;
use std::path::PathBuf;

/// Single source of truth pushed to the settings window's JS layer on every
/// state change. JS re-renders the whole DOM on each update.
#[derive(Clone, Debug, Serialize)]
pub struct SettingsState {
    /// Discord username when signed in, `None` when signed out.
    pub username: Option<String>,
    /// Current watch folder. `None` on first-run before the user picks one.
    pub watch_folder: Option<PathBuf>,
    /// Pre-formatted display label for the watch folder button (truncated).
    /// `None` when `watch_folder` is `None`.
    pub watch_folder_label: Option<String>,
    /// Whether the upload pipeline is currently soft-paused.
    pub paused: bool,
    /// CARGO_PKG_VERSION at build time.
    pub version: &'static str,
}

/// Truncate a path to a button-friendly label.
/// - Paths shorter than `max_chars` are returned as-is.
/// - Longer paths are truncated with a leading ellipsis: "…/foo/bar".
/// - Multi-byte safe: counts chars, not bytes.
pub fn format_path_for_button(path: &std::path::Path, max_chars: usize) -> String {
    let s = path.display().to_string();
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s;
    }
    let keep = max_chars.saturating_sub(1);
    let suffix: String = s.chars().rev().take(keep).collect::<Vec<char>>().into_iter().rev().collect();
    format!("…{}", suffix)
}

/// Maximum chars in the watch-folder button label before truncation.
const PATH_LABEL_MAX: usize = 28;

/// Pure builder — assembles a SettingsState from already-loaded inputs.
/// Real callers use `current_state(app)` which loads from config/auth/pipeline.
fn build_state(
    username: Option<String>,
    watch_folder: Option<PathBuf>,
    paused: bool,
) -> SettingsState {
    let watch_folder_label = watch_folder
        .as_deref()
        .map(|p| format_path_for_button(p, PATH_LABEL_MAX));
    SettingsState {
        username,
        watch_folder,
        watch_folder_label,
        paused,
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[cfg(test)]
fn build_state_for_test(
    username: Option<String>,
    watch_folder: Option<PathBuf>,
    paused: bool,
) -> SettingsState {
    build_state(username, watch_folder, paused)
}

/// Snapshot the current settings state from production sources:
/// auth (for signed-in username — performs a /api/auth/me check on each call),
/// config (for watch_folder), and pipeline (for paused).
pub fn current_state(_app: &tauri::AppHandle) -> SettingsState {
    let vault_url = crate::auth::vault_url_from_env();
    let auth_state = crate::auth::current_state(&vault_url);
    let username = auth_state.username;

    let watch_folder = crate::config::load()
        .ok()
        .and_then(|c| c.watch_folder)
        .map(PathBuf::from);

    let paused = crate::tray::PIPELINE
        .get()
        .map(|p| p.is_paused())
        .unwrap_or(false);

    build_state(username, watch_folder, paused)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_short_path_returns_unchanged() {
        let p = PathBuf::from("/short");
        assert_eq!(format_path_for_button(&p, 28), "/short");
    }

    #[test]
    fn format_exact_length_path_returns_unchanged() {
        let p = PathBuf::from("/exactly-twenty-eight-chars/");
        assert_eq!(p.display().to_string().chars().count(), 28);
        assert_eq!(format_path_for_button(&p, 28), "/exactly-twenty-eight-chars/");
    }

    #[test]
    fn format_long_path_is_truncated_with_leading_ellipsis() {
        let p = PathBuf::from("/Users/ryan/Movies/Recordings/2026/clips/raw");
        let out = format_path_for_button(&p, 28);
        assert!(out.starts_with("…"));
        assert_eq!(out.chars().count(), 28);
        assert!(out.ends_with("clips/raw"));
    }

    #[test]
    fn format_path_with_multibyte_chars() {
        let p = PathBuf::from("/Users/ryan/Vidéos/clips-éphémères/raw/2026");
        let out = format_path_for_button(&p, 20);
        assert_eq!(out.chars().count(), 20);
        assert!(out.starts_with("…"));
    }

    #[test]
    fn format_root_path() {
        let p = PathBuf::from("/");
        assert_eq!(format_path_for_button(&p, 28), "/");
    }

    #[test]
    fn format_empty_max_chars_zero() {
        let p = PathBuf::from("/some/path");
        assert_eq!(format_path_for_button(&p, 0), "…");
    }

    #[test]
    fn build_state_signed_out_no_folder() {
        let s = build_state_for_test(None, None, false);
        assert!(s.username.is_none());
        assert!(s.watch_folder.is_none());
        assert!(s.watch_folder_label.is_none());
        assert!(!s.paused);
        assert_eq!(s.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn build_state_signed_in_with_folder() {
        let p = PathBuf::from("/Users/ryan/clips");
        let s = build_state_for_test(Some("ryan".to_string()), Some(p.clone()), false);
        assert_eq!(s.username, Some("ryan".to_string()));
        assert_eq!(s.watch_folder, Some(p));
        assert_eq!(s.watch_folder_label, Some("/Users/ryan/clips".to_string()));
        assert!(!s.paused);
    }

    #[test]
    fn build_state_paused_long_path() {
        let p = PathBuf::from("/Users/ryan/Movies/Recordings/2026/clips/raw");
        let s = build_state_for_test(Some("ryan".to_string()), Some(p), true);
        assert!(s.paused);
        let label = s.watch_folder_label.unwrap();
        assert!(label.starts_with("…"));
        assert_eq!(label.chars().count(), 28);
    }
}
