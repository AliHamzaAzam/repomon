use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use repomon_core::Config;
use repomon_core::client::DaemonClient;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub const CONNECTION_EVENT: &str = "connection-state";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub uptime_secs: u64,
    pub repos: usize,
    pub lanes: usize,
    pub db_size_bytes: u64,
    pub version: String,
    #[serde(default)]
    pub protocol_revision: Option<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectionSnapshot {
    pub phase: String,
    pub endpoint: String,
    pub message: Option<String>,
    pub daemon: Option<DaemonStatus>,
}

impl ConnectionSnapshot {
    pub fn starting(endpoint: impl Into<String>) -> Self {
        Self::new("starting", endpoint, None, None)
    }

    pub fn connecting(endpoint: impl Into<String>) -> Self {
        Self::new("connecting", endpoint, None, None)
    }

    pub fn connected(endpoint: impl Into<String>, daemon: DaemonStatus) -> Self {
        Self::new("connected", endpoint, None, Some(daemon))
    }

    pub fn retrying(endpoint: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new("retrying", endpoint, Some(message.into()), None)
    }

    pub fn stopped(endpoint: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new("stopped", endpoint, Some(message.into()), None)
    }

    fn new(
        phase: &str,
        endpoint: impl Into<String>,
        message: Option<String>,
        daemon: Option<DaemonStatus>,
    ) -> Self {
        Self {
            phase: phase.into(),
            endpoint: endpoint.into(),
            message,
            daemon,
        }
    }
}

pub async fn fetch_daemon_status(client: &DaemonClient) -> Result<DaemonStatus> {
    client.call_typed("daemon.status", None).await
}

pub async fn supervise(app: AppHandle, config: Config, socket_override: Option<PathBuf>) {
    let endpoint = app.state::<AppState>().endpoint().to_string();
    publish(&app, ConnectionSnapshot::connecting(&endpoint)).await;

    let client = loop {
        if app.state::<AppState>().manual_stop.load(std::sync::atomic::Ordering::SeqCst) {
            publish(
                &app,
                ConnectionSnapshot::stopped(&endpoint, "Daemon is stopped"),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        match repomon_core::launch::ensure_daemon(&config, socket_override.clone()).await {
            Ok(client) => break client,
            Err(error) => {
                if app.state::<AppState>().manual_stop.load(std::sync::atomic::Ordering::SeqCst) {
                    publish(
                        &app,
                        ConnectionSnapshot::stopped(&endpoint, "Daemon is stopped"),
                    )
                    .await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }

                publish(
                    &app,
                    ConnectionSnapshot::retrying(&endpoint, error.to_string()),
                )
                .await;
                tokio::time::sleep(Duration::from_secs(1)).await;
                publish(&app, ConnectionSnapshot::connecting(&endpoint)).await;
            }
        }
    };

    let state = app.state::<AppState>();
    let _ = state.client.set(client);

    loop {
        if app.state::<AppState>().manual_stop.load(std::sync::atomic::Ordering::SeqCst) {
            publish(
                &app,
                ConnectionSnapshot::stopped(&endpoint, "Daemon is stopped"),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        let client = app
            .state::<AppState>()
            .client
            .get()
            .expect("connection supervisor initialized the daemon client")
            .clone();

        match fetch_daemon_status(&client).await {
            Ok(status) => {
                if !app.state::<AppState>().manual_stop.load(std::sync::atomic::Ordering::SeqCst) {
                    publish(&app, ConnectionSnapshot::connected(&endpoint, status)).await;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(error) => {
                if app.state::<AppState>().manual_stop.load(std::sync::atomic::Ordering::SeqCst) {
                    publish(
                        &app,
                        ConnectionSnapshot::stopped(&endpoint, "Daemon is stopped"),
                    )
                    .await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }

                publish(
                    &app,
                    ConnectionSnapshot::retrying(&endpoint, error.to_string()),
                )
                .await;

                // Ensure a daemon is bound again. The OnceCell keeps the original shared client;
                // its next status call transparently reconnects to the restored endpoint.
                let _ = repomon_core::launch::ensure_daemon(&config, socket_override.clone()).await;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

pub async fn publish(app: &AppHandle, snapshot: ConnectionSnapshot) {
    let state = app.state::<AppState>();
    *state.connection.write().unwrap() = snapshot.clone();
    let _ = app.emit(CONNECTION_EVENT, snapshot);
}

#[cfg(test)]
mod tests {
    // The framed-socket test binds a real Unix socket; on Windows the daemon transport is a
    // named pipe and the equivalent end-to-end path is covered by the daemon's own tests.
    #[cfg(unix)]
    use repomon_core::client::DaemonClient;
    #[cfg(unix)]
    use repomon_core::protocol::{Request, Response, read_frame, write_message};
    #[cfg(unix)]
    use serde_json::json;
    #[cfg(unix)]
    use tokio::net::UnixListener;

    #[cfg(unix)]
    use super::fetch_daemon_status;
    use super::{ConnectionSnapshot, DaemonStatus};

    #[test]
    fn snapshots_keep_phase_endpoint_and_status_together() {
        let endpoint = "/tmp/repomon-test.sock";
        let status = DaemonStatus {
            uptime_secs: 75,
            repos: 4,
            lanes: 7,
            db_size_bytes: 8192,
            version: "0.5.0".into(),
            protocol_revision: Some(2),
            capabilities: vec!["terminal.checkpoint.v1".into()],
        };

        let connecting = ConnectionSnapshot::connecting(endpoint);
        assert_eq!(connecting.phase, "connecting");
        assert_eq!(connecting.endpoint, endpoint);
        assert!(connecting.daemon.is_none());

        let connected = ConnectionSnapshot::connected(endpoint, status.clone());
        assert_eq!(connected.phase, "connected");
        assert_eq!(connected.daemon, Some(status));

        let retrying = ConnectionSnapshot::retrying(endpoint, "socket closed");
        assert_eq!(retrying.phase, "retrying");
        assert_eq!(retrying.message.as_deref(), Some("socket closed"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn maps_daemon_status_from_a_framed_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("desktop-status.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = stream.into_split();
            let frame = read_frame(&mut read).await.unwrap().unwrap();
            let request: Request = serde_json::from_slice(&frame).unwrap();
            assert_eq!(request.method, "daemon.status");
            write_message(
                &mut write,
                &Response::ok(
                    request.id,
                    json!({
                        "uptime_secs": 61,
                        "repos": 3,
                        "lanes": 5,
                        "db_size_bytes": 4096,
                        "version": "0.5.0",
                        "protocol_revision": 2,
                        "capabilities": ["terminal.checkpoint.v1"]
                    }),
                ),
            )
            .await
            .unwrap();
        });

        let client = DaemonClient::connect(&socket).await.unwrap();
        let status = fetch_daemon_status(&client).await.unwrap();

        assert_eq!(status.uptime_secs, 61);
        assert_eq!(status.repos, 3);
        assert_eq!(status.lanes, 5);
        assert_eq!(status.version, "0.5.0");
        assert_eq!(status.protocol_revision, Some(2));
        assert_eq!(status.capabilities, vec!["terminal.checkpoint.v1"]);
    }
}
