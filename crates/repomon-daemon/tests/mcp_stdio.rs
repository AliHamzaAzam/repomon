//! End-to-end: a real `repomond mcp` child process speaking MCP over stdio against a real
//! in-process daemon. This is the orchestrator's actual transport, so it's the one place we
//! exercise the whole chain: daemon socket -> `repomond mcp` -> newline-delimited JSON-RPC.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::{Config, Store};
use repomon_daemon::{Ctx, serve};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// Every read (daemon socket or MCP child) is guarded by this — a hang must fail the test, not
/// wedge CI.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

fn git(dir: &Path, args: &[&str]) {
    let ok = StdCommand::new("git")
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

/// Call the daemon directly over its length-prefixed JSON-RPC socket — same helper as
/// tests/integration.rs's `call()`. Used to seed a repo before the MCP child ever connects.
async fn daemon_call(
    stream: &mut UnixStream,
    id: u64,
    method: &str,
    params: Option<Value>,
) -> Response {
    let req = Request::new(id, method, params);
    protocol::write_message(stream, &req).await.unwrap();
    let frame = tokio::time::timeout(READ_TIMEOUT, protocol::read_frame(stream))
        .await
        .expect("timed out waiting for daemon response")
        .unwrap()
        .expect("response frame");
    serde_json::from_slice(&frame).unwrap()
}

/// Boot an in-process daemon on a short temp socket path (macOS caps UDS paths at ~104 chars),
/// exactly like tests/integration.rs, and return a connected control stream for seeding state.
/// Config and repo-notes paths live in the returned tempdir (kept alive by the caller) so the
/// daemon under test never touches the real `~/.config` / data dir.
async fn boot_daemon(tag: &str) -> (PathBuf, UnixStream, tempfile::TempDir) {
    let store = Store::open_in_memory().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new_with_paths(
        store,
        Config::default(),
        None,
        state_dir.path().join("config.toml"),
        state_dir.path().join("repo-notes"),
    );

    let sock =
        std::env::temp_dir().join(format!("repomon-mcpit-{tag}-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let server_sock = sock.clone();
    tokio::spawn(async move { serve(ctx, &server_sock).await });

    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let stream = UnixStream::connect(&sock).await.expect("connect to daemon");
    (sock, stream, state_dir)
}

/// Register a repo with one commit and return its (tempdir, main lane id).
async fn seed_repo_lane(stream: &mut UnixStream) -> (tempfile::TempDir, i64) {
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    git(repo_dir.path(), &["commit", "--allow-empty", "-m", "init"]);

    let r = daemon_call(
        stream,
        1,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    assert!(r.error.is_none(), "repo.add errored: {:?}", r.error);

    let lanes = daemon_call(stream, 2, "lane.list", None)
        .await
        .result
        .unwrap();
    let lane_id = lanes[0]["id"].as_i64().unwrap();
    (repo_dir, lane_id)
}

/// Spawn `repomond mcp` wired for newline-delimited JSON-RPC on its stdio, against `sock`.
fn spawn_mcp_child(sock: &Path, extra_env: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_repomond"));
    cmd.args(["--socket", &sock.to_string_lossy(), "mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn repomond mcp");

    // Drain stderr in the background instead of leaving it piped-and-unread (which can deadlock
    // the child once its OS pipe buffer fills); surface it on the test's stderr for diagnosis.
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[repomond mcp stderr] {line}");
            }
        });
    }
    child
}

async fn mcp_send(stdin: &mut ChildStdin, msg: &Value) {
    let mut line = msg.to_string();
    line.push('\n');
    stdin.write_all(line.as_bytes()).await.unwrap();
    stdin.flush().await.unwrap();
}

async fn mcp_request(stdin: &mut ChildStdin, id: u64, method: &str, params: Value) {
    mcp_send(
        stdin,
        &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
    )
    .await;
}

async fn mcp_notify(stdin: &mut ChildStdin, method: &str) {
    mcp_send(stdin, &json!({ "jsonrpc": "2.0", "method": method })).await;
}

async fn mcp_read(lines: &mut tokio::io::Lines<BufReader<ChildStdout>>) -> Value {
    let line = tokio::time::timeout(READ_TIMEOUT, lines.next_line())
        .await
        .expect("timed out waiting for an MCP response")
        .unwrap()
        .expect("MCP child closed stdout unexpectedly");
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad MCP JSON ({e}): {line}"))
}

/// Pull `(text, isError)` out of a `tools/call` response's single text content item.
fn tool_result(resp: &Value) -> (String, bool) {
    let content = &resp["result"]["content"];
    let text = content[0]["text"].as_str().unwrap_or_default().to_string();
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
    (text, is_error)
}

/// Close stdin (EOF, the documented clean-shutdown path — see mcp.rs's `run_stdio`) and wait for
/// the child to exit; fall back to a hard kill if it doesn't.
async fn shutdown_mcp_child(mut child: Child, stdin: ChildStdin) {
    drop(stdin);
    if tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
    }
}

