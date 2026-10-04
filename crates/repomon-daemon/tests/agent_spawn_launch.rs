//! Launch failures through an isolated daemon and tmux server, without real agent CLIs.
#![cfg(unix)]

mod common;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::serve;
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

async fn connect_retry(sock: &std::path::Path) -> IpcStream {
    for _ in 0..100 {
        if let Ok(s) = transport::connect(&Endpoint::from_path(sock)).await {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("daemon endpoint {} never came up", sock.display());
}
fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@e.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@e.com")
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
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
#[cfg(unix)]
async fn agent_spawn_reports_immediate_exit_with_pane_error() {
    use std::os::unix::fs::PermissionsExt;

    assert!(TmuxRuntime::available(), "this regression requires tmux");
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var(
            "REPOMON_ANTIGRAVITY_MCP_CONFIG",
            dir.path().join("agy.json"),
        );
    }
    let log = dir.path().join("daemon.log");
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::ERROR)
            .with_writer(std::fs::File::create(&log).unwrap())
            .finish(),
    )
    .unwrap();
    let mut config = Config::default();
    let message = "error: unexpected argument '--obsolete-flag' found";
    for agent in ["codex", "claude", "opencode", "agy", "hermes"] {
        let command = dir.path().join(agent);
        std::fs::write(
            &command,
            format!(
                "#!/bin/sh\n[ \"$1\" = config ] && exit 0\nprintf '%s' \"$REPOMON_MCP_IDENTITY_TOKEN\" > \"$0.token\"\n{}printf '%s\\n' \"{message}\" >&2\nexit 2\n",
                if agent == "claude" { "sleep 0.3\n" } else { "" }
            ),
        )
        .unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
        config
            .agents
            .insert(agent.into(), command.display().to_string());
    }
    let fixture = common::Fixture::new(Store::open_in_memory().unwrap(), config, None);
    let sock = dir.path().join("launch.sock");
    let server = {
        let ctx = fixture.ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["commit", "--allow-empty", "-m", "init"]);
    call(&mut stream, 1, "repo.add", Some(json!({"path": repo}))).await;
    let lanes = call(&mut stream, 2, "lane.list", None)
        .await
        .result
        .unwrap();
    let lane = lanes[0]["id"].as_i64().unwrap();
    for agent in ["codex", "claude", "opencode", "agy", "hermes"] {
        for task in [Some("inspect this repository"), None] {
            let response = call(
                &mut stream,
                3,
                "agent.spawn",
                Some(json!({"lane_id": lane, "agent": agent, "task": task})),
            )
            .await;
            assert!(
                response.result.is_none(),
                "{agent}: false success: {response:?}"
            );
            let error = response
                .error
                .expect("dead launch must return an RPC error");
            eprintln!("{agent} task={}: {}", task.is_some(), error.message);
            assert!(error.message.contains(message), "{agent}: {error:?}");
            assert!(
                error.message.contains("Agent launch failed"),
                "{agent}: {error:?}"
            );
            let token = std::fs::read_to_string(dir.path().join(format!("{agent}.token"))).unwrap();
            assert!(!token.is_empty());
            assert!(
                fixture
                    .ctx
                    .store
                    .resolve_mcp_identity(token)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(fixture.ctx.backend.list_windows().unwrap().is_empty());
        }
    }
    let logged = std::fs::read_to_string(log).unwrap();
    assert!(logged.contains("agent.spawn failed"), "{logged}");
    assert!(logged.contains(message), "{logged}");
    server.abort();
}
