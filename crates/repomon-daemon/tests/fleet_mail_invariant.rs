//! Universal fleet-mail invariant (A5.1): every managed agent session the daemon creates — via
//! `agent.spawn` or `agent.adopt`, whether its kind wires the MCP server through command-line
//! flags (ClaudeCode) or a global config file (Antigravity, gated behind
//! `REPOMON_ANTIGRAVITY_MCP_CONFIG`) — must get its own restricted MCP identity, must have the
//! daemon store only a hash of that identity's token (never the plaintext), and any MCP config
//! file the daemon writes to disk must reference the `repomond mcp` command without ever
//! containing the raw token.
//!
//! The daemon never returns the plaintext token over RPC (only the spawned process's environment
//! carries it — see `rpc.rs`'s `agent.spawn`/`agent.adopt` handlers), so this test stands in fake
//! `claude`/`agy` binaries on `PATH` that dump `$REPOMON_MCP_IDENTITY_TOKEN` to a file and exit.
//! That recovered plaintext token is then fed back through `Store::resolve_mcp_identity` — the
//! same public lookup the real fleet-mail MCP server uses to authenticate a connecting agent —
//! to prove the daemon actually stored a hash that resolves back to this exact session (window,
//! lane, agent kind). A tampered token is asserted to resolve to nothing, so a broken hash check
//! (e.g. one that accepts any string) would fail this test, not just "no RPC error".
//!
//! Mutates process env (`PATH`, `XDG_CONFIG_HOME`, `REPOMON_ANTIGRAVITY_MCP_CONFIG`) — like
//! `orchestrator.rs`, safe only because this file has exactly one test.

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

