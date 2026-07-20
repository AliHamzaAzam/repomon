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
    boot_daemon_cfg(tag, Config::default()).await
}

/// [`boot_daemon`] with a caller-shaped config (custom tmux session, agents, ...). The worktree
/// template is always redirected into the state tempdir so an MCP `create_lane` (which takes no
/// path) can never land a worktree in the real `~/code`.
async fn boot_daemon_cfg(
    tag: &str,
    mut config: Config,
) -> (PathBuf, UnixStream, tempfile::TempDir) {
    let store = Store::open_in_memory().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    config.worktree_template = format!("{}/wt/{{repo}}/{{branch}}", state_dir.path().display());
    let ctx = Ctx::new_with_paths(
        store,
        config,
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

    // 2. tools/list -> exactly the 20 tools (repo-notes, history, playbooks, approvals).
    mcp_request(&mut stdin, 2, "tools/list", json!({})).await;
    let resp = mcp_read(&mut lines).await;
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 20, "tools/list returned: {tools:?}");
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
        "fleet_history",
        "playbook_save",
        "playbook_search",
        "approval_allow",
        "approval_rules",
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

    // create_lane embeds the repo's notes in its own result — the orchestrator gets them with
    // no extra tool call, ready to fold into the worker's task.
    mcp_request(
        &mut stdin,
        7,
        "tools/call",
        json!({
            "name": "create_lane",
            "arguments": { "repo": repo_name, "branch": "feat/notes-embed" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "create_lane errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed["repo_notes"],
        json!("use `pnpm test`, never `npm test`"),
        "create_lane result must embed the notes: {parsed}"
    );

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_spawn_agent_embeds_repo_notes() {
    if !repomon_core::TmuxRuntime::available() {
        eprintln!("tmux not available; skipping spawn_agent notes-embed test");
        return;
    }
    // A unique tmux session (name doubles as the `-L` socket) so parallel CI runs never collide
    // and we never touch the user's real `repomon` session.
    let session = format!("repomon-notes-embed-it-{}", std::process::id());
    let mut config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    // A harmless custom "agent" instead of real `claude`: `true` ignores its arguments and
    // exits 0, exercising the real spawn path without launching an autonomous session.
    config.agents.insert("noop".to_string(), "true".to_string());

    let (sock, mut control, _state_dir) = boot_daemon_cfg("spawn-embed", config).await;
    let (repo_dir, lane_id) = seed_repo_lane(&mut control).await;
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

    mcp_request(
        &mut stdin,
        2,
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

    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({
            "name": "spawn_agent",
            "arguments": { "lane_id": lane_id, "agent": "noop", "task": "run the tests" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "spawn_agent errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed["repo_notes"],
        json!("use `pnpm test`, never `npm test`"),
        "spawn_agent result must embed the notes: {parsed}"
    );

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
    let _ = StdCommand::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}

/// Spawn an MCP child, initialize it, and return (child, stdin, stdout-lines).
async fn init_mcp_child(
    sock: &Path,
    extra_env: &[(&str, &str)],
) -> (Child, ChildStdin, tokio::io::Lines<BufReader<ChildStdout>>) {
    let mut child = spawn_mcp_child(sock, extra_env);
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
    (child, stdin, lines)
}

#[tokio::test]
async fn mcp_stdio_journal_and_cold_start_recap() {
    let (sock, mut control, _state_dir) = boot_daemon("journal").await;
    let (repo_dir, _lane_id) = seed_repo_lane(&mut control).await;
    let repo_name = repo_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // Session 1: one journaled mutation, then a clean exit.
    let (child, mut stdin, mut lines) = init_mcp_child(&sock, &[]).await;
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({
            "name": "repo_notes_write",
            "arguments": { "repo": repo_name, "content": "use `pnpm test`" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "repo_notes_write errored: {text}");
    shutdown_mcp_child(child, stdin).await;

    // Session 2 (the cold start): the FIRST fleet_status carries the recap...
    let (child, mut stdin, mut lines) = init_mcp_child(&sock, &[]).await;
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({ "name": "fleet_status", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "fleet_status errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let recap = &parsed["since_you_last_looked"];
    assert!(
        recap.is_object(),
        "first fleet_status must carry since_you_last_looked: {parsed}"
    );
    assert!(
        recap.to_string().contains("repo_notes_write"),
        "recap should mention the previous session's action: {recap}"
    );

    // ...and the second omits it (it's a cold-start block, not a per-call one).
    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({ "name": "fleet_status", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, _) = tool_result(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert!(
        parsed.get("since_you_last_looked").is_none(),
        "recap must appear only on the first call: {parsed}"
    );

    // fleet_history searches the journal.
    mcp_request(
        &mut stdin,
        4,
        "tools/call",
        json!({ "name": "fleet_history", "arguments": { "query": "repo_notes_write" } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "fleet_history errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let entries = parsed["entries"].as_array().unwrap();
    assert!(!entries.is_empty(), "fleet_history found nothing: {parsed}");
    assert_eq!(entries[0]["action"], json!("repo_notes_write"));
    assert_eq!(entries[0]["outcome"], json!("ok"));

    // Failed mutations are journaled too, with outcome error.
    mcp_request(
        &mut stdin,
        5,
        "tools/call",
        json!({
            "name": "repo_notes_write",
            "arguments": { "repo": "no-such-repo", "content": "x" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (_, is_error) = tool_result(&resp);
    assert!(is_error);
    mcp_request(
        &mut stdin,
        6,
        "tools/call",
        json!({ "name": "fleet_history", "arguments": { "query": "no-such-repo" } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, _) = tool_result(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let entries = parsed["entries"].as_array().unwrap();
    assert!(
        entries.iter().any(|e| e["outcome"] == json!("error")),
        "failed mutation should be journaled with outcome=error: {parsed}"
    );

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_read_only_allows_fleet_history() {
    let (sock, mut control, _state_dir) = boot_daemon("journal-ro").await;
    let (_repo_dir, _lane_id) = seed_repo_lane(&mut control).await;

    let (child, mut stdin, mut lines) =
        init_mcp_child(&sock, &[("REPOMON_MCP_AUTONOMY", "read-only")]).await;
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({ "name": "fleet_history", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        !is_error,
        "fleet_history (read) must work under read-only autonomy, got: {text}"
    );
    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_playbook_draft_approval_flow() {
    let (sock, mut control, _state_dir) = boot_daemon("playbook").await;
    let (_repo_dir, _lane_id) = seed_repo_lane(&mut control).await;

    let (child, mut stdin, mut lines) = init_mcp_child(&sock, &[]).await;

    // Draft a playbook; the result must say it is a draft awaiting human approval.
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({
            "name": "playbook_save",
            "arguments": { "name": "release-all", "content": "1. lane per repo\n2. verify\n3. merge" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "playbook_save errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["status"], json!("draft"));
    assert!(
        parsed["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("approve"),
        "save result must explain the approval step: {parsed}"
    );

    // Unapproved: search returns nothing, with a hint.
    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({ "name": "playbook_search", "arguments": { "query": "release" } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "playbook_search errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["playbooks"], json!([]));
    assert!(
        parsed["hint"].is_string(),
        "empty search should hint: {parsed}"
    );

    // Human approves out-of-band (the CLI path drives this same RPC).
    let r = daemon_call(
        &mut control,
        50,
        "playbook.approve",
        Some(json!({ "name": "release-all" })),
    )
    .await;
    assert!(r.error.is_none(), "approve errored: {:?}", r.error);

    // Now the playbook is followable.
    mcp_request(
        &mut stdin,
        4,
        "tools/call",
        json!({ "name": "playbook_search", "arguments": { "query": "release" } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, _) = tool_result(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let books = parsed["playbooks"].as_array().unwrap();
    assert_eq!(books.len(), 1);
    assert!(
        books[0]["content"]
            .as_str()
            .unwrap()
            .contains("lane per repo")
    );

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_read_only_playbooks() {
    let (sock, mut control, _state_dir) = boot_daemon("playbook-ro").await;
    let (_repo_dir, _lane_id) = seed_repo_lane(&mut control).await;

    let (child, mut stdin, mut lines) =
        init_mcp_child(&sock, &[("REPOMON_MCP_AUTONOMY", "read-only")]).await;
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({ "name": "playbook_save", "arguments": { "name": "x", "content": "y" } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        is_error && text.contains("read-only"),
        "playbook_save should be refused under read-only autonomy, got: {text}"
    );
    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({ "name": "playbook_search", "arguments": { "query": "x" } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        !is_error,
        "playbook_search (read) must work under read-only autonomy, got: {text}"
    );
    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_unattended_refuses_merge_and_delete() {
    let (sock, mut control, _state_dir) = boot_daemon("unattended").await;
    let (_repo_dir, lane_id) = seed_repo_lane(&mut control).await;

    // Unattended + fully autonomous: structural autonomy is NOT the gate here.
    let (child, mut stdin, mut lines) = init_mcp_child(
        &sock,
        &[
            ("REPOMON_MCP_UNATTENDED", "1"),
            ("REPOMON_MCP_AUTONOMY", "autonomous"),
        ],
    )
    .await;

    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({ "name": "merge_lane", "arguments": { "lane_id": lane_id } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        is_error && text.contains("unattended"),
        "merge_lane must be refused in unattended mode, got: {text}"
    );

    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({ "name": "delete_lane", "arguments": { "lane_id": lane_id } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        is_error && text.contains("unattended"),
        "delete_lane must be refused in unattended mode, got: {text}"
    );

    // Reads and non-structural mutations stay available (only caps bound them).
    mcp_request(
        &mut stdin,
        4,
        "tools/call",
        json!({ "name": "fleet_status", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "fleet_status must work unattended: {text}");

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_approval_allow_two_phase() {
    let (sock, mut control, _state_dir) = boot_daemon("approval").await;
    let (_repo_dir, _lane_id) = seed_repo_lane(&mut control).await;

    let (child, mut stdin, mut lines) = init_mcp_child(&sock, &[]).await;

    // No rules yet.
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({ "name": "approval_rules", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "approval_rules errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["rules"], json!([]));

    // Phase 1: no confirm mints a token and stores nothing.
    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({
            "name": "approval_allow",
            "arguments": { "repo": "api", "pattern": "cargo test" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "phase 1 errored: {text}");
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let token = parsed["confirm_token"].as_str().expect("token").to_string();
    assert!(
        parsed["impact"]
            .as_str()
            .unwrap_or_default()
            .contains("auto-approve"),
        "impact must explain the consequence: {parsed}"
    );
    mcp_request(
        &mut stdin,
        4,
        "tools/call",
        json!({ "name": "approval_rules", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, _) = tool_result(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed["rules"],
        json!([]),
        "phase 1 must not store the rule"
    );

    // A fabricated token is rejected.
    mcp_request(
        &mut stdin,
        5,
        "tools/call",
        json!({
            "name": "approval_allow",
            "arguments": { "repo": "api", "pattern": "cargo test", "confirm": "deadbeef" },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(is_error, "bogus token must fail: {text}");

    // Phase 2 with the real token stores the rule.
    mcp_request(
        &mut stdin,
        6,
        "tools/call",
        json!({
            "name": "approval_allow",
            "arguments": { "repo": "api", "pattern": "cargo test", "confirm": token },
        }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(!is_error, "phase 2 errored: {text}");
    mcp_request(
        &mut stdin,
        7,
        "tools/call",
        json!({ "name": "approval_rules", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, _) = tool_result(&resp);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let rules = parsed["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["pattern"], json!("cargo test"));

    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn mcp_stdio_read_only_approvals() {
    let (sock, mut control, _state_dir) = boot_daemon("approval-ro").await;
    let (_repo_dir, _lane_id) = seed_repo_lane(&mut control).await;

    let (child, mut stdin, mut lines) =
        init_mcp_child(&sock, &[("REPOMON_MCP_AUTONOMY", "read-only")]).await;
    mcp_request(
        &mut stdin,
        2,
        "tools/call",
        json!({ "name": "approval_allow", "arguments": { "repo": "a", "pattern": "x y" } }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        is_error && text.contains("read-only"),
        "approval_allow must be refused read-only, got: {text}"
    );
    mcp_request(
        &mut stdin,
        3,
        "tools/call",
        json!({ "name": "approval_rules", "arguments": {} }),
    )
    .await;
    let resp = mcp_read(&mut lines).await;
    let (text, is_error) = tool_result(&resp);
    assert!(
        !is_error,
        "approval_rules (read) must work read-only: {text}"
    );
    shutdown_mcp_child(child, stdin).await;
    let _ = std::fs::remove_file(&sock);
}
