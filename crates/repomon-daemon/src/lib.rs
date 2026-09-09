//! Exposes shared daemon state, JSON-RPC dispatch, IPC serving, and event delivery for the binary
//! and integration tests.

pub mod auto_continue;
pub mod bytes_stream;
pub mod conn;
pub mod ext;
pub mod files;
pub mod inject;
pub mod mail;
pub mod notify_watch;
pub mod path_env;
pub mod pubsub;
pub mod push;
pub mod reap;
pub mod remote;
pub mod repomind;
pub mod rpc;
pub mod socket;
mod spawn_input;
pub mod standing;
pub mod supervision;
pub mod usage_ingest;
pub mod usage_query;
pub mod usage_rates;
pub mod usage_watch;
pub mod worktree_watch;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use repomon_core::agent::backend::{CaptureOpts, SessionBackend};
use repomon_core::model::{Lane, LaneId};
use repomon_core::protocol::Notification;
use repomon_core::{Config, Lanes, Registry, Store, TmuxRuntime, Watcher, config};
use serde_json::Value;
use tokio::sync::{Mutex, Notify, RwLock, broadcast};

use conn::{ConnKind, ConnSession};

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

/// Validate the generation after I/O so an invalidated computation cannot publish a stale overlay.
pub struct OverlayCache {
    generation: u64,
    entry: Option<(Instant, Vec<Lane>)>,
}

impl OverlayCache {
    fn new() -> Self {
        Self {
            generation: 0,
            entry: None,
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn entry(&self) -> Option<&(Instant, Vec<Lane>)> {
        self.entry.as_ref()
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.entry = None;
    }

    pub(crate) fn publish(&mut self, generation: u64, lanes: Vec<Lane>) -> bool {
        if self.generation != generation {
            return false;
        }
        self.entry = Some((Instant::now(), lanes));
        true
    }
}

/// Names the fallback window for adopted orchestrators without a recorded controller-lane window.
pub(crate) const ORCHESTRATOR_WINDOW: &str = "orchestrator";

/// Identifies the orchestrator CLI and its backend-specific capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrchestratorBackend {
    /// Claude Code - the default, and the only backend with full monitoring: its `~/.claude`
    /// JSONL transcript is parseable and `--session-id` pins it at spawn.
    Claude,
    /// Runs Codex orchestration with pane-based attention and no orchestrator transcript lookup.
    Codex,
    /// Antigravity CLI - MCP-capable, but no reliably parseable on-disk transcript (protobuf
    /// payload without a stable status contract) - pane-based dialog detection only.
    Antigravity,
    /// OpenCode CLI - MCP-capable, but no reliably parseable on-disk transcript - pane-based
    /// dialog detection only.
    OpenCode,
}

impl OrchestratorBackend {
    /// The wire word for the `backend` field of `orchestrator.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::OpenCode => "opencode",
        }
    }

    /// Reports transcript support; unsupported backends must skip transcript lookup to avoid
    /// attributing another Claude session’s chat to the orchestrator.
    pub fn has_transcript(self) -> bool {
        matches!(self, Self::Claude)
    }
}

/// Tracks the orchestrator’s controller-lane or adopted fallback window, with unknown launch
/// autonomy represented by None.
#[derive(Clone)]
pub struct OrchestratorSession {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub window: String,
    pub autonomy: Option<String>,
    /// Tracks the orchestrator backend, inferred from the request configuration when adopting a
    /// window.
    pub backend: OrchestratorBackend,
    /// Pins transcript lookup to the captured Claude session ID, or permits recency fallback when
    /// adoption did not recover an ID.
    pub session_id: Option<String>,
}

/// One `gate_cache` entry: the ledger's mtime when last read, and the verdict parsed then.
pub type GateCacheEntry = (
    Option<std::time::SystemTime>,
    Option<repomon_core::agent::gate::GateVerdict>,
);