/// Write an executable shell script standing in for a real agent CLI: it dumps
/// `$REPOMON_MCP_IDENTITY_TOKEN` verbatim (no trailing newline) to `token_file` and exits,
/// ignoring whatever flags it was launched with (`--mcp-config …`, `--continue`, …).
fn write_token_dumper(bin_path: &std::path::Path, token_file: &std::path::Path) {
    let script = format!(
        "#!/bin/sh\nprintf '%s' \"$REPOMON_MCP_IDENTITY_TOKEN\" > '{}'\n",
        token_file.display()
    );
    std::fs::write(bin_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Poll for the fake CLI to have written its token (the tmux pane's shell needs a beat to fork
/// and exec after `agent.spawn`/`agent.adopt` returns).
async fn wait_for_token(token_file: &std::path::Path) -> String {
    for _ in 0..150 {
        if let Ok(content) = std::fs::read_to_string(token_file) {
            if !content.is_empty() {
                return content;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "fake agent CLI never wrote its identity token to {}",
        token_file.display()
    );
}

#[tokio::test]
async fn fleet_mail_identity_survives_spawn_and_adopt_for_every_wiring_style() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping fleet-mail invariant test");
        return;
    }
    let session = format!("repomon-fleetmail-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock =
        std::env::temp_dir().join(format!("repomon-fleetmail-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };

    // Isolate every path these spawn/adopt calls can write to: the per-window Claude MCP config
    // (`write_agent_mcp_config`, under `config_dir()/agent-mcp`) lives under `XDG_CONFIG_HOME`;
    // Antigravity's global registration file is pinned to a tempdir via its override env var.
    let cfg_home = tempfile::tempdir().expect("tempdir");
    let antigravity_cfg = cfg_home.path().join("antigravity_mcp_config.json");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", cfg_home.path());
        std::env::set_var("REPOMON_ANTIGRAVITY_MCP_CONFIG", &antigravity_cfg);
    }

    // Fake `claude` / `agy` binaries on PATH, ahead of any real ones: this test must not depend
    // on either CLI being installed, and must never actually launch a real coding agent.
    let bin_dir = tempfile::tempdir().expect("tempdir");
    let claude_token_file = bin_dir.path().join("claude.token");
    let agy_token_file = bin_dir.path().join("agy.token");
    write_token_dumper(&bin_dir.path().join("claude"), &claude_token_file);
    write_token_dumper(&bin_dir.path().join("agy"), &agy_token_file);
    let old_path = std::env::var_os("PATH");
    let new_path = match &old_path {
        Some(p) => format!("{}:{}", bin_dir.path().display(), p.to_string_lossy()),
        None => bin_dir.path().display().to_string(),
    };
    unsafe {
        std::env::set_var("PATH", &new_path);
    }

    let mut stream = connect_retry(&sock).await;

    // Register a repo and grab its lane — every spawn/adopt call below targets it.
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

    let repomond = repomon_core::service::repomond_path();
    let mut next_id = 10u64;

    // ---- ClaudeCode: agent.spawn (command-line-flag wiring) ------------------------------
    let _ = std::fs::remove_file(&claude_token_file);
    let r = call(
        &mut stream,
        next_id,
        "agent.spawn",
        Some(json!({ "lane_id": lane_id, "agent": "claude-code" })),
    )
    .await;
    next_id += 1;
    assert!(
        r.error.is_none(),
        "claude-code spawn errored: {:?}",
        r.error
    );
    let window = r.result.unwrap()["window"]
        .as_str()
        .expect("spawn returns a window")
        .to_string();
    let token = wait_for_token(&claude_token_file).await;
    assert_eq!(
        token.len(),
        64,
        "identity token should be 32 random bytes as hex: {token:?}"
    );
    let identity = ctx
        .store
        .resolve_mcp_identity(token.clone())
        .await
        .unwrap()
        .expect("daemon must resolve the token it minted for the spawned claude-code session");
    assert_eq!(identity.lane_id, Some(lane_id), "identity: {identity:?}");
    assert_eq!(
        identity.window.as_deref(),
        Some(window.as_str()),
        "identity: {identity:?}"
    );
    assert_eq!(
        identity.agent_kind.as_deref(),
        Some("claude-code"),
        "identity: {identity:?}"
    );
    // A tampered token must not resolve — this is what actually fails if hash-checking breaks.
    assert!(
        ctx.store
            .resolve_mcp_identity(format!("{token}00"))
            .await
            .unwrap()
            .is_none(),
        "a mutated token must not resolve to any identity"
    );
    // The per-window Claude MCP config file references repomond's `mcp` command, never the token.
    let claude_cfg_path = repomon_core::config::config_dir()
        .join("agent-mcp")
        .join(format!("{window}.json"));
    let claude_cfg_content = std::fs::read_to_string(&claude_cfg_path)
        .expect("claude per-window mcp config must be written");
    let claude_cfg: Value = serde_json::from_str(&claude_cfg_content).unwrap();
    assert_eq!(
        claude_cfg["mcpServers"]["repomon"]["command"],
        json!(repomond.to_string_lossy())
    );
    assert_eq!(claude_cfg["mcpServers"]["repomon"]["args"], json!(["mcp"]));
    assert!(
        !claude_cfg_content.contains(&token),
        "claude per-window mcp config must never contain the raw identity token: {claude_cfg_content}"
    );

    // ---- ClaudeCode: agent.adopt (same wiring, adopt call path) ---------------------------
    let _ = std::fs::remove_file(&claude_token_file);
    let r = call(
        &mut stream,
        next_id,
        "agent.adopt",
        Some(json!({ "lane_id": lane_id, "agent": "claude-code" })),
    )
    .await;
    next_id += 1;
    assert!(
        r.error.is_none(),
        "claude-code adopt errored: {:?}",
        r.error
    );
    let window = r.result.unwrap()["window"]
        .as_str()
        .expect("adopt returns a window")
        .to_string();
    let token = wait_for_token(&claude_token_file).await;
    let identity = ctx
        .store
        .resolve_mcp_identity(token.clone())
        .await
        .unwrap()
        .expect("daemon must resolve the token it minted for the adopted claude-code session");
    assert_eq!(identity.lane_id, Some(lane_id), "identity: {identity:?}");
    assert_eq!(
        identity.window.as_deref(),
        Some(window.as_str()),
        "identity: {identity:?}"
    );
    assert_eq!(
        identity.agent_kind.as_deref(),
        Some("claude-code"),
        "identity: {identity:?}"
    );
    assert!(
        ctx.store
            .resolve_mcp_identity(format!("{token}00"))
            .await
            .unwrap()
            .is_none(),
        "a mutated token must not resolve to any identity"
    );
    let claude_cfg_path = repomon_core::config::config_dir()
        .join("agent-mcp")
        .join(format!("{window}.json"));
    let claude_cfg_content = std::fs::read_to_string(&claude_cfg_path)
        .expect("adopted claude per-window mcp config must be written");
    assert!(
        !claude_cfg_content.contains(&token),
        "adopted claude per-window mcp config must never contain the raw identity token: {claude_cfg_content}"
    );

    // ---- Antigravity: agent.spawn (global config-file wiring) -----------------------------
    let _ = std::fs::remove_file(&agy_token_file);
    let r = call(
        &mut stream,
        next_id,
        "agent.spawn",
        Some(json!({ "lane_id": lane_id, "agent": "antigravity" })),
    )
    .await;
    next_id += 1;
    assert!(
        r.error.is_none(),
        "antigravity spawn errored: {:?}",
        r.error
    );
    let window = r.result.unwrap()["window"]
        .as_str()
        .expect("spawn returns a window")
        .to_string();
    let token = wait_for_token(&agy_token_file).await;
    let identity = ctx
        .store
        .resolve_mcp_identity(token.clone())
        .await
        .unwrap()
        .expect("daemon must resolve the token it minted for the spawned antigravity session");
    assert_eq!(identity.lane_id, Some(lane_id), "identity: {identity:?}");
    assert_eq!(
        identity.window.as_deref(),
        Some(window.as_str()),
        "identity: {identity:?}"
    );
    assert_eq!(
        identity.agent_kind.as_deref(),
        Some("antigravity"),
        "identity: {identity:?}"
    );
    assert!(
        ctx.store
            .resolve_mcp_identity(format!("{token}00"))
            .await
            .unwrap()
            .is_none(),
        "a mutated token must not resolve to any identity"
    );
    assert!(
        antigravity_cfg.exists(),
        "antigravity global mcp config must be registered after spawn"
    );
    let agy_cfg_content = std::fs::read_to_string(&antigravity_cfg).unwrap();
    let agy_cfg: Value = serde_json::from_str(&agy_cfg_content).unwrap();
    assert_eq!(
        agy_cfg["mcpServers"]["repomon"]["command"],
        json!(repomond.to_string_lossy())
    );
    assert_eq!(agy_cfg["mcpServers"]["repomon"]["args"], json!(["mcp"]));
    assert!(
        !agy_cfg_content.contains(&token),
        "antigravity global mcp config must never contain the raw identity token: {agy_cfg_content}"
    );

    // ---- Antigravity: agent.adopt (same wiring, adopt call path) --------------------------
    let _ = std::fs::remove_file(&agy_token_file);
    let r = call(
        &mut stream,
        next_id,
        "agent.adopt",
        Some(json!({ "lane_id": lane_id, "agent": "antigravity" })),
    )
    .await;
    assert!(
        r.error.is_none(),
        "antigravity adopt errored: {:?}",
        r.error
    );
    let window = r.result.unwrap()["window"]
        .as_str()
        .expect("adopt returns a window")
        .to_string();
    let token = wait_for_token(&agy_token_file).await;
    let identity = ctx
        .store
        .resolve_mcp_identity(token.clone())
        .await
        .unwrap()
        .expect("daemon must resolve the token it minted for the adopted antigravity session");
    assert_eq!(identity.lane_id, Some(lane_id), "identity: {identity:?}");
    assert_eq!(
        identity.window.as_deref(),
        Some(window.as_str()),
        "identity: {identity:?}"
    );
    assert_eq!(
        identity.agent_kind.as_deref(),
        Some("antigravity"),
        "identity: {identity:?}"
    );
    assert!(
        ctx.store
            .resolve_mcp_identity(format!("{token}00"))
            .await
            .unwrap()
            .is_none(),
        "a mutated token must not resolve to any identity"
    );
    let agy_cfg_content = std::fs::read_to_string(&antigravity_cfg).unwrap();
    assert!(
        !agy_cfg_content.contains(&token),
        "antigravity global mcp config must never contain the raw identity token after adopt: {agy_cfg_content}"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new(repomon_core::agent::tmux_program())
        .args(["-L", &session, "kill-server"])
        .output();
    unsafe {
        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("REPOMON_ANTIGRAVITY_MCP_CONFIG");
    }
}