#[tokio::test]
async fn mcp_stdio_end_to_end() {
    let (sock, mut control, _state_dir) = boot_daemon("e2e").await;
    let (repo_dir, lane_id) = seed_repo_lane(&mut control).await;

    let mut child = spawn_mcp_child(&sock, &[]);
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("child stdout")).lines();

    // 1. initialize -> serverInfo.name == "repomon".
    mcp_request(
        &mut stdin,
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "repomon-it", "version": "0.0.0" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    assert_eq!(resp["id"], json!(1));
    assert_eq!(resp["result"]["serverInfo"]["name"], json!("repomon"));

    mcp_notify(&mut stdin, "notifications/initialized").await;

    // 2. tools/list -> exactly the 15 tools (13 v1 + the repo-notes pair).
    mcp_request(&mut stdin, 2, "tools/list", json!({})).await;
    let resp = mcp_read(&mut lines).await;
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 15, "tools/list returned: {tools:?}");
    let names: BTreeSet<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    let expected: BTreeSet<&str> = [
        "fleet_status",
        "read_agent",
        "spawn_agent",
        "send_to_agent",
        "approve_agent",
        "interrupt_agent",
        "stop_agent",
        "create_lane",
        "delete_lane",
        "merge_lane",
        "lane_diff",
        "list_repos",
        "wait_for_change",
        "repo_notes",
        "repo_notes_write",
    ]
    .into_iter()
    .collect();
    assert_eq!(names, expected);

    // 3. tools/call list_repos -> the registered repo appears.
    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({ "name": "list_repos", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "list_repos errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let repos = parsed["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 1, "list_repos result: {parsed}");
    let repo_dir_name = repo_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        repos[0]["path"]
            .as_str()
            .unwrap_or_default()
            .contains(&repo_dir_name),
        "expected the registered repo's path to contain {repo_dir_name:?}, got: {parsed}"
    );

    // 4. tools/call fleet_status -> one lane (the repo's main lane) present.
    mcp_request(
        &mut stdin,
        4,
        "tools/call",
        json!({ "name": "fleet_status", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "fleet_status errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let lanes = parsed["lanes"].as_array().unwrap();
    assert_eq!(lanes.len(), 1, "fleet_status result: {parsed}");
    assert_eq!(lanes[0]["lane_id"], json!(lane_id));

    // 5. tools/call read_agent with a bogus lane_id -> MCP tool error (isError: true).
    mcp_request(
        &mut stdin,
        5,
        "tools/call",
        json!({ "name": "read_agent", "arguments": { "lane_id": 999_999_999 } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        is_error,
        "expected read_agent on a bogus lane to be a tool error, got: {text}"
    );
    assert!(
        text.to_lowercase().contains("not found"),
        "unexpected error text: {text}"
    );

    // 6. Concurrency: wait_for_change (2s) must not block a concurrent ping behind it — each
    // tools/call runs in its own task (see mcp.rs), so the cheap request answers first.
    mcp_request(
        &mut stdin,
        6,
        "tools/call",
        json!({ "name": "wait_for_change", "arguments": { "timeout_secs": 2 } }),
    )
    .await;
    mcp_request(&mut stdin, 7, "ping", json!({})).await;

    let first = mcp_read(&mut lines).await;
    assert_eq!(
        first["id"],
        json!(7),
        "ping should answer before wait_for_change resolves: {first}"
    );
    assert_eq!(first["result"], json!({}));

    let second = mcp_read(&mut lines).await;
    assert_eq!(second["id"], json!(6));

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_read_only_denies_spawn_agent() {
    let (sock, mut control, _state_dir) = boot_daemon("readonly").await;
    let (_repo_dir, lane_id) = seed_repo_lane(&mut control).await;

    let mut child = spawn_mcp_child(&sock, &[("REPOMON_MCP_AUTONOMY", "read-only")]);
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("child stdout")).lines();

    mcp_request(
        &mut stdin,
        1,
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "repomon-it", "version": "0.0.0" } }),
    )
    .await;
    let _ = mcp_read(&mut lines).await;

    // 7. Read-only policy: spawn_agent must be refused with the read-only message.
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({
            "name": "spawn_agent",
            "arguments": { "lane_id": lane_id, "task": "do the thing" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        is_error,
        "spawn_agent should be refused under read-only autonomy, got: {text}"
    );
    assert!(
        text.contains("read-only") && text.contains("this tool only observes"),
        "unexpected error text: {text}"
    );

    // repo_notes_write is a mutation: refused. The repo_notes read stays available.
    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({
            "name": "repo_notes_write",
            "arguments": { "repo": "whatever", "content": "x" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        is_error && text.contains("read-only"),
        "repo_notes_write should be refused under read-only autonomy, got: {text}"
    );

    let repo_name = _repo_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    mcp_request(
        &mut stdin,
        4,
        "tools/call",
        json!({ "name": "repo_notes", "arguments": { "repo": repo_name } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        !is_error,
        "repo_notes (read) must work under read-only autonomy, got: {text}"
    );

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_repo_notes_round_trip() {
    let (sock, mut control, state_dir) = boot_daemon("notes").await;
    let (repo_dir, _lane_id) = seed_repo_lane(&mut control).await;
    let repo_name = repo_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let mut child = spawn_mcp_child(&sock, &[]);
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("child stdout")).lines();

    mcp_request(
        &mut stdin,
        1,
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "repomon-it", "version": "0.0.0" } }),
    )
    .await;
    let _ = mcp_read(&mut lines).await;

    // Empty notes: not an error, and a hint tells the orchestrator how to seed them.
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({ "name": "repo_notes", "arguments": { "repo": repo_name } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "repo_notes on empty errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["content"], json!(""));
    assert!(parsed["hint"].is_string(), "expected a hint, got: {parsed}");

    // Write by repo *name* (the orchestrator's handle), then read back.
    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({
            "name": "repo_notes_write",
            "arguments": { "repo": repo_name, "content": "use `pnpm test`, never `npm test`" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "repo_notes_write errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["ok"], json!(true));
    let path = PathBuf::from(parsed["path"].as_str().unwrap());
    assert!(
        path.starts_with(state_dir.path().join("repo-notes")),
        "notes path {path:?} escaped the injected dir"
    );

    mcp_request(
        &mut stdin,
        4,
        "tools/call",
        json!({ "name": "repo_notes", "arguments": { "repo": repo_name } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "repo_notes errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed["content"],
        json!("use `pnpm test`, never `npm test`")
    );
    assert!(
        parsed["hint"].is_null(),
        "no hint once notes exist: {parsed}"
    );

    // Unknown repo: error that points at list_repos.
    mcp_request(
        &mut stdin,
        5,
        "tools/call",
        json!({ "name": "repo_notes", "arguments": { "repo": "no-such-repo" } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(is_error, "unknown repo should error, got: {text}");
    assert!(text.contains("list_repos"), "unhelpful error: {text}");

    // Over the cap: rejected with an error naming the limit.
    mcp_request(
        &mut stdin,
        6,
        "tools/call",
        json!({
            "name": "repo_notes_write",
            "arguments": { "repo": repo_name, "content": "x".repeat(8193) },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(is_error, "oversized write should error, got: {text}");
    assert!(text.contains("8192"), "unhelpful error: {text}");

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}
