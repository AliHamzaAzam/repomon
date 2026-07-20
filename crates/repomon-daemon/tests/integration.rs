//! End-to-end: start the daemon on a temp socket and exercise the JSON-RPC surface.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::{Ctx, serve};
use serde_json::json;
use tokio::net::UnixStream;

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

/// Like `git`, but pins the author+committer date so commits land outside "today".
fn git_dated(dir: &Path, args: &[&str], date: &str) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@e.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@e.com")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

async fn call(
    stream: &mut UnixStream,
    id: u64,
    method: &str,
    params: Option<serde_json::Value>,
) -> Response {
    let req = Request::new(id, method, params);
    protocol::write_message(stream, &req).await.unwrap();
    let frame = protocol::read_frame(stream)
        .await
        .unwrap()
        .expect("response frame");
    serde_json::from_slice(&frame).unwrap()
}

#[tokio::test]
async fn daemon_serves_repo_and_lane_methods() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);

    // Short socket path (macOS caps UDS paths at ~104 chars).
    let sock = std::env::temp_dir().join(format!("repomon-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };

    // Wait for the socket to come up.
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Empty fleet to start.
    let r = call(&mut stream, 1, "repo.list", None).await;
    assert_eq!(r.result.unwrap(), json!([]));

    // Add a real git repo.
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "init"]);

    let r = call(
        &mut stream,
        2,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    assert!(r.error.is_none(), "repo.add errored: {:?}", r.error);

    // The main worktree appears as a lane.
    let r = call(&mut stream, 3, "lane.list", None).await;
    let lanes = r.result.unwrap();
    let arr = lanes.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["worktree"]["is_main"], json!(true));
    assert_eq!(arr[0]["state"]["branch"], json!("main"));

    // daemon.status reports our version.
    let r = call(&mut stream, 4, "daemon.status", None).await;
    let status = r.result.unwrap();
    assert_eq!(status["version"], json!(repomon_core::version()));
    assert_eq!(status["repos"], json!(1));

    // Unknown method is a proper JSON-RPC error.
    let r = call(&mut stream, 5, "no.such.method", None).await;
    assert!(r.result.is_none());
    assert_eq!(r.error.unwrap().code, -32601);

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn daemon_spawns_and_drives_an_agent() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping agent spawn test");
        return;
    }
    // A unique tmux session so we never touch the user's real `repomon` session.
    let session = format!("repomon-agent-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!("repomon-agent-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Register a repo and grab its lane.
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    git(repo_dir.path(), &["commit", "--allow-empty", "-m", "init"]);
    call(
        &mut stream,
        1,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    let lanes = call(&mut stream, 2, "lane.list", None)
        .await
        .result
        .unwrap();
    let lane_id = lanes[0]["id"].as_i64().unwrap();

    // Spawn a plain shell as the "agent", drive it, and read its output back.
    let spawned = call(
        &mut stream,
        3,
        "agent.spawn",
        Some(json!({ "lane_id": lane_id, "agent": "bash" })),
    )
    .await;
    assert!(
        spawned.error.is_none(),
        "agent.spawn errored: {:?}",
        spawned.error
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    call(
        &mut stream,
        4,
        "agent.send_input",
        Some(json!({ "lane_id": lane_id, "text": "echo HELLO_FROM_AGENT_XYZ" })),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;

    let captured = call(
        &mut stream,
        5,
        "agent.capture",
        Some(json!({ "lane_id": lane_id })),
    )
    .await;
    let content = captured.result.unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        content.contains("HELLO_FROM_AGENT_XYZ"),
        "captured pane was: {content:?}"
    );

    // Force an overlay computation while the agent is still running, so the daemon's tmux-window
    // cache (`last_good_windows`) is populated with the live window — the precondition for the
    // regression checked below (the total-vanish debounce only has something stale to fall back
    // on once it has seen the window at least once).
    let lanes = call(&mut stream, 6, "lane.list", None)
        .await
        .result
        .unwrap();
    let lane = lanes
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"].as_i64() == Some(lane_id))
        .expect("spawned lane");
    assert!(
        lane["agent_sessions"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "expected a live agent session before stop: {lane:?}"
    );

    // Stop the agent.
    call(
        &mut stream,
        7,
        "agent.stop",
        Some(json!({ "lane_id": lane_id })),
    )
    .await;

    // Regression check: immediately after `agent.stop` returns — no sleep, no poll — the lane
    // must report no agent. This is the bug: `agent.stop` kills the tmux window but used to leave
    // the daemon's window-liveness caches stale, so `resolve_windows`'s `EMPTY_WINDOWS_CONFIRM`
    // debounce (meant to ride out a *transient* tmux-server bounce) misread our own deliberate
    // kill as one of those and held the dead window in `last_good_windows` for one more tick —
    // long enough for an immediately-following read (e.g. `delete_lane`'s impact summary) to see
    // the just-stopped agent as still live.
    let lanes = call(&mut stream, 8, "lane.list", None)
        .await
        .result
        .unwrap();
    let lane = lanes
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"].as_i64() == Some(lane_id))
        .expect("lane still present after stop");
    assert_eq!(
        lane["agent_sessions"],
        json!([]),
        "agent session should be gone immediately after stop (no polling): {lane:?}"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}

#[tokio::test]
async fn streams_agent_output_for_visible_lanes() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping streaming test");
        return;
    }
    let session = format!("repomon-stream-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!("repomon-stream-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    // The output streamer is what we're testing.
    tokio::spawn(repomon_daemon::stream_output(ctx.clone()));

    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Do all request/response setup BEFORE subscribing, so responses aren't interleaved
    // with pushed event notifications.
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    git(repo_dir.path(), &["commit", "--allow-empty", "-m", "init"]);
    call(
        &mut stream,
        1,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    let lanes = call(&mut stream, 2, "lane.list", None)
        .await
        .result
        .unwrap();
    let lane_id = lanes[0]["id"].as_i64().unwrap();

    call(
        &mut stream,
        3,
        "agent.spawn",
        Some(json!({ "lane_id": lane_id, "agent": "bash" })),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    call(
        &mut stream,
        4,
        "agent.send_input",
        Some(json!({ "lane_id": lane_id, "text": "echo STREAM_MARKER_XYZ" })),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Subscribe last, then mark the lane visible — the streamer should push its pane content.
    call(
        &mut stream,
        5,
        "subscribe",
        Some(json!({ "topics": ["*"] })),
    )
    .await;
    call(
        &mut stream,
        6,
        "viewport.set",
        Some(json!({ "lane_ids": [lane_id] })),
    )
    .await;

    // Read pushed notifications looking for our marker.
    let mut found = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Ok(Some(frame))) = tokio::time::timeout(
            Duration::from_millis(500),
            protocol::read_frame(&mut stream),
        )
        .await
        {
            if let Ok(note) = serde_json::from_slice::<protocol::Notification>(&frame) {
                if note.method == "event.agent.output"
                    && note
                        .params
                        .get("content")
                        .and_then(|c| c.as_str())
                        .map(|s| s.contains("STREAM_MARKER_XYZ"))
                        .unwrap_or(false)
                {
                    found = true;
                    break;
                }
            }
        }
    }
    assert!(
        found,
        "did not receive streamed agent output with the marker"
    );

    // (No further requests here — we're subscribed, so responses and events interleave.)
    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}

#[tokio::test]
async fn dashboard_timeline_sessions_search() {
    use repomon_core::{Indexer, Registry};

    // Build a repo with two commits 15 minutes apart so a session is detected.
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    let now = chrono::Utc::now().timestamp();
    git_commit_at(repo_dir.path(), now - 1200, "feat: alpha change");
    git_commit_at(repo_dir.path(), now - 300, "feat: beta change");

    // Index history deterministically (don't rely on the background spawn).
    let store = Store::open_in_memory().unwrap();
    let reg = Registry::new(store.clone());
    let repo = reg.add(repo_dir.path()).await.unwrap();
    let report = Indexer::new(store.clone(), reg.clone())
        .sync(&repo)
        .await
        .unwrap();
    assert_eq!(report.commits_added, 2);

    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-dash-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    let from = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    let to = chrono::Utc::now().to_rfc3339();

    // search
    let r = call(
        &mut stream,
        1,
        "commit.search",
        Some(json!({ "query": "feat" })),
    )
    .await;
    assert_eq!(r.result.unwrap().as_array().unwrap().len(), 2);

    // timeline: one repo row with some density
    let r = call(
        &mut stream,
        2,
        "timeline",
        Some(json!({ "from_iso": from, "to_iso": to, "bucket_secs": 3600 })),
    )
    .await;
    let t = r.result.unwrap();
    assert_eq!(t["rows"].as_array().unwrap().len(), 1);

    // sessions: the two commits (15 min span) form one session
    let r = call(
        &mut stream,
        3,
        "sessions",
        Some(json!({ "from_iso": from, "to_iso": to })),
    )
    .await;
    let sessions = r.result.unwrap();
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["commit_count"], json!(2));

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

fn git_commit_at(dir: &Path, epoch: i64, msg: &str) {
    let date = format!("@{epoch} +0000");
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "--allow-empty", "-m", msg])
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@e.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@e.com")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git commit at {epoch}");
}

#[tokio::test]
async fn fs_browse_marks_repos_and_added() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-browse-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // root/{myrepo(.git), plain, .hidden}
    let root = tempfile::tempdir().unwrap();
    let myrepo = root.path().join("myrepo");
    std::fs::create_dir(&myrepo).unwrap();
    git(&myrepo, &["init", "-b", "main"]);
    std::fs::create_dir(root.path().join("plain")).unwrap();
    std::fs::create_dir(root.path().join(".hidden")).unwrap();

    let r = call(
        &mut stream,
        1,
        "fs.browse",
        Some(json!({ "path": root.path().to_string_lossy() })),
    )
    .await;
    let res = r.result.unwrap();
    let entries = res["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"myrepo"));
    assert!(names.contains(&"plain"));
    assert!(!names.contains(&".hidden"), "hidden dirs are skipped");
    let mr = entries.iter().find(|e| e["name"] == "myrepo").unwrap();
    assert_eq!(mr["is_repo"], json!(true));
    assert_eq!(mr["added"], json!(false));
    assert!(res["parent"].is_string());

    // After registering it, the browser marks it added.
    call(
        &mut stream,
        2,
        "repo.add",
        Some(json!({ "path": myrepo.to_string_lossy() })),
    )
    .await;
    let r = call(
        &mut stream,
        3,
        "fs.browse",
        Some(json!({ "path": root.path().to_string_lossy() })),
    )
    .await;
    let entries = r.result.unwrap();
    let mr = entries["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "myrepo")
        .unwrap()
        .clone();
    assert_eq!(mr["added"], json!(true));

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn agent_detect_lists_builtins_and_customs() {
    let mut config = Config::default();
    config.agents.insert(
        "yolo".into(),
        "claude --dangerously-skip-permissions".into(),
    );
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!("repomon-detect-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    let r = call(&mut stream, 1, "agent.detect", None).await;
    let choices = r.result.unwrap();
    let arr = choices.as_array().unwrap();
    let names: Vec<&str> = arr.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"claude-code"));
    assert!(names.contains(&"codex"));
    assert!(names.contains(&"aider"));
    let yolo = arr
        .iter()
        .find(|c| c["name"] == "yolo")
        .expect("custom agent listed");
    assert_eq!(yolo["custom"], json!(true));
    assert_eq!(
        yolo["command"],
        json!("claude --dangerously-skip-permissions")
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn agent_spawn_uses_custom_command() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping custom-command spawn test");
        return;
    }
    let session = format!("repomon-custom-it-{}", std::process::id());
    let mut config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    // A "custom agent" that's just a marker so we can prove the command launched.
    config.agents.insert(
        "marker".into(),
        "bash -c 'echo CUSTOM_AGENT_OK; sleep 30'".into(),
    );
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!("repomon-custom-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    git(repo_dir.path(), &["commit", "--allow-empty", "-m", "init"]);
    call(
        &mut stream,
        1,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    let lanes = call(&mut stream, 2, "lane.list", None)
        .await
        .result
        .unwrap();
    let lane_id = lanes[0]["id"].as_i64().unwrap();

    call(
        &mut stream,
        3,
        "agent.spawn",
        Some(json!({ "lane_id": lane_id, "agent": "marker" })),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let cap = call(
        &mut stream,
        4,
        "agent.capture",
        Some(json!({ "lane_id": lane_id })),
    )
    .await;
    let content = cap.result.unwrap()["content"].as_str().unwrap().to_string();
    assert!(
        content.contains("CUSTOM_AGENT_OK"),
        "custom command output: {content:?}"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}

#[tokio::test]
async fn agent_manager_add_set_default_and_remove() {
    let store = Store::open_in_memory().unwrap();
    // Isolate config writes to a tempdir so we never touch the real ~/.config/repomon.
    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("config.toml");
    let ctx = Ctx::new_with_config_path(store, Config::default(), None, cfg_path.clone());
    let sock = std::env::temp_dir().join(format!("repomon-agmgr-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Add a custom agent → it appears in detect, marked custom, and is persisted to disk.
    let r = call(
        &mut stream,
        1,
        "agent.add",
        Some(json!({ "name": "yolo", "command": "claude --dangerously-skip-permissions" })),
    )
    .await;
    assert!(r.error.is_none(), "agent.add: {:?}", r.error);
    let saved = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(saved.contains("yolo"), "config.toml: {saved}");

    let arr = call(&mut stream, 2, "agent.detect", None)
        .await
        .result
        .unwrap();
    let yolo = arr
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "yolo")
        .expect("custom listed");
    assert_eq!(yolo["custom"], json!(true));
    assert_eq!(yolo["default"], json!(false));

    // Adding under a built-in name is rejected.
    let r = call(
        &mut stream,
        3,
        "agent.add",
        Some(json!({ "name": "claude-code", "command": "claude" })),
    )
    .await;
    assert!(r.error.is_some(), "built-in name should be rejected");

    // Set the custom agent as default → detect reflects it, and so does the file.
    let r = call(
        &mut stream,
        4,
        "agent.set_default",
        Some(json!({ "name": "yolo" })),
    )
    .await;
    assert!(r.error.is_none(), "set_default: {:?}", r.error);
    let arr = call(&mut stream, 5, "agent.detect", None)
        .await
        .result
        .unwrap();
    let yolo = arr
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "yolo")
        .unwrap();
    assert_eq!(yolo["default"], json!(true));
    assert!(
        std::fs::read_to_string(&cfg_path)
            .unwrap()
            .contains("default_agent")
    );

    // A built-in can also be the default.
    let r = call(
        &mut stream,
        6,
        "agent.set_default",
        Some(json!({ "name": "claude-code" })),
    )
    .await;
    assert!(r.error.is_none());

    // Removing a built-in is rejected; removing the custom agent succeeds.
    let r = call(
        &mut stream,
        7,
        "agent.remove",
        Some(json!({ "name": "codex" })),
    )
    .await;
    assert!(r.error.is_some(), "removing a built-in should be rejected");
    let r = call(
        &mut stream,
        8,
        "agent.remove",
        Some(json!({ "name": "yolo" })),
    )
    .await;
    assert!(r.error.is_none(), "remove yolo: {:?}", r.error);
    let arr = call(&mut stream, 9, "agent.detect", None)
        .await
        .result
        .unwrap();
    assert!(
        !arr.as_array().unwrap().iter().any(|c| c["name"] == "yolo"),
        "yolo should be gone"
    );

    // Set-default on an unknown agent is rejected.
    let r = call(
        &mut stream,
        10,
        "agent.set_default",
        Some(json!({ "name": "ghost" })),
    )
    .await;
    assert!(r.error.is_some(), "unknown default should be rejected");

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn commit_recent_returns_latest_even_when_none_today() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-recent-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // A repo whose only commits are from last year — nothing "today".
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    git_dated(
        repo_dir.path(),
        &["commit", "--allow-empty", "-m", "old one"],
        "2024-01-01T00:00:00",
    );
    git_dated(
        repo_dir.path(),
        &["commit", "--allow-empty", "-m", "old two"],
        "2024-01-02T00:00:00",
    );
    call(
        &mut stream,
        1,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    let lanes = call(&mut stream, 2, "lane.list", None)
        .await
        .result
        .unwrap();
    let lane_id = lanes[0]["id"].as_i64().unwrap();

    // Nothing today...
    let today = call(&mut stream, 3, "commit.today", None)
        .await
        .result
        .unwrap();
    assert!(
        today.as_array().unwrap().is_empty(),
        "expected no commits today, got {today}"
    );

    // ...but commit.recent still returns the branch's latest, newest first.
    let recent = call(
        &mut stream,
        4,
        "commit.recent",
        Some(json!({ "lane_id": lane_id, "limit": 5 })),
    )
    .await
    .result
    .unwrap();
    let arr = recent.as_array().unwrap();
    assert_eq!(arr.len(), 2, "recent commits: {recent}");
    assert_eq!(arr[0]["summary"], json!("old two")); // newest first
    assert_eq!(arr[1]["summary"], json!("old one"));

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

// No orchestrator is ever started here, so `reconcile_orchestrator` returns false on its
// first check (no tracked session) without touching tmux — this doesn't need a live tmux server.
#[tokio::test]
async fn orchestrator_input_errors_loudly_when_not_running() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-orch-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Typing to a dead orchestrator must fail loudly instead of silently no-op'ing at the tmux
    // layer.
    let r = call(
        &mut stream,
        1,
        "orchestrator.send_input",
        Some(json!({ "text": "hello", "enter": false })),
    )
    .await;
    assert!(r.result.is_none());
    let err = r
        .error
        .expect("send_input to a dead orchestrator should error");
    assert!(
        err.message.contains("repomind isn't running"),
        "unexpected message: {}",
        err.message
    );

    // Same reconcile-first guard applies to `orchestrator.key`.
    let r = call(
        &mut stream,
        2,
        "orchestrator.key",
        Some(json!({ "key": "Enter", "literal": false })),
    )
    .await;
    assert!(r.error.is_some(), "key to a dead orchestrator should error");

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn lane_diff_reports_commits_ahead_and_uncommitted_stat() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-diff-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // A repo with one commit on main.
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "init"]);
    let r = call(
        &mut stream,
        1,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    let repo_id = r.result.unwrap()["id"].as_i64().unwrap();

    // A lane branched off main.
    let wt_parent = tempfile::tempdir().unwrap();
    let wt_path = wt_parent.path().join("feat");
    let r = call(
        &mut stream,
        2,
        "lane.create",
        Some(json!({
            "repo_id": repo_id,
            "branch": "feat/thing",
            "source_branch": "main",
            "path": wt_path.to_string_lossy(),
        })),
    )
    .await;
    assert!(r.error.is_none(), "lane.create errored: {:?}", r.error);
    let lane_id = r.result.unwrap()["id"].as_i64().unwrap();

    // One commit ahead of main...
    std::fs::write(wt_path.join("a.txt"), "a\n").unwrap();
    git(&wt_path, &["add", "a.txt"]);
    git(&wt_path, &["commit", "-m", "feat: add a"]);
    // ...plus an uncommitted (unstaged) change and an untracked file.
    std::fs::write(wt_path.join("README.md"), "changed\n").unwrap();
    std::fs::write(wt_path.join("scratch.txt"), "scratch\n").unwrap();
    // No file watcher runs against this bare `Ctx` (that's wired up in `main.rs`), so the cached
    // clean state from `lane.create`'s listing needs an explicit nudge to re-walk — same as
    // `repomon_core::lane::tests::worktree_file_activity_is_detected`.
    ctx.lanes.invalidate_state(&wt_path);

    let r = call(
        &mut stream,
        3,
        "lane.diff",
        Some(json!({ "lane_id": lane_id })),
    )
    .await;
    assert!(r.error.is_none(), "lane.diff errored: {:?}", r.error);
    let d = r.result.unwrap();
    assert_eq!(d["base"], json!("main"));
    assert!(d["merge_base"].as_str().unwrap_or_default().len() >= 7);
    let commits = d["commits"].as_str().unwrap();
    assert!(commits.contains("feat: add a"), "commits was: {commits:?}");
    assert!(d.get("commits_truncated").is_none());
    let committed_stat = d["committed_stat"].as_str().unwrap();
    assert!(
        committed_stat.contains("a.txt"),
        "committed_stat was: {committed_stat:?}"
    );
    let uncommitted_stat = d["uncommitted_stat"].as_str().unwrap();
    assert!(
        uncommitted_stat.contains("README.md"),
        "uncommitted_stat was: {uncommitted_stat:?}"
    );
    assert_eq!(d["untracked"], json!(1));
    // No patch without include_patch.
    assert!(d.get("patch").is_none());

    // include_patch=true honors a tiny max_patch_chars cap.
    let r = call(
        &mut stream,
        4,
        "lane.diff",
        Some(json!({ "lane_id": lane_id, "include_patch": true, "max_patch_chars": 10 })),
    )
    .await;
    assert!(r.error.is_none(), "lane.diff errored: {:?}", r.error);
    let d = r.result.unwrap();
    let patch = d["patch"].as_str().unwrap();
    assert_eq!(patch.chars().count(), 10, "patch was: {patch:?}");
    assert_eq!(d["patch_truncated"], json!(true));

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn repo_notes_get_set_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let cfg_dir = tempfile::tempdir().unwrap();
    let notes_dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new_with_paths(
        store,
        Config::default(),
        None,
        cfg_dir.path().join("config.toml"),
        notes_dir.path().to_path_buf(),
    );
    let sock = std::env::temp_dir().join(format!("repomon-notes-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "init"]);
    let r = call(
        &mut stream,
        1,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    let repo = r.result.unwrap();
    let repo_id = repo["id"].as_i64().unwrap();
    let repo_name = repo["name"].as_str().unwrap().to_string();

    // Fresh repo: no notes yet.
    let r = call(
        &mut stream,
        2,
        "repo.notes.get",
        Some(json!({ "repo_id": repo_id })),
    )
    .await;
    assert!(r.error.is_none(), "get errored: {:?}", r.error);
    let got = r.result.unwrap();
    assert_eq!(got["exists"], json!(false));
    assert_eq!(got["content"], json!(""));
    assert_eq!(got["repo_id"], json!(repo_id));
    assert_eq!(got["name"], json!(repo_name));

    // Set → get round-trips and reports the path inside the injected notes dir.
    let r = call(
        &mut stream,
        3,
        "repo.notes.set",
        Some(json!({ "repo_id": repo_id, "content": "use `pnpm test`, never `npm test`" })),
    )
    .await;
    assert!(r.error.is_none(), "set errored: {:?}", r.error);
    let r = call(
        &mut stream,
        4,
        "repo.notes.get",
        Some(json!({ "repo_id": repo_id })),
    )
    .await;
    let got = r.result.unwrap();
    assert_eq!(got["exists"], json!(true));
    assert_eq!(got["content"], json!("use `pnpm test`, never `npm test`"));
    let path = std::path::PathBuf::from(got["path"].as_str().unwrap());
    assert!(path.starts_with(notes_dir.path()), "path was {path:?}");

    // Over the cap: rejected with an error that names the limit.
    let r = call(
        &mut stream,
        5,
        "repo.notes.set",
        Some(json!({ "repo_id": repo_id, "content": "x".repeat(8193) })),
    )
    .await;
    let err = r.error.expect("oversized set must error");
    assert!(
        err.message.contains("8192"),
        "unhelpful error: {}",
        err.message
    );

    // Unknown repo: not found.
    let r = call(
        &mut stream,
        6,
        "repo.notes.get",
        Some(json!({ "repo_id": 999_999 })),
    )
    .await;
    assert!(r.error.is_some(), "bogus repo_id must error");

    // Hand-edited file (the human contract): get picks it up directly.
    std::fs::write(&path, "hand-edited\n").unwrap();
    let r = call(
        &mut stream,
        7,
        "repo.notes.get",
        Some(json!({ "repo_id": repo_id })),
    )
    .await;
    assert_eq!(r.result.unwrap()["content"], json!("hand-edited\n"));

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn journal_append_and_query() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-jrnl-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Two sessions: "a" (the previous one) and "b" (the current one).
    for (i, (session, action, params)) in [
        ("a", "session_start", None),
        (
            "a",
            "spawn_agent",
            Some("{\"task\":\"fix the auth refactor\"}"),
        ),
        ("a", "merge_lane", None),
        ("b", "session_start", None),
    ]
    .iter()
    .enumerate()
    {
        let r = call(
            &mut stream,
            i as u64 + 1,
            "journal.append",
            Some(json!({ "session": session, "action": action, "params": params })),
        )
        .await;
        assert!(r.error.is_none(), "append errored: {:?}", r.error);
        assert!(r.result.unwrap()["id"].as_i64().unwrap() > 0);
    }

    // Plain query: newest first.
    let r = call(&mut stream, 10, "journal.query", Some(json!({}))).await;
    let entries = r.result.unwrap()["entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0]["action"], json!("session_start"));
    assert_eq!(entries[0]["session"], json!("b"));

    // Search filters (case-insensitive substring over params).
    let r = call(
        &mut stream,
        11,
        "journal.query",
        Some(json!({ "query": "AUTH" })),
    )
    .await;
    let entries = r.result.unwrap()["entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 1, "entries: {entries:?}");
    assert_eq!(entries[0]["action"], json!("spawn_agent"));

    // Recap: everything after session a's start, ascending.
    let r = call(
        &mut stream,
        12,
        "journal.query",
        Some(json!({ "since_last_session": true })),
    )
    .await;
    let entries = r.result.unwrap()["entries"].as_array().unwrap().clone();
    let actions: Vec<&str> = entries
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["spawn_agent", "merge_lane", "session_start"]);

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn playbook_lifecycle_over_rpc() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-pb-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Save a draft.
    let r = call(
        &mut stream,
        1,
        "playbook.save",
        Some(json!({ "name": "release-all", "content": "1. lane per repo\n2. pnpm test" })),
    )
    .await;
    assert!(r.error.is_none(), "save errored: {:?}", r.error);
    assert_eq!(r.result.unwrap()["status"], json!("draft"));

    // Drafts are invisible to search.
    let r = call(
        &mut stream,
        2,
        "playbook.search",
        Some(json!({ "query": "release" })),
    )
    .await;
    assert_eq!(r.result.unwrap()["playbooks"], json!([]));

    // Approve, then search hits.
    let r = call(
        &mut stream,
        3,
        "playbook.approve",
        Some(json!({ "name": "release-all" })),
    )
    .await;
    assert!(r.error.is_none(), "approve errored: {:?}", r.error);
    let r = call(
        &mut stream,
        4,
        "playbook.search",
        Some(json!({ "query": "pnpm" })),
    )
    .await;
    let books = r.result.unwrap()["playbooks"].as_array().unwrap().clone();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0]["name"], json!("release-all"));

    // Saving over an approved playbook keeps the approved text live.
    let r = call(
        &mut stream,
        5,
        "playbook.save",
        Some(json!({ "name": "release-all", "content": "v2 steps" })),
    )
    .await;
    assert_eq!(r.result.unwrap()["status"], json!("approved"));
    let r = call(
        &mut stream,
        6,
        "playbook.search",
        Some(json!({ "query": "release" })),
    )
    .await;
    let books = r.result.unwrap()["playbooks"].as_array().unwrap().clone();
    assert!(
        books[0]["content"].as_str().unwrap().contains("pnpm test"),
        "approved content must stay live until re-approval: {books:?}"
    );

    // list shows the pending revision for the approval surface.
    let r = call(&mut stream, 7, "playbook.list", None).await;
    let books = r.result.unwrap()["playbooks"].as_array().unwrap().clone();
    assert_eq!(books[0]["draft_content"], json!("v2 steps"));

    // Validation: hostile name and oversized content are rejected with the limits named.
    let r = call(
        &mut stream,
        8,
        "playbook.save",
        Some(json!({ "name": "bad name!", "content": "x" })),
    )
    .await;
    let err = r.error.expect("bad name must error");
    assert!(err.message.contains("64"), "unhelpful: {}", err.message);
    let r = call(
        &mut stream,
        9,
        "playbook.save",
        Some(json!({ "name": "big", "content": "x".repeat(16385) })),
    )
    .await;
    let err = r.error.expect("oversized must error");
    assert!(err.message.contains("16384"), "unhelpful: {}", err.message);

    // Unknown-name approve/delete error.
    let r = call(
        &mut stream,
        10,
        "playbook.delete",
        Some(json!({ "name": "nope" })),
    )
    .await;
    assert!(r.error.is_some());

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn schedule_add_list_remove() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-sch-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Valid add returns the row plus its computed next firing.
    let r = call(
        &mut stream,
        1,
        "schedule.add",
        Some(json!({ "spec": "daily 09:00", "prompt": "morning fleet briefing" })),
    )
    .await;
    assert!(r.error.is_none(), "add errored: {:?}", r.error);
    let sched = r.result.unwrap();
    let id = sched["id"].as_i64().unwrap();
    assert_eq!(sched["max_actions"], json!(10), "default cap should be 10");
    assert!(sched["next_run"].is_string(), "missing next_run: {sched}");

    // Bad spec teaches the grammar.
    let r = call(
        &mut stream,
        2,
        "schedule.add",
        Some(json!({ "spec": "tuesdays 09:00", "prompt": "x" })),
    )
    .await;
    let err = r.error.expect("bad spec must error");
    assert!(err.message.contains("daily"), "unhelpful: {}", err.message);

    // Empty prompt rejected; oversized max_actions clamped to 50.
    let r = call(
        &mut stream,
        3,
        "schedule.add",
        Some(json!({ "spec": "every 30m", "prompt": "" })),
    )
    .await;
    assert!(r.error.is_some(), "empty prompt must error");
    let r = call(
        &mut stream,
        4,
        "schedule.add",
        Some(json!({ "spec": "every 30m", "prompt": "sweep", "max_actions": 500 })),
    )
    .await;
    assert_eq!(r.result.unwrap()["max_actions"], json!(50));

    // List shows both with next_run.
    let r = call(&mut stream, 5, "schedule.list", None).await;
    let scheds = r.result.unwrap()["schedules"].as_array().unwrap().clone();
    assert_eq!(scheds.len(), 2);
    assert!(scheds.iter().all(|s| s["next_run"].is_string()));

    // Remove; second remove errors.
    let r = call(&mut stream, 6, "schedule.remove", Some(json!({ "id": id }))).await;
    assert!(r.error.is_none(), "remove errored: {:?}", r.error);
    let r = call(&mut stream, 7, "schedule.remove", Some(json!({ "id": id }))).await;
    assert!(r.error.is_some(), "double remove must error");

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn approval_record_and_rules_lifecycle() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-ap-it-{}.sock", std::process::id()));
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
    let mut stream = UnixStream::connect(&sock).await.expect("connect");

    // Three consistent approvals: third one proposes an allowlist entry.
    for (i, expect_propose) in [(1u64, false), (2, false), (3, true)] {
        let r = call(
            &mut stream,
            i,
            "approval.record",
            Some(json!({ "repo": "api", "command": "cargo test -p foo", "verdict": "approve" })),
        )
        .await;
        assert!(r.error.is_none(), "record errored: {:?}", r.error);
        let v = r.result.unwrap();
        assert_eq!(v["pattern"], json!("cargo test"));
        assert_eq!(v["approvals"], json!(i));
        assert_eq!(v["propose"], json!(expect_propose), "at approval {i}: {v}");
    }

    // Confirmed rule: listed, and further records say rule_exists instead of proposing.
    let r = call(
        &mut stream,
        4,
        "approval.allow",
        Some(json!({ "repo": "api", "pattern": "cargo test" })),
    )
    .await;
    assert!(r.error.is_none(), "allow errored: {:?}", r.error);
    let r = call(&mut stream, 5, "approval.list", None).await;
    let rules = r.result.unwrap()["rules"].as_array().unwrap().clone();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["pattern"], json!("cargo test"));
    let r = call(
        &mut stream,
        6,
        "approval.record",
        Some(json!({ "repo": "api", "command": "cargo test --lib", "verdict": "approve" })),
    )
    .await;
    let v = r.result.unwrap();
    assert_eq!(v["rule_exists"], json!(true));
    assert_eq!(v["propose"], json!(false));

    // Always-escalate commands never propose, no matter the streak.
    for i in 10..14u64 {
        let r = call(
            &mut stream,
            i,
            "approval.record",
            Some(json!({ "repo": "api", "command": "git push --force", "verdict": "approve" })),
        )
        .await;
        let v = r.result.unwrap();
        assert_eq!(v["propose"], json!(false), "always-escalate proposed: {v}");
    }

    // A deny resets the streak.
    let r = call(
        &mut stream,
        20,
        "approval.record",
        Some(json!({ "repo": "api", "command": "npm run build", "verdict": "approve" })),
    )
    .await;
    assert_eq!(r.result.unwrap()["approvals"], json!(1));
    let r = call(
        &mut stream,
        21,
        "approval.record",
        Some(json!({ "repo": "api", "command": "npm run build", "verdict": "deny" })),
    )
    .await;
    assert_eq!(r.result.unwrap()["approvals"], json!(0));

    // Remove; second remove errors.
    let r = call(
        &mut stream,
        22,
        "approval.remove",
        Some(json!({ "repo": "api", "pattern": "cargo test" })),
    )
    .await;
    assert!(r.error.is_none());
    let r = call(
        &mut stream,
        23,
        "approval.remove",
        Some(json!({ "repo": "api", "pattern": "cargo test" })),
    )
    .await;
    assert!(r.error.is_some());

    server.abort();
    let _ = std::fs::remove_file(&sock);
}
