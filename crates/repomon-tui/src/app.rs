//! Application state and the interactive event loop.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use repomon_core::TmuxRuntime;
use repomon_core::model::{
    AgentChoice, AgentSession, AgentStatus, BrowseEntry, BrowseResult, Commit, Lane, LaneId, Repo,
    RepoId, TimelineData, WorkSession,
};
use repomon_core::notify::{
    SessKey, activity_allows_refire, diff_session_transitions, session_by_key, session_statuses,
    slot_by_key,
};
use repomon_core::protocol::Notification;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

use crate::client::DaemonClient;
use crate::keybinds::{self, Action, View};
use crate::notify::{self, NotifEvent, NotifKind};
use crate::theme::Theme;
use crate::view;

/// How long an in-app notification banner stays up before reverting to the footer hints.
pub const NOTIF_BANNER_TTL: Duration = Duration::from_secs(6);
/// Don't re-fire the same session's notification within this window (suppresses status flapping).
const NOTIF_DEBOUNCE: Duration = Duration::from_secs(30);
/// How long to keep an alert's activity latch after its session leaves the snapshot, so a
/// vanish+reappear can't slip a repeat through (mirrors the daemon's `LATCH_GRACE`).
const NOTIF_LATCH_GRACE: Duration = Duration::from_secs(6 * 60 * 60);
/// Cap on the in-app notification history feed.
const NOTIF_HISTORY_CAP: usize = 200;

/// Agent kinds offered when creating a lane (cycled with Tab).
pub const AGENT_KINDS: &[&str] = &["claude-code", "codex", "aider"];

/// Timeline zoom levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Day,
    Week,
    Month,
}

/// Which field the agent add/edit form is typing into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgField {
    Name,
    Command,
}

impl Zoom {
    /// (lookback seconds, minimum bucket seconds, label). The actual bucket size is derived
    /// from the terminal width at load time (see `load_timeline`), floored at the minimum so a
    /// huge terminal doesn't ask for sub-minute buckets nobody can read.
    fn params(self) -> (i64, i64, &'static str) {
        match self {
            Zoom::Day => (24 * 3600, 5 * 60, "day"),
            Zoom::Week => (7 * 24 * 3600, 30 * 60, "week"),
            Zoom::Month => (30 * 24 * 3600, 2 * 3600, "month"),
        }
    }
}

/// All UI state. `view` reads these fields directly.
/// A lane's captured pane: the raw text (for selection/copy and scrollback) plus its styled
/// lines, parsed once per `event.agent.output` delta so the render path only has to slice.
pub struct Pane {
    pub raw: String,
    pub lines: Vec<Line<'static>>,
    /// The agent pane's cursor `(col, row)` (0-based, from the top-left of the captured pane),
    /// when the daemon reported one for this (focused) lane. The render places the terminal cursor
    /// here while you're typing into the agent. `None` for background panes / hidden cursors.
    pub cursor: Option<(u16, u16)>,
}

/// A clickable lane region recorded during render (in `view.rs`) and hit-tested by the mouse
/// handler. `interactive` = a single-click focuses the lane for typing in place (Grid tiles,
/// Split panes/rows); otherwise a single-click only selects it (Fleet rows, which show no pane).
#[derive(Clone, Copy)]
pub struct ClickZone {
    pub rect: Rect,
    pub lane: LaneId,
    /// Which agent within the lane this row targets, when the sidebar is expanded into per-agent
    /// rows; `None` for a normal lane row / lane header.
    pub session: Option<usize>,
    pub interactive: bool,
}

/// A row in the fleet sidebar: a lane (header), or — when `expand_agents` is on and the lane runs
/// several agents — one of that lane's agent sub-rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FleetRow {
    /// Index into [`App::visible_lanes`].
    pub lane_idx: usize,
    /// `Some(i)` = the lane's `i`-th agent sub-row; `None` = the lane header / single-agent row.
    pub session: Option<usize>,
    /// The pinned "repomind" orchestrator row at the top of the fleet (always row 0). When set,
    /// `lane_idx`/`session` are meaningless; the row targets [`View::Orchestrator`], not a lane.
    pub orchestrator: bool,
}

/// Two left-clicks on the same lane within this window count as a double-click (→ open the real
/// terminal). A single click focuses the lane for typing in place.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Whether a click on `lane` at `now` completes a double-click, given the previous click (time +
/// lane). True only when the previous click was on the *same* lane within [`DOUBLE_CLICK`].
fn is_double_click(
    prev: Option<(std::time::Instant, LaneId)>,
    lane: LaneId,
    now: std::time::Instant,
) -> bool {
    matches!(prev, Some((t, l)) if l == lane && now.duration_since(t) < DOUBLE_CLICK)
}

/// Editable settings shown in the Settings view (a subset of the daemon config).
#[derive(Default)]
pub struct Settings {
    pub accent: String,
    pub default_agent: String,
    pub auto_continue: bool,
    pub auto_continue_message: String,
    pub worktree_template: String,
    pub spawn_prompt: bool,
    pub notify_enabled: bool,
    pub notify_needs_you: bool,
    pub notify_rate_limited: bool,
    pub notify_resumed: bool,
    pub notify_idle: bool,
    pub notify_sound: bool,
    pub notify_show_why: bool,
    pub notify_coalesce: bool,
    pub notify_click_focus: bool,
    pub notify_subagents: bool,
    pub usage_probe: bool,
    pub expand_agents: bool,
    /// The Claude account that powers the repomind orchestrator (empty = bare `claude`).
    pub orchestrator_agent: String,
    /// The orchestrator's model override (empty = the account default; e.g. `opus`/`sonnet`).
    pub orchestrator_model: String,
}

/// Accent choices the Settings view cycles through (`mono` = no color).
const ACCENTS: &[&str] = &[
    "cyan", "green", "magenta", "amber", "blue", "red", "white", "mono",
];

/// Number of editable rows in the Settings view.
const SETTINGS_COUNT: usize = 20;

/// Model choices the orchestrator setting cycles through (empty = the account's default model).
const ORCH_MODELS: &[&str] = &["", "opus", "sonnet"];

pub struct App {
    pub client: DaemonClient,
    pub theme: Theme,
    pub lanes: Vec<Lane>,
    pub commits: Vec<Commit>,
    pub repos: Vec<Repo>,
    pub view: View,
    pub selected: usize,
    pub filter: String,
    pub filtering: bool,
    /// Inline rename of the selected agent sub-row (expanded sidebar): active flag, edit buffer,
    /// and the pinned target — the agent's durable transcript `session_id`, captured when the
    /// rename starts so a cursor move / refresh during the edit can't retarget it.
    pub renaming: bool,
    pub rename_buf: String,
    rename_target: Option<String>,
    pub status: String,
    pub nl_repo_idx: usize,
    pub nl_branch: String,
    pub nl_agent_idx: usize,
    pub nl_agents: Vec<AgentChoice>,
    pub should_quit: bool,
    pub cd_target: Option<PathBuf>,
    /// Latest captured pane per lane (raw text + pre-parsed styled lines), pushed by
    /// `event.agent.output`. Parsing once on update keeps it off the per-frame render path.
    pub output: HashMap<LaneId, Pane>,
    /// Whether Focus is in insert mode (keystrokes forwarded live to the agent).
    pub focus_insert: bool,
    /// True while Focus is driving a repomon-managed agent — so when it exits (`/exit` or
    /// stop) we can drop back out of Focus instead of staring at a dead pane.
    focus_managed: bool,
    /// Consecutive lane refreshes the focused agent has been absent. Detach (Focus → Split) only
    /// once it reaches [`FOCUS_DETACH_GRACE`], so a transient overlay flap that drops the session
    /// for a refresh or two doesn't kick the user out of a still-running agent.
    focus_missing_ticks: u8,
    /// Whether repomon captures the mouse (for scroll-wheel nav). When off, the terminal owns
    /// the mouse so you can drag-select and copy the rendered output natively.
    pub mouse_on: bool,
    /// Scrollback: lines scrolled up from the live tail in Focus (0 = following). `scroll_buf`
    /// holds a deep capture taken when you start scrolling.
    pub scroll: usize,
    pub scroll_buf: Option<String>,
    /// Net pane-scroll ticks accumulated from a wheel/PgUp burst (+ up, − down). The event loop
    /// flushes the net into a single `agent.scroll` per frame, so a fast trackpad flick is one
    /// forwarded scroll instead of dozens of blocking RPCs (which overshot and froze the UI).
    pending_scroll: isize,
    /// Printable characters typed/pasted to the agent, buffered so a paste of N chars becomes one
    /// `send_input` instead of N blocking `send-keys` RPCs (which froze the UI). Flushed by the
    /// event loop after each input burst, and before any control key so ordering is preserved.
    pending_input: String,
    /// The scrollback snapshot's pre-parsed styled lines (parsed once when `scroll_buf` is set).
    pub scroll_lines: Option<Vec<Line<'static>>>,
    /// Focus drag-selection (buffer line indices). On release the range is copied to the
    /// clipboard. `focus_geom` = (first output screen row, window-start line, visible count),
    /// set during render so the mouse handler can map a screen row to a buffer line.
    pub sel_anchor: Option<usize>,
    pub sel_head: Option<usize>,
    pub focus_geom: std::cell::Cell<(u16, usize, usize)>,
    /// The focused agent pane's content size `(cols, rows)`, recorded during render of the Split /
    /// Focus view. The event loop resizes the agent's tmux window to match so it reflows to the
    /// visible width (no right-edge clipping) instead of staying at a stale `attach` width.
    pub focus_pane_dims: std::cell::Cell<Option<(u16, u16)>>,
    /// The `(lane, window, cols, rows)` of the last `agent.resize` sent, so it fires only on a real
    /// size/focus change rather than every tick.
    last_resize: Option<(LaneId, String, u16, u16)>,
    /// The `(cols, rows)` of the last `orchestrator.resize` sent, so it fires only on a real size
    /// change. Cleared after an attach (which restores the window's client-follow size).
    last_orch_resize: Option<(u16, u16)>,
    /// Clickable lane regions for the current frame, recorded during render and read by the mouse
    /// handler. Cleared and repopulated every render.
    pub click_zones: RefCell<Vec<ClickZone>>,
    /// Last left-click (time + lane) for double-click detection.
    last_click: Option<(std::time::Instant, LaneId)>,
    /// The lane the mouse is currently hovering (highlighted on render). `None` = not over a lane.
    pub hover_lane: Option<LaneId>,
    /// The last column a *positioned* mouse event reported (move/click/drag). Wheel events report
    /// column 0 on many terminals, so this is used to tell whether the wheel is over the sidebar
    /// (navigate lanes) or the agent pane (scroll it).
    last_mouse_col: u16,
    /// Set by `on_notification` for non-output events; the event loop coalesces a notification
    /// burst into a single `refresh()`.
    refresh_pending: bool,
    /// Lanes where the user disabled auto-continue this session (echoes the daemon's set so the
    /// `C` key can toggle without a round-trip).
    pub ac_off: HashSet<LaneId>,
    /// Active tile in the babysit grid.
    pub grid_active: usize,
    /// Settings view state.
    pub settings: Settings,
    pub settings_idx: usize,
    pub settings_editing: bool,
    /// Screen row of the first settings item (for click hit-testing), set during render.
    pub settings_geom: std::cell::Cell<u16>,
    /// Last-seen status per real agent session, for notification edge-detection. A session that
    /// left the snapshot is expressed by key absence (there is no `None` value), which is what
    /// lets each agent in a shared lane fire its own alerts instead of one rolled-up status.
    prev_status: HashMap<(LaneId, SessKey), AgentStatus>,
    /// True once the first lane list has seeded `prev_status` (so startup doesn't notify for
    /// every already-running agent at once).
    notif_seeded: bool,
    /// Set when the TUI returns from a full-screen attach: forces the next `detect_notifications`
    /// to re-seed instead of diffing. The daemon owned desktop popups while the TUI was parked
    /// (its heartbeat went stale), so replaying the gap's transitions would double-fire them.
    notif_reseed: bool,
    /// Whether `prev_status` was built including subagents (the `notify_subagents` toggle). When it
    /// flips, the tracked key set changes wholesale, so `detect_notifications` re-seeds.
    notif_subagents: bool,
    /// Debounce keyed by (lane, session, kind): the last time each *kind* of alert fired for a
    /// session. Keying on the kind suppresses a flapping identical alert without swallowing a
    /// genuinely different transition (e.g. a usage-limit alert right after a needs-you one);
    /// keying on the session lets two agents in one lane each raise the same kind of alert.
    notif_debounce: HashMap<(LaneId, SessKey, NotifKind), Instant>,
    /// Activity-anchored re-fire latch: the session's `last_activity_at` (transcript mtime) when
    /// each (lane, session, kind) last fired. A repeat fires only once that advances — real work
    /// since the last alert — so the status flapping a 30s debounce can't catch (idle-decay, lsof
    /// undercount, sniff wobble) doesn't re-alert. Covers NeedsYou/RateLimited/Resumed; Idle keeps
    /// to `notif_debounce`. The `Instant` is for pruning only.
    notif_latch: HashMap<(LaneId, SessKey, NotifKind), (DateTime<Utc>, Instant)>,
    /// In-app notification history (newest last), shown in the Notifications view.
    pub notifications: VecDeque<NotifEvent>,
    /// Cursor in the Notifications view: offset into the feed, newest-first (0 = newest).
    /// The render derives its scroll window from this so the cursor stays visible.
    pub notif_sel: usize,
    /// A transient banner shown above the footer after a notification fires; auto-clears.
    pub notif_banner: Option<(String, Instant)>,
    /// Spawn-picker state: which agent row is highlighted, the lane to spawn into, and the view
    /// to return to on cancel.
    pub spawn_pick_idx: usize,
    pub spawn_pick_lane: Option<LaneId>,
    spawn_return: Option<View>,
    /// Fleet shows only lanes needing attention (waiting / stuck on a limit) when set.
    pub urgent_only: bool,
    /// Lane-switcher state: the typed query, the highlighted match, and the view to return to
    /// on cancel.
    pub jump_query: String,
    pub jump_idx: usize,
    jump_return: Option<View>,
    pub timeline: Option<TimelineData>,
    pub timeline_zoom: Zoom,
    pub sessions: Vec<WorkSession>,
    pub search_query: String,
    pub search_results: Vec<Commit>,
    /// Latest commits for the selected lane's worktree (its branch history) — shown in the
    /// Split detail, with a fallback when there's nothing today.
    pub recent_commits: Vec<Commit>,
    recent_commits_lane: Option<LaneId>,
    /// Which of the selected lane's agent sessions is highlighted (for adopt). Several
    /// concurrent agents can run in one worktree; this cursor picks among them.
    pub session_idx: usize,
    session_lane: Option<LaneId>,
    /// The agent each lane had selected when you last left it, so returning to a multi-agent
    /// lane restores your pick instead of snapping back to the first slot. Keyed by a stable
    /// identity (tmux window / transcript id), so it survives the session list reordering.
    session_memory: HashMap<LaneId, SessionRef>,
    /// After spawning an agent, the (lane, tmux window) to move the session cursor onto once it
    /// shows up in `lane.list` — so a fresh spawn lands you on the *new* agent, not the old one.
    /// Cleared when matched (or after a few refreshes if the window never appears).
    pending_focus_window: Option<(LaneId, String)>,
    pending_focus_ticks: u8,
    /// Plain shell terminals open for the selected lane (tmux window names).
    pub terminals: Vec<String>,
    terminals_lane: Option<LaneId>,
    pub browse_path: String,
    pub browse_parent: Option<String>,
    pub browse_entries: Vec<BrowseEntry>,
    pub browse_selected: usize,
    /// Two-press confirm for unregistering a repo from the browser (`x x`): the repo id armed
    /// by the first press.
    repo_remove_pending: Option<i64>,
    /// Pending bulk repo-discover (root, found paths): armed by a first `d`, committed by a second
    /// — so a recursive scan of a deep folder can't flood the fleet on a single keypress.
    discover_pending: Option<(String, Vec<String>)>,
    /// Two-press confirm for unregistering a whole repo from the Fleet (`X`): the repo id armed
    /// by the first press. Cleared by any other action, so navigating away cancels it.
    repo_remove_armed: Option<RepoId>,
    /// Agent-manager state: the list, the cursor, and the add/edit form.
    pub agents: Vec<AgentChoice>,
    pub agents_selected: usize,
    pub ag_editing: bool,
    pub ag_is_new: bool,
    pub ag_field: AgField,
    pub ag_name: String,
    pub ag_command: String,
    /// Original name when editing an existing agent (for rename detection).
    pub ag_orig: Option<String>,
    /// Where `esc` returns from the Agents view (e.g. back to New Lane).
    agents_return: Option<View>,
    last_viewport: Vec<LaneId>,
    last_viewport_focus: Option<(LaneId, String)>,
    /// Last terminal title emitted (OSC 2), to skip redundant writes.
    last_title: String,
    attach_request: Option<LaneId>,
    /// A tmux target (e.g. a terminal window) the loop should attach to next.
    attach_target: Option<String>,
    /// When set, the stdin-reader thread pauses (so tmux owns the terminal during an attach).
    input_suspended: Arc<AtomicBool>,
    /// Set by the reader thread once it has actually entered its paused branch — so an attach waits
    /// for a CONFIRMED handoff (the reader is no longer touching stdin) instead of a guessed sleep,
    /// preventing the reader from fighting tmux for the terminal (split input / spurious detach).
    reader_parked: Arc<AtomicBool>,
    /// Per-account Claude usage (from the daemon's `/usage` probe), shown in the bottom-right
    /// corner for the focused agent's account. Empty unless `usage_probe` is enabled.
    pub usage: Vec<repomon_core::agent::AccountUsage>,
    /// When `usage.get` was last fetched, to throttle the refresh well below the 1s tick.
    last_usage_fetch: Option<std::time::Instant>,
    /// The repomind orchestrator's captured pane (parsed once per `event.orchestrator.output`),
    /// rendered on the right of the command-center view. `None` until the first delta arrives.
    pub orch_output: Option<Pane>,
    /// Whether the orchestrator session is running (from `orchestrator.status` / its broadcast).
    pub orch_running: bool,
    /// The orchestrator's resolved agent (Claude account) and model, for the pinned row's label.
    pub orch_agent: Option<String>,
    pub orch_model: Option<String>,
    /// repomind's current attention (from `orchestrator.status`'s `attention` field):
    /// `"permission"`, `"decision"`, or `"end_of_turn"` when it's asking the human something;
    /// `None` (mapped from the wire's `"none"`) otherwise. Drives the pinned row's needs-you
    /// styling and the command-center header.
    pub orch_attention: Option<String>,
    /// A short "why" for `orch_attention` (the pending dialog's question, or a tail of
    /// repomind's last message), from `orchestrator.status`'s `headline` field.
    pub orch_headline: Option<String>,
    /// True once `apply_orchestrator_status` has applied a first status (mirrors `notif_seeded`
    /// for the lane path): gates the "repomind needs you" popup so a cold start where repomind is
    /// already awaiting attention seeds `orch_attention` instead of reading it as a none→attention
    /// edge and firing a spurious popup. Only the popup is seeded — the pinned row/header above
    /// still reflect the real value on this first call.
    orch_notif_seeded: bool,
    /// Last time the orchestrator pane changed; drives the "chatting" vs "idle" pinned-row state.
    pub orch_last_output: Option<Instant>,
    /// INSERT mode in the command-center view: keystrokes forward to `orchestrator.send_input`.
    pub orch_insert: bool,
    /// Two-press confirm for restarting repomind (`r r`) in the command-center — a restart kills
    /// the live session, so a single stray keypress must not do it. Any other key disarms.
    pub orch_restart_armed: bool,
    /// The watch flag we last pushed to the daemon (`orchestrator.watch`), mirrored so `sync_viewport`
    /// only toggles streaming on a real enter/leave of the view.
    orch_watched: bool,
    /// Screen rect of the pinned "repomind" row this frame, so a click on it selects + opens the view.
    pub orch_click: std::cell::Cell<Option<Rect>>,
    /// Screen rect of repomind's live pane this frame (the command-center's right column, and the
    /// Split right column while the pinned row is selected), so a click can focus / open / attach it.
    pub orch_pane_zone: std::cell::Cell<Option<Rect>>,
    /// Last left-click on the command-center pane, for double-click-to-attach detection.
    orch_pane_last_click: Option<Instant>,
}

impl App {
    pub fn new(client: DaemonClient) -> Self {
        App {
            client,
            theme: Theme::default(),
            lanes: Vec::new(),
            commits: Vec::new(),
            repos: Vec::new(),
            view: View::Fleet,
            selected: 0,
            filter: String::new(),
            filtering: false,
            renaming: false,
            rename_buf: String::new(),
            rename_target: None,
            status: String::new(),
            nl_repo_idx: 0,
            nl_branch: String::new(),
            nl_agent_idx: 0,
            nl_agents: Vec::new(),
            should_quit: false,
            cd_target: None,
            output: HashMap::new(),
            focus_insert: false,
            focus_managed: false,
            focus_missing_ticks: 0,
            // Mouse captured so the wheel scrolls the agent pane and drag-selects copy to the
            // clipboard. `y` releases it for native terminal selection/scroll if preferred.
            mouse_on: true,
            scroll: 0,
            scroll_buf: None,
            pending_scroll: 0,
            pending_input: String::new(),
            scroll_lines: None,
            sel_anchor: None,
            sel_head: None,
            focus_geom: std::cell::Cell::new((0, 0, 0)),
            focus_pane_dims: std::cell::Cell::new(None),
            last_resize: None,
            last_orch_resize: None,
            click_zones: RefCell::new(Vec::new()),
            last_click: None,
            hover_lane: None,
            last_mouse_col: 0,
            refresh_pending: false,
            ac_off: HashSet::new(),
            grid_active: 0,
            // Notify defaults mirror the daemon config defaults, so alerts behave sensibly even
            // before the first `config.get` lands (load_settings refreshes them at startup).
            settings: Settings {
                spawn_prompt: true,
                notify_enabled: true,
                notify_needs_you: true,
                notify_rate_limited: true,
                notify_resumed: true,
                notify_idle: false,
                notify_sound: true,
                notify_show_why: true,
                notify_coalesce: true,
                notify_click_focus: true,
                ..Settings::default()
            },
            settings_idx: 0,
            settings_editing: false,
            settings_geom: std::cell::Cell::new(0),
            prev_status: HashMap::new(),
            notif_seeded: false,
            notif_reseed: false,
            notif_subagents: false,
            notif_debounce: HashMap::new(),
            notif_latch: HashMap::new(),
            notifications: VecDeque::new(),
            notif_sel: 0,
            notif_banner: None,
            spawn_pick_idx: 0,
            spawn_pick_lane: None,
            spawn_return: None,
            urgent_only: false,
            jump_query: String::new(),
            jump_idx: 0,
            jump_return: None,
            timeline: None,
            timeline_zoom: Zoom::Day,
            sessions: Vec::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            recent_commits: Vec::new(),
            recent_commits_lane: None,
            session_idx: 0,
            session_lane: None,
            session_memory: HashMap::new(),
            pending_focus_window: None,
            pending_focus_ticks: 0,
            terminals: Vec::new(),
            terminals_lane: None,
            browse_path: String::new(),
            browse_parent: None,
            browse_entries: Vec::new(),
            browse_selected: 0,
            repo_remove_pending: None,
            discover_pending: None,
            repo_remove_armed: None,
            agents: Vec::new(),
            agents_selected: 0,
            ag_editing: false,
            ag_is_new: false,
            ag_field: AgField::Name,
            ag_name: String::new(),
            ag_command: String::new(),
            ag_orig: None,
            agents_return: None,
            last_viewport: Vec::new(),
            last_viewport_focus: None,
            last_title: String::new(),
            attach_request: None,
            attach_target: None,
            input_suspended: Arc::new(AtomicBool::new(false)),
            reader_parked: Arc::new(AtomicBool::new(false)),
            usage: Vec::new(),
            last_usage_fetch: None,
            orch_output: None,
            orch_running: false,
            orch_agent: None,
            orch_model: None,
            orch_attention: None,
            orch_headline: None,
            orch_notif_seeded: false,
            orch_last_output: None,
            orch_insert: false,
            orch_restart_armed: false,
            orch_watched: false,
            orch_click: std::cell::Cell::new(None),
            orch_pane_zone: std::cell::Cell::new(None),
            orch_pane_last_click: None,
        }
    }

