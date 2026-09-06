//! The controller lane's recorded primary window can go stale: a later spawn into the lane
//! (the operator's Spawn, or another `orchestrator.start` after a restart) unconditionally
//! overwrites the record. When that later session ends while an earlier one in the same lane is
//! still live, `repomind.status` and `repomind.instruct` must resolve to the live session rather
//! than reporting, or typing into, a corpse.
//!
//! Every test here points `[repomind] home` at a tempdir and `tmux_session` at a throwaway `-L`
//! session, so the operator's real `~/repomind` and `repomon` tmux session are never touched.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use repomon_core::agent::backend::SpawnSpec;
use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::{Ctx, serve};
use serde_json::{Value, json};

async fn connect_retry(sock: &Path) -> IpcStream {
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

#[tokio::test]
async fn repomind_instruct_reaches_the_live_window_when_the_record_is_stale() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping repomind primary-window staleness test");
        return;
    }
    let session = format!("repomon-primary-it-{}", std::process::id());
    let repomind_home = tempfile::tempdir().expect("repomind home tempdir");
    let mut config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    config.repomind.home = repomind_home
        .path()
        .join("repomind")
        .to_string_lossy()
        .into_owned();
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let home = repomon_daemon::repomind::ensure_home(&ctx)
        .await
        .expect("ensure_home");

    let sock = std::env::temp_dir().join(format!("repomon-primary-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let tmp = std::env::temp_dir();
    // Window one: the genuine, still-running controller session.
    let w1 = ctx
        .backend
        .spawn(home.lane_id, &SpawnSpec::new("sleep 60", &tmp))
        .expect("spawn window one");
    // Window two: a later spawn into the same lane — mirrors the operator's Spawn button (or a
    // second `orchestrator.start` after a restart), which unconditionally records its own window
    // as "the" controller window.
    let w2 = ctx
        .backend
        .spawn(home.lane_id, &SpawnSpec::new("sleep 60", &tmp))
        .expect("spawn window two");
    assert_ne!(w1, w2);
    ctx.store
        .set_lane_tmux_window(home.lane_id, Some(w2.clone()))
        .await
        .unwrap();
    // Window two's session ends; window one is still the live controller. The record now
    // names a corpse.
    ctx.backend.kill_named(&w2).expect("kill window two");

    // `repomind.status` resolves the live window rather than reporting the stale record.
    let status = call(&mut stream, 1, "repomind.status", None).await;
    assert!(
        status.error.is_none(),
        "repomind.status errored: {:?}",
        status.error
    );
    let result = status.result.unwrap();
    assert_eq!(
        result["window"],
        json!(w1),
        "status must report the live window, not the stale record: {result}"
    );

    // `repomind.instruct` reaches the live window despite the stale record — it must not report
    // "no controller is running" for a controller that plainly is.
    let instruct = call(
        &mut stream,
        2,
        "repomind.instruct",
        Some(json!({ "text": "status check" })),
    )
    .await;
    assert!(
        instruct.error.is_none(),
        "repomind.instruct errored: {:?}",
        instruct.error
    );
    let result = instruct.result.unwrap();
    assert_eq!(
        result["window"],
        json!(w1),
        "instruct must target the live window: {result}"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new(repomon_core::agent::tmux_program())
        .args(["-L", &session, "kill-server"])
        .output();
}

/// `repomind.status` reports `window: null` — not the stale record, not an error — when the
/// controller lane exists but nothing in it is currently live.
#[tokio::test]
async fn repomind_status_reports_null_window_when_nothing_is_live() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping repomind primary-window staleness test");
        return;
    }
    let session = format!("repomon-primary-none-it-{}", std::process::id());
    let repomind_home = tempfile::tempdir().expect("repomind home tempdir");
    let mut config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    config.repomind.home = repomind_home
        .path()
        .join("repomind")
        .to_string_lossy()
        .into_owned();
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let home = repomon_daemon::repomind::ensure_home(&ctx)
        .await
        .expect("ensure_home");

    let sock = std::env::temp_dir().join(format!(
        "repomon-primary-none-it-{}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let tmp = std::env::temp_dir();
    let w1 = ctx
        .backend
        .spawn(home.lane_id, &SpawnSpec::new("sleep 60", &tmp))
        .expect("spawn window one");
    ctx.store
        .set_lane_tmux_window(home.lane_id, Some(w1.clone()))
        .await
        .unwrap();
    ctx.backend.kill_named(&w1).expect("kill window one");

    let status = call(&mut stream, 1, "repomind.status", None).await;
    let result = status.result.unwrap();
    assert_eq!(result["window"], Value::Null, "status: {result}");

    let instruct = call(
        &mut stream,
        2,
        "repomind.instruct",
        Some(json!({ "text": "status check" })),
    )
    .await;
    assert!(instruct.result.is_none());
    let err = instruct
        .error
        .expect("instruct must refuse when no controller is live");
    assert!(
        err.message.contains("no controller is running"),
        "unexpected message: {}",
        err.message
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new(repomon_core::agent::tmux_program())
        .args(["-L", &session, "kill-server"])
        .output();
}
