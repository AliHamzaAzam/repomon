//! Concurrent starts must preserve one orchestrator session; this test owns its process so
//! config-environment changes cannot race other tests.

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
async fn concurrent_starts_spawn_exactly_one_orchestrator() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping concurrent orchestrator start test");
        return;
    }
    let session = format!("repomon-orch-concurrent-it-{}", std::process::id());
    // A throwaway repomind home: `orchestrator.start` ensures the home repo exists, and a test
    // must never create or touch the developer's real `~/repomind`.
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
    // Redirect the `--mcp-config` write away from the developer's real `~/.config/repomon`.
    let cfg_home = tempfile::tempdir().expect("tempdir");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", cfg_home.path());
    }
    // Keep the harmless stand-in alive through both starts so the test exercises locking rather
    // than racing process exit.
    {
        let mut cfg = ctx.config.write().await;
        cfg.agents.insert(
            "slowpoke".to_string(),
            "sh -c 'sleep 30' repomon-test".to_string(),
        );
    }

    let mut a = connect_retry(&sock).await;
    let mut b = connect_retry(&sock).await;
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

    // Both racing callers must receive the same single controller window.
    let window = sa["window"].as_str().expect("a start reports its window");
    assert_eq!(sb["window"], json!(window), "both starts name one window");
    let windows = ctx.backend.list_windows().unwrap();
    let count = windows.iter().filter(|w| w.as_str() == window).count();
    assert_eq!(
        count, 1,
        "expected exactly one repomind window, got {windows:?}"
    );

    let r = call(&mut a, 2, "orchestrator.stop", None).await;
    assert!(
        r.error.is_none(),
        "orchestrator.stop errored: {:?}",
        r.error
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
    let _ = Command::new(repomon_core::agent::tmux_program())
        .args(["-L", &session, "kill-server"])
        .output();
}