    /// Pull fresh fleet state from the daemon: lanes, today's commits, and repos. Used at
    /// startup and on git/repo notifications.
    pub async fn refresh(&mut self) {
        self.refresh_lanes().await;
        match self
            .client
            .call_typed::<Vec<Commit>>("commit.today", None)
            .await
        {
            Ok(c) => self.commits = c,
            Err(e) => self.status = format!("commit.today failed: {e}"),
        }
        if let Ok(r) = self.client.call_typed::<Vec<Repo>>("repo.list", None).await {
            self.repos = r;
        }
        self.refresh_orchestrator().await;
    }

    /// Pull the orchestrator's running state for the pinned "repomind" row. Cheap; the live
    /// `event.orchestrator.status` broadcast keeps it fresh between refreshes.
    pub async fn refresh_orchestrator(&mut self) {
        if let Ok(v) = self.client.call("orchestrator.status", None).await {
            self.apply_orchestrator_status(&v);
        }
    }

    /// Update the pinned-row/command-center state from an `orchestrator.status` shape
    /// (`{running, agent, model, window, attention, headline}`). On the none→needs-attention edge —
    /// repomind just raised a dialog or finished a turn — fires the same native popup an agent's
    /// NeedsYou gets (mirrors [`fire_notification`](Self::fire_notification)), unless the user is
    /// already looking at the command-center (its row/header already show it) or notifications are
    /// off. The first application only seeds `orch_attention` (see `orch_notif_seeded`): otherwise
    /// a cold start where repomind is already awaiting attention would read `had_attention == false`
    /// (this struct's fields start unset) as a genuine edge and fire a spurious startup popup — the
    /// same problem `detect_notifications`' `notif_seeded` guard solves for the lane path.
    fn apply_orchestrator_status(&mut self, v: &serde_json::Value) {
        self.orch_running = v.get("running").and_then(|b| b.as_bool()).unwrap_or(false);
        self.orch_agent = v
            .get("agent")
            .and_then(|a| a.as_str())
            .map(|s| s.to_string());
        self.orch_model = v
            .get("model")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string());
        let had_attention = self.orch_attention.is_some();
        self.orch_attention = v
            .get("attention")
            .and_then(|a| a.as_str())
            .filter(|a| *a != "none")
            .map(|s| s.to_string());
        self.orch_headline = v
            .get("headline")
            .and_then(|h| h.as_str())
            .map(|s| s.to_string());
        let seeding = !self.orch_notif_seeded;
        self.orch_notif_seeded = true;
        if self.orch_popup_should_fire(seeding, had_attention) {
            let title = "repomind needs you";
            let body = self.orch_headline.clone().unwrap_or_default();
            notify::send_native(
                title,
                &body,
                self.settings.notify_sound,
                self.settings.notify_click_focus,
            );
            self.notif_banner = Some((format!("{title}  ·  {body}"), Instant::now()));
        }
    }

    /// Whether `apply_orchestrator_status` should pop the "repomind needs you" notification for
    /// the update just applied: `seeding` is `true` only on that call's first-ever application
    /// (see `orch_notif_seeded`) — never fires, no matter how the other conditions read, since a
    /// cold start where repomind is already awaiting attention isn't a real edge. Otherwise mirrors
    /// [`Self::notif_enabled_for`]'s gating plus the command-center's own-coverage check. Split out
    /// as a pure decision so it's unit-testable without invoking the real (OS-popping)
    /// `notify::send_native`.
    fn orch_popup_should_fire(&self, seeding: bool, had_attention: bool) -> bool {
        !seeding
            && !had_attention
            && self.orch_attention.is_some()
            && self.view != View::Orchestrator
            && self.settings.notify_enabled
            && self.settings.notify_needs_you
    }

    /// Pull per-account `/usage` from the daemon, throttled well below the 1s tick — usage moves
    /// slowly and the daemon only re-probes every few minutes. Leaves the previous value on error
    /// (and stays empty when `usage_probe` is off, so the corner falls back / hides).
    async fn sync_usage(&mut self) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(20);
        if self.last_usage_fetch.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.last_usage_fetch = Some(std::time::Instant::now());
        if let Ok(u) = self
            .client
            .call_typed::<Vec<repomon_core::agent::AccountUsage>>("usage.get", None)
            .await
        {
            self.usage = u;
        }
    }

    /// Pull just the lane list — the only thing needing per-second freshness in live views (so
    /// an agent that exits on its own is noticed promptly). Commits/repos change on git events,
    /// which arrive as notifications that trigger a full [`refresh`].
    pub async fn refresh_lanes(&mut self) {
        match self.client.call_typed::<Vec<Lane>>("lane.list", None).await {
            Ok(l) => {
                // Keep the cursor on the same lane (and the same agent sub-row, when expanded)
                // across the attention re-sort below.
                let keep = self.selected_lane().map(|l| l.id);
                let keep_ref = self.selected_session_ref();
                self.lanes = l;
                // Forget remembered agent selections for lanes that no longer exist.
                self.session_memory
                    .retain(|id, _| self.lanes.iter().any(|l| l.id == *id));
                // Run notification edge-detection only on a *successful* fetch. Seeding off a
                // failed first call (empty lanes) would make the next good refresh treat every
                // running agent as a fresh transition — the startup storm seeding prevents.
                self.detect_notifications();
                self.sort_lanes();
                if let Some(id) = keep {
                    self.select_lane_session(id, keep_ref);
                }
                // Track how many consecutive refreshes the focused agent has been missing, so a
                // transient overlay flap (one bad snapshot) doesn't detach the user — only a
                // sustained absence does. Counted here (per data refresh), consumed by
                // `check_focus_alive` (which runs every render tick). See [`FOCUS_DETACH_GRACE`].
                if self.focus_managed {
                    let present = self
                        .selected_lane()
                        .map(|l| l.agent_sessions.iter().any(|s| !s.external))
                        .unwrap_or(false);
                    self.focus_missing_ticks =
                        next_focus_missing(present, self.focus_missing_ticks);
                }
            }
            Err(e) => self.status = format!("lane.list failed: {e}"),
        }
        self.clamp_selection();
    }

    /// Order lanes for display: repo groups keep their original (daemon) order, and within each
    /// group pinned lanes come first, then by attention (waiting > stuck on a limit > running),
    /// then most recent activity. Stable, so ties keep the daemon's order — the cursor is
    /// remapped by the caller since this runs on every refresh.
    fn sort_lanes(&mut self) {
        let mut repo_order: HashMap<i64, usize> = HashMap::new();
        for l in &self.lanes {
            let next = repo_order.len();
            repo_order.entry(l.repo.id).or_insert(next);
        }
        let attention: HashMap<LaneId, u8> = self
            .lanes
            .iter()
            .map(|l| (l.id, self.lane_attention(l)))
            .collect();
        // Within a repo + pin + attention bucket, order by lane id (creation order) — a STABLE
        // key. Sorting by recent activity here made lanes bubble around on every agent output
        // (visible jumbling, worse with the expanded agent tree). Needs-you still floats up via
        // the `attention` bucket; only the within-bucket churn is removed.
        self.lanes
            .sort_by_key(|l| (repo_order[&l.repo.id], !l.pinned, attention[&l.id], l.id));
    }

    /// How urgently this lane needs the user (lower = more urgent), accounting for whether a
    /// rate-limited agent will be auto-continued (global toggle minus this lane's opt-out).
    fn lane_attention(&self, lane: &Lane) -> u8 {
        let armed = self.settings.auto_continue && !self.ac_off.contains(&lane.id);
        attention_rank(&lane.agent_sessions, armed)
    }

    /// Whether this lane is blocked on the user: an agent waiting for input, or rate-limited
    /// with no auto-continue coming.
    fn lane_needs_attention(&self, lane: &Lane) -> bool {
        self.lane_attention(lane) <= 1
    }

    /// Cycle the selection through the lanes blocked on the user, wrapping past the end. While
    /// a notification banner is fresh, the first press goes to the lane that just alerted.
    fn jump_attention(&mut self) {
        let banner_lane = self
            .notif_banner
            .as_ref()
            .filter(|(_, t)| t.elapsed() < NOTIF_BANNER_TTL)
            .and_then(|_| self.notifications.back())
            .map(|e| e.lane_id);

        // Work in lane-index space (rows can be per-agent), then map the chosen lane to its row.
        let cur = self.selected_row().map(|r| r.lane_idx).unwrap_or(0);
        let (target_id, msg) = {
            let lanes = self.visible_lanes();
            let hits: Vec<usize> = lanes
                .iter()
                .enumerate()
                .filter(|(_, l)| self.lane_needs_attention(l))
                .map(|(i, _)| i)
                .collect();
            let banner_idx = banner_lane
                .and_then(|id| lanes.iter().position(|l| l.id == id))
                .filter(|&i| i != cur);
            let label = |i: usize| format!("{}/{}", lanes[i].repo.name, lanes[i].worktree.name);

            if let Some(i) = banner_idx {
                (Some(lanes[i].id), format!("→ {} (just alerted)", label(i)))
            } else if hits.is_empty() {
                (None, "no agents need you".to_string())
            } else {
                let next = hits.iter().copied().find(|&i| i > cur).unwrap_or(hits[0]);
                let pos = hits.iter().position(|&i| i == next).unwrap_or(0);
                (
                    Some(lanes[next].id),
                    format!("needs you {}/{} — {}", pos + 1, hits.len(), label(next)),
                )
            }
        };
        if let Some(id) = target_id {
            self.select_lane_session(id, None);
        }
        self.status = msg;
    }

    /// `G`: jump_attention, then go all the way into the pane — select the session the fresh
    /// banner identified (if any) and request a tmux attach. Does nothing extra when the jump
    /// found no lane blocked on you.
    fn jump_attention_attach(&mut self) {
        let banner_sess = self
            .notif_banner
            .as_ref()
            .filter(|(_, t)| t.elapsed() < NOTIF_BANNER_TTL)
            .and_then(|_| self.notifications.back())
            .map(|e| (e.lane_id, e.session_id.clone()));
        self.jump_attention();
        let Some((lane_id, needs)) = self
            .selected_lane()
            .map(|l| (l.id, self.lane_needs_attention(l)))
        else {
            return;
        };
        let from_banner = banner_sess.as_ref().is_some_and(|(id, _)| *id == lane_id);
        if !needs && !from_banner {
            return; // the jump didn't land on an alerting lane — stay put, no attach
        }
        if let Some((_, sid)) = banner_sess.filter(|(id, _)| *id == lane_id) {
            self.select_session(lane_id, sid.as_deref());
        }
        self.attach_request = Some(lane_id);
    }

    /// Open the fuzzy lane switcher, remembering where to return on cancel.
    fn enter_lane_jump(&mut self) {
        self.jump_query.clear();
        self.jump_idx = 0;
        self.jump_return = Some(self.view);
        self.view = View::LaneJump;
    }

    /// Lanes matching the switcher query, best first: fuzzy score, then attention, then
    /// recency. An empty query lists every lane.
    pub fn lane_jump_matches(&self) -> Vec<&Lane> {
        let mut hits: Vec<(&Lane, u32)> = self
            .lanes
            .iter()
            .filter_map(|l| {
                let name = format!("{}/{}", l.repo.name, l.worktree.name);
                let branch = l.state.branch.as_deref().unwrap_or("");
                let score = match (
                    fuzzy_score(&name, &self.jump_query),
                    fuzzy_score(branch, &self.jump_query),
                ) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }?;
                Some((l, score))
            })
            .collect();
        hits.sort_by_key(|(l, score)| {
            (
                *score,
                self.lane_attention(l),
                std::cmp::Reverse(l.last_activity_at),
            )
        });
        hits.into_iter().map(|(l, _)| l).collect()
    }

    fn lane_jump_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::BackTab => self.jump_idx = self.jump_idx.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab if self.jump_idx + 1 < self.lane_jump_matches().len() => {
                self.jump_idx += 1;
            }
            KeyCode::Enter => {
                let id = self.lane_jump_matches().get(self.jump_idx).map(|l| l.id);
                match id {
                    Some(id) => self.jump_to_lane(id),
                    None => self.status = "no lanes match".into(),
                }
            }
            KeyCode::Esc => self.view = self.jump_return.take().unwrap_or(View::Fleet),
            KeyCode::Backspace => {
                self.jump_query.pop();
                self.jump_idx = 0;
            }
            KeyCode::Char(c) => {
                self.jump_query.push(c);
                self.jump_idx = 0;
            }
            _ => {}
        }
    }

    /// Select lane `id` — clearing any filter that hides it — and open it in Focus.
    fn jump_to_lane(&mut self, id: LaneId) {
        let exists = |me: &Self| me.visible_lanes().iter().any(|l| l.id == id);
        if !exists(self) && (!self.filter.is_empty() || self.urgent_only) {
            self.filter.clear();
            self.filtering = false;
            self.urgent_only = false;
        }
        if exists(self) {
            self.select_lane_session(id, None);
            self.focus_insert = false;
            self.reset_scroll();
            self.view = View::Focus;
        } else {
            self.status = "that lane no longer exists".into();
        }
    }

    /// Diff the freshly-fetched per-session statuses against the previous snapshot and fire a
    /// notification on each meaningful transition. The first call only seeds the snapshot.
    fn detect_notifications(&mut self) {
        // Snapshot the new statuses, one entry per real agent session. Inferred file-activity
        // sessions (worktree-isolated subagents) only count when the user opted in.
        let subagents = self.settings.notify_subagents;
        let now: HashMap<(LaneId, SessKey), AgentStatus> = self
            .lanes
            .iter()
            .flat_map(|l| session_statuses(l.id, &l.agent_sessions, subagents))
            .collect();

        // Re-seed (don't diff) on the first list, or after returning from a full-screen attach.
        // While the TUI was parked the daemon owned desktop popups (its `local_watcher_seen`
        // heartbeat went stale), so replaying the transitions that happened in the gap would
        // double-fire what the daemon already delivered. The subagent toggle flipping likewise
        // changes the tracked key set wholesale, so re-seed there too.
        if !self.notif_seeded || self.notif_reseed || subagents != self.notif_subagents {
            self.prev_status = now;
            self.notif_seeded = true;
            self.notif_reseed = false;
            self.notif_subagents = subagents;
            return;
        }

        let live_lanes: HashSet<LaneId> = self.lanes.iter().map(|l| l.id).collect();
        // Lanes that currently have a managed real session — used by the diff to suppress the
        // identity handoff where the no-transcript `Fallback` key vanishes in the same refresh
        // its `Transcript` key first appears (the agent didn't stop, it became identifiable).
        let lanes_with_managed: HashSet<LaneId> = self
            .lanes
            .iter()
            .filter(|l| l.agent_sessions.iter().any(|s| !s.external && !s.inferred))
            .map(|l| l.id)
            .collect();

        // Decide what to fire first (updating the debounce as we go), then deliver — delivery
        // composes from `self.lanes` and mutates `self`, so it can't run while we still hold a
        // borrow into the lanes here.
        let mut fires: Vec<((LaneId, SessKey), NotifKind)> = Vec::new();
        for (key, kind) in
            diff_session_transitions(&self.prev_status, &now, &live_lanes, &lanes_with_managed)
        {
            if !self.notif_enabled_for(kind) {
                continue;
            }
            let dkey = (key.0, key.1.clone(), kind);
            if let Some(t) = self.notif_debounce.get(&dkey) {
                if t.elapsed() < NOTIF_DEBOUNCE {
                    continue;
                }
            }
            // Activity latch: don't re-fire an alert for a session that hasn't done real work
            // since it last fired — gates out the status flapping (idle-decay, lsof undercount,
            // sniff wobble) the time-debounce can't. Idle has no activity anchor, so it's exempt.
            let activity = self
                .lanes
                .iter()
                .find(|l| l.id == key.0)
                .and_then(|l| session_by_key(l, &key.1, subagents))
                .map(|s| s.last_activity_at);
            if kind != NotifKind::Idle
                && !activity_allows_refire(self.notif_latch.get(&dkey).map(|(t, _)| *t), activity)
            {
                continue;
            }
            self.notif_debounce.insert(dkey.clone(), Instant::now());
            if kind != NotifKind::Idle {
                if let Some(a) = activity {
                    self.notif_latch.insert(dkey, (a, Instant::now()));
                }
            }
            fires.push((key, kind));
        }

        // Replacing the snapshot wholesale prunes dead lanes AND dead sessions in one move. The
        // debounce keeps entries for sessions still in the snapshot, plus a grace window so the
        // maps can't grow across a long session of lane/transcript churn.
        self.prev_status = now;
        let prev = &self.prev_status;
        self.notif_debounce.retain(|(lane, sess, _), t| {
            prev.contains_key(&(*lane, sess.clone())) || t.elapsed() < NOTIF_DEBOUNCE
        });
        // The latch keeps entries through a vanish+reappear (the repeat we're stopping); drop one
        // only once its session has been gone longer than it could plausibly return.
        self.notif_latch.retain(|(lane, sess, _), (_, seen)| {
            prev.contains_key(&(*lane, sess.clone())) || seen.elapsed() < NOTIF_LATCH_GRACE
        });

        // A burst (≥2 alerts in one tick) coalesces into a single popup + banner so the desktop
        // isn't spammed; each event still lands individually (and unread) in the history feed.
        let coalesce = self.settings.notify_coalesce && fires.len() >= 2;
        if coalesce {
            let labels: Vec<(String, NotifKind)> = fires
                .iter()
                .filter_map(|((id, _), kind)| {
                    let l = self.lanes.iter().find(|l| l.id == *id)?;
                    Some((format!("{}/{}", l.repo.name, l.worktree.name), *kind))
                })
                .collect();
            let (title, body) = notify::compose_burst(&labels);
            notify::send_native(
                &title,
                &body,
                self.settings.notify_sound,
                self.settings.notify_click_focus,
            );
            self.notif_banner = Some((format!("{title}  ·  {body}"), Instant::now()));
        }
        for ((lane, key), kind) in fires {
            self.fire_notification(lane, &key, kind, coalesce);
        }
    }

    /// Whether the given notification kind is enabled (master switch ∧ per-kind toggle).
    fn notif_enabled_for(&self, kind: NotifKind) -> bool {
        if !self.settings.notify_enabled {
            return false;
        }
        match kind {
            NotifKind::NeedsYou => self.settings.notify_needs_you,
            NotifKind::RateLimited => self.settings.notify_rate_limited,
            NotifKind::Resumed => self.settings.notify_resumed,
            NotifKind::Idle => self.settings.notify_idle,
        }
    }

    /// Compose + deliver a notification about one session: native popup, banner, history entry.
    /// `quiet` records the event in the feed only — the popup/banner were already covered by a
    /// coalesced burst summary.
    fn fire_notification(&mut self, id: LaneId, key: &SessKey, kind: NotifKind, quiet: bool) {
        // Compose under an immutable borrow that ends before we mutate `self`. The session may
        // be gone when its disappearance was the trigger — compose degrades to a generic line.
        let subagents = self.settings.notify_subagents;
        let Some((title, body, session_id)) = self.lanes.iter().find(|l| l.id == id).map(|l| {
            let sess = session_by_key(l, key, subagents);
            // Which of the lane's side-by-side agents this is (1-based in the body when >1).
            let slot = slot_by_key(l, key, subagents);
            let (t, b) = notify::compose(kind, l, sess, slot, self.settings.notify_show_why);
            (t, b, sess.and_then(|s| s.session_id.clone()))
        }) else {
            return;
        };
        if !quiet {
            notify::send_native(
                &title,
                &body,
                self.settings.notify_sound,
                self.settings.notify_click_focus,
            );
            self.notif_banner = Some((format!("{title}  ·  {body}"), Instant::now()));
        }
        self.notifications.push_back(NotifEvent {
            when: chrono::Local::now(),
            kind,
            lane_id: id,
            session_id,
            read: false,
            title,
            body,
        });
        while self.notifications.len() > NOTIF_HISTORY_CAP {
            self.notifications.pop_front();
        }
    }

    /// Notifications not yet seen (the feed hasn't been opened since they fired) — the ⚑ badge.
    pub fn unread_notifs(&self) -> usize {
        self.notifications.iter().filter(|e| !e.read).count()
    }

    pub fn visible_lanes(&self) -> Vec<&Lane> {
        let f = self.filter.to_lowercase();
        self.lanes
            .iter()
            .filter(|l| !self.urgent_only || self.lane_needs_attention(l))
            .filter(|l| {
                f.is_empty()
                    || l.repo.name.to_lowercase().contains(&f)
                    || l.worktree.name.to_lowercase().contains(&f)
                    || l.state
                        .branch
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&f)
            })
            .collect()
    }

    /// The sidebar rows in display order. One row per visible lane, except a lane running several
    /// agents expands into a header row plus one row per agent when `expand_agents` is on. `selected`
    /// indexes this list. Render and navigation BOTH go through here so they never drift.
    pub fn fleet_rows(&self) -> Vec<FleetRow> {
        let lanes = self.visible_lanes();
        // Row 0 is always the pinned "repomind" orchestrator row, so `selected == 0` selects it
        // and real lane rows start at index 1; the selection math below indexes this list directly.
        let mut rows = vec![FleetRow {
            lane_idx: 0,
            session: None,
            orchestrator: true,
        }];
        for (i, lane) in lanes.iter().enumerate() {
            rows.push(FleetRow {
                lane_idx: i,
                session: None,
                orchestrator: false,
            });
            if self.settings.expand_agents && lane.agent_sessions.len() > 1 {
                // Emit sub-rows in a STABLE order (by durable session identity), not the daemon's
                // newest-active-first order — otherwise renamed rows jump around as agents take
                // turns. `session` keeps the real index so selection still targets the right agent.
                for s in stable_session_order(&lane.agent_sessions) {
                    rows.push(FleetRow {
                        lane_idx: i,
                        session: Some(s),
                        orchestrator: false,
                    });
                }
            }
        }
        rows
    }

    /// Whether the pinned "repomind" row is the current selection.
    pub fn orchestrator_selected(&self) -> bool {
        self.selected_row().map(|r| r.orchestrator).unwrap_or(false)
    }

    fn selected_row(&self) -> Option<FleetRow> {
        self.fleet_rows().get(self.selected).copied()
    }

    fn rows_len(&self) -> usize {
        self.fleet_rows().len()
    }

    pub fn selected_lane(&self) -> Option<&Lane> {
        let row = self.fleet_rows().get(self.selected).copied()?;
        if row.orchestrator {
            return None; // the pinned repomind row targets no lane
        }
        self.visible_lanes().into_iter().nth(row.lane_idx)
    }

    /// The stable identity of the agent on the selected row, when it's an agent sub-row.
    fn selected_session_ref(&self) -> Option<SessionRef> {
        let row = self.selected_row()?;
        let s = row.session?;
        let lane = self.visible_lanes().into_iter().nth(row.lane_idx)?;
        lane.agent_sessions.get(s).and_then(agent_session_ref)
    }

    /// Move the fleet cursor onto `lane_id`, preferring the agent sub-row matching `sref` (stable
    /// across refreshes/reorders), else that lane's header row. No-op when the lane isn't visible.
    fn select_lane_session(&mut self, lane_id: LaneId, sref: Option<SessionRef>) {
        let (lane_pos, want) = {
            let lanes = self.visible_lanes();
            let Some(lane_pos) = lanes.iter().position(|l| l.id == lane_id) else {
                return;
            };
            let want = sref
                .as_ref()
                .and_then(|r| session_index_for_ref(&lanes[lane_pos].agent_sessions, r));
            (lane_pos, want)
        };
        let rows = self.fleet_rows();
        // Exclude the pinned repomind row (its `lane_idx` is a placeholder `0`) so lane 0 never
        // resolves to it.
        let target = rows
            .iter()
            .position(|r| !r.orchestrator && r.lane_idx == lane_pos && r.session == want)
            .or_else(|| {
                rows.iter()
                    .position(|r| !r.orchestrator && r.lane_idx == lane_pos && r.session.is_none())
            });
        if let Some(t) = target {
            self.selected = t;
        }
    }

    /// Lane ids to babysit in the grid: pinned first, then by attention, then most-active.
    pub fn grid_lane_ids(&self) -> Vec<LaneId> {
        let mut lanes: Vec<&Lane> = self.visible_lanes();
        lanes.sort_by(|a, b| {
            let key = |l: &Lane| {
                (
                    !l.pinned,
                    self.lane_attention(l),
                    std::cmp::Reverse(l.last_activity_at),
                )
            };
            key(a).cmp(&key(b))
        });
        lanes.into_iter().take(8).map(|l| l.id).collect()
    }

    fn clamp_selection(&mut self) {
        let n = self.rows_len();
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }

    /// Which lanes the daemon should fast-poll output for, given the current view.
    fn live_lanes(&self) -> Vec<LaneId> {
        match self.view {
            View::Split | View::Focus => {
                self.selected_lane().map(|l| vec![l.id]).unwrap_or_default()
            }
            View::Grid => self.grid_lane_ids(),
            View::Fleet
            | View::NewLane
            | View::Timeline
            | View::Sessions
            | View::Search
            | View::AddRepo
            | View::Agents
            | View::Settings
            | View::Notifications
            | View::SpawnPick
            | View::LaneJump
            | View::Orchestrator => Vec::new(),
        }
    }

    /// Tell the daemon which lanes are visible — and, in Split/Focus, which agent window the
    /// selected lane should stream (so Tab between a lane's agents retargets the pane) — if
    /// either changed.
    pub async fn sync_viewport(&mut self) {
        let live = self.live_lanes();
        // Name the lane the user is actively watching (Split/Focus, or the highlighted Grid tile)
        // so the daemon streams it at the fast cadence and lets the other viewport panes back off.
        let focus = match self.view {
            View::Split | View::Focus | View::Grid => self
                .selected_lane()
                .map(|l| l.id)
                .zip(self.selected_window()),
            _ => None,
        };
        if live != self.last_viewport || focus != self.last_viewport_focus {
            let _ = self
                .client
                .call(
                    "viewport.set",
                    Some(json!({
                        "lane_ids": live,
                        "focus_lane": focus.as_ref().map(|(l, _)| l),
                        "focus_window": focus.as_ref().map(|(_, w)| w),
                    })),
                )
                .await;
            self.last_viewport = live;
            self.last_viewport_focus = focus;
        }
        // Stream the orchestrator pane while the command-center view is open, or while the pinned
        // repomind row is selected in Split (its right column previews the live pane). Toggle the
        // daemon's watch flag only on a real change (it gates `stream_orchestrator`).
        let want_watch = self.view == View::Orchestrator
            || (self.view == View::Split && self.orchestrator_selected());
        if want_watch != self.orch_watched {
            let _ = self
                .client
                .call("orchestrator.watch", Some(json!({ "on": want_watch })))
                .await;
            self.orch_watched = want_watch;
        }
    }

    /// Resize the focused agent's tmux window to match the mediated view's pane, so it reflows to
    /// the visible width (no right-edge clipping). Only fires on a real size/focus change — view
    /// switch, terminal resize, or return-from-attach — not every tick.
    async fn sync_pane_size(&mut self) {
        if !matches!(self.view, View::Split | View::Focus) {
            return;
        }
        let (Some((cols, rows)), Some(lane), Some(window)) = (
            self.focus_pane_dims.get(),
            self.selected_lane().map(|l| l.id),
            self.selected_window(), // a managed window; `None` for external/no-window sessions
        ) else {
            return;
        };
        if cols < 4 || rows < 2 {
            return;
        }
        let key = (lane, window.clone(), cols, rows);
        if self.last_resize.as_ref() == Some(&key) {
            return;
        }
        let _ = self
            .client
            .call(
                "agent.resize",
                Some(json!({ "lane_id": lane, "cols": cols, "rows": rows, "window": window })),
            )
            .await;
        self.last_resize = Some(key);
    }

    /// Size the orchestrator's tmux window to the right-pane area while it's being streamed (the
    /// command-center, or the Split preview when the pinned row is selected), so the captured pane
    /// fills the view exactly instead of overflowing (too wide) or rendering blank (too tall, so
    /// `output_window` would slice off the trailing empty rows). Mirrors `sync_pane_size`.
    async fn sync_orchestrator_size(&mut self) {
        let streaming = self.view == View::Orchestrator
            || (self.view == View::Split && self.orchestrator_selected());
        if !streaming {
            return;
        }
        let Some((cols, rows)) = self.focus_pane_dims.get() else {
            return;
        };
        if cols < 4 || rows < 2 {
            return;
        }
        if self.last_orch_resize == Some((cols, rows)) {
            return;
        }
        let _ = self
            .client
            .call(
                "orchestrator.resize",
                Some(json!({ "cols": cols, "rows": rows })),
            )
            .await;
        self.last_orch_resize = Some((cols, rows));
    }

    /// Load the selected lane's recent commits (its branch history) when the selection
    /// changes. `recent_commits_lane == None` forces a refetch (e.g. after a repo event).
    pub async fn sync_recent_commits(&mut self) {
        let sel = self.selected_lane().map(|l| l.id);
        if sel == self.recent_commits_lane && self.recent_commits_lane.is_some() {
            return;
        }
        self.recent_commits_lane = sel;
        self.recent_commits.clear();
        if let Some(id) = sel {
            if let Ok(c) = self
                .client
                .call_typed::<Vec<Commit>>(
                    "commit.recent",
                    Some(json!({ "lane_id": id, "limit": 8 })),
                )
                .await
            {
                self.recent_commits = c;
            }
        }
    }

    /// Keep the terminal window/tab title on the open repo (OSC 2) so several terminals are
    /// tellable apart at a glance. Re-emitted only when it changes; the shell resets the title
    /// on exit as usual.
    fn sync_title(&mut self) {
        let title = match self.selected_lane() {
            Some(l) => format!("repomon · {}/{}", l.repo.name, l.worktree.name),
            None => "repomon".to_string(),
        };
        if title != self.last_title {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b]2;{title}\x07");
            let _ = out.flush();
            self.last_title = title;
        }
    }

    /// Keep the session cursor in range: reset to 0 when the selected lane changes, and clamp
    /// to the number of sessions on that lane.
    fn sync_session_cursor(&mut self) {
        let sel = self.selected_lane().map(|l| l.id);
        // Honor a just-spawned agent's focus intent FIRST — before the expanded early-return below,
        // which would otherwise skip it. Once its window appears on the spawn lane, point the
        // cursor (and, when expanded, the selected row) at the new agent.
        if let Some((lane, window)) = self.pending_focus_window.clone() {
            if sel == Some(lane) {
                let hit = self.selected_lane().and_then(|l| {
                    l.agent_sessions
                        .iter()
                        .position(|s| s.tmux_window.as_deref() == Some(window.as_str()))
                });
                match hit {
                    Some(i) => {
                        self.session_idx = i;
                        self.pending_focus_window = None;
                        self.pending_focus_ticks = 0;
                        self.reset_scroll();
                        if self.settings.expand_agents {
                            let sref = self
                                .selected_lane()
                                .and_then(|l| l.agent_sessions.get(i))
                                .and_then(agent_session_ref);
                            self.select_lane_session(lane, sref);
                        }
                        return;
                    }
                    None => {
                        self.pending_focus_ticks = self.pending_focus_ticks.saturating_add(1);
                        if self.pending_focus_ticks > PENDING_FOCUS_GIVE_UP_TICKS {
                            self.pending_focus_window = None;
                            self.pending_focus_ticks = 0;
                        }
                    }
                }
            } else {
                // Selection moved off the spawn lane before the agent appeared — drop the intent.
                self.pending_focus_window = None;
                self.pending_focus_ticks = 0;
            }
        }
        // In expanded mode an agent sub-row IS the session cursor — drive `session_idx` straight
        // from the selected row (the memory logic below is for the collapsed lane rows).
        if self.settings.expand_agents {
            if let Some(FleetRow {
                session: Some(s), ..
            }) = self.selected_row()
            {
                let lane_id = self.selected_lane().map(|l| l.id);
                if self.session_idx != s || self.session_lane != lane_id {
                    self.reset_scroll();
                }
                self.session_lane = lane_id;
                self.session_idx = s;
                return;
            }
        }
        if sel != self.session_lane {
            // Remember the agent the outgoing lane had selected, then restore the one we last
            // had on the lane we're arriving at — so returning to a multi-agent project keeps
            // your pick instead of snapping to the first slot. Identity-keyed, so it survives
            // the session list reordering; falls back to the first agent when the remembered
            // one is gone (or this lane was never visited).
            if let Some(old) = self.session_lane {
                if let Some(r) = self
                    .lanes
                    .iter()
                    .find(|l| l.id == old)
                    .and_then(|l| l.agent_sessions.get(self.session_idx))
                    .and_then(agent_session_ref)
                {
                    self.session_memory.insert(old, r);
                }
            }
            self.session_lane = sel;
            self.session_idx = sel
                .and_then(|id| self.session_memory.get(&id))
                .and_then(|r| {
                    self.selected_lane()
                        .and_then(|l| session_index_for_ref(&l.agent_sessions, r))
                })
                .unwrap_or(0);
            self.reset_scroll(); // scrollback buffer belonged to the previous lane
        }
        let n = self
            .selected_lane()
            .map(|l| l.agent_sessions.len())
            .unwrap_or(0);
        if self.session_idx >= n {
            self.session_idx = n.saturating_sub(1);
        }
    }

    /// Drop out of Focus once the agent we were driving exits (its managed session is gone —
    /// e.g. the user typed `/exit`, or it crashed). Avoids sitting on a dead pane.
    fn check_focus_alive(&mut self) {
        if self.view != View::Focus {
            self.focus_managed = false;
            self.focus_missing_ticks = 0;
            return;
        }
        let has_managed = self
            .selected_lane()
            .map(|l| l.agent_sessions.iter().any(|s| !s.external))
            .unwrap_or(false);
        if has_managed {
            self.focus_managed = true;
            self.focus_missing_ticks = 0;
        } else if self.focus_managed && self.focus_missing_ticks >= FOCUS_DETACH_GRACE {
            // Sustained absence (counted per lane refresh in `refresh_lanes`, not per render tick)
            // — a real exit, not a one-snapshot flap. Drop back to Split.
            self.focus_managed = false;
            self.focus_missing_ticks = 0;
            self.focus_insert = false;
            self.view = View::Split;
            self.status = "agent exited".into();
        }
    }

    /// Point the fleet selection at the grid's active tile (so per-lane actions act on it).
    fn select_grid_active(&mut self) {
        let ids = self.grid_lane_ids();
        if let Some(&id) = ids.get(self.grid_active) {
            self.select_lane_session(id, None);
        }
    }

    /// Key handling in the babysit Grid: a linear cursor over the live tiles (arrows move it,
    /// dots show position), `↵` focuses the active tile, and esc/spc/q leave the view.
    async fn grid_key(&mut self, key: KeyEvent) {
        // Click-focused a tile? Keystrokes go straight to that agent (like Split/Focus insert);
        // ^O blurs, as does a click on empty space.
        if self.focus_insert {
            if leaves_insert(&key) {
                self.focus_insert = false;
                return;
            }
            self.send_agent_key(key).await;
            return;
        }
        let n = self.grid_lane_ids().len();
        if self.grid_active >= n {
            self.grid_active = n.saturating_sub(1);
        }
        match key.code {
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') | KeyCode::Char('k') => {
                self.grid_active = self.grid_active.saturating_sub(1);
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') | KeyCode::Char('j')
                if self.grid_active + 1 < n =>
            {
                self.grid_active += 1;
            }
            // ↵ opens the active tile's agent in its real tmux pane (a native terminal).
            KeyCode::Enter if n > 0 => {
                self.select_grid_active();
                self.attach_request = self.selected_lane().map(|l| l.id);
            }
            // Per-tile actions act on the active tile.
            KeyCode::Char('e') => {
                self.select_grid_active();
                self.spawn_agent().await;
            }
            KeyCode::Char('s') => {
                self.select_grid_active();
                self.stop_agent().await;
                self.view = View::Grid;
            }
            KeyCode::Char('p') => {
                self.select_grid_active();
                self.toggle_pin().await;
            }
            // Hop to the next tile whose agent needs you, wrapping.
            KeyCode::Char('g') if n > 0 => {
                let ids = self.grid_lane_ids();
                let need: Vec<usize> = ids
                    .iter()
                    .enumerate()
                    .filter(|(_, id)| {
                        self.lanes
                            .iter()
                            .find(|l| l.id == **id)
                            .is_some_and(|l| self.lane_needs_attention(l))
                    })
                    .map(|(i, _)| i)
                    .collect();
                match need
                    .iter()
                    .copied()
                    .find(|&i| i > self.grid_active)
                    .or(need.first().copied())
                {
                    Some(i) => self.grid_active = i,
                    None => self.status = "no agents need you".into(),
                }
            }
            KeyCode::Char('f') => self.enter_lane_jump(),
            KeyCode::Char(' ') | KeyCode::Esc => self.view = View::Fleet,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    /// The managed tmux window of the session the cursor is on (Tab cycles it) — where keys,
    /// captures, stops, and attaches are routed. `None` falls back to the lane's first slot
    /// (external/inferred sessions have no window of their own).
    fn selected_window(&self) -> Option<String> {
        self.selected_lane()
            .and_then(|l| l.agent_sessions.get(self.session_idx))
            .and_then(|s| s.tmux_window.clone())
    }

    /// Wait (bounded) for the stdin reader thread to confirm it has parked, so `tmux attach` gets
    /// sole ownership of the terminal — a confirmed handoff instead of a guessed sleep, so the
    /// reader can't keep reading stdin and split keystrokes with tmux (which corrupts the terminal
    /// and can feed tmux a sequence it misreads as a detach key). Bounded (~400ms) so a reader
    /// wedged in a blocking read can't hang the attach; a timeout is logged for diagnosis.
    async fn await_reader_parked(&self) {
        for _ in 0..40 {
            if self.reader_parked.load(Ordering::Relaxed) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tui_log("WARN reader did not park before attach; terminal handoff may be racy");
    }

    /// The usage account key of the agent the user is looking at — the selected lane's selected
    /// session — for attributing the usage corner. Claude agents key on their config dir; Codex
    /// keys on `"codex"`; other kinds have no usage probe (`None`). Matches `AccountUsage::key`.
    pub fn focused_account_key(&self) -> Option<String> {
        use repomon_core::model::AgentKind;
        let lane = self.selected_lane()?;
        let sess = lane
            .agent_sessions
            .get(self.session_idx)
            .or_else(|| lane.agent_sessions.first())?;
        match &sess.agent {
            AgentKind::Codex => Some("codex".to_string()),
            AgentKind::ClaudeCode => Some(repomon_core::agent::claude::account_key(
                sess.config_dir.as_deref(),
            )),
            _ => None,
        }
    }

    /// Move the session cursor among the selected lane's agents.
    fn cycle_session(&mut self, forward: bool) {
        let n = self
            .selected_lane()
            .map(|l| l.agent_sessions.len())
            .unwrap_or(0);
        if n <= 1 {
            return;
        }
        self.session_idx = if forward {
            (self.session_idx + 1) % n
        } else {
            (self.session_idx + n - 1) % n
        };
        // The scrollback snapshot belonged to the previous agent's window — drop it so the new
        // agent shows its own live tail instead of stale lines.
        self.reset_scroll();
        // In expanded mode the selected row drives session_idx, so move the cursor onto the new
        // agent's sub-row — otherwise the per-tick cursor sync snaps session_idx straight back.
        if self.settings.expand_agents {
            if let Some(lane_id) = self.selected_lane().map(|l| l.id) {
                let sref = self
                    .selected_lane()
                    .and_then(|l| l.agent_sessions.get(self.session_idx))
                    .and_then(agent_session_ref);
                self.select_lane_session(lane_id, sref);
            }
        }
    }

    async fn on_notification(&mut self, note: Notification) {
        if note.method == "event.agent.output" {
            if let (Some(id), Some(content)) = (
                note.params.get("lane_id").and_then(|v| v.as_i64()),
                note.params.get("content").and_then(|v| v.as_str()),
            ) {
                // Parse the ANSI once, here, instead of on every render of this pane.
                let raw = content.to_string();
                let lines = view::parse_pane(&raw);
                // The daemon attaches the agent pane's cursor for the focused lane (`[col, row]`).
                let cursor = note
                    .params
                    .get("cursor")
                    .and_then(|v| v.as_array())
                    .and_then(|a| Some((a.first()?.as_u64()? as u16, a.get(1)?.as_u64()? as u16)));
                self.output.insert(id, Pane { raw, lines, cursor });
            }
        } else if note.method == "event.orchestrator.output" {
            if let Some(content) = note.params.get("content").and_then(|v| v.as_str()) {
                let raw = content.to_string();
                let lines = view::parse_pane(&raw);
                // repomind's real cursor `[col, row]` (parsed like the lane path), so the mediated
                // pane can draw it where you're typing.
                let cursor = note
                    .params
                    .get("cursor")
                    .and_then(|v| v.as_array())
                    .and_then(|a| Some((a.first()?.as_u64()? as u16, a.get(1)?.as_u64()? as u16)));
                let changed = self.orch_output.as_ref().map(|p| &p.raw) != Some(&raw);
                if changed {
                    self.orch_last_output = Some(Instant::now());
                }
                self.orch_output = Some(Pane { raw, lines, cursor });
            }
        } else if note.method == "event.orchestrator.status" {
            self.apply_orchestrator_status(&note.params);
        } else {
            // Don't refresh inline — the event loop coalesces a burst of notifications into a
            // single refresh (each `refresh()` is a ~100ms lane.list, and bursts/exit-focus
            // backlogs would otherwise stack into a multi-hundred-ms stall).
            self.refresh_pending = true;
            // A repo may have new commits; refetch the selected lane's history next tick.
            self.recent_commits_lane = None;
        }
    }

    async fn handle_event(&mut self, ev: Event) {
        let key = match ev {
            Event::Key(key) => key,
            // Views render from the live frame size every draw, so a resize redraws correctly
            // on its own; the timeline additionally refetches so its bucket count tracks the
            // new width (the renderer resamples in the meantime).
            Event::Resize(_, _) => {
                if self.view == View::Timeline {
                    self.load_timeline().await;
                }
                return;
            }
            Event::Mouse(me) => {
                // Inline rename is modal: ignore the pointer too, so a click/scroll can't retarget
                // the rename to a different agent.
                if self.renaming {
                    return;
                }
                use ratatui::crossterm::event::{MouseButton, MouseEventKind};
                // Remember the real pointer column from positioned events — wheel events report
                // column 0 on many terminals, so this is how we know which pane the wheel is over.
                if !matches!(
                    me.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) {
                    self.last_mouse_col = me.column;
                }
                // Bare movement just updates the hovered lane (highlighted on render).
                if matches!(me.kind, MouseEventKind::Moved) {
                    self.update_hover(me.column, me.row);
                    return;
                }
                // In Focus the wheel scrolls the agent's output and a drag selects lines (copied
                // to the clipboard on release); elsewhere the wheel moves the cursor.
                match self.view {
                    View::Focus => match me.kind {
                        MouseEventKind::ScrollUp => self.pane_scroll(true, 1),
                        MouseEventKind::ScrollDown => self.pane_scroll(false, 1),
                        MouseEventKind::Down(MouseButton::Left) => {
                            self.sel_anchor = self.focus_line_at(me.row);
                            self.sel_head = self.sel_anchor;
                        }
                        MouseEventKind::Drag(MouseButton::Left) if self.sel_anchor.is_some() => {
                            if let Some(i) = self.focus_line_at(me.row) {
                                self.sel_head = Some(i);
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => self.copy_selection(),
                        _ => {}
                    },
                    // Settings: a click selects + activates the row under the cursor.
                    View::Settings => {
                        if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                            self.settings_click(me.row).await;
                        }
                    }
                    // Split: route the wheel by which side the pointer is over — the agent pane
                    // (right of the 26-col sidebar + 1-col divider) scrolls its output; the sidebar
                    // navigates lanes. Wheel events report column 0 on many terminals, so fall back
                    // to the last positioned pointer column. A left-click focuses the clicked lane.
                    View::Split => {
                        let col = if me.column > 0 {
                            me.column
                        } else {
                            self.last_mouse_col
                        };
                        let over_pane = col > 26;
                        match me.kind {
                            MouseEventKind::ScrollUp if over_pane => self.pane_scroll(true, 1),
                            MouseEventKind::ScrollDown if over_pane => self.pane_scroll(false, 1),
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                                self.handle_mouse(me)
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                self.handle_click(me.column, me.row).await
                            }
                            _ => {}
                        }
                    }
                    // Command-center: the wheel scrolls repomind's pane; a left-click jumps to a
                    // needs-you lane (left column) or focuses/attaches the pane (right column).
                    View::Orchestrator => match me.kind {
                        MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_add(3),
                        MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(3),
                        MouseEventKind::Down(MouseButton::Left) => {
                            self.orchestrator_click(me.column, me.row).await
                        }
                        _ => {}
                    },
                    // Grid/Fleet: a left-click focuses the clicked lane (double-click opens its real
                    // terminal, a click on empty space blurs); the wheel still navigates.
                    _ => match me.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            self.handle_click(me.column, me.row).await
                        }
                        _ => self.handle_mouse(me),
                    },
                }
                return;
            }
            _ => return,
        };
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.status.clear();
        match self.view {
            // Inline session rename is modal: while active it swallows every key, in any view.
            _ if self.renaming => self.rename_key(key).await,
            View::NewLane => self.new_lane_key(key).await,
            View::Split => self.split_key(key).await,
            View::Focus => self.focus_key(key).await,
            View::Grid => self.grid_key(key).await,
            View::Search => self.search_key(key).await,
            View::Timeline => self.timeline_key(key).await,
            View::Sessions => self.sessions_key(key).await,
            View::AddRepo => self.addrepo_key(key).await,
            View::Agents => self.agents_key(key).await,
            View::Settings => self.settings_key(key).await,
            View::Notifications => self.notifications_key(key),
            View::SpawnPick => self.spawn_pick_key(key).await,
            View::LaneJump => self.lane_jump_key(key),
            View::Orchestrator => self.orchestrator_key(key).await,
            _ if self.filtering => self.filter_key(key),
            _ => {
                // `R` renames the selected agent sub-row in the expanded fleet sidebar.
                if key.code == KeyCode::Char('R') {
                    self.start_rename();
                } else if let Some(action) = keybinds::nav(key) {
                    self.apply(action).await;
                }
            }
        }
    }

    /// Begin renaming the agent on the selected sub-row. Pins the target's transcript `session_id`
    /// now (resolved from the selected ROW, not the laggy `session_idx`) so a cursor move or a 1s
    /// refresh during the edit can't retarget the rename. No-op unless an agent row is selected.
    fn start_rename(&mut self) {
        let Some(FleetRow {
            lane_idx,
            session: Some(s),
            ..
        }) = self.selected_row()
        else {
            self.status =
                "rename: select an agent row (turn on 'expand agent rows' in settings)".into();
            return;
        };
        let (sid, label) = {
            let lanes = self.visible_lanes();
            let Some(sess) = lanes
                .into_iter()
                .nth(lane_idx)
                .and_then(|l| l.agent_sessions.get(s))
            else {
                return;
            };
            (sess.session_id.clone(), sess.custom_label.clone())
        };
        let Some(sid) = sid else {
            self.status = "can't rename — this session has no transcript id yet".into();
            return;
        };
        self.rename_target = Some(sid);
        self.rename_buf = label.unwrap_or_default();
        self.renaming = true;
    }

    async fn rename_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) => self.rename_buf.push(c),
            KeyCode::Backspace => {
                self.rename_buf.pop();
            }
            KeyCode::Enter => self.commit_rename().await,
            KeyCode::Esc => {
                self.renaming = false;
                self.rename_buf.clear();
                self.rename_target = None;
            }
            _ => {}
        }
    }

    /// Persist the rename (empty buffer clears the label), keyed by the pinned transcript id.
    async fn commit_rename(&mut self) {
        self.renaming = false;
        let label = std::mem::take(&mut self.rename_buf).trim().to_string();
        let Some(session_id) = self.rename_target.take() else {
            return;
        };
        let label_val = if label.is_empty() {
            serde_json::Value::Null
        } else {
            json!(label)
        };
        match self
            .client
            .call(
                "session.rename",
                Some(json!({ "session_id": session_id, "label": label_val })),
            )
            .await
        {
            Ok(_) => {
                self.status = if label.is_empty() {
                    "label cleared".into()
                } else {
                    format!("renamed → {label}")
                };
                self.refresh_lanes().await; // show the new label at once
            }
            Err(e) => self.status = format!("rename failed: {e}"),
        }
    }

    async fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.run_search().await;
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.run_search().await;
            }
            KeyCode::Enter => self.run_search().await,
            KeyCode::Esc | KeyCode::Left => self.view = View::Fleet,
            _ => {}
        }
    }

    async fn timeline_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('d') => self.set_zoom(Zoom::Day).await,
            KeyCode::Char('w') => self.set_zoom(Zoom::Week).await,
            KeyCode::Char('m') => self.set_zoom(Zoom::Month).await,
            _ => {
                if let Some(a) = keybinds::nav(key) {
                    self.apply(a).await;
                }
            }
        }
    }

    async fn sessions_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('e') => self.export_sessions(),
            _ => {
                if let Some(a) = keybinds::nav(key) {
                    self.apply(a).await;
                }
            }
        }
    }

    async fn addrepo_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.browse_selected = self.browse_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j')
                if self.browse_selected + 1 < self.browse_entries.len() =>
            {
                self.browse_selected += 1
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(e) = self.browse_entries.get(self.browse_selected) {
                    let p = e.path.to_string_lossy().into_owned();
                    self.load_browse(Some(p)).await;
                }
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
                if let Some(parent) = self.browse_parent.clone() {
                    self.load_browse(Some(parent)).await;
                }
            }
            KeyCode::Char('a') => self.add_browsed().await,
            KeyCode::Char('d') => self.discover_here().await,
            KeyCode::Char('x') => self.remove_browsed().await,
            KeyCode::Esc => self.view = View::Fleet,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
        // Any key other than the confirming second `x` / `d` disarms a pending removal / discover.
        if key.code != KeyCode::Char('x') {
            self.repo_remove_pending = None;
        }
        if key.code != KeyCode::Char('d') {
            self.discover_pending = None;
        }
    }

    /// Unregister the selected repo (must already be registered — marked `+`). Two presses of
    /// `x`: the first arms, the second removes. Only repomon's bookkeeping goes away — the
    /// project, its worktrees, and any running agents on disk are untouched.
    async fn remove_browsed(&mut self) {
        let Some(entry) = self.browse_entries.get(self.browse_selected) else {
            return;
        };
        if !entry.added {
            self.status = "not a registered repo (only + entries can be removed)".into();
            self.repo_remove_pending = None;
            return;
        }
        let Some(repo) = self.repos.iter().find(|r| r.path == entry.path) else {
            self.status = "couldn't match this entry to a registered repo".into();
            self.repo_remove_pending = None;
            return;
        };
        let (id, name) = (repo.id, repo.name.clone());
        if self.repo_remove_pending != Some(id) {
            self.repo_remove_pending = Some(id);
            self.status = format!("press x again to remove {name} (files on disk stay)");
            return;
        }
        self.repo_remove_pending = None;
        match self
            .client
            .call("repo.remove", Some(json!({ "repo_id": id })))
            .await
        {
            Ok(_) => {
                self.status = format!("removed {name} — files on disk untouched");
                self.refresh().await;
                let here = self.browse_path.clone();
                self.load_browse(Some(here)).await;
            }
            Err(e) => self.status = format!("remove failed: {e}"),
        }
    }

    async fn load_browse(&mut self, path: Option<String>) {
        match self
            .client
            .call_typed::<BrowseResult>("fs.browse", Some(json!({ "path": path })))
            .await
        {
            Ok(r) => {
                self.browse_path = r.path.to_string_lossy().into_owned();
                self.browse_parent = r.parent.map(|p| p.to_string_lossy().into_owned());
                self.browse_entries = r.entries;
                self.browse_selected = 0;
            }
            Err(e) => self.status = format!("browse failed: {e}"),
        }
    }

    async fn add_browsed(&mut self) {
        let entry = self.browse_entries.get(self.browse_selected).cloned();
        match entry {
            None => {}
            Some(e) if e.added => self.status = format!("{} is already registered", e.name),
            Some(e) if !e.is_repo => {
                self.status = format!(
                    "{} is not a git repo — ↵ to enter, d to discover inside",
                    e.name
                )
            }
            Some(e) => {
                let path = e.path.to_string_lossy().into_owned();
                match self
                    .client
                    .call("repo.add", Some(json!({ "path": path })))
                    .await
                {
                    Ok(_) => {
                        self.status = format!("added {}", e.name);
                        self.refresh().await;
                        let here = self.browse_path.clone();
                        self.load_browse(Some(here)).await; // refresh "added" markers
                    }
                    Err(err) => self.status = format!("add failed: {err}"),
                }
            }
        }
    }

    /// Discover git repos under the browsed folder and register them — behind a confirming second
    /// press (like repo removal), since a recursive scan of a deep folder can register dozens of
    /// repos at once. First `d` scans and reports the count; second `d` commits the add.
    async fn discover_here(&mut self) {
        // Second press: commit the pending add.
        if let Some((root, found)) = self.discover_pending.take() {
            let mut added = 0;
            for path in &found {
                if self
                    .client
                    .call("repo.add", Some(json!({ "path": path })))
                    .await
                    .is_ok()
                {
                    added += 1;
                }
            }
            self.status = format!("added {added} repo(s) under {root}");
            self.refresh().await;
            let here = self.browse_path.clone();
            self.load_browse(Some(here)).await;
            return;
        }
        // First press: scan and arm (no repos added yet).
        let root = self.browse_path.clone();
        let found: Vec<String> = self
            .client
            .call_typed(
                "repo.discover",
                Some(json!({ "root": root, "max_depth": 4 })),
            )
            .await
            .unwrap_or_default();
        if found.is_empty() {
            self.status = format!("no unregistered git repos under {root}");
            return;
        }
        self.status = format!(
            "found {} repo(s) under {root} — press d again to add all, any other key to cancel",
            found.len()
        );
        self.discover_pending = Some((root, found));
    }

    /// Fetch the spawnable agents (built-ins + configured customs) for the New Lane picker,
    /// preselecting the configured default.
    async fn load_agents(&mut self) {
        match self
            .client
            .call_typed::<Vec<AgentChoice>>("agent.detect", None)
            .await
        {
            Ok(a) if !a.is_empty() => self.nl_agents = a,
            _ => {
                self.nl_agents = AGENT_KINDS
                    .iter()
                    .map(|k| AgentChoice {
                        name: (*k).to_string(),
                        command: (*k).to_string(),
                        detected: true,
                        custom: false,
                        default: false,
                    })
                    .collect()
            }
        }
        self.nl_agent_idx = self.nl_agents.iter().position(|a| a.default).unwrap_or(0);
    }

    /// Open the agent manager, remembering where `esc` should return.
    async fn enter_agents(&mut self, return_to: Option<View>) {
        self.agents_return = return_to;
        self.view = View::Agents;
        self.ag_editing = false;
        self.agents_selected = 0;
        self.refresh_agents().await;
    }

    /// Reload the agent list (built-ins + customs) from the daemon, keeping the cursor in range.
    async fn refresh_agents(&mut self) {
        match self
            .client
            .call_typed::<Vec<AgentChoice>>("agent.detect", None)
            .await
        {
            Ok(a) => self.agents = a,
            Err(e) => self.status = format!("agent.detect failed: {e}"),
        }
        if self.agents_selected >= self.agents.len() {
            self.agents_selected = self.agents.len().saturating_sub(1);
        }
    }

    /// Key handling for the agent manager: a list of agents plus an add/edit form.
    async fn load_settings(&mut self) {
        self.settings_idx = 0;
        self.settings_editing = false;
        self.load_agents().await; // populate nl_agents for the default-agent picker
        if let Ok(v) = self.client.call("config.get", None).await {
            self.apply_settings_value(&v);
        }
    }

    fn apply_settings_value(&mut self, v: &serde_json::Value) {
        let s = |x: &serde_json::Value| x.as_str().map(|t| t.to_string());
        if let Some(a) = v.get("accent") {
            self.settings.accent = s(a).unwrap_or_else(|| "cyan".to_string());
        }
        if let Some(d) = v.get("default_agent") {
            self.settings.default_agent = s(d).unwrap_or_default();
        }
        if let Some(b) = v.get("auto_continue").and_then(|x| x.as_bool()) {
            self.settings.auto_continue = b;
        }
        if let Some(m) = v.get("auto_continue_message").and_then(|x| x.as_str()) {
            self.settings.auto_continue_message = m.to_string();
        }
        if let Some(w) = v.get("worktree_template").and_then(|x| x.as_str()) {
            self.settings.worktree_template = w.to_string();
        }
        let b = |key: &str| v.get(key).and_then(|x| x.as_bool());
        if let Some(x) = b("spawn_prompt") {
            self.settings.spawn_prompt = x;
        }
        if let Some(x) = b("notify_enabled") {
            self.settings.notify_enabled = x;
        }
        if let Some(x) = b("notify_needs_you") {
            self.settings.notify_needs_you = x;
        }
        if let Some(x) = b("notify_rate_limited") {
            self.settings.notify_rate_limited = x;
        }
        if let Some(x) = b("notify_resumed") {
            self.settings.notify_resumed = x;
        }
        if let Some(x) = b("notify_idle") {
            self.settings.notify_idle = x;
        }
        if let Some(x) = b("notify_sound") {
            self.settings.notify_sound = x;
        }
        if let Some(x) = b("notify_show_why") {
            self.settings.notify_show_why = x;
        }
        if let Some(x) = b("notify_coalesce") {
            self.settings.notify_coalesce = x;
        }
        if let Some(x) = b("notify_click_focus") {
            self.settings.notify_click_focus = x;
        }
        if let Some(x) = b("notify_subagents") {
            self.settings.notify_subagents = x;
        }
        if let Some(x) = b("usage_probe") {
            self.settings.usage_probe = x;
        }
        if let Some(x) = b("expand_agents") {
            self.settings.expand_agents = x;
        }
        // Orchestrator overrides are `Option<String>` daemon-side; a null/absent value clears them.
        if let Some(a) = v.get("orchestrator_agent") {
            self.settings.orchestrator_agent = s(a).unwrap_or_default();
        }
        if let Some(m) = v.get("orchestrator_model") {
            self.settings.orchestrator_model = s(m).unwrap_or_default();
        }
    }

    /// Persist the current settings to the daemon config and apply the accent live.
    async fn save_settings(&mut self) {
        let default_agent = if self.settings.default_agent.is_empty() {
            serde_json::Value::Null
        } else {
            json!(self.settings.default_agent)
        };
        let params = json!({
            "accent": self.settings.accent,
            "default_agent": default_agent,
            "auto_continue": self.settings.auto_continue,
            "auto_continue_message": self.settings.auto_continue_message,
            "worktree_template": self.settings.worktree_template,
            "spawn_prompt": self.settings.spawn_prompt,
            "notify_enabled": self.settings.notify_enabled,
            "notify_needs_you": self.settings.notify_needs_you,
            "notify_rate_limited": self.settings.notify_rate_limited,
            "notify_resumed": self.settings.notify_resumed,
            "notify_idle": self.settings.notify_idle,
            "notify_sound": self.settings.notify_sound,
            "notify_show_why": self.settings.notify_show_why,
            "notify_coalesce": self.settings.notify_coalesce,
            "notify_click_focus": self.settings.notify_click_focus,
            "notify_subagents": self.settings.notify_subagents,
            "usage_probe": self.settings.usage_probe,
            "expand_agents": self.settings.expand_agents,
            // Empty string clears the override daemon-side (back to the account default).
            "orchestrator_agent": self.settings.orchestrator_agent,
            "orchestrator_model": self.settings.orchestrator_model,
        });
        match self.client.call("config.set", Some(params)).await {
            Ok(v) => self.apply_settings_value(&v),
            Err(e) => self.status = format!("settings save failed: {e}"),
        }
        // Re-theme the whole TUI from the new accent immediately.
        self.theme = Theme::from_accent(Some(&self.settings.accent));
    }

    /// The text field edited by the current row, if it's a text setting.
    fn settings_edit_field(&mut self) -> Option<&mut String> {
        match self.settings_idx {
            3 => Some(&mut self.settings.auto_continue_message),
            4 => Some(&mut self.settings.worktree_template),
            _ => None,
        }
    }

    async fn settings_key(&mut self, key: KeyEvent) {
        if self.settings_editing {
            match key.code {
                KeyCode::Char(c) => {
                    if let Some(f) = self.settings_edit_field() {
                        f.push(c);
                    }
                }
                KeyCode::Backspace => {
                    if let Some(f) = self.settings_edit_field() {
                        f.pop();
                    }
                }
                KeyCode::Enter => {
                    self.settings_editing = false;
                    self.save_settings().await;
                }
                KeyCode::Esc => {
                    self.settings_editing = false;
                    self.load_settings().await; // discard the in-progress edit
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_idx = self.settings_idx.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings_idx = (self.settings_idx + 1).min(SETTINGS_COUNT - 1)
            }
            KeyCode::Left => self.adjust_setting(false).await,
            KeyCode::Right => self.adjust_setting(true).await,
            KeyCode::Char(' ') | KeyCode::Enter => self.activate_setting().await,
            KeyCode::Esc => self.view = View::Fleet,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    /// Click on a settings row (from the mouse): select it and activate it.
    async fn settings_click(&mut self, row: u16) {
        let first = self.settings_geom.get();
        if row >= first {
            let idx = (row - first) as usize;
            if idx < SETTINGS_COUNT {
                self.settings_idx = idx;
                self.activate_setting().await;
            }
        }
    }

    async fn adjust_setting(&mut self, forward: bool) {
        match self.settings_idx {
            0 => {
                self.settings.accent = cycle(ACCENTS, &self.settings.accent, forward);
                self.save_settings().await;
            }
            1 => {
                let names: Vec<String> = self.nl_agents.iter().map(|a| a.name.clone()).collect();
                if !names.is_empty() {
                    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                    self.settings.default_agent =
                        cycle(&refs, &self.settings.default_agent, forward);
                    self.save_settings().await;
                }
            }
            2 => {
                self.settings.auto_continue = !self.settings.auto_continue;
                self.save_settings().await;
            }
            5 => {
                self.settings.spawn_prompt = !self.settings.spawn_prompt;
                self.save_settings().await;
            }
            6 => {
                self.settings.notify_enabled = !self.settings.notify_enabled;
                self.save_settings().await;
            }
            7 => {
                self.settings.notify_needs_you = !self.settings.notify_needs_you;
                self.save_settings().await;
            }
            8 => {
                self.settings.notify_rate_limited = !self.settings.notify_rate_limited;
                self.save_settings().await;
            }
            9 => {
                self.settings.notify_resumed = !self.settings.notify_resumed;
                self.save_settings().await;
            }
            10 => {
                self.settings.notify_idle = !self.settings.notify_idle;
                self.save_settings().await;
            }
            11 => {
                self.settings.notify_sound = !self.settings.notify_sound;
                // Preview the chime immediately so "is sound working?" is answered on the spot.
                if self.settings.notify_sound {
                    notify::play_chime();
                }
                self.save_settings().await;
            }
            12 => {
                self.settings.notify_show_why = !self.settings.notify_show_why;
                self.save_settings().await;
            }
            13 => {
                self.settings.notify_coalesce = !self.settings.notify_coalesce;
                self.save_settings().await;
            }
            14 => {
                self.settings.notify_click_focus = !self.settings.notify_click_focus;
                self.save_settings().await;
            }
            15 => {
                self.settings.notify_subagents = !self.settings.notify_subagents;
                self.save_settings().await;
            }
            16 => {
                self.settings.usage_probe = !self.settings.usage_probe;
                self.save_settings().await;
            }
            17 => {
                // Toggling changes the fleet row count, so re-anchor the cursor to the same
                // lane/agent afterward (else `selected` drifts to a different — or out-of-range — row).
                let keep = self.selected_lane().map(|l| l.id);
                let keep_ref = self.selected_session_ref();
                self.settings.expand_agents = !self.settings.expand_agents;
                self.save_settings().await;
                match keep {
                    Some(id) => self.select_lane_session(id, keep_ref),
                    None => self.clamp_selection(),
                }
            }
            18 => {
                // The orchestrator's Claude account: cycle "(default)" + claude variants + customs.
                let mut options: Vec<String> = vec![String::new()];
                options.extend(self.orchestrator_agent_choices());
                let refs: Vec<&str> = options.iter().map(|s| s.as_str()).collect();
                self.settings.orchestrator_agent =
                    cycle(&refs, &self.settings.orchestrator_agent, forward);
                self.save_settings().await;
            }
            19 => {
                self.settings.orchestrator_model =
                    cycle(ORCH_MODELS, &self.settings.orchestrator_model, forward);
                self.save_settings().await;
            }
            _ => {}
        }
    }

    /// The agent names the orchestrator can run under: Claude account variants, custom agents,
    /// and `codex` — the MCP-capable CLIs. Aider stays excluded (no MCP client, so it can't
    /// drive the fleet tools; the daemon would reject it anyway). Built from the `agent.detect`
    /// list already loaded into `nl_agents`.
    fn orchestrator_agent_choices(&self) -> Vec<String> {
        self.nl_agents
            .iter()
            .filter(|a| a.custom || a.name.starts_with("claude") || a.name == "codex")
            .map(|a| a.name.clone())
            .collect()
    }

    async fn activate_setting(&mut self) {
        match self.settings_idx {
            0..=2 | 5..=19 => self.adjust_setting(true).await,
            3..=4 => self.settings_editing = true,
            _ => {}
        }
    }

    /// Key handling for the Notifications history view: move the cursor, open or attach to
    /// the event's agent, dismiss one, clear all, or leave.
    fn notifications_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.notif_sel = self.notif_sel.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Clamp to the last event so the cursor can't run off the feed.
                let max = self.notifications.len().saturating_sub(1);
                self.notif_sel = (self.notif_sel + 1).min(max);
            }
            // Dismiss the selected event (the feed is newest-first; the deque is newest-last).
            KeyCode::Char('d') if !self.notifications.is_empty() => {
                let idx = self.notifications.len() - 1 - self.notif_sel;
                self.notifications.remove(idx);
                let max = self.notifications.len().saturating_sub(1);
                self.notif_sel = self.notif_sel.min(max);
            }
            KeyCode::Char('c') => {
                self.notifications.clear();
                self.notif_sel = 0;
            }
            KeyCode::Enter | KeyCode::Right => self.open_selected_notif(false),
            KeyCode::Char('t') => self.open_selected_notif(true),
            KeyCode::Esc | KeyCode::Left => self.view = View::Fleet,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    /// Open the lane behind the cursor's event in Focus, pointed at the exact session that
    /// fired (when the event recorded one). `attach` goes all the way into its tmux pane.
    fn open_selected_notif(&mut self, attach: bool) {
        let Some((lane_id, sid)) = self
            .notifications
            .iter()
            .rev()
            .nth(self.notif_sel)
            .map(|e| (e.lane_id, e.session_id.clone()))
        else {
            return;
        };
        self.jump_to_lane(lane_id);
        // jump_to_lane reports a stale lane via status; only proceed if it landed.
        if self.selected_lane().map(|l| l.id) != Some(lane_id) {
            return;
        }
        self.select_session(lane_id, sid.as_deref());
        if attach {
            self.attach_request = Some(lane_id);
        }
    }

    /// Point the session cursor at `session_id` on the selected lane (no-op when the session
    /// is gone or wasn't recorded — the lane's current selection stands).
    fn select_session(&mut self, lane_id: LaneId, session_id: Option<&str>) {
        let Some(sid) = session_id else { return };
        let idx = self.lanes.iter().find(|l| l.id == lane_id).and_then(|l| {
            l.agent_sessions
                .iter()
                .position(|s| s.session_id.as_deref() == Some(sid))
        });
        if let Some(i) = idx {
            self.session_lane = Some(lane_id); // keep sync_session_cursor from resetting it
            self.session_idx = i;
            // In expanded mode, also land the fleet cursor on that agent's row so the per-tick
            // cursor sync (which keys off the selected row) keeps it there.
            if self.settings.expand_agents {
                let sref = self
                    .lanes
                    .iter()
                    .find(|l| l.id == lane_id)
                    .and_then(|l| l.agent_sessions.get(i))
                    .and_then(agent_session_ref);
                self.select_lane_session(lane_id, sref);
            }
        }
    }

    async fn agents_key(&mut self, key: KeyEvent) {
        if self.ag_editing {
            match key.code {
                KeyCode::Tab | KeyCode::BackTab => {
                    self.ag_field = match self.ag_field {
                        AgField::Name => AgField::Command,
                        AgField::Command => AgField::Name,
                    };
                }
                KeyCode::Char(c) => match self.ag_field {
                    AgField::Name => self.ag_name.push(c),
                    AgField::Command => self.ag_command.push(c),
                },
                KeyCode::Backspace => {
                    match self.ag_field {
                        AgField::Name => self.ag_name.pop(),
                        AgField::Command => self.ag_command.pop(),
                    };
                }
                KeyCode::Enter => self.submit_agent_form().await,
                KeyCode::Esc => self.ag_editing = false,
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.agents_selected = self.agents_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') if self.agents_selected + 1 < self.agents.len() => {
                self.agents_selected += 1
            }
            KeyCode::Char('n') => self.begin_agent_edit(true),
            KeyCode::Char('e') | KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                self.begin_agent_edit(false)
            }
            KeyCode::Char('d') | KeyCode::Char('x') => self.delete_selected_agent().await,
            KeyCode::Char('*') | KeyCode::Char(' ') => self.toggle_default_agent().await,
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                let back = self.agents_return.take().unwrap_or(View::Fleet);
                self.view = back;
                if back == View::NewLane {
                    // Reflect manager changes in the picker but keep the user's current pick
                    // (load_agents would otherwise snap back to the default).
                    let prev = self
                        .nl_agents
                        .get(self.nl_agent_idx)
                        .map(|a| a.name.clone());
                    self.load_agents().await;
                    if let Some(name) = prev {
                        if let Some(i) = self.nl_agents.iter().position(|a| a.name == name) {
                            self.nl_agent_idx = i;
                        }
                    }
                }
            }
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    /// Begin adding a new custom agent, or editing the selected one (built-ins are read-only).
    fn begin_agent_edit(&mut self, is_new: bool) {
        if is_new {
            self.ag_editing = true;
            self.ag_is_new = true;
            self.ag_field = AgField::Name;
            self.ag_name.clear();
            self.ag_command.clear();
            self.ag_orig = None;
            return;
        }
        match self.agents.get(self.agents_selected) {
            Some(a) if a.custom => {
                self.ag_editing = true;
                self.ag_is_new = false;
                self.ag_field = AgField::Command;
                self.ag_name = a.name.clone();
                self.ag_command = a.command.clone();
                self.ag_orig = Some(a.name.clone());
            }
            Some(_) => {
                self.status = "built-in agents are read-only — press n to add a custom one".into()
            }
            None => {}
        }
    }

    /// Save the add/edit form: persist via `agent.add`, then drop the old entry on a rename.
    /// Adding *first* means a rejected save (e.g. an invalid name) leaves the original intact.
    /// A rename that was the default carries the default over to the new name.
    async fn submit_agent_form(&mut self) {
        let name = self.ag_name.trim().to_string();
        let command = self.ag_command.trim().to_string();
        if name.is_empty() || command.is_empty() {
            self.status = "name and command are required".into();
            return;
        }
        // Was the agent being edited the default? (Checked before we mutate anything.)
        let orig_was_default = self
            .ag_orig
            .as_ref()
            .and_then(|o| self.agents.iter().find(|a| &a.name == o))
            .map(|a| a.default)
            .unwrap_or(false);

        match self
            .client
            .call(
                "agent.add",
                Some(json!({ "name": name, "command": command })),
            )
            .await
        {
            Ok(_) => {
                self.ag_editing = false;
                self.status = format!("saved agent {name}");
                // On a rename the new entry now exists, so the old one is safe to remove.
                if let Some(orig) = self.ag_orig.clone() {
                    if orig != name {
                        match self
                            .client
                            .call("agent.remove", Some(json!({ "name": orig })))
                            .await
                        {
                            // Carry the default over to the renamed agent.
                            Ok(_) if orig_was_default => {
                                let _ = self
                                    .client
                                    .call("agent.set_default", Some(json!({ "name": name })))
                                    .await;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                self.status =
                                    format!("saved {name}, but removing old '{orig}' failed: {e}")
                            }
                        }
                    }
                }
                self.refresh_agents().await;
                if let Some(i) = self.agents.iter().position(|a| a.name == name) {
                    self.agents_selected = i;
                }
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    async fn delete_selected_agent(&mut self) {
        match self.agents.get(self.agents_selected).cloned() {
            Some(a) if a.custom => {
                match self
                    .client
                    .call("agent.remove", Some(json!({ "name": a.name })))
                    .await
                {
                    Ok(_) => {
                        self.status = format!("removed {}", a.name);
                        self.refresh_agents().await;
                    }
                    Err(e) => self.status = format!("remove failed: {e}"),
                }
            }
            Some(_) => self.status = "built-in agents can't be removed".into(),
            None => {}
        }
    }

    /// Toggle the selected agent as the default (preselected in New Lane).
    async fn toggle_default_agent(&mut self) {
        let (name, is_default) = match self.agents.get(self.agents_selected) {
            Some(a) => (a.name.clone(), a.default),
            None => return,
        };
        let new_default = if is_default { None } else { Some(name.clone()) };
        match self
            .client
            .call("agent.set_default", Some(json!({ "name": new_default })))
            .await
        {
            Ok(_) => {
                self.status = match &new_default {
                    Some(n) => format!("default agent: {n}"),
                    None => "cleared default agent".into(),
                };
                self.refresh_agents().await;
            }
            Err(e) => self.status = format!("set default failed: {e}"),
        }
    }

    async fn set_zoom(&mut self, zoom: Zoom) {
        self.timeline_zoom = zoom;
        self.load_timeline().await;
    }

    async fn load_timeline(&mut self) {
        let (lookback, min_bucket, _) = self.timeline_zoom.params();
        // Size buckets to the terminal so the strip fills the width (the renderer resamples to
        // cover the gap until a resize refetch lands). 26 ≈ repo-label column + margins.
        let width = ratatui::crossterm::terminal::size()
            .map(|(w, _)| w as i64)
            .unwrap_or(100);
        let buckets = (width - 26).clamp(24, 240);
        let bucket = (lookback / buckets).max(min_bucket);
        let to = chrono::Utc::now();
        let from = to - chrono::Duration::seconds(lookback);
        let params = json!({
            "from_iso": from.to_rfc3339(),
            "to_iso": to.to_rfc3339(),
            "bucket_secs": bucket,
        });
        match self
            .client
            .call_typed::<TimelineData>("timeline", Some(params))
            .await
        {
            Ok(t) => self.timeline = Some(t),
            Err(e) => self.status = format!("timeline failed: {e}"),
        }
    }

    async fn load_sessions(&mut self) {
        let to = chrono::Utc::now();
        let from = to - chrono::Duration::days(7);
        let params = json!({ "from_iso": from.to_rfc3339(), "to_iso": to.to_rfc3339() });
        match self
            .client
            .call_typed::<Vec<WorkSession>>("sessions", Some(params))
            .await
        {
            Ok(s) => self.sessions = s,
            Err(e) => self.status = format!("sessions failed: {e}"),
        }
    }

    async fn run_search(&mut self) {
        if self.search_query.trim().is_empty() {
            self.search_results.clear();
            return;
        }
        let params = json!({ "query": self.search_query, "limit": 100 });
        match self
            .client
            .call_typed::<Vec<Commit>>("commit.search", Some(params))
            .await
        {
            Ok(r) => self.search_results = r,
            Err(e) => self.status = format!("search failed: {e}"),
        }
    }

    fn export_sessions(&mut self) {
        let mut md = String::from("# repomon work sessions\n\n");
        for s in &self.sessions {
            let from = s
                .from
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M");
            let to = s.to.with_timezone(&chrono::Local).format("%H:%M");
            md.push_str(&format!(
                "- **{from} – {to}** ({} min, {:?}) — {} · {} commits\n",
                s.duration_minutes(),
                s.kind,
                s.repo_names.join(", "),
                s.commit_count
            ));
        }
        let path = std::env::current_dir()
            .unwrap_or_default()
            .join("repomon-sessions.md");
        match std::fs::write(&path, md) {
            Ok(_) => {
                self.status = format!(
                    "exported {} sessions to {}",
                    self.sessions.len(),
                    path.display()
                )
            }
            Err(e) => self.status = format!("export failed: {e}"),
        }
    }

    /// Make `id` the active lane: move the Fleet cursor to it and, in Grid, the tile cursor too,
    /// so `selected_lane()` (which key-forwarding targets) points at it.
    fn activate_lane(&mut self, id: LaneId, session: Option<usize>) {
        let sref = match session {
            Some(s) => self
                .visible_lanes()
                .iter()
                .find(|l| l.id == id)
                .and_then(|l| l.agent_sessions.get(s))
                .and_then(agent_session_ref),
            None => None,
        };
        self.select_lane_session(id, sref);
        if self.view == View::Grid {
            if let Some(gi) = self.grid_lane_ids().iter().position(|&l| l == id) {
                self.grid_active = gi;
            }
        }
    }

    /// Update the hovered lane from the mouse position by hit-testing the recorded click zones.
    fn update_hover(&mut self, col: u16, row: u16) {
        self.hover_lane = self
            .click_zones
            .borrow()
            .iter()
            .find(|z| z.rect.contains(Position { x: col, y: row }))
            .map(|z| z.lane);
    }

    /// A left-click in Grid/Fleet/Split. Hit-test the lane regions recorded during render: a click
    /// inside one focuses that lane (single-click → type in place for interactive zones; another
    /// click within `DOUBLE_CLICK` → open its real terminal). A click on empty space blurs.
    async fn handle_click(&mut self, col: u16, row: u16) {
        // The pinned "repomind" row isn't a lane click-zone; hit-test it first: a click selects it
        // and opens the command-center view.
        if let Some(rect) = self.orch_click.get() {
            if rect.contains(Position { x: col, y: row }) {
                self.selected = 0;
                self.open_orchestrator().await;
                return;
            }
        }
        // In Split with the pinned row selected, the right column previews repomind; a click there
        // opens the full command-center.
        if let Some(rect) = self.orch_pane_zone.get() {
            if rect.contains(Position { x: col, y: row }) {
                self.open_orchestrator().await;
                return;
            }
        }
        let hit = self
            .click_zones
            .borrow()
            .iter()
            .find(|z| z.rect.contains(Position { x: col, y: row }))
            .copied();
        // Clicking a lane (or the gutter) leaves any repomind insert mode, since the selection is
        // moving off the pinned row.
        self.orch_insert = false;
        match hit {
            Some(z) => {
                let now = std::time::Instant::now();
                let dbl = is_double_click(self.last_click, z.lane, now);
                self.last_click = Some((now, z.lane));
                self.activate_lane(z.lane, z.session);
                if dbl {
                    self.focus_insert = false;
                    self.attach_request = Some(z.lane); // double-click → real terminal
                } else {
                    self.focus_insert = z.interactive; // single-click → type in place / select
                }
            }
            None => {
                self.focus_insert = false; // clicked the gutter → blur
                self.last_click = None;
            }
        }
    }

    /// Scroll-wheel navigation: move the selection (or the grid cursor) up/down.
    fn handle_mouse(&mut self, me: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::MouseEventKind;
        let down = match me.kind {
            MouseEventKind::ScrollDown => true,
            MouseEventKind::ScrollUp => false,
            _ => return,
        };
        if self.view == View::Grid {
            let n = self.grid_lane_ids().len();
            if down && self.grid_active + 1 < n {
                self.grid_active += 1;
            } else if !down {
                self.grid_active = self.grid_active.saturating_sub(1);
            }
        } else {
            let n = self.rows_len();
            if down && n > 0 && self.selected + 1 < n {
                self.selected += 1;
            } else if !down {
                self.selected = self.selected.saturating_sub(1);
            }
        }
    }

    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) => self.filter.push(c),
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Enter => self.filtering = false,
            KeyCode::Esc => {
                self.filtering = false;
                self.filter.clear();
            }
            _ => {}
        }
        self.clamp_selection();
    }

    async fn new_lane_key(&mut self, key: KeyEvent) {
        let repos = self.repos.len();
        match key.code {
            KeyCode::Up if repos > 0 => self.nl_repo_idx = (self.nl_repo_idx + repos - 1) % repos,
            KeyCode::Down if repos > 0 => self.nl_repo_idx = (self.nl_repo_idx + 1) % repos,
            KeyCode::Tab if !self.nl_agents.is_empty() => {
                self.nl_agent_idx = (self.nl_agent_idx + 1) % self.nl_agents.len()
            }
            KeyCode::BackTab if !self.nl_agents.is_empty() => {
                let n = self.nl_agents.len();
                self.nl_agent_idx = (self.nl_agent_idx + n - 1) % n;
            }
            // Ctrl+A jumps to the agent manager and returns here afterwards (a plain `a`
            // types into the branch name).
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.enter_agents(Some(View::NewLane)).await;
            }
            KeyCode::Char(c) => self.nl_branch.push(c),
            KeyCode::Backspace => {
                self.nl_branch.pop();
            }
            KeyCode::Esc => self.view = View::Fleet,
            KeyCode::Enter => self.submit_new_lane().await,
            _ => {}
        }
    }

    /// Auto-start the orchestrator session (idempotent) and begin watching its pane when entering
    /// the command-center view. `sync_viewport` flips the daemon's watch flag the same loop.
    async fn load_orchestrator(&mut self) {
        self.reset_scroll();
        self.orch_insert = false;
        self.orch_restart_armed = false;
        match self
            .client
            .call("orchestrator.start", Some(json!({})))
            .await
        {
            Ok(v) => self.apply_orchestrator_status(&v),
            Err(e) => self.status = format!("repomind start failed: {e}"),
        }
    }

    /// Open the command-center view (used by the pinned-row click). The keyboard path goes through
    /// `apply(Action::Goto)`.
    async fn open_orchestrator(&mut self) {
        self.view = View::Orchestrator;
        self.load_orchestrator().await;
    }

    /// Leave the command-center view back to Fleet; `sync_viewport` stops the pane stream.
    async fn leave_orchestrator(&mut self) {
        self.reset_scroll();
        self.orch_insert = false;
        self.orch_restart_armed = false;
        self.view = View::Fleet;
    }

    /// Restart repomind with the saved settings: stop the live session, then start fresh — so a
    /// changed `repomind agent` (backend) / model applies without leaving the TUI. Autonomy
    /// resets to the default, exactly like the view's idempotent auto-start (`load_orchestrator`)
    /// — the daemon's `orchestrator.start` re-reads `orchestrator_agent`/`orchestrator_model`
    /// from config on a genuine spawn.
    async fn restart_orchestrator(&mut self) {
        self.orch_restart_armed = false;
        if let Err(e) = self.client.call("orchestrator.stop", None).await {
            self.status = format!("repomind stop failed: {e}");
            return;
        }
        match self
            .client
            .call("orchestrator.start", Some(json!({})))
            .await
        {
            Ok(v) => {
                self.apply_orchestrator_status(&v);
                let agent = v
                    .get("agent")
                    .and_then(|a| a.as_str())
                    .unwrap_or("claude (default)");
                self.status = format!("repomind restarted · agent: {agent}");
            }
            Err(e) => self.status = format!("repomind restart failed: {e}"),
        }
    }

    /// Key handling in the command-center view: INSERT forwards to the orchestrator (mirrors
    /// `focus_key`/`split_key`); `↵`/`→` attaches into its real tmux pane; `i` types in place.
    async fn orchestrator_key(&mut self, key: KeyEvent) {
        if self.orch_insert {
            if leaves_insert(&key) {
                self.orch_insert = false;
                return;
            }
            // PgUp/PgDn scroll the captured pane even while typing (always reach repomon).
            match key.code {
                KeyCode::PageUp => {
                    self.scroll = self.scroll.saturating_add(8);
                    return;
                }
                KeyCode::PageDown => {
                    self.scroll = self.scroll.saturating_sub(8);
                    return;
                }
                _ => {}
            }
            if self.scroll > 0 {
                self.reset_scroll();
            }
            self.send_orch_key(key).await;
            return;
        }
        // Any key other than the confirming second `r` disarms a pending restart (mirrors the
        // Fleet view's `X X` repo-removal confirm).
        let restart_armed = std::mem::take(&mut self.orch_restart_armed);
        match key.code {
            // ↵ / → "go all the way in" = attach to repomind's real tmux pane.
            KeyCode::Enter | KeyCode::Right => {
                self.reset_scroll();
                self.attach_orchestrator().await;
            }
            // `i` types straight to repomind without leaving the view (mediated send-keys).
            KeyCode::Char('i') => {
                self.reset_scroll();
                self.orch_insert = true;
            }
            // `r r` restarts repomind with the saved settings — how a `repomind agent` (backend)
            // or model change in Settings gets applied to a live session without leaving the TUI.
            KeyCode::Char('r') if restart_armed => self.restart_orchestrator().await,
            KeyCode::Char('r') => {
                self.orch_restart_armed = true;
                self.status =
                    "restart repomind with saved settings? it ends the live session — press r \
                     again to confirm"
                        .to_string();
            }
            KeyCode::PageUp | KeyCode::Up => self.scroll = self.scroll.saturating_add(8),
            KeyCode::PageDown | KeyCode::Down => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::Esc | KeyCode::Left if self.scroll > 0 => self.reset_scroll(),
            KeyCode::Esc | KeyCode::Left => self.leave_orchestrator().await,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    /// Forward one keystroke to the orchestrator window (mirrors `send_agent_key`, but the
    /// orchestrator is a singleton so there's no lane/window to target).
    async fn send_orch_key(&mut self, key: KeyEvent) {
        let Some((spec, literal)) = translate_key(&key) else {
            return;
        };
        if literal {
            // Buffer printables; the event loop flushes the run as one `orchestrator.send_input`.
            self.pending_input.push_str(&spec);
            return;
        }
        // A control key: send any buffered text first so order holds, then the key.
        self.flush_pending_input().await;
        if let Err(e) = self
            .client
            .call(
                "orchestrator.key",
                Some(json!({ "key": spec, "literal": false })),
            )
            .await
        {
            self.status = format!("repomind: {e}");
        }
    }

    /// Resolve the orchestrator's tmux target and queue a full `tmux attach` (reuses the generic
    /// `do_attach_target` suspend/reinit path).
    async fn attach_orchestrator(&mut self) {
        self.flush_pending_input().await;
        match self.client.call("orchestrator.target", None).await {
            Ok(v) => {
                let target = v
                    .get("target")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let available = v
                    .get("available")
                    .and_then(|a| a.as_bool())
                    .unwrap_or(false);
                if available && !target.is_empty() {
                    self.attach_target = Some(target);
                } else {
                    self.status = "repomind isn't running yet".into();
                }
            }
            Err(e) => self.status = format!("attach failed: {e}"),
        }
    }

    /// A left-click in the command-center view: a needs-you lane in the left summary jumps to that
    /// lane (Focus); the right column focuses repomind for typing, and a double-click attaches into
    /// its real terminal. The click rects are registered each frame by `render_orchestrator`.
    async fn orchestrator_click(&mut self, col: u16, row: u16) {
        let pos = Position { x: col, y: row };
        let lane = self
            .click_zones
            .borrow()
            .iter()
            .find(|z| z.rect.contains(pos))
            .map(|z| z.lane);
        if let Some(id) = lane {
            self.jump_to_lane(id); // selects the lane and opens it in Focus
            return;
        }
        if let Some(rect) = self.orch_pane_zone.get() {
            if rect.contains(pos) {
                let now = std::time::Instant::now();
                let dbl = self
                    .orch_pane_last_click
                    .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK);
                self.orch_pane_last_click = Some(now);
                if dbl {
                    self.orch_insert = false;
                    self.attach_orchestrator().await; // double-click → full attach
                } else {
                    self.orch_insert = true; // single-click → type to repomind
                }
                return;
            }
        }
        self.orch_insert = false; // clicked the gutter → blur
    }

    /// Forward one keystroke live to the selected lane's agent (insert-mode passthrough),
    /// so its own UI works (Shift+Tab cycles modes, arrows navigate menus, Ctrl-C interrupts).
    async fn send_agent_key(&mut self, key: KeyEvent) {
        let (Some(id), Some((spec, literal))) =
            (self.selected_lane().map(|l| l.id), translate_key(&key))
        else {
            return;
        };
        if literal {
            // A printable character — buffer it. A whole paste accumulates here and the event loop
            // flushes it as a single send_input, instead of one blocking send-keys per character.
            self.pending_input.push_str(&spec);
            return;
        }
        // A control key (Enter, arrow, ^C, Esc, …): send any buffered text first so order holds.
        self.flush_pending_input().await;
        let _ = self
            .client
            .call(
                "agent.key",
                Some(json!({
                    "lane_id": id,
                    "key": spec,
                    "literal": false,
                    "window": self.selected_window(),
                })),
            )
            .await;
    }

    /// Send the buffered keystrokes/paste to the agent as one `send_input` (literal, no Enter).
    async fn flush_pending_input(&mut self) {
        if self.pending_input.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending_input);
        // The command-center, or the Split preview while typing to repomind (`orch_insert`), targets
        // the orchestrator window rather than a lane.
        if self.view == View::Orchestrator || self.orch_insert {
            if let Err(e) = self
                .client
                .call(
                    "orchestrator.send_input",
                    Some(json!({ "text": text, "enter": false })),
                )
                .await
            {
                self.status = format!("repomind: {e}");
            }
            return;
        }
        let Some(id) = self.selected_lane().map(|l| l.id) else {
            return;
        };
        let _ = self
            .client
            .call(
                "agent.send_input",
                Some(json!({
                    "lane_id": id,
                    "text": text,
                    "enter": false,
                    "window": self.selected_window(),
                })),
            )
            .await;
    }

    /// Key handling in the Split view: fleet sidebar + the selected lane's live output. Like
    /// Focus, `i` enters insert mode to type straight to the agent here (esc returns); ↵/→
    /// still zooms into full-screen Focus.
    async fn split_key(&mut self, key: KeyEvent) {
        // Typing to repomind from the Split preview (the pinned row is selected): mirror a lane's
        // insert mode, but forward keystrokes to the orchestrator. `^O` leaves insert.
        if self.orch_insert {
            if leaves_insert(&key) {
                self.orch_insert = false;
                return;
            }
            self.send_orch_key(key).await;
            return;
        }
        if self.focus_insert {
            if leaves_insert(&key) {
                self.focus_insert = false;
                return;
            }
            // PgUp/PgDn scroll the captured output even while typing (always reach repomon); any
            // other key returns to the live tail and goes to the agent.
            match key.code {
                KeyCode::PageUp => return self.pane_scroll(true, 8),
                KeyCode::PageDown => return self.pane_scroll(false, 8),
                _ => {}
            }
            if self.scroll > 0 {
                self.reset_scroll();
            }
            self.send_agent_key(key).await;
            return;
        }
        if self.filtering {
            self.filter_key(key);
            return;
        }
        // The pinned repomind row has no lane: `i` quick-types to repomind (exactly like `i` on a
        // selected lane), and `↵`/`→` open the full command-center.
        if self.orchestrator_selected() {
            match key.code {
                KeyCode::Char('i') => {
                    self.reset_scroll();
                    self.orch_insert = true;
                    return;
                }
                KeyCode::Enter | KeyCode::Right => {
                    self.open_orchestrator().await;
                    return;
                }
                _ => {}
            }
        }
        // esc returns to the live tail before it would zoom out.
        if key.code == KeyCode::Esc && self.scroll > 0 {
            self.reset_scroll();
            return;
        }
        // ↵ opens the selected agent in its real tmux pane (a native terminal); → zooms to the
        // Focus monitor; `i` is a quick mediated type without leaving repomon.
        if key.code == KeyCode::Enter {
            self.reset_scroll();
            self.attach_request = self.selected_lane().map(|l| l.id);
            return;
        }
        if key.code == KeyCode::Char('i') {
            self.reset_scroll();
            self.focus_insert = true;
            return;
        }
        // `R` renames the selected agent sub-row (the editor shows in the sidebar tree).
        if key.code == KeyCode::Char('R') {
            self.start_rename();
            return;
        }
        match key.code {
            KeyCode::Tab => return self.cycle_session(true),
            KeyCode::BackTab => return self.cycle_session(false),
            // PgUp/PgDn scroll the pane; arrows still navigate the fleet.
            KeyCode::PageUp => return self.pane_scroll(true, 8),
            KeyCode::PageDown => return self.pane_scroll(false, 8),
            _ => {}
        }
        if let Some(action) = keybinds::nav(key) {
            self.apply(action).await;
        }
    }

    /// Key handling in the Focus view: command mode + insert (live passthrough to the agent).
    async fn focus_key(&mut self, key: KeyEvent) {
        if self.focus_insert {
            if leaves_insert(&key) {
                self.focus_insert = false;
                return;
            }
            // Scroll the captured history even while typing (the wheel is unreliable through
            // tmux/terminals; these keys always reach repomon). Other keys go to the agent, and
            // typing returns to the live tail.
            match key.code {
                KeyCode::PageUp => return self.pane_scroll(true, 8),
                KeyCode::PageDown => return self.pane_scroll(false, 8),
                _ => {}
            }
            if self.scroll > 0 {
                self.reset_scroll();
            }
            self.send_agent_key(key).await;
            return;
        }
        match key.code {
            // ↵ / → "go all the way in" = attach to the agent's real tmux pane: a genuine
            // terminal with native scroll, selection/copy, and image paste. `a` is an alias.
            KeyCode::Enter | KeyCode::Right => {
                self.reset_scroll();
                self.attach_request = self.selected_lane().map(|l| l.id);
            }
            // `i` is the lightweight alternative: type to the agent without leaving repomon's
            // chrome (mediated send-keys — handy for a quick one-liner).
            KeyCode::Char('i') => {
                self.reset_scroll();
                self.focus_insert = true;
            }
            // Scroll back through the agent's output (e.g. to read a long plan).
            KeyCode::PageUp | KeyCode::Up => self.pane_scroll(true, 8),
            KeyCode::PageDown | KeyCode::Down => self.pane_scroll(false, 8),
            KeyCode::Tab => self.cycle_session(true),
            KeyCode::BackTab => self.cycle_session(false),
            KeyCode::Char('e') => self.spawn_agent().await,
            KeyCode::Char('o') => self.adopt_agent().await,
            KeyCode::Char('t') => self.open_terminal().await,
            KeyCode::Char('T') => self.attach_latest_terminal().await,
            KeyCode::Char('s') => self.stop_agent().await,
            KeyCode::Char('a') => self.attach_request = self.selected_lane().map(|l| l.id),
            KeyCode::Char('m') => self.merge_lane().await,
            KeyCode::Char('c') => self.cd_to_lane(),
            KeyCode::Char('v') => self.paste_image().await,
            KeyCode::Char('y') => self.toggle_mouse(),
            // Triage without leaving Focus: retarget to the next agent needing you, or pull up
            // the lane switcher.
            KeyCode::Char('g') => {
                self.reset_scroll();
                self.jump_attention();
            }
            KeyCode::Char('f') => self.enter_lane_jump(),
            // esc/← stops scrolling first, then leaves to Split.
            KeyCode::Esc | KeyCode::Left if self.scroll > 0 => self.reset_scroll(),
            KeyCode::Esc | KeyCode::Left => self.view = View::Split,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    async fn submit_new_lane(&mut self) {
        let repo = self.repos.get(self.nl_repo_idx).cloned();
        match repo {
            None => self.status = "no repos registered — add one with `repomon add <path>`".into(),
            Some(_) if self.nl_branch.is_empty() => self.status = "enter a branch name".into(),
            Some(repo) => {
                let params = json!({
                    "repo_id": repo.id,
                    "branch": self.nl_branch,
                    "source_branch": null,
                    "copy_files": [],
                });
                let agent = self
                    .nl_agents
                    .get(self.nl_agent_idx)
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| "claude-code".to_string());
                match self.client.call("lane.create", Some(params)).await {
                    Ok(lane) => {
                        // Spin up the chosen agent in the new lane straight away.
                        if let Some(id) = lane.get("id").and_then(|v| v.as_i64()) {
                            let _ = self
                                .client
                                .call(
                                    "agent.spawn",
                                    Some(json!({ "lane_id": id, "agent": agent })),
                                )
                                .await;
                        }
                        self.status = format!("created lane {} + spawned {agent}", self.nl_branch);
                        self.view = View::Fleet;
                        self.refresh().await;
                    }
                    Err(e) => self.status = format!("create failed: {e}"),
                }
            }
        }
    }

    async fn spawn_agent(&mut self) {
        let Some(id) = self.selected_lane().map(|l| l.id) else {
            return;
        };
        // With the picker on (default), `e` prompts for which agent every time; otherwise it
        // spawns the configured default immediately.
        if self.settings.spawn_prompt {
            self.enter_spawn_pick(id).await;
        } else {
            let agent = self.default_agent_name().await;
            self.do_spawn(id, &agent).await;
        }
    }

    /// Open the quick agent picker for `lane`, remembering where to return on cancel.
    async fn enter_spawn_pick(&mut self, lane: LaneId) {
        self.load_agents().await; // populates nl_agents (+ marks the default)
        self.spawn_pick_idx = self.nl_agents.iter().position(|a| a.default).unwrap_or(0);
        self.spawn_pick_lane = Some(lane);
        self.spawn_return = Some(self.view);
        self.view = View::SpawnPick;
    }

    /// Spawn `agent` into `lane`: on success drop into Focus on the *new* agent, ready to drive
    /// it. The daemon surfaces the new window right away (a window-only placeholder until its
    /// transcript lands), so an immediate refresh plus the pending-focus intent put the cursor on
    /// it instead of leaving you on the lane's existing agent.
    async fn do_spawn(&mut self, id: LaneId, agent: &str) {
        match self
            .client
            .call(
                "agent.spawn",
                Some(json!({ "lane_id": id, "agent": agent })),
            )
            .await
        {
            Ok(v) => {
                self.status = format!("spawned {agent}");
                self.view = View::Focus;
                self.focus_insert = false;
                if let Some(window) = v.get("window").and_then(|w| w.as_str()) {
                    self.pending_focus_window = Some((id, window.to_string()));
                    self.pending_focus_ticks = 0;
                }
                self.refresh_lanes().await;
            }
            Err(e) => self.status = format!("spawn failed: {e}"),
        }
    }

    /// Key handling for the spawn picker: move, jump by number, spawn, or cancel.
    async fn spawn_pick_key(&mut self, key: KeyEvent) {
        let n = self.nl_agents.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.spawn_pick_idx = (self.spawn_pick_idx + n - 1) % n;
            }
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                self.spawn_pick_idx = (self.spawn_pick_idx + 1) % n;
            }
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let i = (c as usize) - ('1' as usize);
                if i < n {
                    self.spawn_pick_idx = i;
                    self.confirm_spawn_pick().await;
                }
            }
            KeyCode::Enter | KeyCode::Right => self.confirm_spawn_pick().await,
            KeyCode::Esc | KeyCode::Left => {
                self.view = self.spawn_return.take().unwrap_or(View::Fleet);
            }
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    /// Spawn the highlighted agent into the remembered lane.
    async fn confirm_spawn_pick(&mut self) {
        let agent = self
            .nl_agents
            .get(self.spawn_pick_idx)
            .map(|a| a.name.clone());
        let lane = self.spawn_pick_lane.take();
        self.spawn_return = None;
        match (agent, lane) {
            (Some(agent), Some(id)) => self.do_spawn(id, &agent).await,
            _ => self.view = View::Fleet,
        }
    }

    /// The configured default agent's name (what New Lane preselects), falling back to claude.
    async fn default_agent_name(&self) -> String {
        self.client
            .call_typed::<Vec<AgentChoice>>("agent.detect", None)
            .await
            .ok()
            .and_then(|v| v.into_iter().find(|a| a.default).map(|a| a.name))
            .unwrap_or_else(|| "claude-code".to_string())
    }

    /// Adopt the highlighted external agent (one running in another terminal): the daemon
    /// resumes that exact session — against the right Claude account — in a managed tmux lane.
    async fn adopt_agent(&mut self) {
        let idx = self.session_idx;
        let target = self.selected_lane().and_then(|l| {
            l.agent_sessions
                .get(idx)
                .filter(|s| s.external)
                .map(|s| (l.id, s.session_id.clone()))
        });
        let (id, session_id) = match target {
            Some(t) => t,
            None => {
                self.status = "select an external agent to adopt (tab to switch)".into();
                return;
            }
        };
        let mut params = json!({ "lane_id": id });
        if let Some(sid) = session_id {
            params["session_id"] = json!(sid);
        }
        match self.client.call("agent.adopt", Some(params)).await {
            Ok(_) => {
                self.status = "adopted — resuming in repomon (close the original terminal)".into();
                self.view = View::Focus;
                self.focus_insert = false;
                self.refresh().await;
            }
            Err(e) => self.status = format!("adopt failed: {e}"),
        }
    }

    /// Open a fresh shell terminal in the selected lane's worktree and attach to it. The loop
    /// performs the actual attach (it owns the terminal handle).
    async fn open_terminal(&mut self) {
        let Some(id) = self.selected_lane().map(|l| l.id) else {
            return;
        };
        match self
            .client
            .call("terminal.open", Some(json!({ "lane_id": id })))
            .await
        {
            Ok(v) => {
                if let Some(t) = v.get("target").and_then(|t| t.as_str()) {
                    self.attach_target = Some(t.to_string());
                }
                self.terminals_lane = None; // refetch the list after we return
            }
            Err(e) => self.status = format!("terminal failed: {e}"),
        }
    }

    /// Re-attach to the most recent open terminal for the selected lane, or open one if none.
    async fn attach_latest_terminal(&mut self) {
        let Some(id) = self.selected_lane().map(|l| l.id) else {
            return;
        };
        let terms = self
            .client
            .call_typed::<Vec<String>>("terminal.list", Some(json!({ "lane_id": id })))
            .await
            .unwrap_or_default();
        match terms.last() {
            Some(name) => {
                let resp = self
                    .client
                    .call("terminal.target", Some(json!({ "id": name })))
                    .await;
                if let Ok(v) = resp {
                    if let Some(t) = v.get("target").and_then(|t| t.as_str()) {
                        self.attach_target = Some(t.to_string());
                    }
                }
            }
            None => self.open_terminal().await,
        }
    }

    /// Refresh the selected lane's open terminals when the selection changes.
    async fn sync_terminals(&mut self) {
        let sel = self.selected_lane().map(|l| l.id);
        if sel == self.terminals_lane {
            return;
        }
        self.terminals_lane = sel;
        self.terminals.clear();
        if let Some(id) = sel {
            if let Ok(t) = self
                .client
                .call_typed::<Vec<String>>("terminal.list", Some(json!({ "lane_id": id })))
                .await
            {
                self.terminals = t;
            }
        }
    }

    /// `c`: cd the parent shell into the selected worktree on exit. This only works through the
    /// `repomon` shell function (which sets `REPOMON_CD_FD`); without it, quitting just to print
    /// a path is surprising, so we no-op with a hint instead of exiting.
    fn cd_to_lane(&mut self) {
        let Some(path) = self.selected_lane().map(|l| l.worktree.path.clone()) else {
            return;
        };
        if std::env::var_os("REPOMON_CD_FD").is_some() {
            self.cd_target = Some(path);
            self.should_quit = true;
        } else {
            self.status =
                "cd-on-exit needs the `repomon` shell function (see README) — not active".into();
        }
    }

    /// Paste a clipboard image into the focused agent: save it to a temp file and insert the
    /// path into the agent's input (Claude reads images referenced by path). For native paste,
    /// `a` attach + ⌘V is the real-terminal route.
    async fn paste_image(&mut self) {
        let Some(id) = self.selected_lane().map(|l| l.id) else {
            return;
        };
        match clipboard_image_to_file() {
            Some(path) => {
                let _ = self
                    .client
                    .call(
                        "agent.send_input",
                        Some(json!({
                            "lane_id": id,
                            "text": format!("{path} "),
                            "enter": false,
                            "window": self.selected_window(),
                        })),
                    )
                    .await;
                self.reset_scroll();
                self.focus_insert = true;
                self.status = format!("pasted image → {path}");
            }
            None => {
                self.status = "no image in the clipboard (a attach + ⌘V for native paste)".into()
            }
        }
    }

    /// Toggle whether repomon captures the mouse. Off → the terminal owns the mouse, so you can
    /// drag-select and copy the rendered output (and use the terminal's own scrollback).
    fn toggle_mouse(&mut self) {
        self.mouse_on = !self.mouse_on;
        if self.mouse_on {
            enable_mouse();
            self.status =
                "mouse captured — wheel scrolls/navigates · drag-select off (y to release)".into();
        } else {
            disable_mouse();
            self.status = "mouse released — drag to select & copy · y for wheel-scroll".into();
        }
    }

    /// Scroll the focused agent's pane. Full-screen agents (Claude, …) run on the alternate screen
    /// and keep their own scrollback that tmux's capture can't reach, so forward the wheel to the
    /// agent and let it scroll itself (the streamer mirrors the result). Plain-shell agents have
    /// real tmux scrollback, so fall back to the local capture-based scroll.
    /// Queue a pane-scroll request (+ up / − down). Cheap and non-blocking: the event loop drains
    /// a whole wheel/PgUp burst into `pending_scroll` and then [`flush_pane_scroll`] sends a single
    /// `agent.scroll`, so a fast flick can't pile up dozens of RPCs and overshoot.
    fn pane_scroll(&mut self, up: bool, ticks: usize) {
        let d = ticks as isize;
        self.pending_scroll += if up { d } else { -d };
    }

    /// Flush the accumulated pane-scroll as one `agent.scroll` — forwarded to a full-screen agent so
    /// it scrolls its own history, or the local capture scroll when it isn't on the alternate screen.
    async fn flush_pane_scroll(&mut self) {
        let net = std::mem::take(&mut self.pending_scroll);
        if net == 0 {
            return;
        }
        let (up, ticks) = (net > 0, net.unsigned_abs());
        let Some(lane) = self.selected_lane().map(|l| l.id) else {
            return;
        };
        let window = self.selected_window();
        let forwarded = self
            .client
            .call(
                "agent.scroll",
                Some(json!({ "lane_id": lane, "up": up, "ticks": ticks, "window": window })),
            )
            .await
            .ok()
            .and_then(|v| v.get("forwarded").and_then(|f| f.as_bool()))
            .unwrap_or(false);
        if !forwarded {
            if up {
                self.scroll_up(ticks).await;
            } else {
                self.scroll_down(ticks);
            }
        }
    }

    /// Scroll the Focus pane back through history, grabbing a deep capture the first time.
    async fn scroll_up(&mut self, lines: usize) {
        if self.scroll_buf.is_none() {
            if let Some(id) = self.selected_lane().map(|l| l.id) {
                if let Ok(v) = self
                    .client
                    .call(
                        "agent.capture",
                        Some(json!({
                            "lane_id": id,
                            "lines": 2000,
                            "window": self.selected_window(),
                        })),
                    )
                    .await
                {
                    self.scroll_buf = v
                        .get("content")
                        .and_then(|c| c.as_str())
                        .map(str::to_string);
                    self.scroll_lines = self.scroll_buf.as_deref().map(view::parse_pane);
                }
            }
        }
        self.scroll = self.scroll.saturating_add(lines);
        self.clear_selection();
    }

    fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
        if self.scroll == 0 {
            self.scroll_buf = None;
            self.scroll_lines = None;
        }
        self.clear_selection();
    }

    fn reset_scroll(&mut self) {
        self.scroll = 0;
        self.scroll_buf = None;
        self.scroll_lines = None;
        self.clear_selection();
    }

    fn clear_selection(&mut self) {
        self.sel_anchor = None;
        self.sel_head = None;
    }

    /// The plain-text (ANSI-stripped) lines of the focused agent's pane — the same line set the
    /// Focus view renders, so a buffer index maps 1:1 to a rendered row.
    fn focus_buffer(&self) -> Vec<String> {
        let raw = if self.scroll > 0 {
            self.scroll_buf.clone().unwrap_or_default()
        } else {
            self.selected_lane()
                .and_then(|l| self.output.get(&l.id))
                .map(|p| p.raw.clone())
                .unwrap_or_default()
        };
        let mut lines: Vec<String> = strip_ansi(&raw).lines().map(str::to_string).collect();
        while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.pop();
        }
        lines
    }

    /// Map a mouse screen row to a buffer line index, using the geometry the view recorded.
    fn focus_line_at(&self, row: u16) -> Option<usize> {
        let (out_y0, start, count) = self.focus_geom.get();
        if count == 0 || row < out_y0 {
            return None;
        }
        let r = (row - out_y0) as usize;
        if r >= count {
            return None;
        }
        let idx = start + r;
        (idx < self.focus_buffer().len()).then_some(idx)
    }

    /// Copy the current drag-selection (whole lines) to the system clipboard and clear it.
    fn copy_selection(&mut self) {
        let (a, h) = match (self.sel_anchor, self.sel_head) {
            (Some(a), Some(h)) => (a, h),
            _ => return,
        };
        let lines = self.focus_buffer();
        let lo = a.min(h);
        let hi = a.max(h).min(lines.len().saturating_sub(1));
        if lo < lines.len() {
            let text = lines[lo..=hi].join("\n");
            let n = hi - lo + 1;
            copy_to_clipboard(&text);
            self.status = format!(
                "copied {n} line{} to clipboard",
                if n == 1 { "" } else { "s" }
            );
        }
        self.clear_selection();
    }

    async fn stop_agent(&mut self) {
        // On an expanded agent sub-row with no tmux window — an external (not repomon-managed)
        // session — there's nothing to kill, and falling through would kill the lane's primary
        // window by default. The daemon reaps genuinely-orphaned windows on its own.
        if let Some(FleetRow {
            lane_idx,
            session: Some(s),
            ..
        }) = self.selected_row()
        {
            let windowless = self
                .visible_lanes()
                .into_iter()
                .nth(lane_idx)
                .and_then(|l| l.agent_sessions.get(s))
                .is_some_and(|sess| sess.tmux_window.is_none());
            if windowless {
                self.status = "external session — not managed by repomon".into();
                return;
            }
        }
        if let Some(id) = self.selected_lane().map(|l| l.id) {
            let _ = self
                .client
                .call(
                    "agent.stop",
                    Some(json!({ "lane_id": id, "window": self.selected_window() })),
                )
                .await;
            self.status = "stopped agent".into();
            // Don't keep staring at the stopped agent's pane.
            if self.view == View::Focus {
                self.view = View::Split;
            }
            self.focus_insert = false;
            self.focus_managed = false;
            self.refresh().await;
        }
    }

    async fn merge_lane(&mut self) {
        if let Some(id) = self.selected_lane().map(|l| l.id) {
            match self
                .client
                .call("lane.merge", Some(json!({ "lane_id": id })))
                .await
            {
                Ok(v) => {
                    self.status = v
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("merged")
                        .to_string()
                }
                Err(e) => self.status = format!("merge failed: {e}"),
            }
            self.refresh().await;
        }
    }

    async fn toggle_pin(&mut self) {
        if let Some((id, pinned)) = self.selected_lane().map(|l| (l.id, l.pinned)) {
            let _ = self
                .client
                .call(
                    "agent.pin",
                    Some(json!({ "lane_id": id, "pinned": !pinned })),
                )
                .await;
            self.refresh().await;
        }
    }

    /// Arm/disarm auto-continue (resume on usage limit) for the selected lane.
    async fn toggle_auto_continue(&mut self) {
        let Some(id) = self.selected_lane().map(|l| l.id) else {
            return;
        };
        let enabled = self.ac_off.contains(&id); // currently off → turning on
        if enabled {
            self.ac_off.remove(&id);
        } else {
            self.ac_off.insert(id);
        }
        let _ = self
            .client
            .call(
                "agent.auto_continue",
                Some(json!({ "lane_id": id, "enabled": enabled })),
            )
            .await;
        self.status = if enabled {
            "auto-continue on for this lane".into()
        } else {
            "auto-continue off for this lane (resume manually on a usage limit)".into()
        };
        self.refresh().await;
    }

    async fn apply(&mut self, action: Action) {
        // Any action other than a second `X` cancels a pending repo-removal confirm, so moving
        // the cursor (or any other key) safely backs out of it.
        if !matches!(action, Action::RemoveRepo) {
            self.repo_remove_armed = None;
        }
        match action {
            Action::MoveUp => self.selected = self.selected.saturating_sub(1),
            Action::MoveDown => {
                let n = self.rows_len();
                if n > 0 && self.selected + 1 < n {
                    self.selected += 1;
                }
            }
            Action::ZoomIn => {
                self.focus_insert = false; // don't carry click-focus typing across screens
                self.orch_insert = false;
                // ↵/→ on the pinned repomind row opens the command-center instead of zooming a lane.
                if self.orchestrator_selected() && matches!(self.view, View::Fleet | View::Split) {
                    self.open_orchestrator().await;
                } else {
                    self.view = match self.view {
                        View::Fleet => View::Split,
                        View::Split => View::Focus,
                        View::Grid => View::Focus,
                        other => other,
                    }
                }
            }
            Action::ZoomOut => {
                self.focus_insert = false;
                self.orch_insert = false;
                match self.view {
                    View::Focus => self.view = View::Split,
                    View::Split => self.view = View::Fleet,
                    View::Grid => self.view = View::Fleet,
                    View::NewLane => self.view = View::Fleet,
                    View::Timeline
                    | View::Sessions
                    | View::Search
                    | View::AddRepo
                    | View::Agents
                    | View::Settings
                    | View::Notifications
                    | View::SpawnPick
                    | View::LaneJump
                    | View::Orchestrator => self.view = View::Fleet,
                    // Esc in Fleet clears the urgent filter first (like the text filter), then
                    // quits.
                    View::Fleet if self.urgent_only => {
                        self.urgent_only = false;
                        self.clamp_selection();
                    }
                    View::Fleet => self.should_quit = true,
                }
            }
            Action::Goto(target) => {
                self.view = target;
                match target {
                    View::Timeline => self.load_timeline().await,
                    View::Sessions => self.load_sessions().await,
                    View::Search => {
                        self.search_query.clear();
                        self.search_results.clear();
                    }
                    View::AddRepo => {
                        let cwd = std::env::current_dir()
                            .ok()
                            .map(|p| p.to_string_lossy().into_owned());
                        self.load_browse(cwd).await;
                    }
                    View::Agents => self.enter_agents(None).await,
                    View::Settings => self.load_settings().await,
                    View::Notifications => {
                        self.notif_sel = 0;
                        // Opening the feed counts as catching up — clears the ⚑ unread badge.
                        for ev in self.notifications.iter_mut() {
                            ev.read = true;
                        }
                    }
                    View::Orchestrator => {
                        self.selected = 0; // highlight the pinned row on return to Fleet
                        self.load_orchestrator().await;
                    }
                    _ => {}
                }
            }
            Action::Quit => self.should_quit = true,
            Action::NewLane => {
                self.view = View::NewLane;
                self.nl_branch.clear();
                self.nl_repo_idx = 0;
                self.nl_agent_idx = 0;
                self.load_agents().await;
            }
            Action::DeleteLane => {
                if let Some(id) = self.selected_lane().map(|l| l.id) {
                    match self
                        .client
                        .call(
                            "lane.delete",
                            Some(json!({ "lane_id": id, "also_delete_branch": false })),
                        )
                        .await
                    {
                        Ok(_) => {
                            self.status = format!("deleted lane {id}");
                            self.refresh().await;
                        }
                        Err(e) => self.status = format!("delete failed: {e}"),
                    }
                }
            }
            Action::RemoveRepo => {
                // Unregister the selected lane's whole repo (all its lanes), with a two-press
                // confirm. Unregister only: the daemon drops the registry row and stops watching
                // the tree, but worktree files and running agents are left untouched.
                let Some((repo_id, name)) = self
                    .selected_lane()
                    .map(|l| (l.repo.id, l.repo.name.clone()))
                else {
                    return;
                };
                let n = self.lanes.iter().filter(|l| l.repo.id == repo_id).count();
                if self.repo_remove_armed != Some(repo_id) {
                    self.repo_remove_armed = Some(repo_id);
                    self.status = format!(
                        "remove repo {name} ({n} lanes) from repomon? \
                         files & agents left untouched — press X again to confirm"
                    );
                    return;
                }
                self.repo_remove_armed = None;
                match self
                    .client
                    .call("repo.remove", Some(json!({ "repo_id": repo_id })))
                    .await
                {
                    Ok(_) => {
                        self.refresh().await;
                        self.status = format!(
                            "removed repo {name} — worktrees & agents left running; \
                             re-add with `repomon add`"
                        );
                    }
                    Err(e) => self.status = format!("remove failed: {e}"),
                }
            }
            Action::StartFilter => {
                self.filtering = true;
                self.filter.clear();
            }
            Action::Refresh => {
                self.refresh().await;
                self.status = "refreshed".into();
            }
            Action::CdToLane => self.cd_to_lane(),
            Action::ToggleBabysit => {
                self.view = if self.view == View::Grid {
                    View::Fleet
                } else {
                    self.grid_active = 0;
                    View::Grid
                };
            }
            Action::JumpNeedsYou => self.jump_attention(),
            Action::AttachNeedsYou => self.jump_attention_attach(),
            Action::FindLane => self.enter_lane_jump(),
            Action::ToggleUrgent => {
                self.urgent_only = !self.urgent_only;
                self.status = if self.urgent_only {
                    "showing only lanes that need you — ! or esc to clear".into()
                } else {
                    "showing all lanes".into()
                };
                self.clamp_selection();
            }
            Action::StopAgent => self.stop_agent().await,
            Action::Pin => self.toggle_pin().await,
            Action::Merge => self.merge_lane().await,
            Action::SpawnAgent => self.spawn_agent().await,
            Action::AdoptAgent => self.adopt_agent().await,
            Action::OpenTerminal => self.open_terminal().await,
            Action::AttachTerminal => self.attach_latest_terminal().await,
            Action::ToggleMouse => self.toggle_mouse(),
            Action::ToggleAutoContinue => self.toggle_auto_continue().await,
        }
        // Grid uses its own cursor; keep it in range.
        if self.view == View::Grid {
            let n = self.grid_lane_ids().len();
            if n == 0 {
                self.grid_active = 0;
            } else if self.grid_active >= n {
                self.grid_active = n - 1;
            }
        }
    }
}

