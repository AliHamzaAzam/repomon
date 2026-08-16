//! Deterministic coverage for system.doctor when external binaries (tmux, git, agent CLIs) are missing from PATH.

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
async fn system_doctor_reports_unavailable_when_binaries_missing() {
    let empty_dir = tempfile::tempdir().unwrap();
    // Point PATH at an empty dir so no binaries exist on PATH
    unsafe { std::env::set_var("PATH", empty_dir.path()) };

    let mut config = Config::default();
    config.agents.insert("custom-cli".into(), "nonexistent-cmd-abc".into());
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!("repomon-doctor-missing-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let r = call(&mut stream, 1, "system.doctor", None).await;
    let res = r.result.expect("system.doctor result");

    // Both tmux and git should report available: false with null version/path/source
    assert_eq!(res["tmux"]["available"], json!(false));
    assert_eq!(res["tmux"]["version"], json!(null));
    assert_eq!(res["tmux"]["source"], json!(null));
    assert_eq!(res["tmux"]["path"], json!(null));

    assert_eq!(res["git"]["available"], json!(false));
    assert_eq!(res["git"]["version"], json!(null));
    assert_eq!(res["git"]["path"], json!(null));

    // Every agent should have detected: false
    let agents = res["agents"].as_array().expect("agents array");
    assert!(!agents.is_empty());
    for agent in agents {
        assert_eq!(
            agent["detected"],
            json!(false),
            "agent {} should have detected: false",
            agent["name"]
        );
    }

    server.abort();
    let _ = std::fs::remove_file(&sock);
}
