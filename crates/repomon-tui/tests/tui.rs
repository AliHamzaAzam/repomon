//! End-to-end TUI test: embedded daemon -> client -> App -> rendered Fleet frame.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use repomon_core::{Config, Store};
use repomon_daemon::{serve, Ctx};
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
    let orch_raw = "REPOMIND_PANE_SENTINEL hello from repomind\nsecond line of the chat".to_string();
    let orch_lines = repomon_tui::view::parse_pane(&orch_raw);
    app.orch_output = Some(repomon_tui::app::Pane {
        raw: orch_raw,
        lines: orch_lines,
        cursor: None,
    });
    app.selected = 0; // the pinned repomind row is always row 0 of the fleet
    app.view = View::Split;
    assert!(app.orchestrator_selected(), "row 0 must be the pinned repomind row");
    let split = render_to_string(&app, 100, 40).unwrap();
    assert!(
        split.contains("REPOMIND_PANE_SENTINEL"),
        "split right column must show repomind's pane when the pinned row is selected:\n{split}"
    );
    // And when repomind is off, the right column shows the start hint, not a blank.
    app.orch_output = None;
    app.orch_running = false;
    let split_off = render_to_string(&app, 100, 40).unwrap();
    assert!(
        split_off.contains("repomind is off"),
        "split right column must show the off/start hint when repomind isn't running:\n{split_off}"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
}
