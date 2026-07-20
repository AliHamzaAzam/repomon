mod connection;
mod ipc;
mod state;
mod terminal;

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
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::new(endpoint))
        .setup(move |app| {
            let handle = app.handle().clone();
            let config = config.clone();
            let socket_override = socket_override.clone();
            tauri::async_runtime::spawn(async move {
                connection::supervise(handle, config, socket_override).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connection_status,
            ipc::daemon_call,
            ipc::daemon_subscribe,
            terminal::term_watch,
            terminal::term_unwatch
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
