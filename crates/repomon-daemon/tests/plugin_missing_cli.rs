//! Isolates missing-CLI discovery because successful CLAUDE_CLI discovery is cached for the process
//! lifetime.

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