/// Caches pane evidence with the transcript status at capture time so a result from a previous turn
/// state is not reused.
pub type PromptCacheEntry = (
    Instant,
    Option<repomon_core::model::AgentStatus>,
    Option<repomon_core::agent::prompt::PendingDialog>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Everything a request handler needs. Cheap to share via `Arc`.
pub struct Ctx {
    pub store: Store,
    pub registry: Registry,
    pub lanes: Lanes,
    /// User config. Behind a lock because the agent-manager RPCs mutate it (and persist to
    /// disk) at runtime; most fields are static after startup.
    pub config: RwLock<Config>,
    /// Where [`Config::save`] writes - `config::config_path()` in prod, a tempdir in tests.
    pub config_path: PathBuf,
    pub backend: Arc<dyn SessionBackend>,
    /// Serializes slot allocation, MCP identity creation, and agent launch.
    pub spawn_lock: Mutex<()>,
    /// Serializes home initialization across concurrent starts.
    pub repomind_lock: Mutex<()>,
    /// Tracks pending home exports and paths for a batched commit.
    pub repomind_export: Mutex<crate::repomind::export::Pending>,
    /// Wakes [`crate::repomind::export::export_watch`] so a burst of store writes costs one
    /// export instead of one per write.
    pub repomind_export_wake: Notify,
    /// Serializes export runs: the debounced watcher and an explicit `repomind.export` can
    /// otherwise render the same files at once.
    pub repomind_export_lock: Mutex<()>,
    /// Where per-repo notes files live: `data_dir()/repo-notes` in prod, a tempdir in tests
    /// (injected, not env-based: in-process daemon tests can't share process-global env safely).
    pub notes_dir: PathBuf,
    pub started: Instant,
    pub db_path: Option<PathBuf>,
    pub events: pubsub::EventTx,
    /// Tracks each live client’s stream and focus claims for viewport union and resize arbitration.
    pub sessions: Mutex<HashMap<u64, Arc<ConnSession>>>,
    /// Hands out monotonic connection ids for [`Ctx::open_session`].
    pub next_conn: AtomicU64,
    /// Cache of how many live `claude` processes have each working dir (ps/lsof, 10s TTL), so
    /// `/exit`ed sessions whose transcripts linger aren't counted as running.
    pub live_cwds: Mutex<Option<(Instant, HashMap<PathBuf, usize>)>>,
    /// Holds recent higher process counts briefly so one undercount cannot hide and re-add a live
    /// session.
    pub cwds_sticky: Mutex<HashMap<PathBuf, (usize, Instant)>>,
    /// The composite `lane.list` overlay (lanes + live agent sessions), cached for a short TTL so
    /// many clients polling every ~1s don't each re-run the tmux/lsof/transcript scan. Invalidated
    /// on structural changes (spawn/adopt/stop/lane create/delete) so user actions show at once.
    pub overlay_cache: Mutex<OverlayCache>,
    /// Single-flight guard for the overlay recompute. Serializes fresh rebuilds so that when several
    /// callers miss the TTL cache at once (two clients polling `lane.list` plus the notify watcher),
    /// exactly one runs the expensive scan and the rest reuse its result instead of stampeding.
    pub overlay_flight: Mutex<()>,
    /// Expire or invalidate prompt captures on input so dismissed dialogs do not linger.
    pub prompt_cache: Mutex<HashMap<String, PromptCacheEntry>>,
    /// Per window: the last sniffed pane-content hash and when it last CHANGED - the stall
    /// detector's clock. Never TTL-pruned (its point is remembering how long a pane has sat
    /// still); entries drop only when their window vanishes.
    pub pane_seen: Mutex<HashMap<String, (u64, chrono::DateTime<chrono::Utc>)>>,
    /// Per worktree: the dxkit loop ledger's mtime and the verdict parsed from its tail, so
    /// the overlay re-reads only when the gate actually ran again. Keyed by worktree path.
    pub gate_cache: Mutex<HashMap<PathBuf, GateCacheEntry>>,
    /// Live PTY byte watches, keyed by window - the embedded renderer's feed. Each window has one
    /// shared backend stream; the entry refcounts its watching connections (see [`bytes_stream`]).
    /// `Arc<Mutex<…>>` lets the forwarder clean up when the backend detects target closure.
    pub bytes_watches: bytes_stream::Watches,
    /// Agent windows currently paused on a usage limit, with their reset time - written by the
    /// auto-continue watcher and read by `overlay_agents` to surface the `RateLimited` status.
    /// Keyed by slot window (`lane-7-2`), not lane: each slot pauses independently.
    pub rate_limits: Mutex<HashMap<String, auto_continue::RateLimit>>,
    /// Anchor relative countdowns on first observation so stale pane text cannot extend a quota
    /// hold.
    pub quota_deadlines: Mutex<HashMap<String, chrono::DateTime<chrono::Utc>>>,
    /// Per Claude account (config-dir key) usage from the `/usage` probe - written by the usage
    /// watcher, read by `usage.get`. Empty unless `[usage_probe]` is enabled and a local UI is active.
    pub usage: Mutex<HashMap<String, usage_watch::UsageEntry>>,
    /// Wakes a manual probe, bypassing freshness and UI-heartbeat checks while keeping the
    /// opt-in and active-kind gates.
    pub usage_refresh: Notify,
    /// Monotonic ticket for manual requests, including those whose bounded wait expired.
    pub usage_refresh_request: AtomicU64,
    /// Ticket of the queued or running manual pass. Held only for state changes, never probe IO.
    pub usage_refresh_inflight: Mutex<Option<u64>>,
    /// Wakes the usage-ledger ingest loop for an immediate pass.
    pub usage_ingest_wake: Notify,
    /// Offset into the older tail of the ingest walk. The newest half of the scan budget is
    /// visited on every pass; only the remaining half rotates.
    pub usage_scan_rotation: AtomicUsize,
    /// Held for the duration of an ingest pass, so `usage.ingest_now` reports honestly and two
    /// passes never read the same file at once.
    pub usage_ingest_lock: Mutex<()>,
    /// The daily LiteLLM price-refresh state: cached fetch metadata plus the unpriced-model retry
    /// tracker. See [`usage_rates`].
    pub usage_rates: Mutex<usage_rates::RatesRuntime>,
    /// Lanes where the user disabled auto-continue this session (the `C` key).
    pub auto_continue_off: Mutex<HashSet<LaneId>>,
    /// The filesystem watcher (set once the background task brings it up). Held here so `repo.add`
    /// / `repo.remove` can watch / unwatch a tree at runtime - otherwise the watcher only ever
    /// reflects the repos present at startup, and a removed repo keeps churning fsevents.
    pub watcher: Mutex<Option<Watcher>>,
    /// Tracks local watcher freshness so popup ownership can fall back when the UI is absent or
    /// parked.
    pub local_watcher_seen: Mutex<Option<Instant>>,
    /// When a key/text/signal was last sent to each lane's agent. The output streamer reads this
    /// to capture an actively-typed pane at frame-rate (so keystroke echo feels instant), then
    /// relaxes back to the normal cadence once typing stops.
    pub input_seen: Mutex<HashMap<LaneId, Instant>>,
    /// Invalidate process and prompt caches when managed windows disappear.
    pub last_managed_windows: Mutex<HashSet<String>>,
    /// Session IDs of all agent sessions managed by repomon (spawned, adopted, or bound to tmux windows),
    /// so their transcripts are never misclassified as external sessions when their windows close or on probe lag.
    pub known_managed_sessions: Mutex<HashSet<String>>,
    /// Retains the last successful window metadata through transient probe failure.
    pub last_good_windows: Mutex<Vec<repomon_core::agent::WindowMeta>>,
    /// Consecutive empty `list_windows` results. A sudden total-empty is usually a tmux server
    /// bounce, not every agent exiting at once - `resolve_windows` reuses last-good until this
    /// reaches the confirm threshold, so a server restart doesn't mass-fire Idle.
    pub window_empty_misses: Mutex<u8>,
    /// Counts consecutive orphan sightings so a transient bad snapshot cannot trigger window
    /// termination.
    pub orphan_confirm: Mutex<HashMap<String, u32>>,
    /// Counts consecutive empty-backend observations after managed windows were seen, debouncing
    /// total-session-loss alerts.
    pub session_loss_confirm: Mutex<u32>,
    /// Distinguish a fresh boot from the disappearance of previously observed managed windows.
    pub saw_managed_windows: Mutex<bool>,
    /// Last successful per-worktree transcript scan, keyed by worktree path. Reused for one overlay
    /// tick if the scan task panics or its join fails - so a parse panic in one lane can't empty
    /// every lane's sessions. See `rpc::reuse_per_path_on_failure`.
    pub last_good_sessions: Mutex<HashMap<PathBuf, Vec<repomon_core::agent::TranscriptSummary>>>,
    /// What the overlay surfaced per lane on the previous tick, so a session that vanishes this
    /// tick is logged with an attributed reason (idle-drop diagnostic). See
    /// `rpc::diagnose_vanished_sessions`.
    pub last_overlay_sessions: Mutex<HashMap<LaneId, Vec<OverlaySession>>>,
    /// The single daemon-owned repomind orchestrator session, if one is running. `None` until
    /// `orchestrator.start` spawns it; cleared by `orchestrator.stop`.
    pub orchestrator: Mutex<Option<OrchestratorSession>>,
    /// When the orchestrator pane was last typed into (any `orchestrator.send_input`/`key`), so
    /// `stream_orchestrator` captures it at frame-rate while you type to repomind, the same
    /// keystroke-echo speedup `input_seen` gives a focused lane. Goes quiet on its own.
    pub orchestrator_input_seen: Mutex<Option<Instant>>,
    /// Carries current orchestrator attention and an optional headline, refreshed even when
    /// notifications are disabled.
    pub orchestrator_attention: Mutex<(String, Option<String>)>,
    /// Caches remote tokens behind a synchronous lock because the WebSocket handshake callback
    /// cannot await.
    pub remote_tokens: std::sync::RwLock<Vec<(String, Option<String>)>>,
    /// Serializes store mutation and token-cache refresh so a concurrent pair cannot restore a
    /// revoked token from a stale snapshot.
    pub remote_mutate_lock: Mutex<()>,
    /// In-flight local LLM session naming tasks, keyed by transcript session_id, to prevent duplicate background workers.
    pub in_flight_naming: Arc<Mutex<HashSet<String>>>,
    /// Anti-thrashing latch for supervision injection: window -> (expectation fingerprint, when).
    pub inject_latch: Mutex<HashMap<String, (String, Instant)>>,
    /// Cached snapshot of active supervision policies across all enabled lanes.
    pub supervision: RwLock<supervision::PolicySnapshot>,
    /// Wakes durable fleet-mail delivery immediately after storage or when a managed pane
    /// transitions into an injection-safe state. The worker still has a periodic fallback.
    pub mail_delivery: Notify,
    /// Managed windows last observed as injection-eligible. Overlay updates turn each observed
    /// pane's busy-to-idle edge into an event instead of waiting for the fallback sweep.
    pub mail_eligible_windows: Mutex<HashSet<String>>,
    /// Cached file index per lane for `file.index`. Keyed by lane id.
    pub file_indices: Arc<Mutex<HashMap<LaneId, CachedIndex>>>,
    /// Active worktree filesystem watchers, keyed by lane id.
    pub lane_watchers: Mutex<HashMap<LaneId, worktree_watch::WorktreeWatcher>>,
    pub shutdown: Notify,
}

/// One cached file index entry for `file.index`.
#[derive(Clone, Debug)]
pub struct CachedIndex {
    pub generation: u64,
    pub paths: Vec<String>,
    pub truncated: bool,
    pub valid: bool,
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
        Self::new_with_paths(
            store,
            config,
            db_path,
            config_path,
            config::data_dir().join("repo-notes"),
        )
    }

    /// Like [`new_with_config_path`](Self::new_with_config_path) but also with an explicit
    /// repo-notes directory (tests inject a tempdir so notes never touch the real data dir).
    pub fn new_with_paths(
        store: Store,
        config: Config,
        db_path: Option<PathBuf>,
        config_path: PathBuf,
        notes_dir: PathBuf,
    ) -> Arc<Self> {
        #[cfg(unix)]
        let backend: Arc<dyn SessionBackend> =
            Arc::new(TmuxRuntime::new(config.tmux_session.clone()));
        #[cfg(windows)]
        let backend: Arc<dyn SessionBackend> = {
            // Owner identity mirrors `reap::owner_token`: the db path - stable across
            // restarts (so this daemon re-adopts its own hosts) and distinct per instance
            // (so a stray test daemon's hosts are never adopted, reaped, or killed).
            let me = db_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("pid:{}", std::process::id()));
            Arc::new(repomon_core::WindowsBackend::new(
                config.tmux_session.clone(),
                me,
                config::data_dir(),
            ))
        };
        Self::new_with_backend(store, config, db_path, config_path, notes_dir, backend)
    }

    /// Construct [`Ctx`] with an explicit backend implementation (used by injection tests).
    pub fn new_with_backend(
        store: Store,
        config: Config,
        db_path: Option<PathBuf>,
        config_path: PathBuf,
        notes_dir: PathBuf,
        backend: Arc<dyn SessionBackend>,
    ) -> Arc<Self> {
        let registry = Registry::new(store.clone());
        let lanes = Lanes::new(store.clone(), config.clone());
        // Configure the existing backend runtime so adopted sessions survive daemon restarts.
        if backend.session_exists() {
            backend.configure();
        }
        let (events, _rx) = broadcast::channel(2048);
        Arc::new(Ctx {
            store,
            registry,
            lanes,
            config: RwLock::new(config),
            config_path,
            backend,
            spawn_lock: Mutex::new(()),
            repomind_lock: Mutex::new(()),
            repomind_export: Mutex::new(Default::default()),
            repomind_export_wake: Notify::new(),
            repomind_export_lock: Mutex::new(()),
            notes_dir,
            started: Instant::now(),
            db_path,
            events,
            sessions: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(0),
            live_cwds: Mutex::new(None),
            cwds_sticky: Mutex::new(HashMap::new()),
            overlay_cache: Mutex::new(OverlayCache::new()),
            overlay_flight: Mutex::new(()),
            prompt_cache: Mutex::new(HashMap::new()),
            pane_seen: Mutex::new(HashMap::new()),
            gate_cache: Mutex::new(HashMap::new()),
            bytes_watches: Arc::new(Mutex::new(HashMap::new())),
            rate_limits: Mutex::new(HashMap::new()),
            quota_deadlines: Mutex::new(HashMap::new()),
            usage: Mutex::new(HashMap::new()),
            usage_refresh: Notify::new(),
            usage_refresh_request: AtomicU64::new(0),
            usage_refresh_inflight: Mutex::new(None),
            usage_ingest_wake: Notify::new(),
            usage_scan_rotation: AtomicUsize::new(0),
            usage_ingest_lock: Mutex::new(()),
            usage_rates: Mutex::new(usage_rates::RatesRuntime::new()),
            auto_continue_off: Mutex::new(HashSet::new()),
            watcher: Mutex::new(None),
            local_watcher_seen: Mutex::new(None),
            input_seen: Mutex::new(HashMap::new()),
            last_managed_windows: Mutex::new(HashSet::new()),
            known_managed_sessions: Mutex::new(HashSet::new()),
            last_good_windows: Mutex::new(Vec::new()),
            window_empty_misses: Mutex::new(0),
            orphan_confirm: Mutex::new(HashMap::new()),
            session_loss_confirm: Mutex::new(0),
            saw_managed_windows: Mutex::new(false),
            last_good_sessions: Mutex::new(HashMap::new()),
            last_overlay_sessions: Mutex::new(HashMap::new()),
            orchestrator: Mutex::new(None),
            orchestrator_input_seen: Mutex::new(None),
            orchestrator_attention: Mutex::new(("none".to_string(), None)),
            remote_tokens: std::sync::RwLock::new(Vec::new()),
            remote_mutate_lock: Mutex::new(()),
            in_flight_naming: Arc::new(Mutex::new(HashSet::new())),
            inject_latch: Mutex::new(HashMap::new()),
            supervision: RwLock::new(supervision::PolicySnapshot::default()),
            mail_delivery: Notify::new(),
            mail_eligible_windows: Mutex::new(HashSet::new()),
            file_indices: Arc::new(Mutex::new(HashMap::new())),
            lane_watchers: Mutex::new(HashMap::new()),
            shutdown: Notify::new(),
        })
    }

    /// The tmux window last recorded for the controller lane, from the store. Deliberately does
    /// not consult the tracked orchestrator session, so it is safe to call while holding
    /// `self.orchestrator`.
    pub(crate) async fn controller_lane_window(&self) -> Option<String> {
        let lane = self.store.controller_lane().await.ok().flatten()?;
        let metas = self.store.list_lane_meta().await.ok()?;
        metas
            .into_iter()
            .find(|m| m.id == lane)
            .and_then(|m| m.tmux_window)
    }

    /// Reconcile worktree watchers so exactly the lanes currently present in some connection's
    /// viewport have a live watcher running.
    pub async fn reconcile_lane_watchers(&self) {
        let active_lanes: HashSet<LaneId> = {
            let sessions = self.sessions.lock().await;
            let mut set = HashSet::new();
            for sess in sessions.values() {
                for lane in sess.viewport.lock().await.iter() {
                    set.insert(*lane);
                }
            }
            set
        };

        let mut watchers = self.lane_watchers.lock().await;
        watchers.retain(|lane_id, _| active_lanes.contains(lane_id));

        for lane_id in active_lanes {
            if let std::collections::hash_map::Entry::Vacant(e) = watchers.entry(lane_id) {
                if let Ok(lane) = self.lanes.get(lane_id).await {
                    let root = lane.worktree.path.clone();
                    if let Ok(w) = worktree_watch::start_lane_watcher(
                        self.events.clone(),
                        self.file_indices.clone(),
                        lane_id,
                        root,
                    ) {
                        e.insert(w);
                    }
                }
            }
        }
    }

    /// Invalidate a lane's cached file index, incrementing the generation counter.
    pub async fn invalidate_file_index(&self, lane_id: LaneId) {
        let mut indices = self.file_indices.lock().await;
        let entry = indices.entry(lane_id).or_insert_with(|| CachedIndex {
            generation: 0,
            paths: Vec::new(),
            truncated: false,
            valid: false,
        });
        entry.generation = entry.generation.wrapping_add(1);
        entry.valid = false;
        entry.paths.clear();
        entry.truncated = false;
    }

    /// Drop the cached `lane.list` overlay so the next read recomputes - call after a structural
    /// change (spawn / adopt / stop / lane create / delete) so the action shows up immediately
    /// instead of waiting out the cache TTL.
    pub async fn invalidate_overlay(&self) {
        self.overlay_cache.lock().await.invalidate();
    }

    /// Register a new client connection's session and return it. Each transport calls this once on
    /// connect (Local for the Unix socket, Remote for the bridge) and drops the session via
    /// [`close_session`](Self::close_session) - or a `conn::SessionGuard` - on every exit path.
    pub async fn open_session(self: &Arc<Self>, kind: ConnKind) -> Arc<ConnSession> {
        let id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let sess = Arc::new(ConnSession::new(id, kind));
        self.sessions.lock().await.insert(id, sess.clone());
        sess
    }

    /// Remove a connection's session when it disconnects, so its viewport no longer contributes to
    /// the streamed union and its focus no longer arbitrates fits.
    pub async fn close_session(&self, id: u64) {
        if self.sessions.lock().await.remove(&id).is_none() {
            return;
        }
        // Remove watches by session ID regardless of source so stale connection sets cannot retain
        // them.
        crate::bytes_stream::unwatch_all(&self.backend, &self.bytes_watches, id).await;
        self.reconcile_lane_watchers().await;
    }

    /// Combines live clients’ stream targets and identifies windows focused by clients with fresh
    /// heartbeats.
    pub async fn viewport_snapshot(&self) -> ViewportSnapshot {
        let now = Instant::now();
        let sessions: Vec<Arc<ConnSession>> =
            self.sessions.lock().await.values().cloned().collect();
        let mut targets: Vec<(LaneId, String)> = Vec::new();
        let mut focused: HashSet<String> = HashSet::new();
        for sess in &sessions {
            let lanes = sess.viewport.lock().await.clone();
            let focus = sess.viewport_focus.lock().await.clone();
            // One target per visible lane (its resolved window), deduped across sessions by window.
            for lane in &lanes {
                let w = stream_window_for(*lane, &focus);
                if !targets.iter().any(|(_, tw)| tw == &w) {
                    targets.push((*lane, w));
                }
            }
            // Plain terminals the Grid tiles, each with the lane it belongs to. `viewport.set`
            // already filtered these to valid `term-…` windows, so a session can't inject others.
            for w in sess.viewport_windows.lock().await.iter() {
                if let Some(lane) = TmuxRuntime::parse_term_window(w) {
                    if !targets.iter().any(|(_, tw)| tw == w) {
                        targets.push((lane, w.clone()));
                    }
                }
            }
            // A window is focused (fast cadence + cursor) if any FRESH-beat session focuses it.
            let at = *sess.viewport_focus_at.lock().await;
            let fresh = at.is_some_and(|t| now.duration_since(t) < rpc::VIEWPORT_OWNED_TTL);
            if fresh {
                if let Some((_, w)) = &focus {
                    focused.insert(w.clone());
                }
            }
        }
        ViewportSnapshot { targets, focused }
    }

    /// True while any live connection wants the repomind pane streamed. A watcher is stored on
    /// its connection session so another client leaving the view cannot disable the stream.
    pub async fn has_orchestrator_watcher(&self) -> bool {
        let sessions: Vec<Arc<ConnSession>> =
            self.sessions.lock().await.values().cloned().collect();
        for sess in sessions {
            if *sess.orchestrator_watched.lock().await {
                return true;
            }
        }
        false
    }

    /// Publish an `event.<topic>` notification to all subscribers.
    pub fn broadcast(&self, method: &str, params: Value) {
        let note = Notification::new(method, params);
        if let Ok(value) = serde_json::to_value(&note) {
            // Err just means no subscribers; that's fine.
            let _ = self.events.send(value);
        }
    }

    /// Request an immediate durable-mail delivery pass. `notify_one` stores one permit when the
    /// worker is between waits, so an on-send wake cannot be lost; additional sends coalesce.
    pub fn wake_mail_delivery(&self) {
        self.mail_delivery.notify_one();
    }

    /// Signal the accept loop to stop.
    pub fn request_shutdown(&self) {
        self.shutdown.notify_waiters();
    }
}

