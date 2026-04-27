//! Auto-updater state machine + Tauri commands + startup check task.
//! Sub-project E of Theme 1.1. See
//! docs/superpowers/specs/2026-04-26-desktop-watcher-subproject-e-design.md.

use serde::Serialize;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_idle_shows_current_version() {
        assert_eq!(UpdaterState::Idle.status_text("0.5.0"), "up to date · v0.5.0");
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
