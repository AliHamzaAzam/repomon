//! Orchestrator lifecycle (adopt path): a tmux window named `orchestrator` that survives a
//! daemon (re)start must be adopted by `orchestrator.start` rather than duplicated, and a window
//! killed out from under the daemon must be reconciled away instead of read as still running.
//! No `claude` binary is involved — this only exercises the adopt/reconcile bookkeeping.

use std::process::Command;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::{Ctx, serve};
use serde_json::json;
use tokio::net::UnixStream;

async fn call(
    stream: &mut UnixStream,
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
async fn orchestrator_adopts_a_surviving_window() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping orchestrator lifecycle test");
        return;
    }
    // A unique tmux session (name doubles as the `-L` socket) so parallel CI runs never collide
    // and we never touch the user's real `repomon` session.
    let session = format!("repomon-orch-lifecycle-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!(
        "repomon-orch-lifecycle-it-{}.sock",
        std::process::id()
    ));
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

    // 2. Nothing tracked yet.
    let r = call(&mut stream, 1, "orchestrator.status", None).await;
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(false), "status: {status}");

    // 3. Spawn a fake orchestrator window directly via the same TmuxRuntime the daemon uses —
    // as if a window from a previous daemon lifetime survived a restart.
    let home = std::env::temp_dir();
    ctx.tmux
        .spawn_named("orchestrator", &home, "sleep 30")
        .expect("spawn fake orchestrator window");

    // 4. orchestrator.start adopts the surviving window: running:true, and does not spawn a
    // second one.
    let r = call(&mut stream, 2, "orchestrator.start", Some(json!({}))).await;
    assert!(
        r.error.is_none(),
        "orchestrator.start errored: {:?}",
        r.error
    );
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(true), "status: {status}");
    assert_eq!(status["window"], json!("orchestrator"));
    let windows = ctx.tmux.list_windows().unwrap();
    let orchestrator_windows = windows.iter().filter(|w| *w == "orchestrator").count();
    assert_eq!(
        orchestrator_windows, 1,
        "expected exactly one orchestrator window (adopted, not duplicated), got {windows:?}"
    );

    // 5. orchestrator.send_input succeeds against the adopted window.
    let r = call(
        &mut stream,
        3,
        "orchestrator.send_input",
        Some(json!({ "text": "hello", "enter": false })),
    )
    .await;
    assert!(r.error.is_none(), "send_input errored: {:?}", r.error);

    // 6. Kill the window out from under the daemon.
    ctx.tmux
        .kill_named("orchestrator")
        .expect("kill fake orchestrator window");

    // 7. orchestrator.status reconciles to running:false instead of reading a corpse as alive.
    let r = call(&mut stream, 4, "orchestrator.status", None).await;
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(false), "status: {status}");

    // 8. orchestrator.send_input on the now-dead orchestrator errors loudly.
    let r = call(
        &mut stream,
        5,
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

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}
