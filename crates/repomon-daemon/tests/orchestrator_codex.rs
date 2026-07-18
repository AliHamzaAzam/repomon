//! The codex orchestrator backend, exercised through the daemon's own RPC surface: a start with
//! `agent: "codex"` must record the codex backend with no session id (codex can't pin one),
//! `orchestrator.transcript` must read as an empty chat (codex's on-disk session format is not
//! parsed — the pane stream is the view), and an MCP-less agent (`aider`) must be rejected
//! loudly instead of spawning a broken window. Whether a real `codex` binary is installed is
//! deliberately irrelevant: every assertion is on the daemon's own bookkeeping, mirroring
//! `orchestrator.rs`'s approach, and the window is stopped at the end either way. Kept in its
//! own integration binary: like `orchestrator.rs`, it mutates process env (`XDG_CONFIG_HOME`),
//! which is only safe when no other test shares the process.

use std::process::Command;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::{Ctx, serve};
use serde_json::json;

/// Connect to the daemon's IPC endpoint, retrying while it binds. (A socket-file existence
/// check doesn't port: Windows named pipes have no filesystem presence.)
async fn connect_retry(sock: &std::path::Path) -> IpcStream {
    for _ in 0..100 {
        if let Ok(s) = transport::connect(&Endpoint::from_path(sock)).await {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("daemon endpoint {} never came up", sock.display());
}

async fn call(
    stream: &mut IpcStream,
    id: u64,
    method: &str,
    params: Option<serde_json::Value>,
) -> Response {
    let req = Request::new(id, method, params);
    protocol::write_message(stream, &req).await.unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(10), protocol::read_frame(stream))
        .await
        .expect("timed out waiting for daemon response")
        .unwrap()
        .expect("response frame");
    serde_json::from_slice(&frame).unwrap()
}

#[tokio::test]
async fn codex_backend_degrades_and_mcpless_agents_are_rejected() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping codex orchestrator test");
        return;
    }
    let session = format!("repomon-orch-codex-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock =
        std::env::temp_dir().join(format!("repomon-orch-codex-it-{}.sock", std::process::id()));
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
    let mut stream = connect_retry(&sock).await;

    // An agent with no MCP client can't drive the fleet: loud invalid_params, nothing spawned.
    let r = call(
        &mut stream,
        1,
        "orchestrator.start",
        Some(json!({ "agent": "aider" })),
    )
    .await;
    let err = r.error.expect("aider must be rejected as orchestrator");
    assert!(
        err.message.contains("aider"),
        "unexpected message: {}",
        err.message
    );
    let r = call(&mut stream, 2, "orchestrator.status", None).await;
    assert_eq!(r.result.unwrap()["running"], json!(false));

    // A codex start records the codex backend and — unlike a Claude spawn — no session id.
    // `read-only` autonomy so that if a real `codex` binary is installed and boots in the
    // window before the stop below, the MCP policy layer caps what it may do.
    let r = call(
        &mut stream,
        3,
        "orchestrator.start",
        Some(json!({ "agent": "codex", "autonomy": "read-only" })),
    )
    .await;
    assert!(r.error.is_none(), "codex start errored: {:?}", r.error);
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(true), "status: {status}");
    assert_eq!(status["backend"], json!("codex"), "status: {status}");
    assert_eq!(status["agent"], json!("codex"), "status: {status}");
    assert_eq!(status["autonomy"], json!("read-only"), "status: {status}");
    assert!(
        status["session_id"].is_null(),
        "codex can't pin a session id — must be null, got: {status}"
    );

    // The transcript reads as an empty chat for a codex backend — the gate must answer before
    // any ~/.claude scan happens, so no other live Claude session's transcript can ever be
    // misattributed as this orchestrator's. ([] also happens to be the answer if the window
    // already died on a codex-less machine and reconcile cleared the session — both fine.)
    let r = call(&mut stream, 4, "orchestrator.transcript", Some(json!({}))).await;
    assert!(r.error.is_none(), "transcript errored: {:?}", r.error);
    assert_eq!(r.result.unwrap(), json!([]));

    let r = call(&mut stream, 5, "orchestrator.stop", None).await;
    assert!(
        r.error.is_none(),
        "orchestrator.stop errored: {:?}",
        r.error
    );
    assert_eq!(r.result.unwrap()["running"], json!(false));

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}
