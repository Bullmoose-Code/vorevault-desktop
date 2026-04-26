use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Wry,
};

const TRAY_ID: &str = "vorevault-tray";

/// Held while a sign-in/sign-out is in progress, so the tray doesn't dispatch
/// a second worker thread for the same operation.
static OP_IN_PROGRESS: Mutex<bool> = Mutex::new(false);

/// The running pipeline. Set by main.rs after a successful folder-pick
/// or on startup if a folder is already configured. None if no pipeline
/// is currently running.
pub static PIPELINE: std::sync::RwLock<Option<crate::pipeline::Pipeline>> =
    std::sync::RwLock::new(None);

fn read_pipeline_state(_app: &AppHandle) -> Option<crate::pipeline::PipelineState> {
    PIPELINE.read().unwrap().as_ref().map(|p| p.snapshot())
}

/// Install the tray icon at app startup. Called from `main.rs` `setup`.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;

    // Initial menu reflects "loading" — refresh_menu replaces it with the
    // real signed-in/out state once we've checked the server.
    let loading = MenuItem::with_id(app, "loading", "Loading…", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit VoreVault", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&loading, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    let _tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(true)
        .menu(&menu)
        .on_menu_event(handle_menu_event)
        .build(app)?;

    Ok(())
}

/// Recompute the tray menu based on current keychain + server state. Call
/// this on app startup (after `install`) and whenever sign-in/out completes.
/// Performs the network check on the calling thread — caller is responsible
/// for running this off the main thread if it might block on the network.
pub fn refresh_menu(app: &AppHandle, vault_url: &str) {
    let auth_state = crate::auth::current_state(vault_url);
    let pipeline_state = read_pipeline_state(app);

    let menu = match build_menu(app, &auth_state, pipeline_state.as_ref()) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("failed to build tray menu: {}", e);
            return;
        }
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
    }
}

pub fn spawn_sign_in_command(app: tauri::AppHandle) {
    spawn_sign_in(app);
}

