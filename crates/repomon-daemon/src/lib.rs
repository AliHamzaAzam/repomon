//! `repomon_daemon` — the daemon's library surface.
//!
//! Holds the shared [`Ctx`] (store + registry + lanes + config + event bus), the JSON-RPC
//! [`rpc`] dispatch, the [`socket`] server, and [`pubsub`]. The `repomond` binary is a thin
//! wrapper around [`serve`]; the integration tests drive [`Ctx`] + [`serve`] directly.

pub mod auto_continue;
pub mod bytes_stream;
pub mod notify_watch;
pub mod pubsub;
pub mod push;
pub mod reap;
pub mod remote;
pub mod rpc;
pub mod socket;
pub mod usage_watch;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use repomon_core::model::{Lane, LaneId};
use repomon_core::protocol::Notification;
use repomon_core::{Config, Lanes, Registry, Store, TmuxRuntime, Watcher, config};
use serde_json::Value;
use tokio::sync::{Mutex, Notify, RwLock, broadcast};

pub use socket::serve;

/// A session as the overlay last surfaced it, kept per lane so a session that vanishes on the
/// next overlay can be attributed to a cause (the disappearing-sessions diagnostic). Keyed by
/// `key`: the transcript session id, or `win:<window>` / `inferred:<wt>` when there is none.
#[derive(Clone)]
pub struct OverlaySession {
    pub key: String,
    pub external: bool,
    pub inferred: bool,
    pub window: Option<String>,
    /// The transcript file this came from (empty for inferred / window-only placeholders).
    pub manifest: PathBuf,
    /// The lane's worktree path, for the live-process attribution.
    pub worktree: PathBuf,
}

/// The dedicated tmux window the repomind orchestrator runs in. Deliberately NOT a `lane-*` name,
/// so it stays invisible to the lane overlay/reaper and never shows in `lane.list`. Shared by
/// `rpc` (the RPC dispatch), `notify_watch` (the attention/pane watcher), and this module's own
/// pane-streaming loop, so a rename can't desync them.
pub(crate) const ORCHESTRATOR_WINDOW: &str = "orchestrator";

/// Which agent CLI powers the orchestrator session. This is the seam every backend-specific
/// capability routes through: command construction lives in one
/// `rpc::build_{claude,codex}_orchestrator_command` per variant, and everything else asks the
/// predicates here. A future backend is a new variant — the compiler then walks you to every
/// match site that needs an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrchestratorBackend {
    /// Claude Code — the default, and the only backend with full monitoring: its `~/.claude`
    /// JSONL transcript is parseable and `--session-id` pins it at spawn.
    Claude,
    /// Codex CLI — MCP-capable, so it can drive the fleet tools, but monitored best-effort only:
    /// its on-disk session format is unstable (same reason core's `CodexMonitor` reads nothing),
    /// so no transcript chat view, no end-of-turn attention, no session pinning — pane-based
    /// dialog detection only.
    Codex,
}

impl OrchestratorBackend {
    /// The wire word for the `backend` field of `orchestrator.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// Whether this backend has a parseable on-disk transcript the daemon may read
    /// (`orchestrator.transcript`, the end-of-turn attention check). When false, callers must
    /// skip the transcript path entirely — NOT fall through to the "newest `~/.claude` transcript
    /// with content" recency heuristic, which would misattribute some other live Claude session's
    /// chat as this orchestrator's.
    pub fn has_transcript(self) -> bool {
        matches!(self, Self::Claude)
    }
}

