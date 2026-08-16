//! The OpenCode orchestrator backend, exercised through the daemon's own RPC surface: a start
//! with `agent: "opencode"` must record the opencode backend with no session id,
//! `orchestrator.transcript` must read as an empty chat (opencode's on-disk session format is not
//! parsed — the pane stream is the view), and `orchestrator.stop` cleanly stops the window.

use std::process::Command;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::{Ctx, serve};
use serde_json::json;

/// Connect to the daemon's IPC endpoint, retrying while it binds.
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
async fn opencode_backend_starts_degrades_transcript_and_stops() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping opencode orchestrator test");
        return;
    }
    let session = format!("repomon-orch-opencode-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock =
        std::env::temp_dir().join(format!("repomon-orch-opencode-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    // An opencode start records the opencode backend and no session id.
    let r = call(
        &mut stream,
        1,
        "orchestrator.start",
        Some(json!({ "agent": "opencode", "autonomy": "read-only" })),
    )
    .await;
    assert!(r.error.is_none(), "opencode start errored: {:?}", r.error);
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(true), "status: {status}");
    assert_eq!(status["backend"], json!("opencode"), "status: {status}");
    assert_eq!(status["agent"], json!("opencode"), "status: {status}");
    assert_eq!(status["autonomy"], json!("read-only"), "status: {status}");
    assert!(
        status["session_id"].is_null(),
        "opencode can't pin a session id — must be null, got: {status}"
    );

    // The transcript reads as an empty chat for an opencode backend.
    let r = call(&mut stream, 2, "orchestrator.transcript", Some(json!({}))).await;
    assert!(r.error.is_none(), "transcript errored: {:?}", r.error);
    assert_eq!(r.result.unwrap(), json!([]));

    let r = call(&mut stream, 3, "orchestrator.stop", None).await;
    assert!(
        r.error.is_none(),
        "orchestrator.stop errored: {:?}",
        r.error
    );
    assert_eq!(r.result.unwrap()["running"], json!(false));

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new(repomon_core::agent::tmux_program())
        .args(["-L", &session, "kill-server"])
        .output();
}
