//! Application state and the interactive event loop.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use ratatui::DefaultTerminal;
use repomon_core::model::{
    AgentChoice, AgentSession, AgentStatus, BrowseEntry, BrowseResult, Commit, Lane, LaneId, Repo,
    TimelineData, WorkSession,
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
    /// (lookback seconds, bucket seconds, label).
    fn params(self) -> (i64, i64, &'static str) {
        match self {
            Zoom::Day => (24 * 3600, 3600, "day"),
            Zoom::Week => (7 * 24 * 3600, 6 * 3600, "week"),
            Zoom::Month => (30 * 24 * 3600, 24 * 3600, "month"),
        }
    }
}

/// All UI state. `view` reads these fields directly.
/// A lane's captured pane: the raw text (for selection/copy and scrollback) plus its styled
/// lines, parsed once per `event.agent.output` delta so the render path only has to slice.
pub struct Pane {
    pub raw: String,
    pub lines: Vec<Line<'static>>,
}

/// A clickable lane region recorded during render (in `view.rs`) and hit-tested by the mouse
/// handler. `interactive` = a single-click focuses the lane for typing in place (Grid tiles,
/// Split panes/rows); otherwise a single-click only selects it (Fleet rows, which show no pane).
#[derive(Clone, Copy)]
pub struct ClickZone {
    pub rect: Rect,
    pub lane: LaneId,
    pub interactive: bool,
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
}

/// Accent choices the Settings view cycles through (`mono` = no color).
const ACCENTS: &[&str] = &[
    "cyan", "green", "magenta", "amber", "blue", "red", "white", "mono",
];

