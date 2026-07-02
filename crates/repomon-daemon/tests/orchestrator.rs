//! Orchestrator lifecycle: a genuine `orchestrator.start` (no window pre-exists) must record the
//! requested autonomy on the session; a tmux window named `orchestrator` that survives a daemon
//! (re)start must be adopted by `orchestrator.start` rather than duplicated, with its autonomy
//! read back as unknown (the prior process's request is gone); and a window killed out from
//! under the daemon must be reconciled away instead of read as still running. The spawned
//! `claude` process itself is never exercised (nothing here waits on or inspects it) — only the
//! daemon's own spawn/adopt/reconcile bookkeeping.

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

    // A genuine `orchestrator.start` below writes its `--mcp-config` file under
    // `config::config_dir()`; redirect that to a tempdir so the test doesn't touch the
    // developer's real `~/.config/repomon`. Safe to mutate process env here — this is the only
    // test in this integration binary.
    let cfg_home = tempfile::tempdir().expect("tempdir");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", cfg_home.path());
    }
    // Point the genuine-spawn scenario below at a harmless custom "agent" instead of real
    // `claude` — this is a dev machine running Claude Code, so `claude` is almost certainly on
    // PATH, and we do NOT want a test to launch a real autonomous session wired to the fleet MCP
    // tools. `build_orchestrator_command` always appends `--mcp-config ... --append-system-prompt
    // ... --allowedTools ... --session-id ...`; `true` ignores all arguments and exits 0, so it
    // exercises the real `orchestrator_base_command`/`build_orchestrator_command`/
    // `tmux.spawn_named` path safely.
    {
        let mut cfg = ctx.config.write().await;
        cfg.agents.insert("noop".to_string(), "true".to_string());
    }

    // 2. Nothing tracked yet.
    let r = call(&mut stream, 1, "orchestrator.status", None).await;
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(false), "status: {status}");

    // 2b. A genuine start (no window pre-exists, so this hits the real spawn path rather than
    // adopt) records the requested autonomy on the session in `orchestrator.start`'s own
    // response. (Not asserted via a follow-up `orchestrator.status`: `true` — deliberately
    // chosen so this doesn't launch a real `claude` — exits immediately, and `orchestrator.status`
    // reconciles a since-vanished window away, which would flakily race this check.)
    let r = call(
        &mut stream,
        2,
        "orchestrator.start",
        Some(json!({ "agent": "noop", "autonomy": "supervised" })),
    )
    .await;
    assert!(
        r.error.is_none(),
        "orchestrator.start errored: {:?}",
        r.error
    );
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(true), "status: {status}");
    assert_eq!(status["autonomy"], json!("supervised"), "status: {status}");
    // A genuine spawn always mints and pins a `--session-id`, appended to `true`'s command line
    // (which — deliberately — ignores it, exiting 0 regardless); the daemon still records it so
    // the transcript picker can pin to this exact session instead of guessing by recency.
    let session_id = status["session_id"]
        .as_str()
        .expect("genuine spawn must record a non-null session_id");
    assert_eq!(
        session_id.len(),
        36,
        "session_id must be UUID-shaped: {session_id}"
    );

    // Tear it down so the adopt scenario below starts from a clean slate.
    let r = call(&mut stream, 3, "orchestrator.stop", None).await;
    assert!(
        r.error.is_none(),
        "orchestrator.stop errored: {:?}",
        r.error
    );
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(false), "status: {status}");

    // 3. Spawn a fake orchestrator window directly via the same TmuxRuntime the daemon uses —
    // as if a window from a previous daemon lifetime survived a restart.
    let home = std::env::temp_dir();
    ctx.tmux
        .spawn_named("orchestrator", &home, "sleep 30")
        .expect("spawn fake orchestrator window");

    // 4. orchestrator.start adopts the surviving window: running:true, and does not spawn a
    // second one. Its autonomy is unknown (the prior daemon process's request is gone), so it
    // must read null/absent rather than echoing this call's own `autonomy` param.
    let r = call(
        &mut stream,
        4,
        "orchestrator.start",
        Some(json!({ "autonomy": "autonomous" })),
    )
    .await;
    assert!(
        r.error.is_none(),
        "orchestrator.start errored: {:?}",
        r.error
    );
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(true), "status: {status}");
    assert_eq!(status["window"], json!("orchestrator"));
    assert!(
        status["autonomy"].is_null(),
        "adopted session's autonomy should be unknown: {status}"
    );
    assert!(
        status["session_id"].is_null(),
        "adopted session's session_id should be unknown (this process never captured the prior \
         process's --session-id): {status}"
    );
    let windows = ctx.tmux.list_windows().unwrap();
    let orchestrator_windows = windows.iter().filter(|w| *w == "orchestrator").count();
    assert_eq!(
        orchestrator_windows, 1,
        "expected exactly one orchestrator window (adopted, not duplicated), got {windows:?}"
    );

    // 5. orchestrator.send_input succeeds against the adopted window.
    let r = call(
        &mut stream,
        5,
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
    let r = call(&mut stream, 6, "orchestrator.status", None).await;
    let status = r.result.unwrap();
    assert_eq!(status["running"], json!(false), "status: {status}");

    // 8. orchestrator.send_input on the now-dead orchestrator errors loudly.
    let r = call(
        &mut stream,
        7,
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