/// Run the interactive TUI. Returns a path to cd into on exit, if requested.
pub async fn run(client: DaemonClient, theme: Theme) -> Result<Option<PathBuf>> {
    let _ = client
        .call("subscribe", Some(json!({ "topics": ["*"] })))
        .await;
    let mut events = client.subscribe();

    let mut app = App::new(client);
    app.theme = theme;
    app.refresh().await;
    // Prime the notification toggles (and default-agent picker) from the daemon config so alerts
    // honor the user's settings even if they never open the Settings view this session.
    app.load_settings().await;

    let mut terminal = ratatui::init();
    // Log panics to a file before the terminal is restored (which would otherwise scroll the
    // message off-screen, leaving a crash undiagnosable). Chains to ratatui's restore hook.
    {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_default();
            let bt = std::backtrace::Backtrace::force_capture();
            let _ = std::fs::write(
                "/tmp/repomon-panic.log",
                format!("repomon panic at {loc}\n{info}\n\nbacktrace:\n{bt}\n"),
            );
            prev(info);
        }));
    }
    if app.mouse_on {
        enable_mouse();
    }
    let (in_tx, mut in_rx) = mpsc::channel::<Event>(128);
    // Read stdin on a thread, but pause it (via `input_suspended`) during a tmux attach so we
    // don't fight tmux for the terminal — otherwise keystrokes get split and the session
    // misbehaves. Polling (rather than a blocking read) lets us check the flag.
    let suspended = app.input_suspended.clone();
    let parked = app.reader_parked.clone();
    std::thread::spawn(move || {
        use ratatui::crossterm::event;
        loop {
            if suspended.load(Ordering::Relaxed) {
                // Announce we're parked so an attach knows stdin is fully released to tmux, and
                // poll the flag on a short interval so we resume promptly.
                parked.store(true, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            parked.store(false, Ordering::Relaxed);
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if in_tx.blocking_send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });

    let outcome = event_loop(&mut terminal, &mut app, &mut in_rx, &mut events).await;
    // Stop the orchestrator pane stream on exit so the daemon doesn't keep capturing for a gone TUI.
    if app.orch_watched {
        let _ = app
            .client
            .call("orchestrator.watch", Some(json!({ "on": false })))
            .await;
    }
    disable_mouse();
    ratatui::restore();
    // Hand the title back to the shell (most shells re-set it at the next prompt anyway).
    {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b]2;\x07");
        let _ = out.flush();
    }
    outcome?;
    Ok(app.cd_target)
}

/// How urgently a lane's sessions need the user: 0 = waiting on you, 1 = rate-limited with no
/// auto-continue coming, 2 = working, 3 = nothing actionable. Inferred file-activity
/// placeholders never rank — they can't be acted on.
fn attention_rank(sessions: &[AgentSession], auto_continue_armed: bool) -> u8 {
    use AgentStatus::*;
    sessions
        .iter()
        .filter(|s| !s.inferred)
        .map(|s| match s.status {
            Waiting => 0,
            RateLimited if !auto_continue_armed => 1,
            Running | RateLimited => 2,
            Idle | Ended => 3,
        })
        .min()
        .unwrap_or(3)
}

/// A stable identity for one agent session within a lane, used to remember which agent a lane
/// had selected across a switch away and back. The persistent `id` is `0` for daemon-overlaid
/// placeholders so it can't be used; the managed tmux window (preferred) and the Claude
/// transcript id are the durable handles the rest of the app already keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionRef {
    Window(String),
    Transcript(String),
}

