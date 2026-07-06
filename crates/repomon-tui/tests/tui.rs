//! End-to-end TUI test: embedded daemon -> client -> App -> rendered Fleet frame.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use repomon_core::{Config, Store};
use repomon_daemon::{Ctx, serve};
use repomon_tui::app::App;
use repomon_tui::client::DaemonClient;
use repomon_tui::render_to_string;
use serde_json::json;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@e.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@e.com")
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// A managed lane session in the given status, optionally sitting on a pending dialog.
fn fake_session(
    status: repomon_core::model::AgentStatus,
    prompt: Option<&str>,
) -> repomon_core::model::AgentSession {
    use repomon_core::agent::prompt::detect_dialog;
    let dialog = prompt.and_then(|q| detect_dialog(&format!("{q}\n❯ 1. Yes\n  2. No")));
    repomon_core::model::AgentSession {
        id: 0,
        agent: repomon_core::model::AgentKind::ClaudeCode,
        repo_id: 1,
        worktree_id: None,
        started_at: chrono::Utc::now(),
        last_activity_at: chrono::Utc::now(),
        ended_at: None,
        manifest_path: std::path::PathBuf::new(),
        tool_call_count: 0,
        title: None,
        status,
        external: false,
        session_id: Some("uuid-test".into()),
        resume_at: None,
        inferred: false,
        tmux_window: Some("lane-1".into()),
        last_message: None,
        pending_prompt: prompt.map(str::to_string),
        pending_dialog: dialog,
        stale: false,
        stalled_since: None,
        ended_turn: false,
        config_dir: None,
        custom_label: None,
    }
}

