//! One CLI integration test: `repomon repomind status|boot|export` against an isolated daemon.
//!
//! Table rendering is covered by the pure-function unit tests in `cli.rs`
//! (`format_repomind_status`/`format_repomind_boot`/`format_repomind_export`); this test only
//! exercises the RPC round trip through `repomon_tui::cli::handle`, the same entry point the
//! `repomon` binary calls for a headless subcommand.
//!
//! `[repomind] home` points at a tempdir for the whole test: the operator's real `~/repomind` is
//! never created, read, or written.

use std::path::Path;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store};
use repomon_daemon::{Ctx, serve};
use repomon_tui::cli::{Command, RepomindCmd};

async fn connect_retry(sock: &Path) -> IpcStream {
    for _ in 0..100 {
        if let Ok(s) = transport::connect(&Endpoint::from_path(sock)).await {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("daemon endpoint {} never came up", sock.display());
}

async fn call(stream: &mut IpcStream, id: u64, method: &str, params: Option<serde_json::Value>) -> Response {
    let req = Request::new(id, method, params);
    protocol::write_message(stream, &req).await.unwrap();
    let frame = protocol::read_frame(stream)
        .await
        .unwrap()
        .expect("response frame");
    serde_json::from_slice(&frame).unwrap()
}

#[tokio::test]
async fn repomind_cli_status_boot_export_round_trip_through_an_isolated_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config.clone(), None);
    // Ensure the home up front (same as the daemon does on its own start / `orchestrator.start`)
    // so `repomind.boot`/`repomind.export` below have somewhere to write.
    repomon_daemon::repomind::ensure_home(&ctx).await.unwrap();

    let sock =
        std::env::temp_dir().join(format!("repomon-repomind-cli-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let _server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    // Confirm the daemon is actually listening before handing the socket to `cli::handle`: its
    // `connect()` auto-spawns a detached `repomond` on a connect failure, and this test must
    // never fall back to a real installed binary.
    let mut probe = connect_retry(&sock).await;
    let ping = call(&mut probe, 1, "ping", None).await;
    assert!(ping.result.is_some(), "daemon did not come up: {ping:?}");

    repomon_tui::cli::handle(
        Command::Repomind {
            cmd: RepomindCmd::Status,
        },
        &config,
        Some(sock.clone()),
    )
    .await
    .expect("repomind status");

    repomon_tui::cli::handle(
        Command::Repomind {
            cmd: RepomindCmd::Boot,
        },
        &config,
        Some(sock.clone()),
    )
    .await
    .expect("repomind boot");
    assert!(
        home.join(".repomind").join("boot.md").is_file(),
        "repomind boot should have written the boot document"
    );

    repomon_tui::cli::handle(
        Command::Repomind {
            cmd: RepomindCmd::Export,
        },
        &config,
        Some(sock.clone()),
    )
    .await
    .expect("repomind export");
}