/// Consecutive lane refreshes a focused lane may show no managed session before Focus detaches.
/// More than one, so a single transient overlay flap (one bad snapshot — a tmux/lsof probe blip)
/// doesn't kick the user out of a still-running agent; a real exit stays absent and detaches
/// within ~grace refreshes.
const FOCUS_DETACH_GRACE: u8 = 3;

/// How many ~1s refreshes to keep trying to land the cursor on a just-spawned agent's window
/// before giving up (the transcript/window may take a moment to appear).
const PENDING_FOCUS_GIVE_UP_TICKS: u8 = 5;

/// Next consecutive-missing count for the focused agent after a lane refresh: reset to 0 when a
/// managed session is present, else incremented (saturating). Counted per refresh (≈1s), NOT per
/// render tick, so the grace measures seconds rather than event-loop iterations.
fn next_focus_missing(present: bool, missing: u8) -> u8 {
    if present {
        0
    } else {
        missing.saturating_add(1)
    }
}

/// Indices into `sessions` in a STABLE display order — managed agents by tmux **slot** (= spawn
/// order: `lane-N`, `lane-N-2`, `lane-N-3`), then windowless/external sessions by start time — so
/// expanded sub-rows (and their user labels) keep their position instead of reshuffling when the
/// daemon re-sorts sessions by recent activity. `session_id` is only the final tiebreaker.
fn stable_session_order(sessions: &[AgentSession]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..sessions.len()).collect();
    let key = |s: &AgentSession| {
        let slot = s
            .tmux_window
            .as_deref()
            .and_then(TmuxRuntime::slot_of_window)
            .unwrap_or(usize::MAX);
        (slot, s.started_at, s.session_id.clone())
    };
    idx.sort_by(|&a, &b| key(&sessions[a]).cmp(&key(&sessions[b])));
    idx
}

