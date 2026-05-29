//! Application state and the interactive event loop.

use std::collections::HashMap;
use std::path::PathBuf;
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
            grid_active: 0,
            timeline: None,
            timeline_zoom: Zoom::Day,
            sessions: Vec::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            recent_commits: Vec::new(),
            recent_commits_lane: None,
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
                self.handle_mouse(me);
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
            if key.code == KeyCode::Esc {
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
        if key.code == KeyCode::Char('i') {
            self.focus_insert = true;
            return;
        }
        if let Some(action) = keybinds::nav(key) {
            self.apply(action).await;
        }
    }

    /// Key handling in the Focus view: command mode + insert (live passthrough to the agent).
    async fn focus_key(&mut self, key: KeyEvent) {
        if self.focus_insert {
            if key.code == KeyCode::Esc {
                self.focus_insert = false;
                return;
            }
            self.send_agent_key(key).await;
            return;
        }
        match key.code {
            KeyCode::Char('i') | KeyCode::Enter => self.focus_insert = true,
            KeyCode::Char('e') => self.spawn_agent().await,
            KeyCode::Char('s') => self.stop_agent().await,
            KeyCode::Char('a') => self.attach_request = self.selected_lane().map(|l| l.id),
            KeyCode::Char('m') => self.merge_lane().await,
            KeyCode::Char('c') => {
                if let Some(p) = self.selected_lane().map(|l| l.worktree.path.clone()) {
                    self.cd_target = Some(p);
                    self.should_quit = true;
                }
            }
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
        if let Some(id) = self.selected_lane().map(|l| l.id) {
            match self
                .client
                .call(
                    "agent.spawn",
                    Some(json!({ "lane_id": id, "agent": "claude-code" })),
                )
                .await
            {
                Ok(_) => {
                    self.status = "spawned claude".into();
                    self.view = View::Focus;
                    self.focus_insert = false;
                }
                Err(e) => self.status = format!("spawn failed: {e}"),
            }
        }
    }

    async fn stop_agent(&mut self) {
        if let Some(id) = self.selected_lane().map(|l| l.id) {
            let _ = self
                .client
                .call("agent.stop", Some(json!({ "lane_id": id })))
                .await;
            self.status = "stopped agent".into();
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
            Action::CdToLane => {
                if let Some(path) = self.selected_lane().map(|l| l.worktree.path.clone()) {
                    self.cd_target = Some(path);
                    self.should_quit = true;
                }
            }
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
    enable_mouse();
    let (in_tx, mut in_rx) = mpsc::channel::<Event>(128);
    std::thread::spawn(move || {
        while let Ok(ev) = ratatui::crossterm::event::read() {
            if in_tx.blocking_send(ev).is_err() {
                break;
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
        terminal.draw(|f| view::render(f, app))?;
        if app.should_quit {
            return Ok(());
        }
        if let Some(lane) = app.attach_request.take() {
            do_attach(terminal, app, lane).await;
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
            _ = tick.tick() => {}
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
    disable_mouse();
    ratatui::restore();
    let _ = std::process::Command::new("tmux")
        .args(["attach", "-t", &target])
        .status();
    *terminal = ratatui::init();
    enable_mouse();
    let _ = terminal.clear();
    app.last_viewport.clear(); // force a viewport resync after returning
}

/// Translate a key press into a tmux key spec. `(spec, literal)` — literal printable text
/// is sent with `send-keys -l`; named keys (Enter, Tab, BTab, arrows, C-c, …) without it.
fn translate_key(key: &KeyEvent) -> Option<(String, bool)> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let spec = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                return Some((format!("C-{}", c.to_ascii_lowercase()), false));
            }
            return Some((c.to_string(), true)); // literal printable
        }
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
    Some((spec.to_string(), false))
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
