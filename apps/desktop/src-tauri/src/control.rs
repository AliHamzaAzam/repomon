use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use repomon_core::client::DaemonClient;
use repomon_core::service;
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::connection::{ConnectionSnapshot, publish};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonServiceInfo {
    pub service_managed: bool,
    pub status: String,
}

/// Poll `socket` until nothing answers a connect, or `timeout` elapses. A shut-down daemon's
/// listener can take longer than a fixed sleep to actually close (SQLite/watcher teardown,
/// system load) — spawning a replacement before that happens used to race the still-dying
/// process's socket bind, which either failed the respawn outright or (before the transport-level
/// fix) let the new daemon silently steal the socket file out from under a still-running orphan.
/// Polling for a real disconnect makes restart wait exactly as long as the old process needs,
/// no more.
async fn wait_until_unreachable(socket: &PathBuf, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if DaemonClient::connect(socket).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn service_is_installed(status: &str) -> bool {
    let normalized = status.trim().to_lowercase();
    !normalized.is_empty() && !normalized.contains("not installed")
}

pub fn is_service_managed() -> bool {
    service::status()
        .map(|status| service_is_installed(&status))
        .unwrap_or(false)
}

#[tauri::command]
pub fn daemon_service_info() -> DaemonServiceInfo {
    let status_str = service::status().unwrap_or_else(|e| format!("error: {e}"));
    let service_managed = is_service_managed();
    DaemonServiceInfo {
        service_managed,
        status: status_str,
    }
}

#[tauri::command]
pub async fn daemon_stop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.manual_stop.store(true, Ordering::SeqCst);
    let endpoint = state.endpoint().to_string();
    let service_managed = is_service_managed();

    // 1. Call daemon.shutdown RPC and wait for the listener to actually go away, not just a
    // fixed delay (see `wait_until_unreachable`).
    let socket = PathBuf::from(&endpoint);
    if let Ok(client) = DaemonClient::connect(&socket).await {
        let _ = client.call("daemon.shutdown", None).await;
        wait_until_unreachable(&socket, Duration::from_secs(3)).await;
    } else if let Some(client) = state.client.get() {
        let _ = client.call("daemon.shutdown", None).await;
        wait_until_unreachable(&socket, Duration::from_secs(3)).await;
    }

    // 2. If service-managed, stop service so launchd/systemd KeepAlive doesn't revive it
    if service_managed {
        let _ = service::stop();
    }

    publish(
        &app,
        ConnectionSnapshot::stopped(&endpoint, "Daemon is stopped"),
    )
    .await;
    Ok(())
}

#[tauri::command]
pub async fn daemon_start(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.manual_stop.store(false, Ordering::SeqCst);
    let endpoint = state.endpoint().to_string();
    let service_managed = is_service_managed();

    if service_managed {
        service::start().map_err(|e| e.to_string())?;
    } else {
        let socket = PathBuf::from(&endpoint);
        repomon_core::launch::spawn_daemon(&socket).map_err(|e| e.to_string())?;
    }

    publish(&app, ConnectionSnapshot::connecting(&endpoint)).await;
    Ok(())
}

#[tauri::command]
pub async fn daemon_restart(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.manual_stop.store(true, Ordering::SeqCst);
    let endpoint = state.endpoint().to_string();
    let service_managed = is_service_managed();

    // 1. Gracefully shut down the current daemon and wait for the listener to actually go away
    // (see `wait_until_unreachable`) — spawning the replacement before the old one has released
    // the socket used to race its bind.
    let socket = PathBuf::from(&endpoint);
    if let Ok(client) = DaemonClient::connect(&socket).await {
        let _ = client.call("daemon.shutdown", None).await;
        wait_until_unreachable(&socket, Duration::from_secs(3)).await;
    } else if let Some(client) = state.client.get() {
        let _ = client.call("daemon.shutdown", None).await;
        wait_until_unreachable(&socket, Duration::from_secs(3)).await;
    }

    // 2. Restart service or re-spawn process
    if service_managed {
        let _ = service::stop();
        wait_until_unreachable(&socket, Duration::from_secs(3)).await;
        service::start().map_err(|e| e.to_string())?;
    } else {
        let _ = repomon_core::launch::spawn_daemon(&socket);
    }

    state.manual_stop.store(false, Ordering::SeqCst);
    publish(&app, ConnectionSnapshot::connecting(&endpoint)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_is_installed_classifies_correctly() {
        assert!(!service_is_installed("not installed"));
        assert!(!service_is_installed("  NOT INSTALLED \n"));
        assert!(!service_is_installed(""));
        assert!(service_is_installed("active"));
        assert!(service_is_installed("Running"));
        assert!(service_is_installed("com.repomon.daemon = 1234"));
    }
}
