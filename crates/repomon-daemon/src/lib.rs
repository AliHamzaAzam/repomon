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

use repomon_core::protocol::Notification;
use repomon_core::{Config, Lanes, Registry, Store};
use serde_json::Value;
use tokio::sync::{broadcast, Notify};

pub use socket::serve;

/// Everything a request handler needs. Cheap to share via `Arc`.
pub struct Ctx {
    pub store: Store,
    pub registry: Registry,
    pub lanes: Lanes,
    pub config: Config,
    pub started: Instant,
    pub db_path: Option<PathBuf>,
    pub events: pubsub::EventTx,
    pub shutdown: Notify,
}

impl Ctx {
    pub fn new(store: Store, config: Config, db_path: Option<PathBuf>) -> Arc<Self> {
        let registry = Registry::new(store.clone());
        let lanes = Lanes::new(store.clone(), config.clone());
        let (events, _rx) = broadcast::channel(512);
        Arc::new(Ctx {
            store,
            registry,
            lanes,
            config,
            started: Instant::now(),
            db_path,
            events,
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
