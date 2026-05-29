//! Application state and the interactive event loop.

use std::path::PathBuf;

use anyhow::Result;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;
use repomon_core::model::{Commit, Lane, Repo};
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

use crate::client::DaemonClient;
use crate::keybinds::{self, Action, View};
use crate::theme::Theme;
use crate::view;

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
    pub should_quit: bool,
    pub cd_target: Option<PathBuf>,
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
            should_quit: false,
            cd_target: None,
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

    fn clamp_selection(&mut self) {
        let n = self.visible_lanes().len();
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }

    async fn handle_event(&mut self, ev: Event) {
        let Event::Key(key) = ev else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.status.clear();
        if self.view == View::NewLane {
            self.new_lane_key(key).await;
        } else if self.filtering {
            self.filter_key(key);
        } else if let Some(action) = keybinds::nav(key) {
            self.apply(action).await;
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
            KeyCode::Char(c) => self.nl_branch.push(c),
            KeyCode::Backspace => {
                self.nl_branch.pop();
            }
            KeyCode::Esc => self.view = View::Fleet,
            KeyCode::Enter => self.submit_new_lane().await,
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
                match self.client.call("lane.create", Some(params)).await {
                    Ok(_) => {
                        self.status = format!("created lane {}", self.nl_branch);
                        self.view = View::Fleet;
                        self.refresh().await;
                    }
                    Err(e) => self.status = format!("create failed: {e}"),
                }
            }
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
                    View::Split => View::LaneDetail,
                    other => other,
                }
            }
            Action::ZoomOut => match self.view {
                View::LaneDetail => self.view = View::Split,
                View::Split => self.view = View::Fleet,
                View::NewLane => self.view = View::Fleet,
                View::Fleet => self.should_quit = true,
            },
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
                self.status = "babysit grid arrives in Phase 2".into();
            }
        }
    }
}

/// Run the interactive TUI. Returns a path to cd into on exit, if requested.
pub async fn run(client: DaemonClient) -> Result<Option<PathBuf>> {
    // Ask the daemon to stream events, then subscribe locally.
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
    events: &mut broadcast::Receiver<repomon_core::protocol::Notification>,
) -> Result<()> {
    let mut events_alive = true;
    loop {
        terminal.draw(|f| view::render(f, app))?;
        if app.should_quit {
            return Ok(());
        }
        tokio::select! {
            maybe = in_rx.recv() => match maybe {
                Some(ev) => app.handle_event(ev).await,
                None => return Ok(()),
            },
            note = next_event(events), if events_alive => match note {
                Some(()) => app.refresh().await,
                None => events_alive = false, // daemon event stream closed
            },
        }
    }
}

/// Await the next forwardable event, collapsing lag. `None` means the stream closed.
async fn next_event(
    rx: &mut broadcast::Receiver<repomon_core::protocol::Notification>,
) -> Option<()> {
    loop {
        match rx.recv().await {
            Ok(_) => return Some(()),
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return None,
        }
    }
}
