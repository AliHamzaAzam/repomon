//! Application state and the interactive event loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use repomon_core::model::{
    AgentChoice, BrowseEntry, BrowseResult, Commit, Lane, LaneId, Repo, TimelineData, WorkSession,
};
use repomon_core::protocol::Notification;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

use crate::client::DaemonClient;
use crate::keybinds::{self, Action, View};
use crate::theme::Theme;
use crate::view;

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
    /// Latest captured pane content per lane, pushed by `event.agent.output`.
    pub output: HashMap<LaneId, String>,
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
    /// Focus drag-selection (buffer line indices). On release the range is copied to the
    /// clipboard. `focus_geom` = (first output screen row, window-start line, visible count),
    /// set during render so the mouse handler can map a screen row to a buffer line.
    pub sel_anchor: Option<usize>,
    pub sel_head: Option<usize>,
    pub focus_geom: std::cell::Cell<(u16, usize, usize)>,
    /// Active tile in the babysit grid.
    pub grid_active: usize,
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
            sel_anchor: None,
            sel_head: None,
            focus_geom: std::cell::Cell::new((0, 0, 0)),
            grid_active: 0,
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

    /// Pull fresh fleet state from the daemon.
    pub async fn refresh(&mut self) {
        match self.client.call_typed::<Vec<Lane>>("lane.list", None).await {
            Ok(l) => self.lanes = l,
            Err(e) => self.status = format!("lane.list failed: {e}"),
        }
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
        self.clamp_selection();
    }

    pub fn visible_lanes(&self) -> Vec<&Lane> {
        if self.filter.is_empty() {
            return self.lanes.iter().collect();
        }
        let f = self.filter.to_lowercase();
        self.lanes
            .iter()
            .filter(|l| {
                l.repo.name.to_lowercase().contains(&f)
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

    /// Lane ids to babysit in the grid: pinned first, then needs-you, then most-active.
    pub fn grid_lane_ids(&self) -> Vec<LaneId> {
        let mut lanes: Vec<&Lane> = self.visible_lanes();
        lanes.sort_by(|a, b| {
            let key = |l: &Lane| {
                (
                    !l.pinned,
                    !l.agent_sessions.iter().any(|s| s.status.needs_you()),
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
            | View::Agents => Vec::new(),
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
            KeyCode::Char(' ') | KeyCode::Char('f') | KeyCode::Esc => self.view = View::Fleet,
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
                self.output.insert(id, content.to_string());
            }
        } else {
            self.refresh().await;
            // A repo may have new commits; refetch the selected lane's history next tick.
            self.recent_commits_lane = None;
        }
    }

    async fn handle_event(&mut self, ev: Event) {
        let key = match ev {
            Event::Key(key) => key,
            Event::Mouse(me) => {
                use ratatui::crossterm::event::{MouseButton, MouseEventKind};
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
                    _ => self.handle_mouse(me),
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
        let agent = self.default_agent_name().await;
        match self
            .client
            .call(
                "agent.spawn",
                Some(json!({ "lane_id": id, "agent": &agent })),
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
        }
        self.clear_selection();
    }

    fn reset_scroll(&mut self) {
        self.scroll = 0;
        self.scroll_buf = None;
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
                .cloned()
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
                self.view = match self.view {
                    View::Fleet => View::Split,
                    View::Split => View::Focus,
                    View::Grid => View::Focus,
                    other => other,
                }
            }
            Action::ZoomOut => match self.view {
                View::Focus => self.view = View::Split,
                View::Split => self.view = View::Fleet,
                View::Grid => self.view = View::Fleet,
                View::NewLane => self.view = View::Fleet,
                View::Timeline | View::Sessions | View::Search | View::AddRepo | View::Agents => {
                    self.view = View::Fleet
                }
                View::Fleet => self.should_quit = true,
            },
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
            Action::JumpNeedsYou => {
                let target = self
                    .visible_lanes()
                    .iter()
                    .position(|l| l.agent_sessions.iter().any(|s| s.status.needs_you()));
                match target {
                    Some(i) => self.selected = i,
                    None => self.status = "no agents need you".into(),
                }
            }
            Action::StopAgent => self.stop_agent().await,
            Action::Pin => self.toggle_pin().await,
            Action::Merge => self.merge_lane().await,
            Action::SpawnAgent => self.spawn_agent().await,
            Action::AdoptAgent => self.adopt_agent().await,
            Action::OpenTerminal => self.open_terminal().await,
            Action::AttachTerminal => self.attach_latest_terminal().await,
            Action::ToggleMouse => self.toggle_mouse(),
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

fn enable_mouse() {
    use ratatui::crossterm::event::EnableMouseCapture;
    let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
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
        if app.view != View::Focus && app.scroll != 0 {
            app.reset_scroll();
        }
        terminal.draw(|f| view::render(f, app))?;
        if app.should_quit {
            return Ok(());
        }
        if let Some(lane) = app.attach_request.take() {
            do_attach(terminal, app, lane).await;
            continue;
        }
        if let Some(target) = app.attach_target.take() {
            do_attach_target(terminal, app, &target).await;
            continue;
        }
        tokio::select! {
            maybe = in_rx.recv() => match maybe {
                Some(ev) => app.handle_event(ev).await,
                None => return Ok(()),
            },
            note = next_note(events), if events_alive => match note {
                Some(n) => app.on_notification(n).await,
                None => events_alive = false,
            },
            _ = tick.tick() => {
                // Keep agent state fresh in views that show it, so an agent that exits on its
                // own (e.g. `/exit`) is noticed promptly and Focus drops back to Split.
                if matches!(app.view, View::Focus | View::Split | View::Grid) {
                    app.refresh().await;
                }
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
    app.input_suspended.store(false, Ordering::Relaxed);
    app.last_viewport.clear(); // force a viewport resync after returning
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
