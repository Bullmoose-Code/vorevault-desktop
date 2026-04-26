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

/// Install the tray icon at app startup. Called from `main.rs` `setup`.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;

    // Initial menu reflects "loading" — refresh_menu replaces it with the
    // real signed-in/out state once we've checked the server.
    let loading = MenuItem::with_id(app, "loading", "Loading…", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit VoreVault", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&loading, &PredefinedMenuItem::separator(app)?, &quit])?;

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
    let state = crate::auth::current_state(vault_url);

    let menu = match build_menu(app, &state) {
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

fn build_menu(app: &AppHandle, state: &crate::auth::AuthState) -> tauri::Result<Menu<Wry>> {
    let quit = MenuItem::with_id(app, "quit", "Quit VoreVault", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;

    match &state.username {
        Some(username) => {
            let label = MenuItem::with_id(
                app,
                "signed-in-label",
                format!("Signed in as @{}", username),
                false,
                None::<&str>,
            )?;
            let signout = MenuItem::with_id(app, "sign-out", "Sign out", true, None::<&str>)?;
            Menu::with_items(app, &[&label, &sep, &signout, &quit])
        }
        None => {
            let signin = MenuItem::with_id(app, "sign-in", "Sign in", true, None::<&str>)?;
            Menu::with_items(app, &[&signin, &sep, &quit])
        }
    }
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id.as_ref() {
        "sign-in" => spawn_sign_in(app.clone()),
        "sign-out" => spawn_sign_out(app.clone()),
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
        match result {
            Ok(_) => log::info!("sign-in succeeded"),
            Err(e) => log::warn!("sign-in failed: {}", e),
        }
        refresh_menu(&app, &vault_url);
        release_lock();
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
