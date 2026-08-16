//! A6 design call, made explicit: on a broadcast, `reply_to` is not given any special multi-
//! recipient handling — it's passed through unchanged to `send_message` for every expansion
//! result, exactly like `body`. `send_message` already requires a reply to reverse the parent
//! message's sender/recipient pair, so on a fan-out only the one address (if any) that's the
//! actual other party in that thread accepts the reply; every other address reports a
//! `delivery_error` instead of silently misfiling the reply or aborting the whole send.
//!
//! Kept in its own file/process (not folded into `fleet_mail_broadcast.rs`) for two independent
//! reasons: it mutates process-wide `PATH`/`XDG_CONFIG_HOME` env vars like `fleet_mail_invariant.rs`
//! (safe only with exactly one test per file), and it needs its own budget against
//! `send_message`'s per-sender rate limit (ten sends per rolling minute) rather than sharing one
//! with that file's six broadcast scenarios.

use std::process::Command;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::{Ctx, serve};
use serde_json::{Value, json};

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
    let frame = tokio::time::timeout(Duration::from_secs(10), protocol::read_frame(stream))
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

#[tokio::test]
async fn reply_to_on_a_broadcast_only_reverses_for_the_actual_thread_partner() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping fleet-mail broadcast reply_to test");
        return;
    }
    let session = format!("repomon-broadcast-reply-it-{}", std::process::id());
    let config = Config { tmux_session: session.clone(), ..Default::default() };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock =
        std::env::temp_dir().join(format!("repomon-broadcast-reply-it-{}.sock", std::process::id()));
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
    let lane_b = r.result.unwrap()["id"].as_i64().unwrap();

    let token_a1_file = bin_dir.path().join("a1.token");
    let token_a2_file = bin_dir.path().join("a2.token");
    let token_b1_file = bin_dir.path().join("b1.token");
    let token_a1 = spawn_claude(&mut stream, 10, lane_a, bin_dir.path(), &token_a1_file).await;
    let _token_a2 = spawn_claude(&mut stream, 11, lane_a, bin_dir.path(), &token_a2_file).await;
    let token_b1 = spawn_claude(&mut stream, 12, lane_b, bin_dir.path(), &token_b1_file).await;

    let addr_a1 = format!("lane-{lane_a}/1");
    let addr_a2 = format!("lane-{lane_a}/2");
    let addr_b1 = format!("lane-{lane_b}/1");

    // Seed a thread: lane-A/1 -> lane-B/1.
    let seed = call(
        &mut stream,
        20,
        "message.send",
        Some(json!({ "to": addr_b1, "body": "seed-thread", "identity_token": token_a1 })),
    )
    .await
    .result
    .unwrap();
    let parent_id = seed["id"].as_str().unwrap().to_string();
    assert_eq!(seed["sender"]["address"], json!(addr_a1));
    assert_eq!(seed["recipient"]["address"], json!(addr_b1));

    // lane-B/1 broadcasts a reply to [lane-A/1, lane-A/2]: only lane-A/1 is the thread's actual
    // other party, so only it can accept the reply.
    let reply = call(
        &mut stream,
        21,
        "message.send",
        Some(json!({
            "to": [addr_a1.clone(), addr_a2.clone()],
            "body": "reply-body",
            "reply_to": parent_id,
            "identity_token": token_b1,
        })),
    )
    .await
    .result
    .unwrap();
    let by_to: std::collections::HashMap<&str, &str> = reply["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["to"].as_str().unwrap(), r["status"].as_str().unwrap()))
        .collect();
    assert_eq!(by_to[addr_a1.as_str()], "sent", "{reply:?}");
    assert_eq!(by_to[addr_a2.as_str()], "delivery_error", "{reply:?}");
    let a2_error = reply["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["to"] == json!(addr_a2))
        .unwrap()["error"]
        .as_str()
        .unwrap();
    assert!(a2_error.contains("reverse the parent message"), "{a2_error}");

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
