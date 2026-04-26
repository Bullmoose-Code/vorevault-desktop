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
            let is_paused = PIPELINE
                .read()
                .unwrap()
                .as_ref()
                .map(|p| p.is_paused())
                .unwrap_or(false);

            let cfg_for_label = crate::config::load().unwrap_or_default();
            let notif_label = if cfg_for_label.notifications_enabled {
                "Show notifications: On"
            } else {
                "Show notifications: Off"
            };
            let notif =
                MenuItem::with_id(app, "toggle-notifications", notif_label, true, None::<&str>)?;

            let open_settings =
                MenuItem::with_id(app, "open-settings", "Open VoreVault…", true, None::<&str>)?;

            let pause_label = if is_paused {
                "Pause uploads  ✓"
            } else {
                "Pause uploads"
            };
            let pause_item =
                MenuItem::with_id(app, "toggle-pause", pause_label, true, None::<&str>)?;

            let paused_row = if is_paused {
                Some(MenuItem::with_id(
                    app,
                    "paused-status",
                    "⏸ Paused",
                    false,
                    None::<&str>,
                )?)
            } else {
                None
            };

            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let sep3 = PredefinedMenuItem::separator(app)?;

            // Build the items list dynamically with explicit ownership.
            let mut items: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = vec![&signed_in];
            if let Some(w) = &watching {
                items.push(w);
            }
            if let Some(pr) = &paused_row {
                items.push(pr);
            }
            if let Some(u) = &uploading {
                items.push(u);
            }
            if let Some(f) = &failed {
                items.push(f);
            }
            items.push(&sep1);
            items.push(&notif);
            items.push(&pause_item);
            items.push(&sep2);
            items.push(&open_settings);
            items.push(&sep3);
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
            let open_settings =
                MenuItem::with_id(app, "open-settings", "Open VoreVault…", true, None::<&str>)?;
            Menu::with_items(app, &[&signed_in, &sep, &open_settings, &quit])
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
        "toggle-notifications" => spawn_toggle_notifications(app.clone()),
        "open-settings" => {
            crate::settings_window::show(app);
        }
        "toggle-pause" => {
            let new_paused;
            {
                let guard = PIPELINE.read().unwrap();
                if let Some(pipeline) = guard.as_ref() {
                    new_paused = !pipeline.is_paused();
                    pipeline.set_paused(new_paused);
                } else {
                    return;
                }
            }
            let vault_url = crate::auth::vault_url_from_env();
            refresh_menu(app, &vault_url);
            log::info!(
                "pipeline {} via tray",
                if new_paused { "paused" } else { "resumed" }
            );
        }
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
        if signed_in {
            crate::settings_window::emit_state_changed(&app);
        }
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
            crate::settings_window::show_first_run(&app);
        }
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