/// The daemon-owned repomind orchestrator: a single agent session (`backend` says which CLI) in a
/// dedicated tmux window named `orchestrator` (deliberately NOT `lane-*`, so it stays out of the
/// lane overlay/reaper and never pollutes the fleet `lane.list`). `agent`/`model` record what it
/// was launched with. `autonomy` is the autonomy level it was started with
/// (`REPOMON_MCP_AUTONOMY`); `None` when the session was adopted from a tmux window that survived
/// a daemon restart, whose actual autonomy is unknown to this process.
#[derive(Clone)]
pub struct OrchestratorSession {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub window: String,
    pub autonomy: Option<String>,
    /// Which agent CLI this session runs — see [`OrchestratorBackend`]. For an adopted window
    /// this is derived from the current request/config, not from the surviving window itself
    /// (best-effort, the same caveat as `agent`): a wrong guess degrades to an empty transcript
    /// or the recency-heuristic fallback, never an error.
    pub backend: OrchestratorBackend,
    /// The `--session-id` UUID this session's `claude` was launched with (minted at spawn time —
    /// see `rpc::mint_session_id`), so the transcript picker
    /// (`rpc::pick_orchestrator_transcript`) can pin `orchestrator.transcript` and the end-of-turn
    /// attention check to *this* session's own transcript file instead of guessing "the newest
    /// $HOME transcript" — a guess that misattributes any other active Claude session on the
    /// machine as repomind's. `None` — same "unknown" semantics as `autonomy` — when the session
    /// was adopted from a tmux window that survived a daemon restart: this process never captured
    /// the prior one's session id, so the picker falls back to the old recency heuristic.
    pub session_id: Option<String>,
}