/// The stable identity of a session, or `None` for an inferred/keyless one (nothing to pin to).
fn agent_session_ref(s: &AgentSession) -> Option<SessionRef> {
    if let Some(w) = &s.tmux_window {
        Some(SessionRef::Window(w.clone()))
    } else {
        s.session_id.clone().map(SessionRef::Transcript)
    }
}

/// Index of the session matching `r` within `sessions`, or `None` if it's no longer present.
fn session_index_for_ref(sessions: &[AgentSession], r: &SessionRef) -> Option<usize> {
    sessions.iter().position(|s| match r {
        SessionRef::Window(w) => s.tmux_window.as_deref() == Some(w.as_str()),
        SessionRef::Transcript(id) => s.session_id.as_deref() == Some(id.as_str()),
    })
}

/// Case-insensitive subsequence match of `needle` in `haystack` for the lane switcher. Lower
/// scores are better: matches that start earlier and sit closer together rank first. Greedy
/// leftmost matching; an empty needle matches everything with the best score.
fn fuzzy_score(haystack: &str, needle: &str) -> Option<u32> {
    let mut hay = haystack.chars().flat_map(char::to_lowercase).enumerate();
    let mut score = 0u32;
    let mut prev: Option<usize> = None;
    for n in needle.chars().flat_map(char::to_lowercase) {
        let (i, _) = hay.by_ref().find(|&(_, h)| h == n)?;
        score += match prev {
            None => i as u32,              // distance from the start
            Some(p) => (i - p - 1) as u32, // gap since the previous matched char
        };
        prev = Some(i);
    }
    Some(score)
}

