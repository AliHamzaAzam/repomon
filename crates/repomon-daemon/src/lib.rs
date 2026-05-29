//! `repomon_daemon` — the daemon's library surface.
//!
//! Holds the shared [`Ctx`] (store + registry + lanes + config + event bus), the JSON-RPC
//! [`rpc`] dispatch, the [`socket`] server, and [`pubsub`]. The `repomond` binary is a thin
//! wrapper around [`serve`]; the integration tests drive [`Ctx`] + [`serve`] directly.

pub mod pubsub;
pub mod rpc;
pub mod socket;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use repomon_core::model::LaneId;
use repomon_core::protocol::Notification;
use repomon_core::{Config, Lanes, Registry, Store, TmuxRuntime};
use serde_json::Value;
use tokio::sync::{broadcast, Mutex, Notify};

pub use socket::serve;

/// Everything a request handler needs. Cheap to share via `Arc`.
pub struct Ctx {
    pub store: Store,
    pub registry: Registry,
    pub lanes: Lanes,
    pub config: Config,
    pub tmux: TmuxRuntime,
    pub started: Instant,
    pub db_path: Option<PathBuf>,
    pub events: pubsub::EventTx,
    /// Lanes the TUI currently has visible — fast-polled for output (M9).
    pub viewport: Mutex<Vec<LaneId>>,
    pub shutdown: Notify,
}

impl Ctx {
    pub fn new(store: Store, config: Config, db_path: Option<PathBuf>) -> Arc<Self> {
        let registry = Registry::new(store.clone());
        let lanes = Lanes::new(store.clone(), config.clone());
        let tmux = TmuxRuntime::new(config.tmux_session.clone());
        let (events, _rx) = broadcast::channel(512);
        Arc::new(Ctx {
            store,
            registry,
            lanes,
            config,
            tmux,
            started: Instant::now(),
            db_path,
            events,
            viewport: Mutex::new(Vec::new()),
            shutdown: Notify::new(),
        })
    }

    /// Publish an `event.<topic>` notification to all subscribers.
    pub fn broadcast(&self, method: &str, params: Value) {
        let note = Notification::new(method, params);
        if let Ok(value) = serde_json::to_value(&note) {
            // Err just means no subscribers; that's fine.
            let _ = self.events.send(value);
        }
    }

    /// Signal the accept loop to stop.
    pub fn request_shutdown(&self) {
        self.shutdown.notify_waiters();
    }
}

/// Viewport-aware output streaming: fast-poll the tmux panes the TUI currently has visible
/// and push `event.agent.output` deltas. When nothing is visible, this is nearly free.
pub async fn stream_output(ctx: Arc<Ctx>) {
    use std::collections::HashMap;
    let mut last: HashMap<LaneId, String> = HashMap::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        tick.tick().await;
        let lanes: Vec<LaneId> = ctx.viewport.lock().await.clone();
        if lanes.is_empty() {
            last.clear();
            continue;
        }
        last.retain(|k, _| lanes.contains(k));
        for lane in lanes {
            let tmux = ctx.tmux.clone();
            let content = match tokio::task::spawn_blocking(move || tmux.capture(lane, None)).await
            {
                Ok(Ok(c)) => c,
                _ => continue,
            };
            if last.get(&lane).map(|c| c != &content).unwrap_or(true) {
                last.insert(lane, content.clone());
                ctx.broadcast(
                    pubsub::topic::AGENT_OUTPUT,
                    serde_json::json!({ "lane_id": lane, "content": content }),
                );
            }
        }
    }
}
