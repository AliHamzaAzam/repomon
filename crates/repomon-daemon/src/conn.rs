//! Per-connection session state.
//!
//! Each live client connection (the local TUI over the Unix socket, or a companion app over the
//! remote WebSocket bridge) owns one [`ConnSession`]. It holds what THIS device is looking at —
//! the viewport it streams and the window it focuses — so an iPhone, an iPad, and the Mac TUI can
//! each hold their own view at once. The capture poll loop streams the UNION across every live
//! session (see [`crate::Ctx::viewport_snapshot`]), and `agent.fit` arbitrates pane sizing across
//! them (see `rpc::fit_allowed`). This replaces the old daemon-global viewport/focus slots, which
//! a second device would clobber.

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
    /// A companion app over the remote bridge. `device` is the paired device name, or `None` for
    /// the legacy shared `[remote] token`.
    Remote { device: Option<String> },
}

/// One client connection's streaming state — what THIS device is looking at. Replaces the old
/// daemon-global viewport/focus slots so multiple devices can each hold their own view at once.
pub struct ConnSession {
    /// Monotonic connection id (from [`crate::Ctx::next_conn`]); the key in `Ctx::sessions`.
    pub id: u64,
    pub kind: ConnKind,
    /// Lanes this connection currently has visible — fast-polled for output.
    pub viewport: Mutex<Vec<LaneId>>,
    /// Which agent window the focused lane streams (Tab in Focus/Split), if a specific session is
    /// selected. Lanes not named here stream their first slot.
    pub viewport_focus: Mutex<Option<(LaneId, String)>>,
    /// When this connection last (re)asserted its viewport. The client heartbeats `viewport.set`
    /// every few seconds; a focus is treated as size-owning (and cadence-boosting) only while this
    /// is fresh, so a crashed or closed client releases its hold within seconds.
    pub viewport_focus_at: Mutex<Option<Instant>>,
    /// Plain-terminal windows (`term-{lane}-{n}`) this connection has visible as Grid tiles.
    pub viewport_windows: Mutex<Vec<String>>,
    /// Windows this connection byte-watches. std Mutex: read on the event-forward hot path.
    /// (Populated by task A4; the field exists now so the struct is final.)
    pub watched_bytes: std::sync::Mutex<HashSet<String>>,
    /// Snapshot of `(viewport lanes, viewport_windows)` used to filter `event.agent.output` on the
    /// event-forward hot path. Deliberately duplicates the tokio `viewport`/`viewport_windows`
    /// fields: those stay the source of truth for the async poll loop (`viewport_snapshot`), but
    /// the forwarding loops must not `await`, so they read this std-Mutex mirror instead. The
    /// `viewport.set` handler is the single writer and rewrites BOTH the tokio fields and this
    /// snapshot together, so they never diverge. Empty at session creation, matching a connection
    /// that has not yet asserted a viewport (it receives no output events).
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

/// Drops a connection's [`ConnSession`] from `Ctx::sessions` when the connection task ends —
/// including on an early `?`/`return` error path or a panic. `close_session` is async, so cleanup
/// is spawned onto the runtime rather than awaited in `drop`. Both transports hold one of these
/// for the life of a connection so no session ever outlives its socket.
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