/// Cycle `current` to the next/previous option (wrapping). Returns `current` if `options` is empty.
fn cycle(options: &[&str], current: &str, forward: bool) -> String {
    if options.is_empty() {
        return current.to_string();
    }
    let n = options.len();
    let cur = options.iter().position(|o| *o == current).unwrap_or(0);
    let next = if forward {
        (cur + 1) % n
    } else {
        (cur + n - 1) % n
    };
    options[next].to_string()
}

fn enable_mouse() {
    // Button/scroll/drag tracking only. We deliberately do NOT request any-motion tracking
    // (mode 1003): it floods a redraw per mouse move on terminals that report it, and Terminal.app
    // doesn't report it anyway, so hover stayed inert there. (Re-add behind an opt-in flag later.)
    use ratatui::crossterm::event::EnableMouseCapture;
    let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
}

/// Discard any terminal input still buffered after returning from a tmux attach — mouse-tracking
/// reports, the detach key's tail, terminal query replies — so it isn't replayed as a glitchy
/// backlog when the reader thread resumes. Call while the reader is still parked, since only one
/// place may read crossterm events at a time.
fn drain_pending_input() {
    use ratatui::crossterm::event;
    while event::poll(Duration::from_millis(0)).unwrap_or(false) {
        let _ = event::read();
    }
}

