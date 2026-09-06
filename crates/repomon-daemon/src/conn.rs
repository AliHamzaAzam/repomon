//! Keeps viewport and focus claims per connection. Capture subscriptions are combined, while resize
//! ownership is arbitrated between clients.

use std::collections::HashSet;
use std::time::Instant;

use repomon_core::model::LaneId;
use tokio::sync::Mutex;

/// What kind of transport a connection arrived on. `agent.fit` gives a Local (TUI) focus
/// precedence over remote viewers; the device name is carried for a Remote so a session can be
/// attributed to its paired device.
#[derive(Debug, Clone)]
pub enum ConnKind {
    /// The local Unix-socket client (the TUI).
    Local,
    /// Identifies a remote bridge connection by its paired device name, or None for the shared
    /// configuration token.
    Remote { device: Option<String> },
}

/// One client connection's streaming state - what THIS device is looking at. Replaces the old
/// daemon-global viewport/focus slots so multiple devices can each hold their own view at once.
pub struct ConnSession {
    /// Monotonic connection id (from [`crate::Ctx::next_conn`]); the key in `Ctx::sessions`.
    pub id: u64,
    pub kind: ConnKind,
    /// Lanes this connection currently has visible - fast-polled for output.
    pub viewport: Mutex<Vec<LaneId>>,
    /// Which agent window the focused lane streams (Tab in Focus/Split), if a specific session is
    /// selected. Lanes not named here stream their first slot.
    pub viewport_focus: Mutex<Option<(LaneId, String)>>,
    /// Managed agent windows this connection renders concurrently and therefore needs to fit to
    /// its own pane geometry. Unlike `viewport_focus`, this can contain every pane in a grid.
    pub viewport_fit_windows: Mutex<Vec<String>>,
    /// When this connection last (re)asserted its viewport. The client heartbeats `viewport.set`
    /// every few seconds; its focus and fit windows are size-owning only while this is fresh, so a
    /// crashed or closed client releases its hold within seconds. Focus also boosts capture cadence.
    pub viewport_focus_at: Mutex<Option<Instant>>,
    /// Plain-terminal windows (`term-{lane}-{n}`) this connection has visible as Grid tiles.
    pub viewport_windows: Mutex<Vec<String>>,
    /// Tracks byte watches behind a synchronous mutex for event-forward filtering.
    pub watched_bytes: std::sync::Mutex<HashSet<String>>,
    /// Mirrors viewport filters behind a synchronous mutex for forwarding without await;
    /// viewport.set updates both this snapshot and the async state.
    pub output_filter: std::sync::Mutex<(HashSet<LaneId>, HashSet<String>)>,
    /// When this connection last drove an agent (send_input/signal/key/scroll/answer, and a fit
    /// that actually applied). `agent.fit`'s remote-vs-remote arbitration is last-interaction-wins.
    pub last_interaction: Mutex<Option<Instant>>,
    /// Whether this connection currently has the repomind live pane visible. Keeping this per
    /// connection prevents one client from stopping the stream while another still needs it.
    pub orchestrator_watched: Mutex<bool>,
}

impl ConnSession {
    pub fn new(id: u64, kind: ConnKind) -> Self {
        ConnSession {
            id,
            kind,
            viewport: Mutex::new(Vec::new()),
            viewport_focus: Mutex::new(None),
            viewport_fit_windows: Mutex::new(Vec::new()),
            viewport_focus_at: Mutex::new(None),
            viewport_windows: Mutex::new(Vec::new()),
            watched_bytes: std::sync::Mutex::new(HashSet::new()),
            output_filter: std::sync::Mutex::new((HashSet::new(), HashSet::new())),
            last_interaction: Mutex::new(None),
            orchestrator_watched: Mutex::new(false),
        }
    }

    /// True if this connection is the local TUI.
    pub fn is_local(&self) -> bool {
        matches!(self.kind, ConnKind::Local)
    }
}

/// Schedules removal of per-connection session state on every exit path.
pub struct SessionGuard {
    ctx: std::sync::Arc<crate::Ctx>,
    id: u64,
}

impl SessionGuard {
    pub fn new(ctx: std::sync::Arc<crate::Ctx>, id: u64) -> Self {
        SessionGuard { ctx, id }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let ctx = self.ctx.clone();
        let id = self.id;
        tokio::spawn(async move { ctx.close_session(id).await });
    }
}