/// The capture poll loop's view of every live session, from [`Ctx::viewport_snapshot`].
#[derive(Debug, Default, Clone)]
pub struct ViewportSnapshot {
    /// Every window to stream this tick - the union across sessions, deduped by window, each
    /// tagged with the lane it belongs to for the output event payload.
    pub targets: Vec<(LaneId, String)>,
    /// Windows any fresh-beat session focuses: fast cadence floor/cap + cursor capture.
    pub focused: HashSet<String>,
}

/// Viewport-aware output streaming: fast-poll the tmux panes any client currently has visible
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
    // Back off capture of unchanged panes while keeping focused and actively typed panes
    // responsive.
    const FOCUS_FLOOR: Duration = Duration::from_millis(150);
    const FOCUS_CAP: Duration = Duration::from_millis(600);
    const BG_FLOOR: Duration = Duration::from_millis(700);
    const BG_CAP: Duration = Duration::from_millis(3000);
    // While a pane is being actively typed into, capture it at ~frame-rate so keystroke echo
    // feels instant. This regime applies for TYPING_WINDOW after the last key, then relaxes back
    // to the focused/background cadence above - a brief single-pane burst, only while typing.
    const TYPING_FLOOR: Duration = Duration::from_millis(30);
    const TYPING_CAP: Duration = Duration::from_millis(60);
    const TYPING_WINDOW: Duration = Duration::from_millis(400);
    // Bound each tick’s capture work, prioritizing focus while rotating background windows fairly.
    const MAX_PER_TICK: usize = 3;

    let mut state: HashMap<String, St> = HashMap::new();
    let mut rr: usize = 0; // Keep the base tick responsive to typing; per-window limits suppress redundant captures.
    let mut tick = tokio::time::interval(TYPING_FLOOR);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let now = Instant::now();
        // Prune lanes typed into longer ago than TYPING_WINDOW. This runs BEFORE the empty-viewport
        // early-return below so `input_seen` is bounded even when no TUI viewport is set - otherwise
        // a lane typed into while nothing is visible would leak its entry forever.
        {
            let mut m = ctx.input_seen.lock().await;
            m.retain(|_, t| now.saturating_duration_since(*t) < TYPING_WINDOW);
        }
        // The union of every live session's stream targets, plus the windows any fresh-beat
        // session focuses. With one connection this is exactly that connection's viewport and
        // focus window - see `Ctx::viewport_snapshot` for the single-connection equivalence proof.
        let ViewportSnapshot { targets, focused } = ctx.viewport_snapshot().await;
        if targets.is_empty() {
            state.clear();
            continue;
        }
        state.retain(|w, _| targets.iter().any(|(_, tw)| tw == w));
        // Snapshot which lanes were typed into recently - they capture at frame-rate. The map was
        // just pruned above, so this is the live set of within-TYPING_WINDOW lanes.
        let typing_lanes: HashMap<LaneId, Instant> = ctx.input_seen.lock().await.clone();

        // Service focused panes first (so the per-tick cap never starves what a user is watching),
        // then the rest from a rotating offset so every background pane gets a turn.
        let n = targets.len();
        let mut order: Vec<(LaneId, String)> = Vec::with_capacity(n);
        for t in &targets {
            if focused.contains(&t.1) {
                order.push(t.clone());
            }
        }
        for i in 0..n {
            let t = &targets[(rr + i) % n];
            if !focused.contains(&t.1) {
                order.push(t.clone());
            }
        }
        rr = (rr + 1) % n;

        let mut budget = MAX_PER_TICK;
        for (lane, window) in order {
            let is_focused = focused.contains(&window);
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
            // The poll interval, re-clamped to the current regime each tick - so the moment a lane
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
            let tmux = ctx.backend.clone();
            let w = window.clone();
            let content = match tokio::task::spawn_blocking(move || {
                tmux.capture_named(&w, CaptureOpts::visible())
            })
            .await
            {
                Ok(Ok(c)) => c,
                _ => continue,
            };
            // Only the focused pane carries a cursor (the TUI renders it where you're typing) - one
            // extra tmux fork on a single pane, never on background/Grid tiles.
            let cursor = if is_focused {
                let tmux = ctx.backend.clone();
                let cw = window.clone();
                tokio::task::spawn_blocking(move || tmux.cursor_named(&cw))
                    .await
                    .ok()
                    .flatten()
                    .map(|c| (c.col, c.row))
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

/// Resolve the lane’s agent separately from a plain terminal so terminal focus cannot steal its
/// stream.
fn stream_window_for(lane: LaneId, focus: &Option<(LaneId, String)>) -> String {
    match focus {
        Some((l, w)) if *l == lane && TmuxRuntime::parse_term_window(w).is_none() => w.clone(),
        _ => TmuxRuntime::window_name(lane),
    }
}

/// Streams changed orchestrator output to interested clients, accelerating during typing and
/// backing off while the pane remains unchanged.
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
        let watched = ctx.has_orchestrator_watcher().await;
        let window = ctx
            .orchestrator
            .lock()
            .await
            .as_ref()
            .map(|session| session.window.clone());
        let Some(window) = window.filter(|_| watched) else {
            last = None;
            last_cursor = None;
            backoff = WATCH_FLOOR;
            continue;
        };
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
        let tmux = ctx.backend.clone();
        let capture_window = window.clone();
        let content = match tokio::task::spawn_blocking(move || {
            tmux.capture_named(&capture_window, CaptureOpts::visible())
        })
        .await
        {
            Ok(Ok(c)) => c,
            _ => continue,
        };
        // Carry repomind's real cursor so the mediated pane draws it where you're typing (mirrors
        // the focused-lane path in `stream_output`). One extra tmux fork on the single pane.
        let tmux = ctx.backend.clone();
        let cursor_window = window.clone();
        let cursor = tokio::task::spawn_blocking(move || tmux.cursor_named(&cursor_window))
            .await
            .ok()
            .flatten()
            .map(|c| (c.col, c.row));
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
    use std::time::Duration;

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

    #[test]
    fn invalidation_rejects_an_in_flight_overlay_publish() {
        let mut cache = OverlayCache::new();
        let stale_generation = cache.generation();
        assert!(cache.publish(stale_generation, Vec::new()));

        // `agent.spawn` invalidates while a notify-watcher scan that captured the old
        // generation is still running. That scan must not republish its pre-spawn snapshot.
        cache.invalidate();
        assert!(!cache.publish(stale_generation, Vec::new()));
        assert!(
            cache.entry().is_none(),
            "a scan started before invalidation must leave the cache empty"
        );

        let current_generation = cache.generation();
        assert!(cache.publish(current_generation, Vec::new()));
        assert!(cache.entry().is_some());
    }

    async fn test_ctx() -> Arc<Ctx> {
        Ctx::new(Store::open_in_memory().unwrap(), Config::default(), None)
    }

    #[tokio::test]
    async fn viewport_snapshot_single_session_equivalence() {
        // One session's snapshot is exactly what the loop built before: a stream target per lane,
        // then its terminal windows, and its fresh focus window is the sole focused window.
        let ctx = test_ctx().await;
        let s = ctx.open_session(ConnKind::Local).await;
        *s.viewport.lock().await = vec![7, 9];
        *s.viewport_focus.lock().await = Some((7, "lane-7-2".to_string()));
        *s.viewport_focus_at.lock().await = Some(Instant::now());
        *s.viewport_windows.lock().await = vec!["term-9-1".to_string()];

        let snap = ctx.viewport_snapshot().await;
        assert_eq!(
            snap.targets,
            vec![
                (7, "lane-7-2".to_string()),
                (9, "lane-9".to_string()),
                (9, "term-9-1".to_string()),
            ]
        );
        assert_eq!(
            snap.focused,
            HashSet::from(["lane-7-2".to_string()]),
            "the sole fresh focus is the only focused window"
        );
    }

    #[tokio::test]
    async fn viewport_snapshot_unions_overlapping_viewports() {
        // Two devices with an overlapping lane dedup by window, but each contributes its own extras.
        let ctx = test_ctx().await;
        let a = ctx.open_session(ConnKind::Local).await;
        *a.viewport.lock().await = vec![7, 9];
        *a.viewport_focus.lock().await = Some((7, "lane-7".to_string()));
        *a.viewport_focus_at.lock().await = Some(Instant::now());

        let b = ctx.open_session(ConnKind::Remote { device: None }).await;
        *b.viewport.lock().await = vec![9, 12];
        *b.viewport_focus.lock().await = Some((12, "lane-12".to_string()));
        *b.viewport_focus_at.lock().await = Some(Instant::now());

        let snap = ctx.viewport_snapshot().await;
        let windows: HashSet<String> = snap.targets.iter().map(|(_, w)| w.clone()).collect();
        assert_eq!(
            windows,
            HashSet::from([
                "lane-7".to_string(),
                "lane-9".to_string(),
                "lane-12".to_string(),
            ]),
            "the union covers every lane exactly once (lane 9 deduped)"
        );

        assert_eq!(
            snap.targets.iter().filter(|(_, w)| w == "lane-9").count(),
            1
        );

        assert_eq!(
            snap.focused,
            HashSet::from(["lane-7".to_string(), "lane-12".to_string()])
        );
    }

    #[tokio::test]
    async fn viewport_snapshot_focuses_only_fresh_beats() {
        // A session that focuses a window but whose beat has gone stale (or was never stamped) does
        // not contribute to the focused set - though its viewport still streams (it is a target).
        let ctx = test_ctx().await;
        let stale = ctx.open_session(ConnKind::Remote { device: None }).await;
        *stale.viewport.lock().await = vec![7];
        *stale.viewport_focus.lock().await = Some((7, "lane-7".to_string()));
        *stale.viewport_focus_at.lock().await =
            Some(Instant::now() - rpc::VIEWPORT_OWNED_TTL - Duration::from_secs(1));

        let snap = ctx.viewport_snapshot().await;
        assert_eq!(snap.targets, vec![(7, "lane-7".to_string())]);
        assert!(
            snap.focused.is_empty(),
            "a stale beat streams its viewport but claims no fast-cadence focus"
        );
    }

    #[tokio::test]
    async fn orchestrator_watch_is_union_of_live_connections() {
        let ctx = test_ctx().await;
        let a = ctx.open_session(ConnKind::Local).await;
        let b = ctx.open_session(ConnKind::Remote { device: None }).await;
        assert!(!ctx.has_orchestrator_watcher().await);

        *a.orchestrator_watched.lock().await = true;
        *b.orchestrator_watched.lock().await = true;
        assert!(ctx.has_orchestrator_watcher().await);

        *a.orchestrator_watched.lock().await = false;
        assert!(ctx.has_orchestrator_watcher().await);

        ctx.close_session(b.id).await;
        assert!(!ctx.has_orchestrator_watcher().await);
    }
}