/// Number of editable rows in the Settings view.
const SETTINGS_COUNT: usize = 12;

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
    /// Whether repomon captures the mouse (for scroll-wheel nav). When off, the terminal owns
    /// the mouse so you can drag-select and copy the rendered output natively.
    pub mouse_on: bool,
    /// Scrollback: lines scrolled up from the live tail in Focus (0 = following). `scroll_buf`
    /// holds a deep capture taken when you start scrolling.
    pub scroll: usize,
    pub scroll_buf: Option<String>,
    /// The scrollback snapshot's pre-parsed styled lines (parsed once when `scroll_buf` is set).
    pub scroll_lines: Option<Vec<Line<'static>>>,
    /// Focus drag-selection (buffer line indices). On release the range is copied to the
    /// clipboard. `focus_geom` = (first output screen row, window-start line, visible count),
    /// set during render so the mouse handler can map a screen row to a buffer line.
    pub sel_anchor: Option<usize>,
    pub sel_head: Option<usize>,
    pub focus_geom: std::cell::Cell<(u16, usize, usize)>,
    /// Clickable lane regions for the current frame, recorded during render and read by the mouse
    /// handler. Cleared and repopulated every render.
    pub click_zones: RefCell<Vec<ClickZone>>,
    /// Last left-click (time + lane) for double-click detection.
    last_click: Option<(std::time::Instant, LaneId)>,
    /// The lane the mouse is currently hovering (highlighted on render). `None` = not over a lane.
    pub hover_lane: Option<LaneId>,
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
    /// Debounce keyed by (lane, session, kind): the last time each *kind* of alert fired for a
    /// session. Keying on the kind suppresses a flapping identical alert without swallowing a
    /// genuinely different transition (e.g. a usage-limit alert right after a needs-you one);
    /// keying on the session lets two agents in one lane each raise the same kind of alert.
    notif_debounce: HashMap<(LaneId, SessKey, NotifKind), Instant>,
    /// In-app notification history (newest last), shown in the Notifications view.
    pub notifications: VecDeque<NotifEvent>,
    /// Scroll offset (rows from the top) in the Notifications view.
    pub notif_scroll: usize,
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
    /// Plain shell terminals open for the selected lane (tmux window names).
    pub terminals: Vec<String>,
    terminals_lane: Option<LaneId>,
    pub browse_path: String,
    pub browse_parent: Option<String>,
    pub browse_entries: Vec<BrowseEntry>,
    pub browse_selected: usize,
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
    attach_request: Option<LaneId>,
    /// A tmux target (e.g. a terminal window) the loop should attach to next.
    attach_target: Option<String>,
    /// When set, the stdin-reader thread pauses (so tmux owns the terminal during an attach).
    input_suspended: Arc<AtomicBool>,
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
            // Mouse captured so the wheel scrolls the agent pane and drag-selects copy to the
            // clipboard. `y` releases it for native terminal selection/scroll if preferred.
            mouse_on: true,
            scroll: 0,
            scroll_buf: None,
            scroll_lines: None,
            sel_anchor: None,
            sel_head: None,
            focus_geom: std::cell::Cell::new((0, 0, 0)),
            click_zones: RefCell::new(Vec::new()),
            last_click: None,
            hover_lane: None,
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
                ..Settings::default()
            },
            settings_idx: 0,
            settings_editing: false,
            settings_geom: std::cell::Cell::new(0),
            prev_status: HashMap::new(),
            notif_seeded: false,
            notif_debounce: HashMap::new(),
            notifications: VecDeque::new(),
            notif_scroll: 0,
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
            terminals: Vec::new(),
            terminals_lane: None,
            browse_path: String::new(),
            browse_parent: None,
            browse_entries: Vec::new(),
            browse_selected: 0,
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
            attach_request: None,
            attach_target: None,
            input_suspended: Arc::new(AtomicBool::new(false)),
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
    }

    /// Pull just the lane list — the only thing needing per-second freshness in live views (so
    /// an agent that exits on its own is noticed promptly). Commits/repos change on git events,
    /// which arrive as notifications that trigger a full [`refresh`].
    pub async fn refresh_lanes(&mut self) {
        match self.client.call_typed::<Vec<Lane>>("lane.list", None).await {
            Ok(l) => {
                // Keep the cursor on the same lane across the attention re-sort below.
                let keep = self.selected_lane().map(|l| l.id);
                self.lanes = l;
                // Run notification edge-detection only on a *successful* fetch. Seeding off a
                // failed first call (empty lanes) would make the next good refresh treat every
                // running agent as a fresh transition — the startup storm seeding prevents.
                self.detect_notifications();
                self.sort_lanes();
                if let Some(id) = keep {
                    if let Some(i) = self.visible_lanes().iter().position(|l| l.id == id) {
                        self.selected = i;
                    }
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
        self.lanes.sort_by_key(|l| {
            (
                repo_order[&l.repo.id],
                !l.pinned,
                attention[&l.id],
                std::cmp::Reverse(l.last_activity_at),
            )
        });
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

        // Resolve the target under one immutable borrow of the lanes, then mutate.
        let lanes = self.visible_lanes();
        let hits: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| self.lane_needs_attention(l))
            .map(|(i, _)| i)
            .collect();
        let banner_idx = banner_lane
            .and_then(|id| lanes.iter().position(|l| l.id == id))
            .filter(|&i| i != self.selected);
        let label = |i: usize| format!("{}/{}", lanes[i].repo.name, lanes[i].worktree.name);

        let (target, msg) = if let Some(i) = banner_idx {
            (Some(i), format!("→ {} (just alerted)", label(i)))
        } else if hits.is_empty() {
            (None, "no agents need you".to_string())
        } else {
            let next = hits
                .iter()
                .copied()
                .find(|&i| i > self.selected)
                .unwrap_or(hits[0]);
            let pos = hits.iter().position(|&i| i == next).unwrap_or(0);
            (
                Some(next),
                format!("needs you {}/{} — {}", pos + 1, hits.len(), label(next)),
            )
        };
        if let Some(i) = target {
            self.selected = i;
        }
        self.status = msg;
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
        let find = |me: &Self| me.visible_lanes().iter().position(|l| l.id == id);
        let mut idx = find(self);
        if idx.is_none() && (!self.filter.is_empty() || self.urgent_only) {
            self.filter.clear();
            self.filtering = false;
            self.urgent_only = false;
            idx = find(self);
        }
        match idx {
            Some(i) => {
                self.selected = i;
                self.focus_insert = false;
                self.reset_scroll();
                self.view = View::Focus;
            }
            None => self.status = "that lane no longer exists".into(),
        }
    }

    /// Diff the freshly-fetched per-session statuses against the previous snapshot and fire a
    /// notification on each meaningful transition. The first call only seeds the snapshot.
    fn detect_notifications(&mut self) {
        // Snapshot the new statuses, one entry per real agent session.
        let now: HashMap<(LaneId, SessKey), AgentStatus> = self
            .lanes
            .iter()
            .flat_map(|l| session_statuses(l.id, &l.agent_sessions))
            .collect();

        if !self.notif_seeded {
            self.prev_status = now;
            self.notif_seeded = true;
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
            self.notif_debounce.insert(dkey, Instant::now());
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

        for ((lane, key), kind) in fires {
            self.fire_notification(lane, &key, kind);
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
    fn fire_notification(&mut self, id: LaneId, key: &SessKey, kind: NotifKind) {
        // Compose under an immutable borrow that ends before we mutate `self`. The session may
        // be gone when its disappearance was the trigger — compose degrades to a generic line.
        let Some((title, body)) = self
            .lanes
            .iter()
            .find(|l| l.id == id)
            .map(|l| notify::compose(kind, l, session_by_key(l, key)))
        else {
            return;
        };
        notify::send_native(&title, &body, self.settings.notify_sound);
        self.notif_banner = Some((format!("{title}  ·  {body}"), Instant::now()));
        self.notifications.push_back(NotifEvent {
            when: chrono::Local::now(),
            kind,
            lane_id: id,
            title,
            body,
        });
        while self.notifications.len() > NOTIF_HISTORY_CAP {
            self.notifications.pop_front();
        }
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

    pub fn selected_lane(&self) -> Option<&Lane> {
        self.visible_lanes().into_iter().nth(self.selected)
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
        let n = self.visible_lanes().len();
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
            | View::LaneJump => Vec::new(),
        }
    }

    /// Tell the daemon which lanes are visible, if that set changed.
    pub async fn sync_viewport(&mut self) {
        let live = self.live_lanes();
        if live != self.last_viewport {
            let _ = self
                .client
                .call("viewport.set", Some(json!({ "lane_ids": live })))
                .await;
            self.last_viewport = live;
        }
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

    /// Keep the session cursor in range: reset to 0 when the selected lane changes, and clamp
    /// to the number of sessions on that lane.
    fn sync_session_cursor(&mut self) {
        let sel = self.selected_lane().map(|l| l.id);
        if sel != self.session_lane {
            self.session_lane = sel;
            self.session_idx = 0;
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
            return;
        }
        let has_managed = self
            .selected_lane()
            .map(|l| l.agent_sessions.iter().any(|s| !s.external))
            .unwrap_or(false);
        if has_managed {
            self.focus_managed = true;
        } else if self.focus_managed {
            self.focus_managed = false;
            self.focus_insert = false;
            self.view = View::Split;
            self.status = "agent exited".into();
        }
    }

    /// Point the fleet selection at the grid's active tile (so per-lane actions act on it).
    fn select_grid_active(&mut self) {
        let ids = self.grid_lane_ids();
        if let Some(&id) = ids.get(self.grid_active) {
            if let Some(pos) = self.visible_lanes().iter().position(|l| l.id == id) {
                self.selected = pos;
            }
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
                self.output.insert(id, Pane { raw, lines });
            }
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
            Event::Mouse(me) => {
                use ratatui::crossterm::event::{MouseButton, MouseEventKind};
                // Bare movement just updates the hovered lane (highlighted on render).
                if matches!(me.kind, MouseEventKind::Moved) {
                    self.update_hover(me.column, me.row);
                    return;
                }
                // In Focus the wheel scrolls the agent's output and a drag selects lines (copied
                // to the clipboard on release); elsewhere the wheel moves the cursor.
                match self.view {
                    View::Focus => match me.kind {
                        MouseEventKind::ScrollUp => self.scroll_up(3).await,
                        MouseEventKind::ScrollDown => self.scroll_down(3),
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
                    // Grid/Fleet/Split: a left-click focuses the clicked lane (double-click opens
                    // its real terminal, a click on empty space blurs); the wheel still navigates.
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
            _ if self.filtering => self.filter_key(key),
            _ => {
                if let Some(action) = keybinds::nav(key) {
                    self.apply(action).await;
                }
            }
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
            KeyCode::Esc => self.view = View::Fleet,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
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

    async fn discover_here(&mut self) {
        let root = self.browse_path.clone();
        let found: Vec<String> = self
            .client
            .call_typed(
                "repo.discover",
                Some(json!({ "root": root, "max_depth": 4 })),
            )
            .await
            .unwrap_or_default();
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
        self.status = format!("discovered {} repo(s), added {added}", found.len());
        self.refresh().await;
        let here = self.browse_path.clone();
        self.load_browse(Some(here)).await;
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
            _ => {}
        }
    }

    async fn activate_setting(&mut self) {
        match self.settings_idx {
            0..=2 | 5..=11 => self.adjust_setting(true).await,
            3..=4 => self.settings_editing = true,
            _ => {}
        }
    }

    /// Key handling for the Notifications history view: scroll, clear, or leave.
    fn notifications_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.notif_scroll = self.notif_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Clamp to the last event so scrolling past the end can't blank the feed.
                let max = self.notifications.len().saturating_sub(1);
                self.notif_scroll = (self.notif_scroll + 1).min(max);
            }
            KeyCode::Char('c') => {
                self.notifications.clear();
                self.notif_scroll = 0;
            }
            // The feed renders newest-first with `notif_scroll` as the top row; ↵ opens the
            // lane behind that top event (marked ▸).
            KeyCode::Enter | KeyCode::Right => {
                if let Some(id) = self
                    .notifications
                    .iter()
                    .rev()
                    .nth(self.notif_scroll)
                    .map(|e| e.lane_id)
                {
                    self.jump_to_lane(id);
                }
            }
            KeyCode::Esc | KeyCode::Left => self.view = View::Fleet,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
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
        let (lookback, bucket, _) = self.timeline_zoom.params();
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
    fn activate_lane(&mut self, id: LaneId) {
        if let Some(pos) = self.visible_lanes().iter().position(|l| l.id == id) {
            self.selected = pos;
        }
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
        let hit = self
            .click_zones
            .borrow()
            .iter()
            .find(|z| z.rect.contains(Position { x: col, y: row }))
            .copied();
        match hit {
            Some(z) => {
                let now = std::time::Instant::now();
                let dbl = is_double_click(self.last_click, z.lane, now);
                self.last_click = Some((now, z.lane));
                self.activate_lane(z.lane);
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
            let n = self.visible_lanes().len();
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

    /// Forward one keystroke live to the selected lane's agent (insert-mode passthrough),
    /// so its own UI works (Shift+Tab cycles modes, arrows navigate menus, Ctrl-C interrupts).
    async fn send_agent_key(&mut self, key: KeyEvent) {
        if let (Some(id), Some((spec, literal))) =
            (self.selected_lane().map(|l| l.id), translate_key(&key))
        {
            let _ = self
                .client
                .call(
                    "agent.key",
                    Some(json!({ "lane_id": id, "key": spec, "literal": literal })),
                )
                .await;
        }
    }

    /// Key handling in the Split view: fleet sidebar + the selected lane's live output. Like
    /// Focus, `i` enters insert mode to type straight to the agent here (esc returns); ↵/→
    /// still zooms into full-screen Focus.
    async fn split_key(&mut self, key: KeyEvent) {
        if self.focus_insert {
            if leaves_insert(&key) {
                self.focus_insert = false;
                return;
            }
            self.send_agent_key(key).await;
            return;
        }
        if self.filtering {
            self.filter_key(key);
            return;
        }
        // ↵ opens the selected agent in its real tmux pane (a native terminal); → zooms to the
        // Focus monitor; `i` is a quick mediated type without leaving repomon.
        if key.code == KeyCode::Enter {
            self.attach_request = self.selected_lane().map(|l| l.id);
            return;
        }
        if key.code == KeyCode::Char('i') {
            self.focus_insert = true;
            return;
        }
        match key.code {
            KeyCode::Tab => return self.cycle_session(true),
            KeyCode::BackTab => return self.cycle_session(false),
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
                KeyCode::PageUp => return self.scroll_up(10).await,
                KeyCode::PageDown => return self.scroll_down(10),
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
            KeyCode::PageUp | KeyCode::Up => self.scroll_up(10).await,
            KeyCode::PageDown | KeyCode::Down => self.scroll_down(10),
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

    /// Spawn `agent` into `lane`: on success drop into Focus, ready to drive it.
    async fn do_spawn(&mut self, id: LaneId, agent: &str) {
        match self
            .client
            .call(
                "agent.spawn",
                Some(json!({ "lane_id": id, "agent": agent })),
            )
            .await
        {
            Ok(_) => {
                self.status = format!("spawned {agent}");
                self.view = View::Focus;
                self.focus_insert = false;
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
                        Some(json!({ "lane_id": id, "text": format!("{path} "), "enter": false })),
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

    /// Scroll the Focus pane back through history, grabbing a deep capture the first time.
    async fn scroll_up(&mut self, lines: usize) {
        if self.scroll_buf.is_none() {
            if let Some(id) = self.selected_lane().map(|l| l.id) {
                if let Ok(v) = self
                    .client
                    .call(
                        "agent.capture",
                        Some(json!({ "lane_id": id, "lines": 2000 })),
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
        if let Some(id) = self.selected_lane().map(|l| l.id) {
            let _ = self
                .client
                .call("agent.stop", Some(json!({ "lane_id": id })))
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
        match action {
            Action::MoveUp => self.selected = self.selected.saturating_sub(1),
            Action::MoveDown => {
                let n = self.visible_lanes().len();
                if n > 0 && self.selected + 1 < n {
                    self.selected += 1;
                }
            }
            Action::ZoomIn => {
                self.focus_insert = false; // don't carry click-focus typing across screens
                self.view = match self.view {
                    View::Fleet => View::Split,
                    View::Split => View::Focus,
                    View::Grid => View::Focus,
                    other => other,
                }
            }
            Action::ZoomOut => {
                self.focus_insert = false;
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
                    | View::LaneJump => self.view = View::Fleet,
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
                    View::Notifications => self.notif_scroll = 0,
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
    if app.mouse_on {
        enable_mouse();
    }
    let (in_tx, mut in_rx) = mpsc::channel::<Event>(128);
    // Read stdin on a thread, but pause it (via `input_suspended`) during a tmux attach so we
    // don't fight tmux for the terminal — otherwise keystrokes get split and the session
    // misbehaves. Polling (rather than a blocking read) lets us check the flag.
    let suspended = app.input_suspended.clone();
    std::thread::spawn(move || {
        use ratatui::crossterm::event;
        loop {
            if suspended.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(40));
                continue;
            }
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
    disable_mouse();
    ratatui::restore();
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

/// Identifies one real agent session within a lane across refreshes.
///
/// Transcript-backed sessions key on the Claude session id (the transcript filename stem),
/// which is stable across polls. `claude --resume` may continue the same logical work in a new
/// transcript; that reads as one session vanishing and another appearing — acceptable noise. A
/// lane has at most one real session *without* a transcript id per snapshot (the managed
/// no-transcript placeholder or the generic process monitor — mutually exclusive branches in
/// the daemon's `overlay_agents`), so a single `Fallback` sentinel covers it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SessKey {
    Transcript(String),
    Fallback,
}

/// Key/status pairs for one lane's *real* agent sessions. Inferred "file activity" placeholders
/// are dropped so they never drive named alerts. On a (theoretically impossible) duplicate key,
/// the higher-priority status wins — the same order the old per-lane rollup used.
fn session_statuses(
    lane_id: LaneId,
    sessions: &[AgentSession],
) -> Vec<((LaneId, SessKey), AgentStatus)> {
    let mut out: Vec<((LaneId, SessKey), AgentStatus)> = Vec::new();
    for s in sessions.iter().filter(|s| !s.inferred) {
        let key = (
            lane_id,
            s.session_id
                .clone()
                .map(SessKey::Transcript)
                .unwrap_or(SessKey::Fallback),
        );
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, st)) if status_priority(s.status) < status_priority(*st) => *st = s.status,
            Some(_) => {}
            None => out.push((key, s.status)),
        }
    }
    out
}

/// Notification priority of a status (lower = more urgent).
fn status_priority(s: AgentStatus) -> usize {
    use AgentStatus::*;
    [RateLimited, Waiting, Running, Idle, Ended]
        .iter()
        .position(|&x| x == s)
        .unwrap_or(usize::MAX)
}

/// Diff the previous and current per-session status maps into the notifications to fire.
///
/// Sessions present in `now` are edge-detected against their previous status. Sessions that
/// vanished fire as a transition to `None` (→ Idle if they were active), except when their
/// whole lane is gone (deleting a lane isn't an agent going quiet) or when a lane's `Fallback`
/// key was handed off to a transcript-backed key (`lanes_with_managed`): the managed
/// no-transcript placeholder disappears the moment the agent's transcript becomes parseable,
/// and firing Idle there would alert on every spawn.
fn diff_session_transitions(
    prev: &HashMap<(LaneId, SessKey), AgentStatus>,
    now: &HashMap<(LaneId, SessKey), AgentStatus>,
    live_lanes: &HashSet<LaneId>,
    lanes_with_managed: &HashSet<LaneId>,
) -> Vec<((LaneId, SessKey), NotifKind)> {
    let mut out = Vec::new();
    for (key, &status) in now {
        let was = prev.get(key).copied();
        if was == Some(status) {
            continue;
        }
        if let Some(kind) = transition_kind(was, Some(status)) {
            out.push((key.clone(), kind));
        }
    }
    for (key, &was) in prev {
        if now.contains_key(key) || !live_lanes.contains(&key.0) {
            continue;
        }
        if key.1 == SessKey::Fallback && lanes_with_managed.contains(&key.0) {
            continue;
        }
        if let Some(kind) = transition_kind(Some(was), None) {
            out.push((key.clone(), kind));
        }
    }
    out
}

/// Resolve a session key back to the lane's session, for composing the notification text.
/// `None` when the session vanished (i.e. its disappearance was the trigger).
fn session_by_key<'a>(lane: &'a Lane, key: &SessKey) -> Option<&'a AgentSession> {
    lane.agent_sessions
        .iter()
        .filter(|s| !s.inferred)
        .find(|s| match key {
            SessKey::Transcript(id) => s.session_id.as_deref() == Some(id.as_str()),
            SessKey::Fallback => s.session_id.is_none(),
        })
}

/// Map a session's status transition to the notification it should fire, if any. `None` means
/// the session was absent from that snapshot. Priority resolves cases like
/// `Running → RateLimited` to the limit.
fn transition_kind(prev: Option<AgentStatus>, now: Option<AgentStatus>) -> Option<NotifKind> {
    use AgentStatus::*;
    match (prev, now) {
        // Hit a usage limit.
        (p, Some(RateLimited)) if p != Some(RateLimited) => Some(NotifKind::RateLimited),
        // Auto-resumed after a limit.
        (Some(RateLimited), Some(Running)) => Some(NotifKind::Resumed),
        // Finished its turn / needs you.
        (p, Some(Waiting)) if p != Some(Waiting) => Some(NotifKind::NeedsYou),
        // Was active, now quiet (idle / ended / the session went away).
        (Some(Running) | Some(Waiting), Some(Idle) | Some(Ended) | None) => Some(NotifKind::Idle),
        _ => None,
    }
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
        app.sync_viewport().await;
        app.sync_recent_commits().await;
        app.sync_terminals().await;
        app.sync_session_cursor();
        app.check_focus_alive();
        // Expire a stale notification banner so the footer hints come back.
        if let Some((_, since)) = &app.notif_banner {
            if since.elapsed() >= NOTIF_BANNER_TTL {
                app.notif_banner = None;
            }
        }
        if app.view != View::Focus && app.scroll != 0 {
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
                }
                None => return Ok(()),
            },
            note = next_note(events), if events_alive => match note {
                Some(n) => {
                    app.on_notification(n).await;
                    // Coalesce a burst of notifications (and the backlog buffered while parked in
                    // a focused attach) into a single refresh instead of one ~100ms refresh each.
                    loop {
                        match events.try_recv() {
                            Ok(n) => app.on_notification(n).await,
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
            }
        }
    }
}

/// Suspend the TUI, attach to the lane's tmux window, then re-enter.
async fn do_attach(terminal: &mut DefaultTerminal, app: &mut App, lane: LaneId) {
    let resp = app
        .client
        .call("agent.target", Some(json!({ "lane_id": lane })))
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
    tokio::time::sleep(Duration::from_millis(120)).await; // let the reader thread release stdin
    disable_mouse();
    ratatui::restore();
    tmux_attach(&target);
    *terminal = ratatui::init();
    if app.mouse_on {
        enable_mouse();
    }
    let _ = terminal.clear();
    drain_pending_input();
    app.input_suspended.store(false, Ordering::Relaxed);
    app.last_viewport.clear(); // force a viewport resync after returning
    app.status = "back from the agent (it's still running) — ↵ to reopen".into();
}

/// Suspend the TUI, attach to an arbitrary tmux target (e.g. a plain terminal), then re-enter.
async fn do_attach_target(terminal: &mut DefaultTerminal, app: &mut App, target: &str) {
    if target.is_empty() {
        return;
    }
    app.input_suspended.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(120)).await; // let the reader thread release stdin
    disable_mouse();
    ratatui::restore();
    tmux_attach(target);
    *terminal = ratatui::init();
    if app.mouse_on {
        enable_mouse();
    }
    let _ = terminal.clear();
    drain_pending_input();
    app.input_suspended.store(false, Ordering::Relaxed);
    app.last_viewport.clear();
    app.terminals_lane = None; // the shell may have exited; refresh the terminal list
}

/// Attach to a `session:window` target on repomon's dedicated tmux socket (the socket label is
/// the session name). `$TMUX` is dropped so this works even when repomon runs inside tmux —
/// otherwise tmux refuses to attach ("sessions should be nested with care").
fn tmux_attach(target: &str) {
    let socket = target.split(':').next().unwrap_or("repomon");
    let _ = std::process::Command::new("tmux")
        .args(["-L", socket, "attach", "-t", target])
        .env_remove("TMUX")
        .status();
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
    fn notification_transitions() {
        use AgentStatus::*;
        // The headline alerts.
        assert_eq!(
            transition_kind(Some(Running), Some(Waiting)),
            Some(NotifKind::NeedsYou)
        );
        assert_eq!(
            transition_kind(None, Some(Waiting)),
            Some(NotifKind::NeedsYou)
        );
        assert_eq!(
            transition_kind(Some(Running), Some(RateLimited)),
            Some(NotifKind::RateLimited)
        );
        assert_eq!(
            transition_kind(Some(RateLimited), Some(Running)),
            Some(NotifKind::Resumed)
        );
        // Gave up on the limit and now needs you.
        assert_eq!(
            transition_kind(Some(RateLimited), Some(Waiting)),
            Some(NotifKind::NeedsYou)
        );
        // Went quiet (idle / ended / the agent went away).
        assert_eq!(
            transition_kind(Some(Running), Some(Idle)),
            Some(NotifKind::Idle)
        );
        assert_eq!(transition_kind(Some(Waiting), None), Some(NotifKind::Idle));
        // Non-events: you replied, work simply started, or nothing changed.
        assert_eq!(transition_kind(Some(Waiting), Some(Running)), None);
        assert_eq!(transition_kind(None, Some(Running)), None);
        assert_eq!(transition_kind(Some(Idle), Some(Running)), None);
        assert_eq!(transition_kind(Some(Waiting), Some(Waiting)), None);
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
        }
    }

    #[test]
    fn session_statuses_keys_and_filters() {
        use AgentStatus::*;
        let sessions = vec![
            sess(Some("a"), Waiting, false),
            sess(Some("b"), RateLimited, false),
            sess(None, Running, true), // inferred file-activity placeholder — excluded
            sess(None, Running, false),
        ];
        let got = session_statuses(7, &sessions);
        assert_eq!(got.len(), 3);
        assert!(got.contains(&((7, SessKey::Transcript("a".into())), Waiting)));
        assert!(got.contains(&((7, SessKey::Transcript("b".into())), RateLimited)));
        assert!(got.contains(&((7, SessKey::Fallback), Running)));

        // Defensive: a duplicate key keeps the higher-priority status.
        let dup = vec![sess(None, Idle, false), sess(None, Waiting, false)];
        assert_eq!(
            session_statuses(7, &dup),
            vec![((7, SessKey::Fallback), Waiting)]
        );
    }

    #[test]
    fn two_sessions_fire_independent_streams() {
        use AgentStatus::*;
        let k = |id: &str| (1, SessKey::Transcript(id.into()));
        let live: HashSet<LaneId> = [1].into();
        let managed = HashSet::new();

        // One agent finishes its turn while its lane-mate is still rate-limited. The old
        // per-lane rollup saw "RateLimited" before and after and fired nothing — the masking
        // this change exists to fix.
        let prev: HashMap<_, _> = [(k("a"), Running), (k("b"), RateLimited)].into();
        let now: HashMap<_, _> = [(k("a"), Waiting), (k("b"), RateLimited)].into();
        assert_eq!(
            diff_session_transitions(&prev, &now, &live, &managed),
            vec![(k("a"), NotifKind::NeedsYou)]
        );

        // And the rate-limited lane-mate resumes independently.
        let now2: HashMap<_, _> = [(k("a"), Waiting), (k("b"), Running)].into();
        assert_eq!(
            diff_session_transitions(&now, &now2, &live, &managed),
            vec![(k("b"), NotifKind::Resumed)]
        );
    }

    #[test]
    fn disappearance_fires_idle_only_when_lane_lives() {
        use AgentStatus::*;
        let k = (1, SessKey::Transcript("a".into()));
        let prev: HashMap<_, _> = [(k.clone(), Waiting)].into();
        let now = HashMap::new();
        let managed = HashSet::new();

        let live: HashSet<LaneId> = [1].into();
        assert_eq!(
            diff_session_transitions(&prev, &now, &live, &managed),
            vec![(k, NotifKind::Idle)]
        );
        // The whole lane went away (deleted): not an agent going quiet.
        assert!(diff_session_transitions(&prev, &now, &HashSet::new(), &managed).is_empty());
    }

    #[test]
    fn fallback_handoff_does_not_fire_idle() {
        use AgentStatus::*;
        let live: HashSet<LaneId> = [1].into();
        let prev: HashMap<_, _> = [((1, SessKey::Fallback), Running)].into();
        let now: HashMap<_, _> = [((1, SessKey::Transcript("a".into())), Running)].into();

        // The managed spawn's transcript became parseable: Fallback hands off to Transcript
        // within one refresh. The agent didn't stop, so nothing fires.
        let managed: HashSet<LaneId> = [1].into();
        assert!(diff_session_transitions(&prev, &now, &live, &managed).is_empty());

        // But with no managed session left in the lane, a vanished fallback is a real stop.
        let gone = HashMap::new();
        assert_eq!(
            diff_session_transitions(&prev, &gone, &live, &HashSet::new()),
            vec![((1, SessKey::Fallback), NotifKind::Idle)]
        );
    }

    #[test]
    fn new_session_already_waiting_fires_needs_you() {
        use AgentStatus::*;
        let live: HashSet<LaneId> = [1].into();
        let managed = HashSet::new();
        let prev = HashMap::new();
        let k = (1, SessKey::Transcript("a".into()));

        // A second agent appearing mid-run already waiting (e.g. a parallel session that
        // finished between refreshes) is exactly the alert the user wants.
        let waiting: HashMap<_, _> = [(k.clone(), Waiting)].into();
        assert_eq!(
            diff_session_transitions(&prev, &waiting, &live, &managed),
            vec![(k.clone(), NotifKind::NeedsYou)]
        );
        // Appearing already-running is just work starting; stay quiet.
        let running: HashMap<_, _> = [(k, Running)].into();
        assert!(diff_session_transitions(&prev, &running, &live, &managed).is_empty());
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
}
