mod connection;
mod ipc;
mod state;
mod terminal;
mod update;

use std::path::PathBuf;

use repomon_core::{Config, config};

use state::AppState;

#[tauri::command]
fn connection_status(state: tauri::State<'_, AppState>) -> ConnectionSnapshot {
    state.connection.read().unwrap().clone()
}

use connection::ConnectionSnapshot;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = Config::load().unwrap_or_default();
    let socket_override = std::env::var_os("REPOMON_SOCKET").map(PathBuf::from);
    let endpoint = socket_override
        .clone()
        .unwrap_or_else(|| config::socket_path(&config));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // Terminal hyperlinks hand their URL to the system browser. Without this the webview
        // treats a link click as a navigation request, which only raises a "do you want to
        // navigate" prompt and then goes nowhere.
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::new(endpoint))
        .setup(move |app| {
            let handle = app.handle().clone();
            let config = config.clone();
            let socket_override = socket_override.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) =
                    update::apply_pending_daemon_update(&config, socket_override.clone()).await
                {
                    eprintln!("could not update bundled daemon: {error}");
                }
                connection::supervise(handle, config, socket_override).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connection_status,
            ipc::daemon_call,
            ipc::daemon_subscribe,
            terminal::term_watch,
            terminal::term_unwatch,
            update::mark_daemon_update,
            update::clear_daemon_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