fn build_menu(
    app: &AppHandle,
    auth: &crate::auth::AuthState,
    pipe: Option<&crate::pipeline::PipelineState>,
) -> tauri::Result<Menu<Wry>> {
    let quit = MenuItem::with_id(app, "quit", "Quit VoreVault", true, None::<&str>)?;

    match (&auth.username, pipe) {
        (Some(username), Some(p)) => {
            // Signed in WITH pipeline state.
            let signed_in = MenuItem::with_id(
                app,
                "signed-in-label",
                format!("Signed in as @{}", username),
                false,
                None::<&str>,
            )?;
            let watching = if let Some(path) = &p.watching_path {
                Some(MenuItem::with_id(
                    app,
                    "watching-label",
                    format!("Watching: {}", path),
                    false,
                    None::<&str>,
                )?)
            } else {
                None
            };
            let busy = p.queued + p.uploading;
            let uploading = if p.uploading > 0 {
                Some(MenuItem::with_id(
                    app,
                    "uploading-label",
                    format!("Uploading {} of {}…", p.uploading, busy),
                    false,
                    None::<&str>,
                )?)
            } else {
                None
            };
            let failed = if !p.failed_paths.is_empty() {
                Some(MenuItem::with_id(
                    app,
                    "failed-label",
                    format!("⚠ {} failed uploads", p.failed_paths.len()),
                    false,
                    None::<&str>,
                )?)
            } else {
                None
            };
            let pick = MenuItem::with_id(app, "pick-folder", "Pick folder…", true, None::<&str>)?;
            let cfg_for_label = crate::config::load().unwrap_or_default();
            let notif_label = if cfg_for_label.notifications_enabled {
                "Show notifications: On"
            } else {
                "Show notifications: Off"
            };
            let notif =
                MenuItem::with_id(app, "toggle-notifications", notif_label, true, None::<&str>)?;
            let signout = MenuItem::with_id(app, "sign-out", "Sign out", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;

            // Build the items list dynamically with explicit ownership.
            let mut items: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = vec![&signed_in];
            if let Some(w) = &watching {
                items.push(w);
            }
            if let Some(u) = &uploading {
                items.push(u);
            }
            if let Some(f) = &failed {
                items.push(f);
            }
            items.push(&sep1);
            items.push(&pick);
            items.push(&notif);
            items.push(&sep2);
            items.push(&signout);
            items.push(&quit);

            Menu::with_items(app, &items)
        }
        (Some(username), None) => {
            // Signed in WITHOUT pipeline state (no folder configured yet).
            let signed_in = MenuItem::with_id(
                app,
                "signed-in-label",
                format!("Signed in as @{}", username),
                false,
                None::<&str>,
            )?;
            let sep = PredefinedMenuItem::separator(app)?;
            let pick = MenuItem::with_id(app, "pick-folder", "Pick folder…", true, None::<&str>)?;
            let signout = MenuItem::with_id(app, "sign-out", "Sign out", true, None::<&str>)?;
            Menu::with_items(app, &[&signed_in, &sep, &pick, &signout, &quit])
        }
        (None, _) => {
            // Signed out.
            let signin = MenuItem::with_id(app, "sign-in", "Sign in", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            Menu::with_items(app, &[&signin, &sep, &quit])
        }
    }
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id.as_ref() {
        "sign-in" => spawn_sign_in(app.clone()),
        "sign-out" => spawn_sign_out(app.clone()),
        "pick-folder" => spawn_pick_folder(app.clone()),
        "toggle-notifications" => spawn_toggle_notifications(app.clone()),
        "quit" => app.exit(0),
        _ => {}
    }
}

fn spawn_sign_in(app: AppHandle) {
    if !try_acquire_lock() {
        log::info!("sign-in already in progress; ignoring click");
        return;
    }
    std::thread::spawn(move || {
        let vault_url = crate::auth::vault_url_from_env();
        let result = crate::auth::sign_in(&vault_url, |url| {
            tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
        });
        let signed_in = match result {
            Ok(_) => {
                log::info!("sign-in succeeded");
                true
            }
            Err(e) => {
                log::warn!("sign-in failed: {}", e);
                false
            }
        };
        refresh_menu(&app, &vault_url);
        release_lock();

        // Onboarding: if this is the user's first successful sign-in and no
        // watch folder is configured yet, immediately prompt them to pick one.
        if signed_in
            && PIPELINE.read().unwrap().is_none()
            && crate::config::load()
                .ok()
                .and_then(|c| c.watch_folder)
                .is_none()
        {
            do_pick_folder(&app);
        }
    });
}

fn spawn_sign_out(app: AppHandle) {
    if !try_acquire_lock() {
        log::info!("sign-out already in progress; ignoring click");
        return;
    }
    std::thread::spawn(move || {
        let vault_url = crate::auth::vault_url_from_env();
        crate::auth::sign_out(&vault_url);
        refresh_menu(&app, &vault_url);
        release_lock();
    });
}

fn spawn_pick_folder(app: AppHandle) {
    std::thread::spawn(move || {
        do_pick_folder(&app);
    });
}

fn spawn_toggle_notifications(app: AppHandle) {
    std::thread::spawn(move || {
        let mut cfg = crate::config::load().unwrap_or_default();
        cfg.notifications_enabled = !cfg.notifications_enabled;
        if let Err(e) = crate::config::save(&cfg) {
            log::warn!("failed to save notifications toggle: {}", e);
            return;
        }
        log::info!(
            "notifications toggled to {}",
            if cfg.notifications_enabled {
                "on"
            } else {
                "off"
            }
        );
        let vault_url = crate::auth::vault_url_from_env();
        refresh_menu(&app, &vault_url);
    });
}

/// Run the full pick-folder flow synchronously on the calling thread:
/// open the picker, ask about existing files, save config, start the
/// pipeline (if not already running), refresh the menu. Caller is
/// responsible for running this off the main thread.
fn do_pick_folder(app: &AppHandle) {
    let path = match crate::dialogs::pick_folder(app) {
        Some(p) => p,
        None => return,
    };

    let count = count_files_recursive(&path);

    let scan_existing = if count > 0 {
        crate::dialogs::yes_no(
            app,
            "Upload existing files?",
            &format!(
                "Found {} existing files in this folder. Upload them too?",
                count,
            ),
        )
    } else {
        true
    };

    let mut cfg = crate::config::load().unwrap_or_default();
    cfg.watch_folder = Some(path.to_string_lossy().to_string());
    cfg.scan_existing_on_pick = scan_existing;
    if let Err(e) = crate::config::save(&cfg) {
        log::warn!("failed to save config: {}", e);
        return;
    }

    let vault_url = crate::auth::vault_url_from_env();

    if PIPELINE.read().unwrap().is_none() {
        if let Err(e) = crate::start_pipeline_if_configured(app, &vault_url) {
            log::warn!("could not start pipeline after pick: {}", e);
        }
    } else {
        // Pipeline is already running on a previous folder; replacing it is
        // handled by change_watch_folder (Task 9). For now, just save config
        // and let the user restart if needed (this branch is kept as a safety
        // fallback for callers that haven't yet been wired up to the new path).
        log::warn!(
            "pipeline already running on a previous folder; \
             saved new folder to config — restart app to switch"
        );
    }

    refresh_menu(app, &vault_url);
}

fn count_files_recursive(root: &std::path::Path) -> u64 {
    fn walk(p: &std::path::Path, n: &mut u64) {
        if let Ok(entries) = std::fs::read_dir(p) {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_file() {
                    *n += 1;
                } else if path.is_dir() {
                    walk(&path, n);
                }
            }
        }
    }
    let mut n = 0u64;
    walk(root, &mut n);
    n
}

fn try_acquire_lock() -> bool {
    let mut g = OP_IN_PROGRESS.lock().unwrap();
    if *g {
        false
    } else {
        *g = true;
        true
    }
}

fn release_lock() {
    *OP_IN_PROGRESS.lock().unwrap() = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive() {
        // Reset to known state in case prior test left it acquired.
        release_lock();
        assert!(try_acquire_lock());
        assert!(!try_acquire_lock(), "second acquire should fail");
        release_lock();
        assert!(try_acquire_lock(), "after release, can acquire again");
        release_lock();
    }
}
