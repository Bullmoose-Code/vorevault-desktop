//! Auto-updater state machine + Tauri commands + startup check task.
//! Sub-project E of Theme 1.1. See
//! docs/superpowers/specs/2026-04-26-desktop-watcher-subproject-e-design.md.

use serde::Serialize;
use std::sync::RwLock;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// Single source of truth pushed to the settings window's JS layer on every
/// updater state change. JS re-renders the Updates row on each event.
///
/// The variants intentionally don't enforce transitions — any state can flow
/// to any other state. Callers are expected to follow the natural sequence:
/// Idle → Checking → (UpToDate | DownloadingUpdate(v) → Ready(v) | Error(msg)).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value")]
#[allow(dead_code)]
pub enum UpdaterState {
    /// Initial state on app launch, before any check has happened.
    Idle,
    /// A check is in flight (manual button click or startup task).
    Checking,
    /// Last check completed; current binary is the latest published release.
    UpToDate,
    /// Newer version found; installer downloading in background.
    /// Holds the target version string.
    DownloadingUpdate(String),
    /// Update fully downloaded and staged; restart applies it.
    /// Holds the target version string.
    Ready(String),
    /// Last check or download failed; holds a short user-facing message.
    Error(String),
}

#[allow(dead_code)]
impl UpdaterState {
    /// Render a status string for the Updates row in the settings window.
    /// `current` is the running app's version (e.g., `env!("CARGO_PKG_VERSION")`).
    pub fn status_text(&self, current: &str) -> String {
        match self {
            UpdaterState::Idle | UpdaterState::UpToDate => format!("up to date · v{}", current),
            UpdaterState::Checking => "checking…".to_string(),
            UpdaterState::DownloadingUpdate(v) => format!("downloading v{} in background", v),
            UpdaterState::Ready(v) => format!("update v{} ready — restart to apply", v),
            UpdaterState::Error(msg) => format!("couldn't check ({}) · retry", msg),
        }
    }

    /// True when the user can click "Check now" (no in-flight operation).
    pub fn check_button_enabled(&self) -> bool {
        matches!(
            self,
            UpdaterState::Idle | UpdaterState::UpToDate | UpdaterState::Error(_)
        )
    }

    /// True when "Restart now" should be shown.
    pub fn restart_button_visible(&self) -> bool {
        matches!(self, UpdaterState::Ready(_))
    }
}

/// Process-wide updater state cell. Startup task and the 3 Tauri commands
/// all read/write through this. Lock contention is negligible (transitions
/// are infrequent and the lock is held for microseconds).
#[allow(dead_code)]
static STATE: RwLock<UpdaterState> = RwLock::new(UpdaterState::Idle);

/// Read the current state (snapshot).
pub fn snapshot() -> UpdaterState {
    STATE.read().expect("updater STATE lock poisoned").clone()
}

/// Replace the state and emit `updater:state-changed` to all webviews.
/// All transitions go through this so JS always sees changes.
pub fn set_state(app: &AppHandle, new_state: UpdaterState) {
    {
        let mut guard = STATE.write().expect("updater STATE lock poisoned");
        *guard = new_state.clone();
    }
    if let Err(e) = app.emit("updater:state-changed", &new_state) {
        log::warn!("updater: failed to emit state-changed event: {}", e);
    }
}