/// One `gate_cache` entry: the ledger's mtime when last read, and the verdict parsed then.
pub type GateCacheEntry = (
    Option<std::time::SystemTime>,
    Option<repomon_core::agent::gate::GateVerdict>,
);

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
    /// When the viewport was last (re)asserted. The TUI heartbeats `viewport.set` every few
    /// seconds; `agent.fit` treats the focused window as size-owned only while this is fresh,
    /// so a closed or crashed TUI frees the pane for remote reflow within seconds.
    pub viewport_focus_at: Mutex<Option<Instant>>,
    /// Plain-terminal windows (`term-{lane}-{n}`) the TUI has visible as Grid tiles — streamed
    /// alongside the lane panes, each tagged with its window in the output event.
    pub viewport_windows: Mutex<Vec<String>>,
    /// Cache of how many live `claude` processes have each working dir (ps/lsof, 10s TTL), so
    /// `/exit`ed sessions whose transcripts linger aren't counted as running.
    pub live_cwds: Mutex<Option<(Instant, HashMap<PathBuf, usize>)>>,
    /// Per-worktree "highest count seen recently" used to make [`live_cwds`] sticky-high: a single
    /// `pgrep`/`lsof` undercount can otherwise drop a session from the overlay (then re-add it next
    /// probe), churning the lane list and — before the notification activity-latch — re-firing
    /// alerts. We hold the higher count for a short grace so one bad sample can't hide a session.
    pub cwds_sticky: Mutex<HashMap<PathBuf, (usize, Instant)>>,
    /// The composite `lane.list` overlay (lanes + live agent sessions), cached for a short TTL so
    /// many clients polling every ~1s don't each re-run the tmux/lsof/transcript scan. Invalidated
    /// on structural changes (spawn/adopt/stop/lane create/delete) so user actions show at once.
    pub overlay_cache: Mutex<Option<(Instant, Vec<Lane>)>>,
    /// Cache of the pending-prompt pane sniff per tmux window — a `capture-pane` per Running/Waiting
    /// session is the bulk of the overlay's subprocess cost. Short TTL: a dialog appearing is seen
    /// within it; until then the session reads as it last did. Keyed by window name. Any input sent
    /// to a window drops its entry, so an answered dialog can't ride out the TTL as a ghost.
    pub prompt_cache:
        Mutex<HashMap<String, (Instant, Option<repomon_core::agent::prompt::PendingDialog>)>>,
    /// Per window: the last sniffed pane-content hash and when it last CHANGED — the stall
    /// detector's clock. Never TTL-pruned (its point is remembering how long a pane has sat
    /// still); entries drop only when their window vanishes.
    pub pane_seen: Mutex<HashMap<String, (u64, chrono::DateTime<chrono::Utc>)>>,
    /// Per worktree: the dxkit loop ledger's mtime and the verdict parsed from its tail, so
    /// the overlay re-reads only when the gate actually ran again. Keyed by worktree path.
    pub gate_cache: Mutex<HashMap<PathBuf, GateCacheEntry>>,
    /// The single active PTY byte watch (the Focus view's embedded renderer feed), if any.
    pub bytes_watch: Mutex<Option<bytes_stream::BytesWatch>>,
    /// Lanes currently paused on a usage limit, with their reset time — written by the
    /// auto-continue watcher and read by `overlay_agents` to surface the `RateLimited` status.
    pub rate_limits: Mutex<HashMap<LaneId, auto_continue::RateLimit>>,
    /// Per Claude account (config-dir key) usage from the `/usage` probe — written by the usage
    /// watcher, read by `usage.get`. Empty unless `[usage_probe]` is enabled and a TUI is attached.
    pub usage: Mutex<HashMap<String, usage_watch::UsageEntry>>,
    /// Lanes where the user disabled auto-continue this session (the `C` key).
    pub auto_continue_off: Mutex<HashSet<LaneId>>,
    /// The filesystem watcher (set once the background task brings it up). Held here so `repo.add`
    /// / `repo.remove` can watch / unwatch a tree at runtime — otherwise the watcher only ever
    /// reflects the repos present at startup, and a removed repo keeps churning fsevents.
    pub watcher: Mutex<Option<Watcher>>,
    /// When a *local* client (the TUI) was last seen making a request — its 1s `lane.list` refresh
    /// is a built-in heartbeat that stops the moment the TUI parks in an attach or closes. The
    /// notification engine fires desktop popups itself once this goes stale, so an alert still
    /// reaches you when you're heads-down full-screen in an agent.
    pub local_watcher_seen: Mutex<Option<Instant>>,
    /// When a key/text/signal was last sent to each lane's agent. The output streamer reads this
    /// to capture an actively-typed pane at frame-rate (so keystroke echo feels instant), then
    /// relaxes back to the normal cadence once typing stops.
    pub input_seen: Mutex<HashMap<LaneId, Instant>>,
    /// The set of managed (`lane-…`) tmux windows seen on the previous overlay. When one
    /// disappears (an agent `/exit`ed or was stopped), the overlay refreshes the live-process
    /// count immediately so the vanished agent drops from the `×N` count without waiting out the
    /// `live_cwds` cache TTL.
    pub last_managed_windows: Mutex<HashSet<String>>,
    /// Last tmux window list a probe returned successfully. Reused for one overlay tick when
    /// `list_windows` fails transiently (fork/connection fault under load), so a single bad
    /// snapshot doesn't drop every managed agent — see `rpc::resolve_windows`.
    pub last_good_windows: Mutex<Vec<String>>,
    /// Consecutive empty `list_windows` results. A sudden total-empty is usually a tmux server
    /// bounce, not every agent exiting at once — `resolve_windows` reuses last-good until this
    /// reaches the confirm threshold, so a server restart doesn't mass-fire Idle.
    pub window_empty_misses: Mutex<u8>,
    /// Last successful per-worktree transcript scan, keyed by worktree path. Reused for one overlay
    /// tick if the scan task panics or its join fails — so a parse panic in one lane can't empty
    /// every lane's sessions. See `rpc::reuse_per_path_on_failure`.
    pub last_good_sessions: Mutex<HashMap<PathBuf, Vec<repomon_core::agent::TranscriptSummary>>>,
    /// What the overlay surfaced per lane on the previous tick, so a session that vanishes this
    /// tick is logged with an attributed reason (idle-drop diagnostic). See
    /// `rpc::diagnose_vanished_sessions`.
    pub last_overlay_sessions: Mutex<HashMap<LaneId, Vec<OverlaySession>>>,
    /// The single daemon-owned repomind orchestrator session, if one is running. `None` until
    /// `orchestrator.start` spawns it; cleared by `orchestrator.stop`.
    pub orchestrator: Mutex<Option<OrchestratorSession>>,
    /// Whether a client (the TUI's command-center view) currently wants the orchestrator pane
    /// streamed. Gates `stream_orchestrator` so capturing the pane costs nothing when nobody's
    /// watching.
    pub orchestrator_watched: Mutex<bool>,
    /// When the orchestrator pane was last typed into (any `orchestrator.send_input`/`key`), so
    /// `stream_orchestrator` captures it at frame-rate while you type to repomind, the same
    /// keystroke-echo speedup `input_seen` gives a focused lane. Goes quiet on its own.
    pub orchestrator_input_seen: Mutex<Option<Instant>>,
    /// The repomind orchestrator's current attention word (`"none"`, `"permission"`,
    /// `"decision"`, or `"end_of_turn"`) plus an optional headline — computed every
    /// `notify_watch` tick (even while notifications are disabled, so the TUI's pinned row and
    /// command-center header stay live) and folded into `orchestrator_status_value`'s payload on
    /// change. See `notify_watch::check_orchestrator_attention`.
    pub orchestrator_attention: Mutex<(String, Option<String>)>,
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
            viewport_focus_at: Mutex::new(None),
            viewport_windows: Mutex::new(Vec::new()),
            live_cwds: Mutex::new(None),
            cwds_sticky: Mutex::new(HashMap::new()),
            overlay_cache: Mutex::new(None),
            prompt_cache: Mutex::new(HashMap::new()),
            pane_seen: Mutex::new(HashMap::new()),
            gate_cache: Mutex::new(HashMap::new()),
            bytes_watch: Mutex::new(None),
            rate_limits: Mutex::new(HashMap::new()),
            usage: Mutex::new(HashMap::new()),
            auto_continue_off: Mutex::new(HashSet::new()),
            watcher: Mutex::new(None),
            local_watcher_seen: Mutex::new(None),
            input_seen: Mutex::new(HashMap::new()),
            last_managed_windows: Mutex::new(HashSet::new()),
            last_good_windows: Mutex::new(Vec::new()),
            window_empty_misses: Mutex::new(0),
            last_good_sessions: Mutex::new(HashMap::new()),
            last_overlay_sessions: Mutex::new(HashMap::new()),
            orchestrator: Mutex::new(None),
            orchestrator_watched: Mutex::new(false),
            orchestrator_input_seen: Mutex::new(None),
            orchestrator_attention: Mutex::new(("none".to_string(), None)),
            shutdown: Notify::new(),
        })
    }

    /// Drop the cached `lane.list` overlay so the next read recomputes — call after a structural
    /// change (spawn / adopt / stop / lane create / delete) so the action shows up immediately
    /// instead of waiting out the cache TTL.
    pub async fn invalidate_overlay(&self) {
        *self.overlay_cache.lock().await = None;
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
    use std::time::{Duration, Instant};

    /// Per-window streaming state: the last pushed content, the current poll interval, and
    /// when this window was last captured.
    struct St {
        content: String,
        backoff: Duration,
        last_cap: Instant,
        /// The focused pane's last-seen cursor `(col, row)`, so a cursor-only move still re-pushes.
        cursor: Option<(u16, u16)>,
    }
    // Activity-driven cadence: a lane is captured at its FLOOR while its pane keeps changing, and
    // its interval doubles toward a CAP once the pane goes quiet (reset to FLOOR on any change).
    // The FOCUSED lane — what the user is actively driving — streams fast; background/Grid tiles
    // get a slower floor and a longer idle ceiling (a phone mirror needn't render idle panes at
    // 10Hz). This turns the old flat 8-lanes×10Hz tmux-fork storm into a few captures/sec.
    const FOCUS_FLOOR: Duration = Duration::from_millis(150);
    const FOCUS_CAP: Duration = Duration::from_millis(600);
    const BG_FLOOR: Duration = Duration::from_millis(700);
    const BG_CAP: Duration = Duration::from_millis(3000);
    // While a pane is being actively typed into, capture it at ~frame-rate so keystroke echo
    // feels instant. This regime applies for TYPING_WINDOW after the last key, then relaxes back
    // to the focused/background cadence above — a brief single-pane burst, only while typing.
    const TYPING_FLOOR: Duration = Duration::from_millis(30);
    const TYPING_CAP: Duration = Duration::from_millis(60);
    const TYPING_WINDOW: Duration = Duration::from_millis(400);
    // Hard ceiling on captures per tick so entering a busy Grid (every pane "fresh" at once) can't
    // burst the whole viewport in one tick — the focused lane always goes first, the rest are
    // serviced round-robin across ticks.
    const MAX_PER_TICK: usize = 3;

    let mut state: HashMap<String, St> = HashMap::new();
    let mut rr: usize = 0; // round-robin offset so background panes share the per-tick budget fairly
    // The base tick must be at least as fast as the tightest regime (TYPING_FLOOR); per-lane
    // gating below keeps non-typing lanes at their slower cadence, so these extra wakeups are
    // cheap no-ops (no captures) when nothing is being typed.
    let mut tick = tokio::time::interval(TYPING_FLOOR);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let now = Instant::now();
        // Prune lanes typed into longer ago than TYPING_WINDOW. This runs BEFORE the empty-viewport
        // early-return below so `input_seen` is bounded even when no TUI viewport is set — otherwise
        // a lane typed into while nothing is visible would leak its entry forever.
        {
            let mut m = ctx.input_seen.lock().await;
            m.retain(|_, t| now.saturating_duration_since(*t) < TYPING_WINDOW);
        }
        let lanes: Vec<LaneId> = ctx.viewport.lock().await.clone();
        let extra_windows: Vec<String> = ctx.viewport_windows.lock().await.clone();
        if lanes.is_empty() && extra_windows.is_empty() {
            state.clear();
            continue;
        }
        let focus = ctx.viewport_focus.lock().await.clone();
        // One stream target per visible pane: each lane's resolved window, plus any plain
        // terminals the Grid is tiling (each with the lane it belongs to for the event payload).
        let mut targets: Vec<(LaneId, String)> = lanes
            .iter()
            .map(|&lane| (lane, stream_window_for(lane, &focus)))
            .collect();
        for w in &extra_windows {
            if let Some(lane) = TmuxRuntime::parse_term_window(w) {
                if !targets.iter().any(|(_, tw)| tw == w) {
                    targets.push((lane, w.clone()));
                }
            }
        }
        state.retain(|w, _| targets.iter().any(|(_, tw)| tw == w));
        // Snapshot which lanes were typed into recently — they capture at frame-rate. The map was
        // just pruned above, so this is the live set of within-TYPING_WINDOW lanes.
        let typing_lanes: HashMap<LaneId, Instant> = ctx.input_seen.lock().await.clone();

        // Service the focused pane first (so the per-tick cap never starves what the user is
        // watching), then the rest from a rotating offset so every background pane gets a turn.
        let focus_window = focus.as_ref().map(|(_, w)| w.clone());
        let n = targets.len();
        let mut order: Vec<(LaneId, String)> = Vec::with_capacity(n);
        if let Some(fw) = &focus_window {
            if let Some(t) = targets.iter().find(|(_, w)| w == fw) {
                order.push(t.clone());
            }
        }
        for i in 0..n {
            let t = &targets[(rr + i) % n];
            if Some(&t.1) != focus_window.as_ref() {
                order.push(t.clone());
            }
        }
        rr = (rr + 1) % n;

        let mut budget = MAX_PER_TICK;
        for (lane, window) in order {
            let is_focused = focus_window.as_deref() == Some(window.as_str());
            // Cadence regime: a lane typed into within TYPING_WINDOW captures at frame-rate;
            // otherwise the focused pane is fast and background/Grid tiles slow.
            let typing = typing_lanes
                .get(&lane)
                .is_some_and(|t| now.saturating_duration_since(*t) < TYPING_WINDOW);
            let (floor, cap) = if typing {
                (TYPING_FLOOR, TYPING_CAP)
            } else if is_focused {
                (FOCUS_FLOOR, FOCUS_CAP)
            } else {
                (BG_FLOOR, BG_CAP)
            };
            // The poll interval, re-clamped to the current regime each tick — so the moment a lane
            // starts being typed into, a stale 150ms wait shrinks to <=60ms and it captures on the
            // next tick (prompt first-keystroke echo without coupling to the input handler).
            let interval = state
                .get(&window)
                .map(|s| s.backoff.clamp(floor, cap))
                .unwrap_or(floor);
            // Not due yet → leave it for a later tick; costs nothing. A window absent from the
            // map (freshly spawned / first frame / Tab switch) is always due, so fresh output
            // shows immediately.
            if let Some(s) = state.get(&window) {
                if now < s.last_cap + interval {
                    continue;
                }
            }
            // Cap captures per tick; a due pane skipped here is picked up next tick (rr rotates).
            if budget == 0 {
                break;
            }
            budget -= 1;
            let tmux = ctx.tmux.clone();
            let w = window.clone();
            let content =
                match tokio::task::spawn_blocking(move || tmux.capture_named(&w, None)).await {
                    Ok(Ok(c)) => c,
                    _ => continue,
                };
            // Only the focused pane carries a cursor (the TUI renders it where you're typing) — one
            // extra tmux fork on a single pane, never on background/Grid tiles.
            let cursor = if is_focused {
                let tmux = ctx.tmux.clone();
                let cw = window.clone();
                tokio::task::spawn_blocking(move || tmux.cursor_named(&cw))
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            let content_changed = state
                .get(&window)
                .map(|s| s.content != content)
                .unwrap_or(true);
            // The focused pane also re-pushes on a cursor-only move (arrowing within the input box)
            // so the rendered cursor tracks even when the text itself is unchanged.
            let cursor_changed = is_focused
                && state
                    .get(&window)
                    .map(|s| s.cursor != cursor)
                    .unwrap_or(true);
            let changed = content_changed || cursor_changed;
            // Reset to the floor on any change; otherwise double the (clamped) interval toward cap.
            let backoff = if changed {
                floor
            } else {
                (interval * 2).min(cap)
            };
            if changed {
                ctx.broadcast(
                    pubsub::topic::AGENT_OUTPUT,
                    serde_json::json!({
                        "lane_id": lane,
                        "window": window,
                        "content": content.clone(),
                        "cursor": cursor.map(|(x, y)| [x, y]),
                    }),
                );
            }
            state.insert(
                window,
                St {
                    content,
                    backoff,
                    last_cap: now,
                    cursor,
                },
            );
        }
    }
}

