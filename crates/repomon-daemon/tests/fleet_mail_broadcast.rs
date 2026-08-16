//! Multi-recipient / broadcast fleet mail (A6): an agent-mode MCP client can send `message_send`
//! to a JSON array of addresses and to `"lane-X/*"`, each target's durable inbox receives its own
//! copy, the caller gets back a per-recipient result instead of a bare `FleetMessage`, `"*"`
//! excludes the sender's own session while an explicit self-address still delivers, and a plain
//! single-address `to` (the pre-A6 shape) is untouched.
//!
//! Uses the same real-tmux-plus-fake-CLI harness as `fleet_mail_invariant.rs`: three managed
//! `claude-code` sessions across two lanes, each with its own minted MCP identity, standing in
//! for the fleet. Skips (like that test) when tmux is unavailable.

use std::process::{Command, Stdio};
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::{Ctx, serve};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command as TokioCommand};

const READ_TIMEOUT: Duration = Duration::from_secs(10);

async fn connect_retry(sock: &std::path::Path) -> IpcStream {
    for _ in 0..100 {
        if let Ok(s) = transport::connect(&Endpoint::from_path(sock)).await {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("daemon endpoint {} never came up", sock.display());
}

async fn call(stream: &mut IpcStream, id: u64, method: &str, params: Option<Value>) -> Response {
    let req = Request::new(id, method, params);
    protocol::write_message(stream, &req).await.unwrap();
    let frame = tokio::time::timeout(READ_TIMEOUT, protocol::read_frame(stream))
        .await
        .expect("timed out waiting for daemon response")
        .unwrap()
        .expect("response frame");
    serde_json::from_slice(&frame).unwrap()
}

fn git(dir: &std::path::Path, args: &[&str]) {
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

/// A fake `claude` CLI that dumps its identity token then sleeps, so the tmux window it occupies
/// stays alive (and thus counts as an active agent session in the overlay) for the rest of the
/// test instead of exiting the instant it's spawned.
fn write_token_dumper(bin_path: &std::path::Path, token_file: &std::path::Path) {
    let script = format!(
        "#!/bin/sh\nprintf '%s' \"$REPOMON_MCP_IDENTITY_TOKEN\" > '{}'\nsleep 600\n",
        token_file.display()
    );
    std::fs::write(bin_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

async fn wait_for_token(token_file: &std::path::Path) -> String {
    for _ in 0..150 {
        if let Ok(content) = std::fs::read_to_string(token_file) {
            if !content.is_empty() {
                return content;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("fake agent CLI never wrote its identity token to {}", token_file.display());
}

/// Spawn a `claude-code` agent in `lane_id` via the fake `claude` binary on `PATH`, waiting for
/// its minted identity token to land in `token_file` (which must not already contain a stale
/// token from a previous spawn).
async fn spawn_claude(
    stream: &mut IpcStream,
    id: u64,
    lane_id: i64,
    bin_dir: &std::path::Path,
    token_file: &std::path::Path,
) -> String {
    let _ = std::fs::remove_file(token_file);
    write_token_dumper(&bin_dir.join("claude"), token_file);
    let r = call(
        stream,
        id,
        "agent.spawn",
        Some(json!({ "lane_id": lane_id, "agent": "claude-code" })),
    )
    .await;
    assert!(r.error.is_none(), "agent.spawn errored: {:?}", r.error);
    wait_for_token(token_file).await
}

fn spawn_mcp_child(sock: &std::path::Path, extra_env: &[(&str, &str)]) -> Child {
    let mut cmd = TokioCommand::new(env!("CARGO_BIN_EXE_repomond"));
    cmd.args(["--socket", &sock.to_string_lossy(), "mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd.env_remove("REPOMON_MCP_MODE");
    cmd.env_remove("REPOMON_MCP_IDENTITY_TOKEN");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn repomond mcp");
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
    mcp_send(stdin, &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await;
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

/// Pull `(text, isError)` out of a `tools/call` response's single text content item, and parse
/// the text as JSON.
async fn call_tool(
    stdin: &mut ChildStdin,
    lines: &mut tokio::io::Lines<BufReader<ChildStdout>>,
    id: u64,
    name: &str,
    arguments: Value,
) -> (Value, bool) {
    mcp_request(stdin, id, "tools/call", json!({ "name": name, "arguments": arguments })).await;
    let resp = mcp_read(lines).await;
    let content = &resp["result"]["content"];
    let text = content[0]["text"].as_str().unwrap_or_default();
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
    let parsed = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
    (parsed, is_error)
}

async fn shutdown_mcp_child(mut child: Child, stdin: ChildStdin) {
    drop(stdin);
    if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
        let _ = child.start_kill();
    }
}

/// Every body in `page.messages` (a `MessagePage`, newest first).
fn bodies(page: &Value) -> Vec<String> {
    page["messages"]
        .as_array()
        .expect("message page has messages")
        .iter()
        .map(|m| m["body"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn broadcast_and_list_mail_fan_out_and_self_exclude_while_single_send_is_unchanged() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping fleet-mail broadcast test");
        return;
    }
    let session = format!("repomon-broadcast-it-{}", std::process::id());
    let config = Config { tmux_session: session.clone(), ..Default::default() };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!("repomon-broadcast-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };

    let cfg_home = tempfile::tempdir().expect("tempdir");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", cfg_home.path());
    }
    let bin_dir = tempfile::tempdir().expect("tempdir");
    let old_path = std::env::var_os("PATH");
    let new_path = match &old_path {
        Some(p) => format!("{}:{}", bin_dir.path().display(), p.to_string_lossy()),
        None => bin_dir.path().display().to_string(),
    };
    unsafe {
        std::env::set_var("PATH", &new_path);
    }

    let mut stream = connect_retry(&sock).await;

    // Lane A: two managed claude-code sessions (slots 1 and 2). Lane B: one, the sender.
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    git(repo_dir.path(), &["commit", "--allow-empty", "-m", "init"]);
    call(&mut stream, 1, "repo.add", Some(json!({ "path": repo_dir.path().to_string_lossy() }))).await;
    let lanes = call(&mut stream, 2, "lane.list", None).await.result.unwrap();
    let repo_id = lanes[0]["repo"]["id"].as_i64().unwrap();
    let lane_a = lanes[0]["id"].as_i64().unwrap();
    let r = call(
        &mut stream,
        3,
        "lane.create",
        Some(json!({ "repo_id": repo_id, "branch": "lane-b", "source_branch": "main" })),
    )
    .await;
    assert!(r.error.is_none(), "lane.create errored: {:?}", r.error);
    let lane_b = r.result.unwrap()["id"].as_i64().unwrap();

    let token_a1_file = bin_dir.path().join("a1.token");
    let token_a2_file = bin_dir.path().join("a2.token");
    let token_b1_file = bin_dir.path().join("b1.token");
    let token_a1 = spawn_claude(&mut stream, 10, lane_a, bin_dir.path(), &token_a1_file).await;
    let token_a2 = spawn_claude(&mut stream, 11, lane_a, bin_dir.path(), &token_a2_file).await;
    let token_b1 = spawn_claude(&mut stream, 12, lane_b, bin_dir.path(), &token_b1_file).await;

    let addr_a1 = format!("lane-{lane_a}/1");
    let addr_a2 = format!("lane-{lane_a}/2");
    let addr_b1 = format!("lane-{lane_b}/1");

    // Sender: an agent-mode MCP client authenticated as lane-B/1.
    let mut sender = spawn_mcp_child(&sock, &[
        ("REPOMON_MCP_MODE", "agent"),
        ("REPOMON_MCP_IDENTITY_TOKEN", &token_b1),
    ]);
    let mut sender_stdin = sender.stdin.take().expect("child stdin");
    let mut sender_lines = BufReader::new(sender.stdout.take().expect("child stdout")).lines();
    mcp_request(&mut sender_stdin, 1, "initialize", json!({ "protocolVersion": "2025-06-18" })).await;
    let _ = mcp_read(&mut sender_lines).await;
    mcp_notify(&mut sender_stdin, "notifications/initialized").await;

    // ---- 1. list send: ["lane-A/1", "lane-A/2"] ---------------------------------------------
    let (result, is_error) = call_tool(
        &mut sender_stdin,
        &mut sender_lines,
        2,
        "message_send",
        json!({ "to": [addr_a1, addr_a2], "body": "list-body" }),
    )
    .await;
    assert!(!is_error, "list send errored: {result:?}");
    assert_eq!(result["recipient_count"], json!(2), "{result:?}");
    assert_eq!(result["sent_count"], json!(2), "{result:?}");
    let statuses: Vec<&str> = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["sent", "sent"], "{result:?}");

    let inbox_a1 = call(
        &mut stream,
        20,
        "message.inbox",
        Some(json!({ "identity_token": token_a1 })),
    )
    .await
    .result
    .unwrap();
    assert!(bodies(&inbox_a1).contains(&"list-body".to_string()), "{inbox_a1:?}");
    let inbox_a2 = call(
        &mut stream,
        21,
        "message.inbox",
        Some(json!({ "identity_token": token_a2 })),
    )
    .await
    .result
    .unwrap();
    assert!(bodies(&inbox_a2).contains(&"list-body".to_string()), "{inbox_a2:?}");

    // Each fan-out send reuses `send_message`'s existing per-sender rate limiter unchanged (a
    // deliberate A6 design call — see the report), so pace these steps >1s apart to stay clear
    // of its rolling burst window instead of exercising that limiter here.
    tokio::time::sleep(Duration::from_millis(2200)).await;

    // ---- 2. lane wildcard send: "lane-A/*" ---------------------------------------------------
    let (result, is_error) = call_tool(
        &mut sender_stdin,
        &mut sender_lines,
        3,
        "message_send",
        json!({ "to": format!("lane-{lane_a}/*"), "body": "lane-wild-body" }),
    )
    .await;
    assert!(!is_error, "lane wildcard send errored: {result:?}");
    assert_eq!(result["recipient_count"], json!(2), "{result:?}");
    let targets: Vec<&str> = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["to"].as_str().unwrap())
        .collect();
    assert_eq!(targets, [addr_a1.as_str(), addr_a2.as_str()], "{result:?}");

    let inbox_a1 = call(&mut stream, 22, "message.inbox", Some(json!({ "identity_token": token_a1 }))).await.result.unwrap();
    assert!(bodies(&inbox_a1).contains(&"lane-wild-body".to_string()), "{inbox_a1:?}");
    let inbox_a2 = call(&mut stream, 23, "message.inbox", Some(json!({ "identity_token": token_a2 }))).await.result.unwrap();
    assert!(bodies(&inbox_a2).contains(&"lane-wild-body".to_string()), "{inbox_a2:?}");

    tokio::time::sleep(Duration::from_millis(2200)).await;

    // ---- 3. global wildcard send: "*" excludes the sender itself -----------------------------
    let (result, is_error) = call_tool(
        &mut sender_stdin,
        &mut sender_lines,
        4,
        "message_send",
        json!({ "to": "*", "body": "global-wild-body" }),
    )
    .await;
    assert!(!is_error, "global wildcard send errored: {result:?}");
    assert_eq!(result["recipient_count"], json!(2), "{result:?} (sender must self-exclude)");
    let targets: Vec<&str> = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["to"].as_str().unwrap())
        .collect();
    assert!(!targets.contains(&addr_b1.as_str()), "broadcast must not mail the sender: {targets:?}");

    let inbox_a1 = call(&mut stream, 24, "message.inbox", Some(json!({ "identity_token": token_a1 }))).await.result.unwrap();
    assert!(bodies(&inbox_a1).contains(&"global-wild-body".to_string()), "{inbox_a1:?}");
    let inbox_a2 = call(&mut stream, 25, "message.inbox", Some(json!({ "identity_token": token_a2 }))).await.result.unwrap();
    assert!(bodies(&inbox_a2).contains(&"global-wild-body".to_string()), "{inbox_a2:?}");
    let inbox_b1 = call(&mut stream, 26, "message.inbox", Some(json!({ "identity_token": token_b1 }))).await.result.unwrap();
    assert!(!bodies(&inbox_b1).contains(&"global-wild-body".to_string()), "self-broadcast leaked: {inbox_b1:?}");

    tokio::time::sleep(Duration::from_millis(2200)).await;

    // ---- 4. explicit self-address single send still delivers ---------------------------------
    let (result, is_error) = call_tool(
        &mut sender_stdin,
        &mut sender_lines,
        5,
        "message_send",
        json!({ "to": addr_b1, "body": "self-explicit-body" }),
    )
    .await;
    assert!(!is_error, "explicit self-send errored: {result:?}");
    // Legacy single-address shape: a bare FleetMessage, not a fan-out summary.
    assert!(result.get("results").is_none(), "single self-send must not be wrapped: {result:?}");
    assert!(result.get("id").is_some(), "single self-send must return a FleetMessage: {result:?}");
    let inbox_b1 = call(&mut stream, 27, "message.inbox", Some(json!({ "identity_token": token_b1 }))).await.result.unwrap();
    assert!(bodies(&inbox_b1).contains(&"self-explicit-body".to_string()), "{inbox_b1:?}");

    tokio::time::sleep(Duration::from_millis(2200)).await;

    // ---- 5. plain single address: exact pre-A6 shape and behavior ----------------------------
    let (result, is_error) = call_tool(
        &mut sender_stdin,
        &mut sender_lines,
        6,
        "message_send",
        json!({ "to": addr_a1, "body": "single-legacy-body" }),
    )
    .await;
    assert!(!is_error, "single legacy send errored: {result:?}");
    assert!(result.get("results").is_none(), "single send must not be wrapped: {result:?}");
    assert_eq!(result["recipient"]["address"], json!(addr_a1), "{result:?}");
    let inbox_a1 = call(&mut stream, 28, "message.inbox", Some(json!({ "identity_token": token_a1 }))).await.result.unwrap();
    assert!(bodies(&inbox_a1).contains(&"single-legacy-body".to_string()), "{inbox_a1:?}");

    tokio::time::sleep(Duration::from_millis(2200)).await;

    // ---- 6. mixed list: one good address, one malformed --------------------------------------
    let (result, is_error) = call_tool(
        &mut sender_stdin,
        &mut sender_lines,
        7,
        "message_send",
        json!({ "to": [addr_a1, "not-a-real-address"], "body": "mixed-body" }),
    )
    .await;
    assert!(!is_error, "mixed list send itself must not error: {result:?}");
    assert_eq!(result["recipient_count"], json!(2), "{result:?}");
    assert_eq!(result["sent_count"], json!(1), "{result:?}");
    let by_to: std::collections::HashMap<&str, &str> = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["to"].as_str().unwrap(), r["status"].as_str().unwrap()))
        .collect();
    assert_eq!(by_to[addr_a1.as_str()], "sent", "{result:?}");
    assert_eq!(by_to["not-a-real-address"], "no_such_session", "{result:?}");

    // ---- 7. empty list is rejected up front ---------------------------------------------------
    let (_result, is_error) = call_tool(
        &mut sender_stdin,
        &mut sender_lines,
        8,
        "message_send",
        json!({ "to": [], "body": "should-not-send" }),
    )
    .await;
    assert!(is_error, "an empty recipient list must be rejected");

    shutdown_mcp_child(sender, sender_stdin).await;

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new(repomon_core::agent::tmux_program()).args(["-L", &session, "kill-server"]).output();
    unsafe {
        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}

// The reply_to-on-a-broadcast design call is covered separately in
// `fleet_mail_broadcast_reply.rs` — its own file/process, both to keep the "exactly one test
// mutates process env per file" invariant this file also relies on, and to give it its own
// rate-limit budget against `send_message`'s ten-per-minute sender cap.
