//! `repomon_daemon` — the daemon's library surface.
//!
//! Holds the shared [`Ctx`] (store + registry + lanes + config + event bus), the JSON-RPC
//! [`rpc`] dispatch, the [`socket`] server, and [`pubsub`]. The `repomond` binary is a thin
//! wrapper around [`serve`]; the integration tests drive [`Ctx`] + [`serve`] directly.

pub mod auto_continue;
pub mod pubsub;
pub mod rpc;
pub mod socket;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use repomon_core::model::LaneId;
use repomon_core::protocol::Notification;
use repomon_core::{config, Config, Lanes, Registry, Store, TmuxRuntime};
use serde_json::Value;
use tokio::sync::{broadcast, Mutex, Notify, RwLock};

pub use socket::serve;

/// Everything a request handler needs. Cheap to share via `Arc`.
pub struct Ctx {
    pub store: Store,
    pub registry: Registry,
    pub lanes: Lanes,
    /// User config. Behind a lock because the agent-manager RPCs mutate it (and persist to
    /// disk) at runtime; most fields are static after startup.
    pub config: RwLock<Config>,
    /// Where [`Config::save`] writes — `config::config_path()` in prod, a tempdir in tests.
    pub config_path: PathBuf,
    pub tmux: TmuxRuntime,
    pub started: Instant,
    pub db_path: Option<PathBuf>,
    pub events: pubsub::EventTx,
    /// Lanes the TUI currently has visible — fast-polled for output (M9).
    pub viewport: Mutex<Vec<LaneId>>,
    /// Which agent window the focused lane should stream, when the TUI has a specific session
    /// selected (Tab in Focus/Split). Lanes not named here stream their first slot.
    pub viewport_focus: Mutex<Option<(LaneId, String)>>,
    /// Cache of how many live `claude` processes have each working dir (ps/lsof, ~2s TTL), so
    /// `/exit`ed sessions whose transcripts linger aren't counted as running.
    pub live_cwds: Mutex<Option<(Instant, HashMap<PathBuf, usize>)>>,
    /// Lanes currently paused on a usage limit, with their reset time — written by the
    /// auto-continue watcher and read by `overlay_agents` to surface the `RateLimited` status.
    pub rate_limits: Mutex<HashMap<LaneId, auto_continue::RateLimit>>,
    /// Lanes where the user disabled auto-continue this session (the `C` key).
    pub auto_continue_off: Mutex<HashSet<LaneId>>,
    pub shutdown: Notify,
}

impl Ctx {
    pub fn new(store: Store, config: Config, db_path: Option<PathBuf>) -> Arc<Self> {
        Self::new_with_config_path(store, config, db_path, config::config_path())
    }

    /// Like [`new`](Self::new) but with an explicit config-file path (tests use a tempdir so
    /// agent-manager mutations never touch the real `~/.config/repomon/config.toml`).
    pub fn new_with_config_path(
        store: Store,
        config: Config,
        db_path: Option<PathBuf>,
        config_path: PathBuf,
    ) -> Arc<Self> {
        let registry = Registry::new(store.clone());
        let lanes = Lanes::new(store.clone(), config.clone());
        let tmux = TmuxRuntime::new(config.tmux_session.clone());
        // Make any already-running session attach-native (mouse, clipboard, deep scrollback);
        // spawns reapply it, but an existing tmux server outlives a daemon restart.
        if tmux.session_exists() {
            tmux.configure();
        }
        let (events, _rx) = broadcast::channel(512);
        Arc::new(Ctx {
            store,
            registry,
            lanes,
            config: RwLock::new(config),
            config_path,
            tmux,
            started: Instant::now(),
            db_path,
            events,
            viewport: Mutex::new(Vec::new()),
            viewport_focus: Mutex::new(None),
            live_cwds: Mutex::new(None),
            rate_limits: Mutex::new(HashMap::new()),
            auto_continue_off: Mutex::new(HashSet::new()),
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
    // Last pushed (window, content) per lane — a window switch (Tab between a lane's agents)
    // re-pushes even if the new pane happens to look identical.
    let mut last: HashMap<LaneId, (String, String)> = HashMap::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        tick.tick().await;
        let lanes: Vec<LaneId> = ctx.viewport.lock().await.clone();
        if lanes.is_empty() {
            last.clear();
            continue;
        }
        let focus = ctx.viewport_focus.lock().await.clone();
        last.retain(|k, _| lanes.contains(k));
        for lane in lanes {
            // The focused lane streams its selected agent's window; others their first slot.
            let window = match &focus {
                Some((l, w)) if *l == lane => w.clone(),
                _ => TmuxRuntime::window_name(lane),
            };
            let tmux = ctx.tmux.clone();
            let w = window.clone();
            let content =
                match tokio::task::spawn_blocking(move || tmux.capture_named(&w, None)).await {
                    Ok(Ok(c)) => c,
                    _ => continue,
                };
            let fresh = last
                .get(&lane)
                .map(|(pw, pc)| pw != &window || pc != &content)
                .unwrap_or(true);
            if fresh {
                last.insert(lane, (window, content.clone()));
                ctx.broadcast(
                    pubsub::topic::AGENT_OUTPUT,
                    serde_json::json!({ "lane_id": lane, "content": content }),
                );
            }
        }
    }
}
