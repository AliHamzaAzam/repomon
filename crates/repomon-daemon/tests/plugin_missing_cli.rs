//! Deterministic coverage for the "claude CLI not on PATH" (-32021) error path.
//!
//! This lives in its own file (its own test binary/process) rather than alongside the other
//! `plugin.*`/`ext.*` coverage in `tests/integration.rs`. `claude_cli()`'s cache
//! (`static CLAUDE_CLI` in `repomon-daemon/src/rpc.rs`) is process-global and, by design, only
//! ever caches a *successful* detection (see the -32021 fix: a miss re-probes so installing the
//! CLI later doesn't need a daemon restart). That means once any test in a given process
//! successfully detects a real `claude` binary, every later `claude_cli()` call in that same
//! process returns the cached handle immediately, `REPOMON_CLAUDE_BIN` override or not. On a
//! machine that actually has `claude` on PATH (as this one does, since this daemon is developed
//! from inside Claude Code), running this test alongside `tests/integration.rs`'s
//! `extension_rpcs_list_toggle_and_fan_out` / `plugin_details_returns_cli_text_or_structured_error`
//! (both of which exercise `claude_cli()` against the real system binary) made the -32021
//! assertion here flaky-to-always-failing depending on test execution order. A separate file
//! gives this test a pristine `CLAUDE_CLI` static regardless of what else is on PATH or what
//! order tests run in.

use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store};
use repomon_daemon::{Ctx, serve};
use serde_json::json;

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
    let frame = protocol::read_frame(stream)
        .await
        .unwrap()
        .expect("response frame");
    serde_json::from_slice(&frame).unwrap()
}

#[tokio::test]
async fn plugin_update_reports_missing_cli() {
    // Point REPOMON_CLAUDE_BIN at a path that can never exist, so detection deterministically
    // fails regardless of whether a real `claude` happens to be on this machine's PATH.
    unsafe { std::env::set_var("REPOMON_CLAUDE_BIN", "/nonexistent/claude-missing-xyz") };

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-ext3-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;
    let r = call(&mut stream, 1, "plugin.update", Some(json!({}))).await;
    assert_eq!(r.error.unwrap().code, -32021);

    server.abort();
    let _ = std::fs::remove_file(&sock);
    unsafe { std::env::remove_var("REPOMON_CLAUDE_BIN") };
}
