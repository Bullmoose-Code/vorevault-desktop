//! Native OS toast notifications for upload events. Informational only —
//! `tauri-plugin-notification` v2 desktop has no click callback, so toasts
//! cannot navigate. They simply appear, briefly, and dismiss themselves
//! (or get folded into Action Center / Notification Center on click).

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::config::Config;

#[derive(Debug, Clone, PartialEq)]
pub enum NotifyEvent {
    Single {
        filename: String,
    },
    Batch {
        count: u32,
    },
    Failure {
        filename: String,
        watch_folder: String,
    },
}

/// Pure formatter — given an event, return (title, body) for the toast.
pub fn title_and_body(event: &NotifyEvent) -> (String, String) {
    match event {
        NotifyEvent::Single { filename } => {
            ("VoreVault".to_string(), format!("Uploaded {} ✓", filename))
        }
        NotifyEvent::Batch { count } => {
            let noun = if *count == 1 { "clip" } else { "clips" };
            (
                "VoreVault".to_string(),
                format!("Uploaded {} {} ✓", count, noun),
            )
        }
        NotifyEvent::Failure {
            filename,
            watch_folder,
        } => (
            "VoreVault — upload failed".to_string(),
            format!("{} in {}", filename, watch_folder),
        ),
    }
}

/// Tracks whether we've already logged a notification permission warning.
/// macOS denies notification permission silently after the first prompt;
/// without this flag we'd spam the log on every upload.
static PERMISSION_WARNED: AtomicBool = AtomicBool::new(false);

/// Send a toast for the given event. No-ops if `cfg.notifications_enabled`
/// is false. Errors from the OS plugin are logged at warn (once) and
/// otherwise swallowed — uploads must keep working regardless of whether
/// notifications do.
pub fn notify(app: &AppHandle, cfg: &Config, event: NotifyEvent) {
    if !cfg.notifications_enabled {
        return;
    }
    let (title, body) = title_and_body(&event);
    let result = app.notification().builder().title(title).body(body).show();
    if let Err(e) = result {
        if !PERMISSION_WARNED.swap(true, Ordering::Relaxed) {
            log::warn!(
                "notification send failed (further failures will be silent): {}",
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_success_title_and_body() {
        let (title, body) = title_and_body(&NotifyEvent::Single {
            filename: "clip.mp4".into(),
        });
        assert_eq!(title, "VoreVault");
        assert_eq!(body, "Uploaded clip.mp4 ✓");
    }

    #[test]
    fn batch_success_title_and_body() {
        let (title, body) = title_and_body(&NotifyEvent::Batch { count: 5 });
        assert_eq!(title, "VoreVault");
        assert_eq!(body, "Uploaded 5 clips ✓");
    }

    #[test]
    fn batch_count_pluralization_singular_edge() {
        // count=1 should never reach Batch (Single is used instead), but
        // defensively the formatter should still produce sensible output.
        let (_, body) = title_and_body(&NotifyEvent::Batch { count: 1 });
        assert_eq!(body, "Uploaded 1 clip ✓");
    }

    #[test]
    fn failure_title_and_body_includes_folder() {
        let (title, body) = title_and_body(&NotifyEvent::Failure {
            filename: "clip.mp4".into(),
            watch_folder: "C:\\Users\\ryan\\Clips".into(),
        });
        assert_eq!(title, "VoreVault — upload failed");
        assert_eq!(body, "clip.mp4 in C:\\Users\\ryan\\Clips");
    }
}
