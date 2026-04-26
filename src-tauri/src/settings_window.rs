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
}
