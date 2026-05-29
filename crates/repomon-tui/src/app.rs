//! Application state and the interactive event loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;
use repomon_core::model::{Commit, Lane, LaneId, Repo, TimelineData, WorkSession};
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
    pub should_quit: bool,
    pub cd_target: Option<PathBuf>,
    /// Latest captured pane content per lane, pushed by `event.agent.output`.
    pub output: HashMap<LaneId, String>,
    /// Whether Focus is in insert mode (typing to the agent).
    pub focus_insert: bool,
    /// The agent input buffer (Focus insert mode).
    pub input: String,
    /// Active tile in the babysit grid.
    pub grid_active: usize,
    pub timeline: Option<TimelineData>,
    pub timeline_zoom: Zoom,
    pub sessions: Vec<WorkSession>,
    pub search_query: String,
    pub search_results: Vec<Commit>,
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
            should_quit: false,
            cd_target: None,
            output: HashMap::new(),
            focus_insert: false,
            input: String::new(),
            grid_active: 0,
            timeline: None,
            timeline_zoom: Zoom::Day,
            sessions: Vec::new(),
            search_query: String::new(),
            search_results: Vec::new(),
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
            View::Fleet | View::NewLane | View::Timeline | View::Sessions | View::Search => {
                Vec::new()
            }
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
        }
    }

    async fn handle_event(&mut self, ev: Event) {
        let Event::Key(key) = ev else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.status.clear();
        match self.view {
            View::NewLane => self.new_lane_key(key).await,
            View::Focus => self.focus_key(key).await,
            View::Search => self.search_key(key).await,
            View::Timeline => self.timeline_key(key).await,
            View::Sessions => self.sessions_key(key).await,
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
            KeyCode::Tab => self.nl_agent_idx = (self.nl_agent_idx + 1) % AGENT_KINDS.len(),
            KeyCode::Char(c) => self.nl_branch.push(c),
            KeyCode::Backspace => {
                self.nl_branch.pop();
            }
            KeyCode::Esc => self.view = View::Fleet,
            KeyCode::Enter => self.submit_new_lane().await,
            _ => {}
        }
    }

    /// Key handling in the Focus view: command mode + insert (talking to the agent).
    async fn focus_key(&mut self, key: KeyEvent) {
        if self.focus_insert {
            match key.code {
                KeyCode::Char(c) => self.input.push(c),
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Enter => self.send_input().await,
                KeyCode::Esc => self.focus_insert = false,
                _ => {}
            }
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
                let agent = AGENT_KINDS[self.nl_agent_idx % AGENT_KINDS.len()];
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

    async fn send_input(&mut self) {
        if self.input.is_empty() {
            return;
        }
        if let Some(id) = self.selected_lane().map(|l| l.id) {
            let text = std::mem::take(&mut self.input);
            if let Err(e) = self
                .client
                .call(
                    "agent.send_input",
                    Some(json!({ "lane_id": id, "text": text })),
                )
                .await
            {
                self.status = format!("send failed: {e}");
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
                View::Timeline | View::Sessions | View::Search => self.view = View::Fleet,
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
                    _ => {}
                }
            }
            Action::Quit => self.should_quit = true,
            Action::NewLane => {
                self.view = View::NewLane;
                self.nl_branch.clear();
                self.nl_repo_idx = 0;
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
            Action::Attach => self.attach_request = self.selected_lane().map(|l| l.id),
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
pub async fn run(client: DaemonClient) -> Result<Option<PathBuf>> {
    let _ = client
        .call("subscribe", Some(json!({ "topics": ["*"] })))
        .await;
    let mut events = client.subscribe();

    let mut app = App::new(client);
    app.refresh().await;

    let mut terminal = ratatui::init();
    let (in_tx, mut in_rx) = mpsc::channel::<Event>(128);
    std::thread::spawn(move || {
        while let Ok(ev) = ratatui::crossterm::event::read() {
            if in_tx.blocking_send(ev).is_err() {
                break;
            }
        }
    });

    let outcome = event_loop(&mut terminal, &mut app, &mut in_rx, &mut events).await;
    ratatui::restore();
    outcome?;
    Ok(app.cd_target)
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
    ratatui::restore();
    let _ = std::process::Command::new("tmux")
        .args(["attach", "-t", &target])
        .status();
    *terminal = ratatui::init();
    let _ = terminal.clear();
    app.last_viewport.clear(); // force a viewport resync after returning
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
