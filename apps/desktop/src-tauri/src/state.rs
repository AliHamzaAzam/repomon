use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use repomon_core::client::DaemonClient;
use repomon_core::protocol::Notification;
use tokio::sync::OnceCell;
use tokio::sync::{broadcast, oneshot};

use crate::connection::ConnectionSnapshot;
use crate::terminal::RouteFrame;

pub struct AppState {
    pub client: OnceCell<DaemonClient>,
    pub connection: RwLock<ConnectionSnapshot>,
    pub terminal_watches: Arc<Mutex<HashMap<String, oneshot::Sender<oneshot::Sender<()>>>>>,
    /// Per-window byte routes fed by the demux task: each `event.agent.bytes` chunk is
    /// matched, decoded, and routed exactly once instead of being cloned into every mounted
    /// pane's own subscription and filtered N-1 times.
    pub terminal_routes: Arc<Mutex<HashMap<String, broadcast::Sender<Arc<RouteFrame>>>>>,
    /// Non-bytes daemon events, re-broadcast by the demux for `daemon_subscribe` — so UI
    /// event listeners never receive (and drop) the byte firehose.
    pub ui_events: broadcast::Sender<Notification>,
    /// Spawns the demux task exactly once per app run (both `daemon_subscribe` and
    /// `term_watch` race to be first).
    pub demux_started: OnceCell<()>,
    endpoint: String,
}

impl AppState {
    pub fn new(endpoint: PathBuf) -> Self {
        let endpoint = endpoint.to_string_lossy().into_owned();
        Self {
            client: OnceCell::new(),
            connection: RwLock::new(ConnectionSnapshot::starting(&endpoint)),
            terminal_watches: Arc::new(Mutex::new(HashMap::new())),
            terminal_routes: Arc::new(Mutex::new(HashMap::new())),
            ui_events: broadcast::channel(256).0,
            demux_started: OnceCell::new(),
            endpoint,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
