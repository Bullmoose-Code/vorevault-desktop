// Prevents an extra console window from showing up on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod keychain;
mod tray;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            tray::install(&handle)?;

            // Refresh the menu off the main thread so the network call to
            // /api/auth/me doesn't block UI. The "Loading…" placeholder
            // shows until this completes.
            std::thread::spawn(move || {
                let vault_url = auth::vault_url_from_env();
                tray::refresh_menu(&handle, &vault_url);
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app_handle, event| {
            // Tray-only app — closing windows (we have none) shouldn't quit;
            // explicit Quit menu item exits.
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}