/// Internal helper: run one updater check, transition state through the cycle.
/// Used by both the manual `updater_check_now` command and the startup task.
async fn run_check(app: AppHandle) {
    // Drop concurrent invocations: if a check or download is already in flight,
    // do nothing. The settings UI disables the button when state isn't terminal,
    // but the check is racy across threads, so this guard is the source of truth.
    if matches!(
        snapshot(),
        UpdaterState::Checking | UpdaterState::DownloadingUpdate(_)
    ) {
        return;
    }
    set_state(&app, UpdaterState::Checking);

    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            log::error!("updater: handle unavailable: {}", e);
            set_state(&app, UpdaterState::Error("plugin unavailable".to_string()));
            return;
        }
    };

    let maybe_update = match updater.check().await {
        Ok(u) => u,
        Err(e) => {
            log::warn!("updater: check failed: {}", e);
            set_state(&app, UpdaterState::Error(format!("{}", e)));
            return;
        }
    };

    let Some(update) = maybe_update else {
        set_state(&app, UpdaterState::UpToDate);
        return;
    };

    let target_version = update.version.clone();
    log::info!("updater: downloading v{}", target_version);
    set_state(
        &app,
        UpdaterState::DownloadingUpdate(target_version.clone()),
    );

    // download_and_install stages the new installer; the actual swap happens
    // when the app exits (Tauri plugin handles per-platform install on quit).
    let result = update
        .download_and_install(|_chunk, _total| {}, || {})
        .await;

    match result {
        Ok(()) => {
            log::info!("updater: v{} downloaded and staged", target_version);
            set_state(&app, UpdaterState::Ready(target_version));
        }
        Err(e) => {
            log::warn!("updater: download/install failed: {}", e);
            set_state(&app, UpdaterState::Error(format!("download failed: {}", e)));
        }
    }
}

/// Get current state. Settings window calls this on open to render initial UI.
#[tauri::command]
pub fn updater_get_state() -> UpdaterState {
    snapshot()
}

/// Manually trigger a check. Settings window's "Check now" button calls this.
#[tauri::command]
pub async fn updater_check_now(app: AppHandle) {
    run_check(app).await;
}

/// Restart the app to apply a staged update.
/// Only meaningful when state is `Ready(_)`; safe to call in other states.
#[tauri::command]
pub fn updater_install_and_restart(app: AppHandle) {
    log::info!("updater: restart requested");
    app.restart();
}

/// Spawn the post-startup check. Called from main.rs setup.
/// Sleeps 5s on a worker thread (no tokio direct dep needed) and then runs
/// one async check via `block_on` on that thread.
pub fn spawn_startup_check(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        log::info!("updater: running startup check");
        tauri::async_runtime::block_on(run_check(app));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_idle_shows_current_version() {
        assert_eq!(
            UpdaterState::Idle.status_text("0.5.0"),
            "up to date · v0.5.0"
        );
    }

    #[test]
    fn status_text_uptodate_matches_idle() {
        assert_eq!(
            UpdaterState::UpToDate.status_text("0.5.0"),
            UpdaterState::Idle.status_text("0.5.0"),
        );
    }

    #[test]
    fn status_text_ready_shows_target_version() {
        assert_eq!(
            UpdaterState::Ready("0.5.1".to_string()).status_text("0.5.0"),
            "update v0.5.1 ready — restart to apply",
        );
    }

    #[test]
    fn status_text_error_includes_message() {
        let s = UpdaterState::Error("network unreachable".to_string()).status_text("0.5.0");
        assert!(s.contains("network unreachable"));
        assert!(s.starts_with("couldn't check"));
    }

    #[test]
    fn check_button_enabled_only_in_terminal_states() {
        assert!(UpdaterState::Idle.check_button_enabled());
        assert!(UpdaterState::UpToDate.check_button_enabled());
        assert!(UpdaterState::Error("x".into()).check_button_enabled());
        assert!(!UpdaterState::Checking.check_button_enabled());
        assert!(!UpdaterState::DownloadingUpdate("0.5.1".into()).check_button_enabled());
        assert!(!UpdaterState::Ready("0.5.1".into()).check_button_enabled());
    }

    #[test]
    fn restart_visible_only_when_ready() {
        assert!(UpdaterState::Ready("0.5.1".into()).restart_button_visible());
        assert!(!UpdaterState::Idle.restart_button_visible());
        assert!(!UpdaterState::Checking.restart_button_visible());
        assert!(!UpdaterState::Error("x".into()).restart_button_visible());
    }

    #[test]
    fn state_serializes_with_tagged_envelope() {
        let s = serde_json::to_string(&UpdaterState::DownloadingUpdate("0.5.1".into())).unwrap();
        assert!(s.contains("\"kind\":\"DownloadingUpdate\""));
        assert!(s.contains("\"value\":\"0.5.1\""));
    }
}