/// The window a viewport lane streams: the TUI-selected agent window when this lane is the
/// focus (Tab in Focus/Split), else the lane's first slot. A focused plain terminal never
/// hijacks its lane's stream — the terminal is its own target via `viewport_windows`, so the
/// lane's agent tile keeps updating beside it.
fn stream_window_for(lane: LaneId, focus: &Option<(LaneId, String)>) -> String {
    match focus {
        Some((l, w)) if *l == lane && TmuxRuntime::parse_term_window(w).is_none() => w.clone(),
        _ => TmuxRuntime::window_name(lane),
    }
}

/// Stream the repomind orchestrator's pane to subscribed clients. While a session is running AND a
/// client has asked to watch it (`orchestrator_watched`), capture the `orchestrator` window and
/// broadcast `event.orchestrator.output` whenever the pane text or cursor changes. Idle (no session
/// or nobody watching) it does nothing but a cheap flag check.
///
/// Cadence mirrors the focused-lane regime in [`stream_output`], for this single pane: while you are
/// typing to repomind (within `TYPING_WINDOW` of the last `orchestrator.send_input`/`key`) it
/// captures at frame-rate so keystroke echo feels instant; once typing goes quiet it relaxes to a
/// focused cadence and backs off toward a cap while the pane is unchanged. The old flat 200ms tick
/// made typing in repomind echo at ~5fps versus a lane's ~30Hz.
pub async fn stream_orchestrator(ctx: Arc<Ctx>) {
    use std::time::{Duration, Instant};

    const TYPING_FLOOR: Duration = Duration::from_millis(30);
    const TYPING_CAP: Duration = Duration::from_millis(60);
    const TYPING_WINDOW: Duration = Duration::from_millis(400);
    const WATCH_FLOOR: Duration = Duration::from_millis(150);
    const WATCH_CAP: Duration = Duration::from_millis(600);

    let mut last: Option<String> = None;
    let mut last_cursor: Option<(u16, u16)> = None;
    let mut backoff = WATCH_FLOOR;
    let mut last_cap = Instant::now();
    // Wake at the tightest regime; the due-check below keeps a quiet pane at its slower cadence, so
    // the extra wakeups are cheap no-ops (no capture) when nothing is being typed.
    let mut tick = tokio::time::interval(TYPING_FLOOR);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let watched = *ctx.orchestrator_watched.lock().await;
        let running = ctx.orchestrator.lock().await.is_some();
        if !watched || !running {
            last = None;
            last_cursor = None;
            backoff = WATCH_FLOOR;
            continue;
        }
        let now = Instant::now();
        // Frame-rate while typing to repomind, else the watched-but-quiet focused cadence.
        let typing = ctx
            .orchestrator_input_seen
            .lock()
            .await
            .is_some_and(|t| now.saturating_duration_since(t) < TYPING_WINDOW);
        let (floor, cap) = if typing {
            (TYPING_FLOOR, TYPING_CAP)
        } else {
            (WATCH_FLOOR, WATCH_CAP)
        };
        // Re-clamped each tick, so the first keystroke shrinks a stale 150ms wait to <=60ms and the
        // pane captures on the next tick (prompt first-key echo); not due yet otherwise.
        let interval = backoff.clamp(floor, cap);
        if now < last_cap + interval {
            continue;
        }
        last_cap = now;
        let tmux = ctx.tmux.clone();
        let content = match tokio::task::spawn_blocking(move || {
            tmux.capture_named(ORCHESTRATOR_WINDOW, None)
        })
        .await
        {
            Ok(Ok(c)) => c,
            _ => continue,
        };
        // Carry repomind's real cursor so the mediated pane draws it where you're typing (mirrors
        // the focused-lane path in `stream_output`). One extra tmux fork on the single pane.
        let tmux = ctx.tmux.clone();
        let cursor = tokio::task::spawn_blocking(move || tmux.cursor_named(ORCHESTRATOR_WINDOW))
            .await
            .ok()
            .flatten();
        // Re-push on a cursor-only move (arrowing within repomind's input box) so the rendered cursor
        // tracks even when the text is unchanged. Reset to the floor on any change; otherwise double
        // the (clamped) interval toward the cap so a settled pane stops being re-captured.
        let changed = last.as_deref() != Some(content.as_str()) || last_cursor != cursor;
        backoff = if changed {
            floor
        } else {
            (interval * 2).min(cap)
        };
        if changed {
            ctx.broadcast(
                pubsub::topic::ORCHESTRATOR_OUTPUT,
                serde_json::json!({
                    "content": content.clone(),
                    "cursor": cursor.map(|(x, y)| [x, y]),
                }),
            );
            last = Some(content);
            last_cursor = cursor;
        }
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn focused_terminal_never_hijacks_its_lanes_stream() {
        // No focus / focus on another lane: the lane streams its first slot.
        assert_eq!(stream_window_for(7, &None), "lane-7");
        assert_eq!(
            stream_window_for(7, &Some((3, "lane-3-2".into()))),
            "lane-7"
        );
        // The focused lane streams its selected agent window (Tab in Focus/Split).
        assert_eq!(
            stream_window_for(7, &Some((7, "lane-7-2".into()))),
            "lane-7-2"
        );
        // A focused plain terminal is its own stream target (viewport_windows); the lane's
        // pane must keep streaming its agent, or the agent tile freezes beside the terminal.
        assert_eq!(
            stream_window_for(7, &Some((7, "term-7-1".into()))),
            "lane-7"
        );
    }
}
