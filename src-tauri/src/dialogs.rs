//! Native OS dialogs via tauri-plugin-dialog.

use std::path::PathBuf;
use std::sync::mpsc;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// Show the native folder picker. Returns the selected path or None on cancel.
/// Blocks the calling thread until the user picks or cancels — must be called
/// from a worker thread, never the main UI thread.
pub fn pick_folder(app: &AppHandle) -> Option<PathBuf> {
    let (tx, rx) = mpsc::channel::<Option<PathBuf>>();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path.and_then(|p| p.into_path().ok()));
    });
    rx.recv().unwrap_or(None)
}

/// Show a native Yes/No prompt with the given message. Returns true for Yes,
/// false for No or Cancel. Blocks — call from a worker thread.
pub fn yes_no(app: &AppHandle, title: &str, message: &str) -> bool {
    let (tx, rx) = mpsc::channel::<bool>();
    let title = title.to_string();
    let message = message.to_string();
    app.dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::YesNo)
        .show(move |answer| {
            let _ = tx.send(answer);
        });
    rx.recv().unwrap_or(false)
}
