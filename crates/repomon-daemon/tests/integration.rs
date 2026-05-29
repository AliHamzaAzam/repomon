//! End-to-end: start the daemon on a temp socket and exercise the JSON-RPC surface.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::{serve, Ctx};
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

    // Stop the agent.
    call(
        &mut stream,
        6,
        "agent.stop",
        Some(json!({ "lane_id": lane_id })),
    )
    .await;

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &session])
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
        .args(["kill-session", "-t", &session])
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
        .args(["kill-session", "-t", &session])
        .output();
}