#[tokio::test]
async fn waiting_badges_distinguish_attention() {
    use repomon_core::model::AgentStatus;
    use repomon_tui::keybinds::View;

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-tui-badges-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let client = DaemonClient::connect(&sock).await.expect("connect");

    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "feat: initial commit"]);
    client
        .call(
            "repo.add",
            Some(json!({ "path": repo_dir.path().to_string_lossy() })),
        )
        .await
        .expect("repo.add");

    let mut app = App::new(client);
    app.refresh().await;
    assert_eq!(app.lanes.len(), 1);

    // The lane row is the one carrying the agent cell.
    let lane_row = |screen: &str| {
        screen
            .lines()
            .find(|l| l.contains("claude"))
            .map(str::to_string)
            .expect("lane row missing")
    };

    // A routine permission ask: the fleet row wears ⏸; the lane switcher names it.
    app.lanes[0].agent_sessions = vec![fake_session(
        AgentStatus::Waiting,
        Some("Bash command — Do you want to proceed?"),
    )];
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        lane_row(&fleet).contains("⏸"),
        "permission glyph missing:\n{fleet}"
    );
    app.view = View::LaneJump;
    app.jump_query = String::new();
    let jump = render_to_string(&app, 100, 40).unwrap();
    assert!(
        jump.contains("⏸ permission"),
        "permission badge missing:\n{jump}"
    );
    app.view = View::Fleet;

    // A real question the agent is deferring to the human: ? on the row, named in the badge.
    app.lanes[0].agent_sessions = vec![fake_session(
        AgentStatus::Waiting,
        Some("Which auth method should we use?"),
    )];
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        lane_row(&fleet).contains(" ? "),
        "question glyph missing:\n{fleet}"
    );
    app.view = View::LaneJump;
    let jump = render_to_string(&app, 100, 40).unwrap();
    assert!(
        jump.contains("⏸ question"),
        "question badge missing:\n{jump}"
    );
    app.view = View::Fleet;

    // A bare end-of-turn wait (no dialog on screen): ✓, and "done" in the badge. Dirty the
    // worktree so the finished turn does NOT read as shippable (that's the next scenario).
    app.lanes[0].agent_sessions = vec![fake_session(AgentStatus::Waiting, None)];
    app.lanes[0].state.dirty.unstaged = 1;
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        lane_row(&fleet).contains("✓"),
        "done glyph missing:\n{fleet}"
    );
    app.view = View::LaneJump;
    let jump = render_to_string(&app, 100, 40).unwrap();
    assert!(jump.contains("⏸ done"), "done badge missing:\n{jump}");
    app.view = View::Fleet;

    // The same finished turn on a CLEAN lane with a this-turn commit: the review hint.
    app.lanes[0].state.dirty.unstaged = 0;
    app.lanes[0].state.last_commit_at = Some(chrono::Utc::now());
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        lane_row(&fleet).contains("✓"),
        "review glyph missing:\n{fleet}"
    );
    app.view = View::LaneJump;
    let jump = render_to_string(&app, 100, 40).unwrap();
    assert!(jump.contains("✓ review?"), "review badge missing:\n{jump}");
    app.view = View::Fleet;

    // A stalled agent (alive but frozen mid-work): ⚠ on the row, duration in the badge.
    let mut stuck = fake_session(AgentStatus::Running, None);
    stuck.stale = true;
    stuck.stalled_since = Some(chrono::Utc::now() - chrono::Duration::minutes(7));
    app.lanes[0].agent_sessions = vec![stuck];
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        lane_row(&fleet).contains("⚠"),
        "stall glyph missing:\n{fleet}"
    );
    assert!(
        fleet.contains("1 need you"),
        "a stalled lane must count as needing you:\n{fleet}"
    );
    app.view = View::LaneJump;
    let jump = render_to_string(&app, 100, 40).unwrap();
    assert!(
        jump.contains("⚠ stalled 7m"),
        "stall badge missing:\n{jump}"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn peek_popup_shows_the_dialog_and_queue() {
    use repomon_core::model::AgentStatus;

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-tui-peek-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let client = DaemonClient::connect(&sock).await.expect("connect");

    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "feat: initial commit"]);
    client
        .call(
            "repo.add",
            Some(json!({ "path": repo_dir.path().to_string_lossy() })),
        )
        .await
        .expect("repo.add");

    let mut app = App::new(client);
    app.refresh().await;
    assert_eq!(app.lanes.len(), 1);

    // With nothing waiting, `v` reports there is nothing to peek at.
    app.open_peek().await;
    assert!(app.peek.is_none(), "no popup without waiting agents");
    assert!(
        app.status.contains("no prompts"),
        "status should say there's nothing to answer: {}",
        app.status
    );

    // A lane sitting on a permission dialog: the popup shows the question, its options with
    // the cursor, the queue counter, and the triage hints.
    app.lanes[0].agent_sessions = vec![fake_session(
        AgentStatus::Waiting,
        Some("Do you want to proceed?"),
    )];
    app.open_peek().await;
    assert!(app.peek.is_some(), "popup should open on the waiting lane");
    // `open_peek` ends with a live pane re-read whose outcome depends on whatever tmux exists
    // on the machine running this test; pin the popup to the seeded dialog so the rendering
    // assertions are deterministic.
    if let Some(p) = app.peek.as_mut() {
        p.dialog = app.lanes[0].agent_sessions[0].pending_dialog.clone();
        p.sel = 0;
    }
    let frame = render_to_string(&app, 100, 40).unwrap();
    assert!(
        frame.contains("Do you want to proceed?"),
        "question missing:\n{frame}"
    );
    assert!(
        frame.contains("▸ 1. Yes"),
        "cursor option missing:\n{frame}"
    );
    assert!(frame.contains("2. No"), "second option missing:\n{frame}");
    assert!(frame.contains("permission"), "class word missing:\n{frame}");
    assert!(frame.contains("1/1"), "queue counter missing:\n{frame}");
    assert!(frame.contains("esc"), "close hint missing:\n{frame}");

    // Arrow keys steer the local selection cursor.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.peek_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .await;
    let frame = render_to_string(&app, 100, 40).unwrap();
    assert!(
        frame.contains("▸ 2. No"),
        "cursor should move to option 2:\n{frame}"
    );

    // Esc closes without sending anything.
    app.peek_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .await;
    assert!(app.peek.is_none(), "esc must close the popup");

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn grid_tiles_plain_shell_terminals() {
    use repomon_core::model::AgentStatus;
    use repomon_tui::keybinds::View;

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-tui-grid-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let client = DaemonClient::connect(&sock).await.expect("connect");

    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "feat: initial commit"]);
    client
        .call(
            "repo.add",
            Some(json!({ "path": repo_dir.path().to_string_lossy() })),
        )
        .await
        .expect("repo.add");

    let mut app = App::new(client);
    app.refresh().await;
    let lane_id = app.lanes[0].id;

    // A lane with a running agent AND an open plain terminal: the grid tiles both.
    app.lanes[0].agent_sessions = vec![fake_session(AgentStatus::Running, None)];
    let term = format!("term-{lane_id}-1");
    app.term_windows = vec![(lane_id, term.clone())];
    let raw = "SHELL_TILE_SENTINEL $ cargo build".to_string();
    app.term_output.insert(
        term.clone(),
        repomon_tui::app::Pane {
            lines: repomon_tui::view::parse_pane(&raw),
            raw,
            cursor: None,
        },
    );
    app.view = View::Grid;
    let grid = render_to_string(&app, 120, 40).unwrap();
    assert!(
        grid.contains("2 live panes"),
        "grid must count the shell tile:\n{grid}"
    );
    assert!(
        grid.contains(&term),
        "shell tile header must name its terminal window:\n{grid}"
    );
    assert!(
        grid.contains("SHELL_TILE_SENTINEL"),
        "shell tile must render the terminal's streamed pane:\n{grid}"
    );

    // The terminal's pane content must not bleed into the agent tile (separate keying).
    assert!(
        grid.contains("no live output"),
        "the agent tile (no streamed output) keeps its own empty state:\n{grid}"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn renders_fleet_with_a_registered_repo() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-tui-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let client = DaemonClient::connect(&sock).await.expect("connect");

    // Register a repo through the daemon.
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "feat: initial commit"]);
    let repo_name = repo_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    client
        .call(
            "repo.add",
            Some(json!({ "path": repo_dir.path().to_string_lossy() })),
        )
        .await
        .expect("repo.add");

    // Build the app, refresh from the daemon, render the Fleet view.
    let mut app = App::new(client);
    app.refresh().await;
    assert_eq!(app.lanes.len(), 1, "one main-worktree lane");

    let screen = render_to_string(&app, 100, 40);
    let screen = screen.unwrap();
    assert!(screen.contains("REPOMON"), "header missing:\n{screen}");
    assert!(screen.contains("FLEET"), "fleet summary missing:\n{screen}");
    assert!(screen.contains(&repo_name), "repo name missing:\n{screen}");
    assert!(screen.contains("main"), "main worktree missing:\n{screen}");
    assert!(
        screen.contains("feat: initial commit"),
        "today commit missing:\n{screen}"
    );
    assert!(screen.contains("click select"), "footer missing:\n{screen}");
    // The footer now chunks groups with the muted "│" rail (was "  ·  ") and, at this width, the
    // long Fleet hints truncate with an ellipsis instead of being clipped mid-word.
    let footer_row = screen
        .lines()
        .find(|l| l.contains("click select"))
        .expect("footer row missing");
    assert!(
        footer_row.contains('│'),
        "footer group rail missing:\n{footer_row}"
    );
    assert!(
        footer_row.contains('…'),
        "footer should truncate with an ellipsis at width 100:\n{footer_row}"
    );

    // The spawn picker lists the agents for the selected lane, with the default tagged and a
    // PATH warning for an undetected command.
    use repomon_core::model::AgentChoice;
    use repomon_tui::keybinds::View;
    app.nl_agents = vec![
        AgentChoice {
            name: "claude-code".into(),
            command: "claude".into(),
            detected: true,
            custom: false,
            default: true,
        },
        AgentChoice {
            name: "codex".into(),
            command: "codex".into(),
            detected: false,
            custom: false,
            default: false,
        },
    ];
    app.spawn_pick_idx = 0;
    app.spawn_pick_lane = Some(app.lanes[0].id);
    app.view = View::SpawnPick;
    let pick = render_to_string(&app, 100, 40).unwrap();
    assert!(
        pick.contains("SPAWN AGENT"),
        "picker header missing:\n{pick}"
    );
    assert!(pick.contains("claude-code"), "agent name missing:\n{pick}");
    assert!(pick.contains("default"), "default tag missing:\n{pick}");
    assert!(
        pick.contains("not on PATH"),
        "PATH warning missing:\n{pick}"
    );

    // The lane switcher lists every lane with an empty query and reports a miss for a query
    // that matches nothing.
    app.jump_query = String::new();
    app.jump_idx = 0;
    app.view = View::LaneJump;
    let jump = render_to_string(&app, 100, 40).unwrap();
    assert!(
        jump.contains("FIND LANE"),
        "switcher header missing:\n{jump}"
    );
    assert!(jump.contains(&repo_name), "lane row missing:\n{jump}");
    app.jump_query = "zzzznope".into();
    let jump = render_to_string(&app, 100, 40).unwrap();
    assert!(
        jump.contains("no lanes match"),
        "miss text missing:\n{jump}"
    );

    // Timeline: the density strip resamples to fill the terminal width; axis + correlation
    // meter render.
    use repomon_core::model::{Correlation, TimelineData, TimelineRow};
    app.timeline = Some(TimelineData {
        from: chrono::Utc::now() - chrono::Duration::days(30),
        to: chrono::Utc::now(),
        bucket_secs: 24 * 3600,
        rows: vec![TimelineRow {
            repo_id: 1,
            repo_name: "alpha".into(),
            density: vec![0, 1, 2, 3, 4, 5],
        }],
        correlations: vec![Correlation {
            a: "alpha".into(),
            b: "beta".into(),
            windows: 5,
            overlap: 0.42,
        }],
    });
    app.view = View::Timeline;
    let tl = render_to_string(&app, 100, 40).unwrap();
    assert!(tl.contains("TIMELINE"), "header missing:\n{tl}");
    assert!(tl.contains("CORRELATIONS"), "correlations missing:\n{tl}");
    assert!(tl.contains("0.42 overlap"), "overlap missing:\n{tl}");
    // 6 source buckets must stretch to fill ~all of the 100-col width, ending in a full block.
    let strip = tl
        .lines()
        .find(|l| l.contains("alpha") && l.contains('█'))
        .expect("density strip missing");
    assert!(
        strip.trim_end().chars().count() > 80,
        "strip not stretched to width:\n{tl}"
    );

    // Notifications: the unread ⚑ badge shows in every view's header; the feed renders a
    // cursor (▸) on the selected row, an unread dot, the new-count, and the action footer.
    use repomon_tui::notify::{NotifEvent, NotifKind};
    app.notifications.push_back(NotifEvent {
        when: chrono::Local::now(),
        kind: NotifKind::NeedsYou,
        lane_id: app.lanes[0].id,
        session_id: None,
        read: true,
        title: "⏸ claude needs you — alpha".into(),
        body: "main · “want me to continue?”".into(),
    });
    app.notifications.push_back(NotifEvent {
        when: chrono::Local::now(),
        kind: NotifKind::RateLimited,
        lane_id: app.lanes[0].id,
        session_id: None,
        read: false,
        title: "⏳ claude hit a usage limit — beta".into(),
        body: "main · resets 06:00".into(),
    });
    app.view = View::Fleet;
    let fl = render_to_string(&app, 100, 40).unwrap();
    assert!(fl.contains("⚑ 1"), "unread badge missing:\n{fl}");
    app.view = View::Notifications;
    app.notif_sel = 1; // cursor on the older (read) event, not the top row
    let nf = render_to_string(&app, 100, 40).unwrap();
    assert!(
        nf.contains("2 event(s) · 1 new"),
        "new count missing:\n{nf}"
    );
    assert!(nf.contains("● "), "unread dot missing:\n{nf}");
    assert!(nf.contains("t attach"), "action footer missing:\n{nf}");
    let cursor_row = nf
        .lines()
        .find(|l| l.trim_start().starts_with('▸'))
        .expect("cursor row missing");
    assert!(
        cursor_row.contains("needs you — alpha"),
        "cursor not on the selected (older) event:\n{nf}"
    );

    // Settings: columns line up even with the longest label and value present.
    app.settings.accent = "amber".into();
    app.settings.default_agent = "claude-work".into();
    app.settings.auto_continue = true;
    app.settings.auto_continue_message = "continue".into();
    app.settings.worktree_template = "~/code/{repo}-wt/{branch}".into();
    app.settings.spawn_prompt = true;
    app.settings.notify_enabled = true;
    app.view = View::Settings;
    let st = render_to_string(&app, 120, 40).unwrap();
    // The long label must not collide with its value (the old "spawnon" bug), and the value
    // column must start at the same screen column on every row.
    assert!(!st.contains("spawnon"), "label/value collision:\n{st}");
    // Value column starts at the same screen column on every row. Measure the column in cells
    // (chars), not bytes, so the multi-byte ▸ marker on the selected row doesn't skew it. Use
    // values that are unique (not a word inside any label) so we match the value, not the label.
    let val_col = |row_label: &str, value: &str| {
        let line = st.lines().find(|l| l.contains(row_label)).unwrap();
        let byte = line.find(value).unwrap();
        line[..byte].chars().count()
    };
    let amber = val_col("accent", "amber");
    let agent = val_col("default agent", "claude-work");
    let template = val_col("worktree template", "~/code");
    assert_eq!(amber, agent, "value column misaligned (agent row):\n{st}");
    assert_eq!(amber, template, "value column misaligned (template):\n{st}");

    // Split view with the pinned repomind row selected (selected == 0): the right column must
    // render repomind's live pane (not blank, not "(no lane selected)").
    app.orch_running = true;
    let orch_raw =
        "REPOMIND_PANE_SENTINEL hello from repomind\nsecond line of the chat".to_string();
    let orch_lines = repomon_tui::view::parse_pane(&orch_raw);
    app.orch_output = Some(repomon_tui::app::Pane {
        raw: orch_raw,
        lines: orch_lines,
        cursor: None,
    });
    app.selected = 0; // the pinned repomind row is always row 0 of the fleet
    app.view = View::Split;
    assert!(
        app.orchestrator_selected(),
        "row 0 must be the pinned repomind row"
    );
    let split = render_to_string(&app, 100, 40).unwrap();
    assert!(
        split.contains("REPOMIND_PANE_SENTINEL"),
        "split right column must show repomind's pane when the pinned row is selected:\n{split}"
    );
    // The pinned row offers the same quick-type entry as a selected lane, plus opening the view.
    assert!(
        split.contains("i type to repomind"),
        "split mode line must offer typing to repomind:\n{split}"
    );
    assert!(
        split.contains("open the full command-center"),
        "split mode line must still offer opening the command-center:\n{split}"
    );
    // While typing to repomind the mode line flips to INSERT (keys forward to repomind).
    app.orch_insert = true;
    let split_insert = render_to_string(&app, 100, 40).unwrap();
    assert!(
        split_insert.contains("keys go to repomind"),
        "split insert mode line must show keys going to repomind:\n{split_insert}"
    );
    app.orch_insert = false;
    // And when repomind is off, the right column shows the start hint, not a blank.
    app.orch_output = None;
    app.orch_running = false;
    let split_off = render_to_string(&app, 100, 40).unwrap();
    assert!(
        split_off.contains("repomind is off"),
        "split right column must show the off/start hint when repomind isn't running:\n{split_off}"
    );

    // repomind attention (B4: the human<->repomind escalation loop): the pinned fleet row wears
    // the needs-you wording when repomind is asking the human something, and the command-center
    // header shows the attention word plus a headline.
    app.orch_running = true;
    app.orch_attention = Some("decision".into());
    app.orch_headline = Some("which auth method?".into());
    app.selected = 0; // the pinned repomind row is always row 0 of the fleet
    app.view = View::Fleet;
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        fleet.contains("repomind · question for you"),
        "pinned row must show the question wording for a decision:\n{fleet}"
    );

    app.orch_attention = Some("permission".into());
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        fleet.contains("repomind · question for you"),
        "pinned row must show the question wording for a permission ask too:\n{fleet}"
    );

    app.orch_attention = Some("end_of_turn".into());
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        fleet.contains("repomind · waiting for you"),
        "pinned row must show the waiting wording for end_of_turn:\n{fleet}"
    );

    app.view = View::Orchestrator;
    let center = render_to_string(&app, 100, 40).unwrap();
    assert!(
        center.contains("end of turn"),
        "command-center header must show the attention word:\n{center}"
    );
    assert!(
        center.contains("which auth method?"),
        "command-center header must show the headline:\n{center}"
    );

    // Back to no attention: the pinned row reverts to the plain chatting/idle/off wording.
    app.orch_attention = None;
    app.orch_headline = None;
    app.view = View::Fleet;
    let fleet = render_to_string(&app, 100, 40).unwrap();
    assert!(
        !fleet.contains("question for you") && !fleet.contains("waiting for you"),
        "pinned row must not show needs-you wording once attention clears:\n{fleet}"
    );

    // `?` help overlay: one row per hint of the current view (parsed from the same strings the
    // footer uses, so they can never drift) plus the global view-switching keys.
    app.help_open = true;
    let help = render_to_string(&app, 100, 44).unwrap();
    assert!(help.contains("KEYS — FLEET"), "help title missing:\n{help}");
    assert!(help.contains("spawn"), "fleet hint rows missing:\n{help}");
    assert!(help.contains("peek"), "peek hint missing:\n{help}");
    assert!(
        help.contains("notifications"),
        "global section missing:\n{help}"
    );
    app.help_open = false;
    let plain = render_to_string(&app, 100, 44).unwrap();
    assert!(!plain.contains("KEYS — FLEET"), "help must close:\n{plain}");

    server.abort();
    let _ = std::fs::remove_file(&sock);
}
