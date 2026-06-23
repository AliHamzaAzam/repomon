//! `repomon_daemon` — the daemon's library surface.
//!
//! Holds the shared [`Ctx`] (store + registry + lanes + config + event bus), the JSON-RPC
//! [`rpc`] dispatch, the [`socket`] server, and [`pubsub`]. The `repomond` binary is a thin
//! wrapper around [`serve`]; the integration tests drive [`Ctx`] + [`serve`] directly.

pub mod auto_continue;
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
use repomon_core::{config, Config, Lanes, Registry, Store, TmuxRuntime, Watcher};
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
    /// within it; until then the session reads as it last did. Keyed by window name.
    pub prompt_cache: Mutex<HashMap<String, (Instant, Option<String>)>>,
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
            cwds_sticky: Mutex::new(HashMap::new()),
            overlay_cache: Mutex::new(None),
            prompt_cache: Mutex::new(HashMap::new()),
            rate_limits: Mutex::new(HashMap::new()),
            usage: Mutex::new(HashMap::new()),
            auto_continue_off: Mutex::new(HashSet::new()),
            watcher: Mutex::new(None),
            local_watcher_seen: Mutex::new(None),
            input_seen: Mutex::new(HashMap::new()),
            last_managed_windows: Mutex::new(HashSet::new()),
            last_good_windows: Mutex::new(Vec::new()),
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

    /// Per-lane streaming state: the last pushed window+content, the current poll interval, and
    /// when this lane was last captured.
    struct St {
        window: String,
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

    let mut state: HashMap<LaneId, St> = HashMap::new();
    let mut rr: usize = 0; // round-robin offset so background lanes share the per-tick budget fairly
    // The base tick must be at least as fast as the tightest regime (TYPING_FLOOR); per-lane
    // gating below keeps non-typing lanes at their slower cadence, so these extra wakeups are
    // cheap no-ops (no captures) when nothing is being typed.
    let mut tick = tokio::time::interval(TYPING_FLOOR);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let lanes: Vec<LaneId> = ctx.viewport.lock().await.clone();
        if lanes.is_empty() {
            state.clear();
            continue;
        }
        let focus = ctx.viewport_focus.lock().await.clone();
        let focus_lane = focus.as_ref().map(|(l, _)| *l);
        state.retain(|k, _| lanes.contains(k));
        let now = Instant::now();
        // Snapshot (and prune) which lanes were typed into recently — they capture at frame-rate.
        let typing_lanes: HashMap<LaneId, Instant> = {
            let mut m = ctx.input_seen.lock().await;
            m.retain(|_, t| now.saturating_duration_since(*t) < TYPING_WINDOW);
            m.clone()
        };

        // Service the focused lane first (so the per-tick cap never starves what the user is
        // watching), then the rest from a rotating offset so every background pane gets a turn.
        let n = lanes.len();
        let mut order: Vec<LaneId> = Vec::with_capacity(n);
        if let Some(fl) = focus_lane {
            if lanes.contains(&fl) {
                order.push(fl);
            }
        }
        for i in 0..n {
            let lane = lanes[(rr + i) % n];
            if Some(lane) != focus_lane {
                order.push(lane);
            }
        }
        rr = (rr + 1) % n;

        let mut budget = MAX_PER_TICK;
        for lane in order {
            let is_focused = focus_lane == Some(lane);
            // The focused lane streams its selected agent's window; others their first slot.
            let window = match &focus {
                Some((l, w)) if *l == lane => w.clone(),
                _ => TmuxRuntime::window_name(lane),
            };
            // Cadence regime: a lane typed into within TYPING_WINDOW captures at frame-rate;
            // otherwise the focused lane is fast and background/Grid tiles slow.
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
                .get(&lane)
                .map(|s| s.backoff.clamp(floor, cap))
                .unwrap_or(floor);
            // Not due yet (and window unchanged) → leave it for a later tick; costs nothing. A
            // lane absent from the map (freshly spawned / first frame) or whose window switched
            // (Tab) is always due, so fresh output shows immediately.
            if let Some(s) = state.get(&lane) {
                if s.window == window && now < s.last_cap + interval {
                    continue;
                }
            }
            // Cap captures per tick; a due lane skipped here is picked up next tick (rr rotates).
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
                .get(&lane)
                .map(|s| s.window != window || s.content != content)
                .unwrap_or(true);
            // The focused pane also re-pushes on a cursor-only move (arrowing within the input box)
            // so the rendered cursor tracks even when the text itself is unchanged.
            let cursor_changed =
                is_focused && state.get(&lane).map(|s| s.cursor != cursor).unwrap_or(true);
            let changed = content_changed || cursor_changed;
            // Reset to the floor on any change; otherwise double the (clamped) interval toward cap.
            let backoff = if changed { floor } else { (interval * 2).min(cap) };
            if changed {
                ctx.broadcast(
                    pubsub::topic::AGENT_OUTPUT,
                    serde_json::json!({
                        "lane_id": lane,
                        "content": content.clone(),
                        "cursor": cursor.map(|(x, y)| [x, y]),
                    }),
                );
            }
            state.insert(
                lane,
                St { window, content, backoff, last_cap: now, cursor },
            );
        }
    }
}
