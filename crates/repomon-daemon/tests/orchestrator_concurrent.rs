//! Unix-only until the daemon/client speak the portable IPC transport (next PR in this track).
#![cfg(unix)]

//! Two `orchestrator.start` calls racing on separate connections must resolve to a single
//! orchestrator: one spawn, both responses describing the same session. Before the handler held
//! the session lock across its check → spawn → record sequence, the second caller could observe
//! "not running" mid-spawn and either spawn a duplicate `orchestrator` window (tmux happily
//! allows duplicate names) or take the adopt branch on the first caller's fresh window and
//! overwrite its just-recorded session id. Kept in its own integration binary: like
//! `orchestrator.rs`, it mutates process env (`XDG_CONFIG_HOME`), which is only safe when no
//! other test shares the process.

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
async fn concurrent_starts_spawn_exactly_one_orchestrator() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping concurrent orchestrator start test");
        return;
    }
    let session = format!("repomon-orch-concurrent-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!(
        "repomon-orch-concurrent-it-{}.sock",
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

    // Redirect the `--mcp-config` write away from the developer's real `~/.config/repomon`.
    let cfg_home = tempfile::tempdir().expect("tempdir");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", cfg_home.path());
    }
    // A harmless long-lived stand-in for `claude`: the window must OUTLIVE both racing starts
    // (unlike `orchestrator.rs`'s instantly-exiting `true`) or the second caller's adopt/spawn
    // decision races the first window's death instead of the lock. `sh -c 'sleep 30' repomon-test`
    // swallows the appended Claude flags as positional params — including the multi-line
    // `--append-system-prompt` persona, which stays one single-quoted argument.
    {
        let mut cfg = ctx.config.write().await;
        cfg.agents.insert(
            "slowpoke".to_string(),
            "sh -c 'sleep 30' repomon-test".to_string(),
        );
    }

    let mut a = UnixStream::connect(&sock).await.expect("connect a");
    let mut b = UnixStream::connect(&sock).await.expect("connect b");
    let params = json!({ "agent": "slowpoke", "autonomy": "supervised" });
    let (ra, rb) = tokio::join!(
        call(&mut a, 1, "orchestrator.start", Some(params.clone())),
        call(&mut b, 1, "orchestrator.start", Some(params.clone())),
    );

    for (name, r) in [("a", &ra), ("b", &rb)] {
        assert!(
            r.error.is_none(),
            "orchestrator.start on {name} errored: {:?}",
            r.error
        );
    }
    let sa = ra.result.unwrap();
    let sb = rb.result.unwrap();
    assert_eq!(sa["running"], json!(true), "a: {sa}");
    assert_eq!(sb["running"], json!(true), "b: {sb}");
    // Both callers must describe the SAME session: a genuine spawn mints a session id, and the
    // loser of the race must be answered with the winner's session, not a duplicate spawn's
    // (different id) or an adopt-overwrite's (null id).
    let ida = sa["session_id"]
        .as_str()
        .expect("start a must report the spawned session's id");
    let idb = sb["session_id"]
        .as_str()
        .expect("start b must report the spawned session's id");
    assert_eq!(ida, idb, "both starts must resolve to one session");

    let windows = ctx.tmux.list_windows().unwrap();
    let count = windows.iter().filter(|w| *w == "orchestrator").count();
    assert_eq!(
        count, 1,
        "expected exactly one orchestrator window, got {windows:?}"
    );

    let r = call(&mut a, 2, "orchestrator.stop", None).await;
    assert!(
        r.error.is_none(),
        "orchestrator.stop errored: {:?}",
        r.error
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}