fn disable_mouse() {
    use ratatui::crossterm::event::DisableMouseCapture;
    let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    in_rx: &mut mpsc::Receiver<Event>,
    events: &mut broadcast::Receiver<Notification>,
) -> Result<()> {
    let mut events_alive = true;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        // Resolve the selected session/window BEFORE syncing the viewport, so a just-spawned
        // window isn't momentarily streamed as the wrong pane (selected_window() is correct).
        // Diagnostic: time the whole sync block with a per-call breakdown. Each call is awaited on
        // this critical path, so several sub-threshold RPCs can add up to a visible stall that no
        // single slow-RPC line would flag (e.g. the resync forced right after a detach).
        let sync_t = std::time::Instant::now();
        app.sync_session_cursor();
        app.sync_viewport().await;
        let t_viewport = sync_t.elapsed();
        app.sync_pane_size().await;
        app.sync_orchestrator_size().await;
        let t_panesize = sync_t.elapsed();
        app.sync_recent_commits().await;
        let t_commits = sync_t.elapsed();
        app.sync_terminals().await;
        let t_terminals = sync_t.elapsed();
        if t_terminals >= Duration::from_millis(250) {
            tui_log(&format!(
                "slow sync block: total={:.2}s viewport={:.0}ms panesize={:.0}ms commits={:.0}ms terminals={:.0}ms",
                t_terminals.as_secs_f32(),
                t_viewport.as_millis(),
                (t_panesize - t_viewport).as_millis(),
                (t_commits - t_panesize).as_millis(),
                (t_terminals - t_commits).as_millis()
            ));
        }
        app.sync_title();
        app.check_focus_alive();
        // Expire a stale notification banner so the footer hints come back.
        if let Some((_, since)) = &app.notif_banner {
            if since.elapsed() >= NOTIF_BANNER_TTL {
                app.notif_banner = None;
            }
        }
        if !matches!(app.view, View::Focus | View::Orchestrator) && app.scroll != 0 {
            app.reset_scroll();
        }
        terminal.draw(|f| view::render(f, app))?;
        if app.should_quit {
            return Ok(());
        }
        if let Some(lane) = app.attach_request.take() {
            do_attach(terminal, app, lane).await;
            while in_rx.try_recv().is_ok() {} // drop anything queued before the reader parked
            continue;
        }
        if let Some(target) = app.attach_target.take() {
            do_attach_target(terminal, app, &target).await;
            while in_rx.try_recv().is_ok() {}
            continue;
        }
        tokio::select! {
            maybe = in_rx.recv() => match maybe {
                Some(ev) => {
                    app.handle_event(ev).await;
                    // Coalesce already-buffered input (paste bursts, any post-attach backlog)
                    // into one frame instead of one redraw per event. Stop early if an event
                    // requests an attach or quit so it's handled at the top of the next loop.
                    while app.attach_request.is_none()
                        && app.attach_target.is_none()
                        && !app.should_quit
                    {
                        match in_rx.try_recv() {
                            Ok(ev) => app.handle_event(ev).await,
                            Err(_) => break,
                        }
                    }
                    // Send the whole drained burst at once: one scroll, and one send_input for a
                    // pasted/typed run of characters — not one blocking RPC per event.
                    app.flush_pane_scroll().await;
                    app.flush_pending_input().await;
                }
                None => return Ok(()),
            },
            note = next_note(events), if events_alive => match note {
                Some(n) => {
                    // Diagnostic: time the drain. The backlog buffered while parked in a focused
                    // attach is processed here on return; if parsing it (ANSI -> styled lines per
                    // pane) is what stalls the UI after a detach, this records it with a count.
                    let drain_started = std::time::Instant::now();
                    let mut drained = 1usize;
                    app.on_notification(n).await;
                    // Coalesce a burst of notifications (and the backlog buffered while parked in
                    // a focused attach) into a single refresh instead of one ~100ms refresh each.
                    loop {
                        match events.try_recv() {
                            Ok(n) => {
                                drained += 1;
                                app.on_notification(n).await;
                            }
                            Err(broadcast::error::TryRecvError::Empty) => break,
                            Err(broadcast::error::TryRecvError::Lagged(_)) => {
                                app.refresh_pending = true;
                            }
                            Err(broadcast::error::TryRecvError::Closed) => {
                                events_alive = false;
                                break;
                            }
                        }
                    }
                    if std::mem::take(&mut app.refresh_pending) {
                        app.refresh().await;
                    }
                    let drain_elapsed = drain_started.elapsed();
                    if drain_elapsed >= Duration::from_millis(300) {
                        tui_log(&format!(
                            "slow notif drain: {drained} notes in {:.2}s",
                            drain_elapsed.as_secs_f32()
                        ));
                    }
                }
                None => events_alive = false,
            },
            _ = tick.tick() => {
                // Refresh the lane list every second in *all* views: it keeps agent state fresh
                // (an agent that exits on its own is noticed promptly, Focus drops back to Split)
                // AND it's what drives notification edge-detection, which must work even when the
                // user is looking at Fleet or another view. lane.list is cheap (~55ms) at 1Hz and
                // only runs while the TUI is open; commits/repos still refresh on git events.
                app.refresh_lanes().await;
                // Account-usage corner: a slow, self-throttled poll (no-op most ticks).
                app.sync_usage().await;
            }
        }
    }
}

/// Suspend the TUI, attach to the lane's tmux window (the selected agent's, when several run
/// side by side), then re-enter.
async fn do_attach(terminal: &mut DefaultTerminal, app: &mut App, lane: LaneId) {
    // Flush any input buffered just before the attach (e.g. a paste finished as the user hit attach)
    // so it isn't silently dropped when the TUI suspends — attach requested outside the input burst
    // skips the event-loop's post-burst flush.
    app.flush_pending_input().await;
    let window = app.selected_window();
    let resp = app
        .client
        .call(
            "agent.target",
            Some(json!({ "lane_id": lane, "window": window })),
        )
        .await;
    let (target, available) = match resp {
        Ok(v) => (
            v.get("target")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            v.get("available")
                .and_then(|a| a.as_bool())
                .unwrap_or(false),
        ),
        Err(e) => {
            app.status = format!("attach failed: {e}");
            return;
        }
    };
    if !available || target.is_empty() {
        app.status = "no agent running in this lane".into();
        return;
    }
    app.input_suspended.store(true, Ordering::Relaxed);
    // Tell the daemon we're parking now so it takes over desktop popups on its next tick rather
    // than waiting out LOCAL_TTL for our heartbeat to go stale — closes the handoff gap.
    let _ = app.client.call("watcher.park", None).await;
    app.await_reader_parked().await; // confirmed handoff: reader has released stdin to tmux
    disable_mouse();
    ratatui::restore();
    let keepalive = spawn_attach_keepalive(&app.client);
    tmux_attach(&target);
    keepalive.abort();
    // Diagnostic: time the terminal re-init + first paint after a detach. None of these touch the
    // daemon, so a stall here (vs a slow RPC) points at ratatui::init / drain / draw rather than the
    // network path — narrowing the "hangs for a bit on exit" report.
    let reinit_t = std::time::Instant::now();
    *terminal = ratatui::init();
    if app.mouse_on {
        enable_mouse();
    }
    let _ = terminal.clear();
    drain_pending_input();
    let after_drain = reinit_t.elapsed();
    app.input_suspended.store(false, Ordering::Relaxed);
    app.last_viewport.clear(); // force a viewport resync after returning
    app.last_title.clear(); // tmux set its own title; re-assert ours next tick
    // The daemon fired desktop popups while we were parked (our heartbeat went stale); re-seed
    // notification edge-detection so the next refresh doesn't replay — and double-fire — them.
    app.notif_reseed = true;
    app.status = "back from the agent (it's still running) — ↵ to reopen".into();
    // Snap straight back to FleetView: paint now, in the freshly re-init'd alternate screen, so the
    // user doesn't sit looking at tmux's "[detached]" line + a stale/garbled screen while the next
    // loop iteration's sync RPCs (which run before its own draw) complete.
    let _ = terminal.draw(|f| view::render(f, app));
    let reinit_total = reinit_t.elapsed();
    if reinit_total >= Duration::from_millis(200) {
        tui_log(&format!(
            "post-detach reinit: total={:.2}s init+clear+drain={:.0}ms draw={:.0}ms",
            reinit_total.as_secs_f32(),
            after_drain.as_millis(),
            (reinit_total - after_drain).as_millis()
        ));
    }
}

/// Suspend the TUI, attach to an arbitrary tmux target (e.g. a plain terminal), then re-enter.
async fn do_attach_target(terminal: &mut DefaultTerminal, app: &mut App, target: &str) {
    if target.is_empty() {
        return;
    }
    app.input_suspended.store(true, Ordering::Relaxed);
    // Signal the park so the daemon covers desktop popups immediately (see do_attach).
    let _ = app.client.call("watcher.park", None).await;
    app.await_reader_parked().await; // confirmed handoff: reader has released stdin to tmux
    disable_mouse();
    ratatui::restore();
    let keepalive = spawn_attach_keepalive(&app.client);
    tmux_attach(target);
    keepalive.abort();
    *terminal = ratatui::init();
    if app.mouse_on {
        enable_mouse();
    }
    let _ = terminal.clear();
    drain_pending_input();
    app.input_suspended.store(false, Ordering::Relaxed);
    app.last_viewport.clear();
    // The attach restored the orchestrator window's client-follow (full-terminal) size, so force a
    // re-fit to the mediated pane on the next tick.
    app.last_orch_resize = None;
    app.last_title.clear(); // tmux set its own title; re-assert ours next tick
    app.terminals_lane = None; // the shell may have exited; refresh the terminal list
    // Re-seed notification edge-detection — the daemon owned popups while we were parked.
    app.notif_reseed = true;
    // Paint immediately on return (see do_attach) so the detach message + stale screen don't linger.
    let _ = terminal.draw(|f| view::render(f, app));
}

