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

static ENV_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn system_doctor_reports_unavailable_when_binaries_missing() {
    let _lock = ENV_MUTEX.lock().await;
    let old_path = std::env::var_os("PATH");
    let old_tmux = std::env::var_os("REPOMON_TMUX");
    let empty_dir = tempfile::tempdir().unwrap();
    // Point PATH at an empty dir so no binaries exist on PATH
    unsafe {
        std::env::set_var("PATH", empty_dir.path());
        std::env::remove_var("REPOMON_TMUX");
    }

    let mut config = Config::default();
    config
        .agents
        .insert("custom-cli".into(), "nonexistent-cmd-abc".into());
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let sock = std::env::temp_dir().join(format!(
        "repomon-doctor-missing-{}.sock",
        std::process::id()
    ));
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
    unsafe {
        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        } else {
            std::env::remove_var("PATH");
        }
        if let Some(t) = old_tmux {
            std::env::set_var("REPOMON_TMUX", t);
        } else {
            std::env::remove_var("REPOMON_TMUX");
        }
    }
}

#[tokio::test]
async fn system_doctor_honors_repomon_tmux_env_override() {
    let _lock = ENV_MUTEX.lock().await;
    let old_tmux = std::env::var_os("REPOMON_TMUX");
    let dir = tempfile::tempdir().unwrap();
    let fake_tmux = dir.path().join("custom-tmux");
    std::fs::write(&fake_tmux, b"#!/bin/sh\necho tmux 3.4\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    unsafe { std::env::set_var("REPOMON_TMUX", &fake_tmux) };

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-doctor-env-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let r = call(&mut stream, 1, "system.doctor", None).await;
    let res = r.result.expect("system.doctor result");

    assert_eq!(res["tmux"]["available"], json!(true));
    assert_eq!(res["tmux"]["source"], json!("system"));
    assert_eq!(res["tmux"]["path"], json!(fake_tmux.to_str().unwrap()));

    server.abort();
    let _ = std::fs::remove_file(&sock);
    unsafe {
        if let Some(t) = old_tmux {
            std::env::set_var("REPOMON_TMUX", t);
        } else {
            std::env::remove_var("REPOMON_TMUX");
        }
    }
}