/// Escape sequence emitted after `tmux attach` returns, to wipe tmux's "[detached (from session
/// …)]" line off the PRIMARY screen so it doesn't resurface when repomon leaves its alternate
/// screen on quit. It MUST erase the single message line IN PLACE (`\x1b[2K`) rather than do a
/// full-screen clear (`\x1b[2J`/`\x1b[3J`): macOS Terminal.app scrolls a full-screen erase into the
/// scrollback buffer (and exposes no clear-scrollback capability), so the line would survive there
/// and reappear above the post-quit prompt. The message always sits exactly one line above the
/// cursor on return, hence `up one, erase line, carriage return`.
const DETACH_MSG_CLEANUP: &str = "\x1b[1A\x1b[2K\r";

/// Keep the daemon connection alive across a (blocking) tmux attach. While parked the TUI sends no
/// requests, and the daemon reaps any connection that's been silent for its READ_IDLE_TIMEOUT
/// (120s) — after which the first RPC on return is written into a dead socket and the UI hangs for
/// the full ~15s client timeout (confirmed: silent connection closed at exactly 120.0s). A
/// `watcher.park` ping every 45s keeps the connection live AND re-asserts the parked state, so the
/// daemon keeps owning desktop popups. The runtime is multi-threaded, so this task runs on another
/// worker while `tmux_attach` blocks the event loop's worker; the client's reader task (still
/// draining the socket while parked) resolves each ping's response. Abort it on return.
fn spawn_attach_keepalive(client: &DaemonClient) -> tokio::task::JoinHandle<()> {
    let client = client.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(45)).await;
            if client.call("watcher.park", None).await.is_err() {
                break; // connection already gone — nothing left to keep alive
            }
        }
    })
}

/// Attach to a `session:window` target on repomon's dedicated tmux socket (the socket label is
/// the session name). `$TMUX` is dropped so this works even when repomon runs inside tmux —
/// otherwise tmux refuses to attach ("sessions should be nested with care").
fn tmux_attach(target: &str) {
    let socket = target.split(':').next().unwrap_or("repomon");
    tui_log(&format!("attach -> {target}"));
    let start = std::time::Instant::now();
    let status = std::process::Command::new("tmux")
        .args(["-L", socket, "attach", "-t", target])
        .env_remove("TMUX")
        .status();
    // Log when (and how) the attach ended so an *unexpected* detach is traceable after the fact:
    // a very short duration / non-zero exit points at a failed attach or an external detach.
    tui_log(&format!(
        "attach <- {target} after {:.1}s status={:?}",
        start.elapsed().as_secs_f32(),
        status.as_ref().ok().map(|s| s.code())
    ));
    // On detach, tmux prints "[detached (from session …)]\r\n" to the PRIMARY screen (it just left
    // its own alternate screen), leaving the cursor on the line directly below that message. repomon
    // re-enters its alternate screen and hides it during use, but it resurfaces when repomon finally
    // leaves the alternate screen on quit. A full-screen erase (\x1b[2J) is the WRONG tool here:
    // macOS Terminal.app (and others) scroll erased content into the scrollback buffer instead of
    // discarding it, so the line survives there and reappears above the post-quit prompt (Terminal.app
    // has no clear-scrollback capability either, so \x1b[3J can't help). Erase just the message line
    // IN PLACE — \x1b[2K never scrolls — which removes it on every terminal while preserving the
    // user's earlier scrollback. The message is always exactly one line above the cursor: a normal
    // print lands it one line up; a print on the bottom row scrolls it up one with the cursor — same
    // offset either way.
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = write!(out, "{DETACH_MSG_CLEANUP}");
    let _ = out.flush();
}

/// Append a timestamped diagnostic line to the TUI log. The TUI can't use tracing/stderr (ratatui
/// owns the screen), so attach/detach diagnostics go straight to a file next to the daemon log.
pub(crate) fn tui_log(line: &str) {
    use std::io::Write;
    let dir = repomon_core::service::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("repomon-tui.log"))
    {
        let _ = writeln!(f, "{} {line}", chrono::Utc::now().to_rfc3339());
    }
}

/// Translate a key press into a tmux key spec. `(spec, literal)` — literal printable text
/// is sent with `send-keys -l`; named keys (Enter, Tab, BTab, arrows, C-c, …) without it.
/// Strip ANSI escape sequences (CSI and simple `ESC x`) to get plain text for selection/copy.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break; // end of the CSI sequence
                    }
                }
            } else {
                chars.next(); // a one-character escape
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Copy `text` to the system clipboard (macOS `pbcopy`, falling back to Linux tools).
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for prog in ["pbcopy", "wl-copy", "xclip"] {
        let mut cmd = Command::new(prog);
        if prog == "xclip" {
            cmd.args(["-selection", "clipboard"]);
        }
        if let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
    }
}

/// Save a clipboard image to a temp PNG and return its path (macOS), so it can be referenced
/// to an agent. Tries `pngpaste`, then AppleScript. `None` if the clipboard has no image.
fn clipboard_image_to_file() -> Option<String> {
    use std::process::Command;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = format!("/tmp/repomon-paste-{}-{nanos}.png", std::process::id());

    if Command::new("pngpaste")
        .arg(&path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && std::path::Path::new(&path).exists()
    {
        return Some(path);
    }

    let set_f = format!("set f to open for access POSIX file \"{path}\" with write permission");
    let lines: [&str; 10] = [
        "try",
        "set d to the clipboard as «class PNGf»",
        "on error",
        "return \"noimage\"",
        "end try",
        &set_f,
        "set eof f to 0",
        "write d to f",
        "close access f",
        "return \"ok\"",
    ];
    let mut cmd = Command::new("osascript");
    for l in &lines {
        cmd.arg("-e").arg(l);
    }
    let out = cmd.output().ok()?;
    if String::from_utf8_lossy(&out.stdout).trim() == "ok" && std::path::Path::new(&path).exists() {
        Some(path)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}

/// The keystroke that leaves insert mode. It's `Ctrl-O` (not `Esc`) because the agent itself
/// needs `Esc` for interrupt/clear, so `Esc` is forwarded rather than captured.
fn leaves_insert(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL))
}

fn translate_key(key: &KeyEvent) -> Option<(String, bool)> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if let KeyCode::Char(c) = key.code {
        if ctrl {
            return Some((format!("C-{}", c.to_ascii_lowercase()), false));
        }
        // Alt+<char> (e.g. the terminal sending Option as Meta): forward as a tmux M- key.
        if alt {
            return Some((format!("M-{c}"), false));
        }
        return Some((c.to_string(), true)); // literal printable
    }
    let base = match key.code {
        KeyCode::Esc => "Escape", // the agent needs Esc (interrupt / clear); ^O leaves insert
        KeyCode::Enter => "Enter",
        KeyCode::Backspace => "BSpace",
        KeyCode::Tab => "Tab",
        KeyCode::BackTab => "BTab", // Shift+Tab — cycles agent modes
        KeyCode::Up => "Up",
        KeyCode::Down => "Down",
        KeyCode::Left => "Left",
        KeyCode::Right => "Right",
        KeyCode::Delete => "DC",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        _ => return None,
    };
    // Carry Alt/Ctrl so Option+Arrow (word jump), Ctrl+Arrow, Alt+Backspace (word delete), …
    // reach the agent as tmux M-/C- keys.
    let prefix = if ctrl {
        "C-"
    } else if alt {
        "M-"
    } else {
        ""
    };
    Some((format!("{prefix}{base}"), false))
}

/// Await the next forwardable event, collapsing lag. `None` means the stream closed.
async fn next_note(rx: &mut broadcast::Receiver<Notification>) -> Option<Notification> {
    loop {
        match rx.recv().await {
            Ok(n) => return Some(n),
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detach_cleanup_erases_line_in_place_not_fullscreen() {
        // macOS Terminal.app scrolls a full-screen erase into scrollback, where tmux's
        // "[detached …]" line then survives to the post-quit prompt. The cleanup must erase only
        // the message line in place. Guards against regressing to the \x1b[2J approach.
        assert!(
            DETACH_MSG_CLEANUP.contains("\x1b[2K"),
            "must erase the detach line in place (\\x1b[2K)"
        );
        assert!(
            !DETACH_MSG_CLEANUP.contains("2J"),
            "must not full-screen clear (\\x1b[2J scrolls into Terminal.app scrollback)"
        );
        assert!(
            !DETACH_MSG_CLEANUP.contains("3J"),
            "must not clear scrollback (\\x1b[3J is unsupported on Terminal.app anyway)"
        );
    }

    /// A minimal real-or-inferred session, mirroring the daemon's `overlay_agents` literals.
    fn sess(session_id: Option<&str>, status: AgentStatus, inferred: bool) -> AgentSession {
        AgentSession {
            id: 0,
            agent: repomon_core::model::AgentKind::ClaudeCode,
            repo_id: 1,
            worktree_id: None,
            started_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            ended_at: None,
            manifest_path: PathBuf::new(),
            tool_call_count: 0,
            title: None,
            status,
            external: false,
            session_id: session_id.map(str::to_string),
            resume_at: None,
            inferred,
            tmux_window: None,
            last_message: None,
            pending_prompt: None,
            config_dir: None,
            custom_label: None,
        }
    }

    /// A managed session in a tmux window, optionally transcript-backed.
    fn managed(window: &str, session_id: Option<&str>) -> AgentSession {
        let mut s = sess(session_id, AgentStatus::Running, false);
        s.tmux_window = Some(window.to_string());
        s
    }

    #[test]
    fn agent_session_ref_prefers_window_then_transcript() {
        // A managed agent keys on its window even when it also has a transcript id.
        assert_eq!(
            agent_session_ref(&managed("lane-7-2", Some("uuid-1"))),
            Some(SessionRef::Window("lane-7-2".into()))
        );
        // An external/transcript-only session keys on the transcript id.
        assert_eq!(
            agent_session_ref(&sess(Some("uuid-2"), AgentStatus::Waiting, false)),
            Some(SessionRef::Transcript("uuid-2".into()))
        );
        // An inferred file-activity placeholder (no window, no id) has no stable handle.
        assert_eq!(
            agent_session_ref(&sess(None, AgentStatus::Idle, true)),
            None
        );
    }

    #[test]
    fn session_index_for_ref_finds_by_window_and_transcript_else_none() {
        // Two managed agents side by side, plus an external transcript-only one.
        let sessions = [
            managed("lane-7", Some("uuid-a")),
            managed("lane-7-2", Some("uuid-b")),
            sess(Some("uuid-ext"), AgentStatus::Waiting, false),
        ];
        // The window identity lands on the right slot regardless of position.
        assert_eq!(
            session_index_for_ref(&sessions, &SessionRef::Window("lane-7-2".into())),
            Some(1)
        );
        assert_eq!(
            session_index_for_ref(&sessions, &SessionRef::Transcript("uuid-ext".into())),
            Some(2)
        );
        // A remembered agent that has since exited matches nothing → caller falls back to slot 0.
        assert_eq!(
            session_index_for_ref(&sessions, &SessionRef::Window("lane-7-3".into())),
            None
        );
    }

    #[test]
    fn attention_rank_orders_urgency() {
        use AgentStatus::*;
        // Waiting always tops; a rate-limited agent only needs you when nothing will resume it.
        assert_eq!(attention_rank(&[sess(Some("a"), Waiting, false)], true), 0);
        assert_eq!(
            attention_rank(&[sess(Some("a"), RateLimited, false)], false),
            1
        );
        assert_eq!(
            attention_rank(&[sess(Some("a"), RateLimited, false)], true),
            2
        );
        assert_eq!(attention_rank(&[sess(Some("a"), Running, false)], true), 2);
        assert_eq!(attention_rank(&[sess(Some("a"), Idle, false)], true), 3);
        // The most urgent session wins; inferred placeholders never rank.
        assert_eq!(
            attention_rank(
                &[
                    sess(Some("a"), Running, false),
                    sess(Some("b"), Waiting, false)
                ],
                true
            ),
            0
        );
        assert_eq!(attention_rank(&[sess(None, Waiting, true)], true), 3);
        assert_eq!(attention_rank(&[], true), 3);
    }

    #[test]
    fn focus_detach_waits_for_sustained_absence() {
        // A present managed session resets the miss count (recovered flap).
        assert_eq!(next_focus_missing(true, 2), 0);
        // Consecutive absences accumulate, staying below the grace for the first couple refreshes…
        let mut m = 0u8;
        m = next_focus_missing(false, m);
        assert_eq!(m, 1);
        assert!(m < FOCUS_DETACH_GRACE, "one flap must not detach");
        m = next_focus_missing(false, m);
        assert_eq!(m, 2);
        assert!(m < FOCUS_DETACH_GRACE, "a two-tick flap must not detach");
        // …until a sustained absence reaches the grace (a real exit).
        m = next_focus_missing(false, m);
        assert_eq!(m, 3);
        assert!(m >= FOCUS_DETACH_GRACE, "sustained absence detaches");
        // A flicker back resets, so the next single absence won't detach.
        assert_eq!(next_focus_missing(true, m), 0);
    }

    #[test]
    fn stable_session_order_follows_spawn_slot() {
        // Managed agents order by tmux slot (lane-N, lane-N-2, lane-N-3), NOT by session_id and
        // NOT by the daemon's input order (it churns by recent activity) — so sub-rows hold their
        // position instead of reshuffling. session_ids here are anti-correlated with slot to prove
        // the slot drives the order.
        let sessions = [
            managed("lane-7", Some("zzz")),
            managed("lane-7-2", Some("mmm")),
            managed("lane-7-3", Some("aaa")),
        ];
        let ordered: Vec<&str> = stable_session_order(&sessions)
            .into_iter()
            .map(|i| sessions[i].tmux_window.as_deref().unwrap())
            .collect();
        assert_eq!(ordered, vec!["lane-7", "lane-7-2", "lane-7-3"]);

        // A windowless (external) session sorts after every managed one, regardless of its id.
        let mixed = [
            sess(Some("aaa"), AgentStatus::Running, false),
            managed("lane-7-2", Some("mmm")),
            managed("lane-7", Some("zzz")),
        ];
        assert_eq!(stable_session_order(&mixed), vec![2, 1, 0]);
    }

    #[test]
    fn fuzzy_score_prefers_early_contiguous() {
        // Contiguous prefix beats a gappy subsequence; case-insensitive; misses are None.
        let hay = "repomon/feat-x";
        assert_eq!(fuzzy_score(hay, ""), Some(0));
        assert_eq!(fuzzy_score(hay, "repo"), Some(0));
        assert!(fuzzy_score(hay, "repo") < fuzzy_score(hay, "rmn"));
        assert!(fuzzy_score(hay, "feat") < fuzzy_score(hay, "ftx"));
        assert_eq!(fuzzy_score(hay, "REPO"), fuzzy_score(hay, "repo"));
        assert_eq!(fuzzy_score(hay, "zzz"), None);
        // Later starts cost more, so "feat" in a later position scores worse than at the front.
        assert!(fuzzy_score("feat-x", "feat") < fuzzy_score("repomon/feat-x", "feat"));
    }

    #[test]
    fn double_click_needs_same_lane_within_window() {
        let t0 = std::time::Instant::now();
        // Same lane, well within the window → double-click.
        assert!(is_double_click(
            Some((t0, 5)),
            5,
            t0 + Duration::from_millis(100)
        ));
        // Same lane but too slow → single clicks.
        assert!(!is_double_click(
            Some((t0, 5)),
            5,
            t0 + Duration::from_millis(500)
        ));
        // A different lane is never a double-click, however fast.
        assert!(!is_double_click(
            Some((t0, 5)),
            7,
            t0 + Duration::from_millis(50)
        ));
        // No previous click → not a double-click.
        assert!(!is_double_click(None, 5, t0));
    }

    #[test]
    fn esc_is_forwarded_not_captured() {
        // Esc must reach the agent (interrupt / clear), so it maps to the tmux key name...
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(translate_key(&esc), Some(("Escape".to_string(), false)));
        // ...and it does NOT leave insert mode.
        assert!(!leaves_insert(&esc));
    }

    #[test]
    fn alt_and_ctrl_arrows_forward_word_jump() {
        // Option/Alt + Arrow (word jump) and Ctrl + Arrow reach the agent as tmux M-/C- keys.
        let alt_left = KeyEvent::new(KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(
            translate_key(&alt_left),
            Some(("M-Left".to_string(), false))
        );
        let ctrl_right = KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(
            translate_key(&ctrl_right),
            Some(("C-Right".to_string(), false))
        );
        // Alt+Backspace (delete word) and Alt+<char> too.
        let alt_bs = KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(
            translate_key(&alt_bs),
            Some(("M-BSpace".to_string(), false))
        );
        let alt_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(translate_key(&alt_b), Some(("M-b".to_string(), false)));
        // A plain arrow is unmodified.
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(translate_key(&left), Some(("Left".to_string(), false)));
    }

    #[test]
    fn strip_ansi_removes_escape_sequences() {
        assert_eq!(
            strip_ansi("\x1b[31mhello\x1b[0m world"),
            "hello world".to_string()
        );
        // Cursor-move and SGR sequences both go; plain text is untouched.
        assert_eq!(strip_ansi("a\x1b[2Kb\x1b[1;32mc"), "abc".to_string());
        assert_eq!(strip_ansi("plain"), "plain".to_string());
    }

    #[test]
    fn ctrl_o_leaves_insert_plain_o_does_not() {
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert!(leaves_insert(&ctrl_o));

        let plain_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
        assert!(!leaves_insert(&plain_o));
        // A plain 'o' is a literal char for the agent; Ctrl-O would be C-o if forwarded.
        assert_eq!(translate_key(&plain_o), Some(("o".to_string(), true)));
        assert_eq!(translate_key(&ctrl_o), Some(("C-o".to_string(), false)));
    }

    /// `apply_orchestrator_status` doesn't touch the network — it just needs *a* connected
    /// `DaemonClient` to build an `App` around (`App::new` has no other constructor). A listener
    /// that accepts once and goes quiet is enough; no daemon RPC is exercised.
    async fn app_with_dummy_client() -> App {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            let _ = listener.accept().await;
            // Keep the accepted stream alive for the test's duration instead of dropping it
            // immediately, so the client doesn't see an instant EOF/reconnect churn.
            std::future::pending::<()>().await;
        });
        let client = DaemonClient::connect(&sock).await.expect("connect");
        App::new(client)
    }

    #[tokio::test]
    async fn apply_orchestrator_status_parses_attention_and_headline() {
        let mut app = app_with_dummy_client().await;
        // Notifications must not fire real OS popups from a unit test.
        app.settings.notify_enabled = false;

        app.apply_orchestrator_status(&json!({
            "running": true,
            "agent": "claude-work",
            "model": "opus",
            "window": "orchestrator",
            "attention": "permission",
            "headline": "Do you want to proceed?",
        }));
        assert_eq!(app.orch_attention.as_deref(), Some("permission"));
        assert_eq!(
            app.orch_headline.as_deref(),
            Some("Do you want to proceed?")
        );

        app.apply_orchestrator_status(&json!({
            "running": true,
            "agent": "claude-work",
            "model": "opus",
            "window": "orchestrator",
            "attention": "end_of_turn",
            "headline": null,
        }));
        assert_eq!(app.orch_attention.as_deref(), Some("end_of_turn"));
        assert_eq!(app.orch_headline, None);

        // "none" on the wire clears both fields — not `Some("none")`.
        app.apply_orchestrator_status(&json!({
            "running": true,
            "agent": "claude-work",
            "model": "opus",
            "window": "orchestrator",
            "attention": "none",
            "headline": null,
        }));
        assert_eq!(app.orch_attention, None);
        assert_eq!(app.orch_headline, None);

        // A payload missing the fields entirely (an older daemon) also reads as no attention.
        app.apply_orchestrator_status(&json!({ "running": false }));
        assert_eq!(app.orch_attention, None);
    }

    #[tokio::test]
    async fn orchestrator_attention_edge_is_suppressed_by_view_and_settings() {
        // Deliberately never flips both `notify_enabled` and `notify_needs_you` on together here:
        // that combination reaches `notify::send_native`, which shells out to a real OS
        // notification on this platform — not something a unit test should trigger. The two gates
        // are instead verified independently, each suppressing on its own.
        let mut app = app_with_dummy_client().await;
        // Seed first (a no-attention application, matching a real cold start with nothing
        // pending): otherwise the fresh app's very first `apply_orchestrator_status` call below
        // would itself be the seed call and pass for the wrong reason, masking whether the view/
        // settings gates below actually suppress anything.
        app.apply_orchestrator_status(&json!({ "running": true, "attention": "none" }));
        assert_eq!(app.orch_attention, None);

        // Gate 1: already looking at the command-center — its row/header cover it, so the
        // none→attention edge must not bank a popup banner even with notifications on.
        app.settings.notify_enabled = true;
        app.settings.notify_needs_you = true;
        app.view = View::Orchestrator;
        app.apply_orchestrator_status(&json!({
            "running": true, "attention": "decision", "headline": "pick one"
        }));
        assert_eq!(app.orch_attention.as_deref(), Some("decision"));
        assert!(
            app.notif_banner.is_none(),
            "must not banner while already on the Orchestrator view"
        );

        // Gate 2: elsewhere in the TUI, but notifications are off — still no banner.
        app.orch_attention = None; // reset to none so the next call is a real edge
        app.settings.notify_enabled = false;
        app.view = View::Fleet;
        app.apply_orchestrator_status(&json!({
            "running": true, "attention": "permission", "headline": "proceed?"
        }));
        assert_eq!(app.orch_attention.as_deref(), Some("permission"));
        assert!(
            app.notif_banner.is_none(),
            "must not banner while notifications are disabled"
        );
    }

    #[tokio::test]
    async fn orchestrator_popup_is_seeded_not_fired_on_first_application() {
        // Cold start: repomind is already awaiting attention (e.g. it raised a permission dialog
        // before the TUI attached). The very first `apply_orchestrator_status` call must seed
        // `orch_attention` from this value rather than read the jump from the struct's default
        // `None` as a genuine none→attention edge — mirrors `detect_notifications`'s
        // `notif_seeded` guard for the lane path (see the `orch_notif_seeded` field doc).
        //
        // Both notify gates are deliberately on here (unlike the suppression test above): if
        // seeding didn't short-circuit before the gate check, this would reach the real
        // (OS-popping) `notify::send_native`, so a clean `notif_banner.is_none()` here is what
        // actually proves the seed path was taken — not just that some other gate happened to be
        // closed.
        let mut app = app_with_dummy_client().await;
        app.settings.notify_enabled = true;
        app.settings.notify_needs_you = true;
        app.view = View::Fleet;

        app.apply_orchestrator_status(&json!({
            "running": true, "attention": "permission", "headline": "already pending"
        }));
        assert_eq!(app.orch_attention.as_deref(), Some("permission"));
        assert!(
            app.notif_banner.is_none(),
            "must not banner on the first (seed) status application"
        );

        // A subsequent genuine none→attention edge, after seeding, does fire. Checked through the
        // pure `orch_popup_should_fire` predicate rather than by feeding another payload through
        // `apply_orchestrator_status` — with both gates on, a real edge there would reach the
        // actual (OS-popping) `notify::send_native`, exactly what this module avoids in tests.
        assert!(
            app.orch_popup_should_fire(false, false),
            "a genuine none->attention edge after seeding must fire"
        );
        // The seed call itself must never fire, regardless of what a genuine edge would do.
        assert!(!app.orch_popup_should_fire(true, false));
    }
}
