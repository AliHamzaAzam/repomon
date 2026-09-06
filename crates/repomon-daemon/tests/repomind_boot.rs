//! Tests boot regeneration and status with temporary homes that isolate the operator’s fleet
//! memory.

mod common;

use std::path::Path;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store};
use repomon_daemon::serve;
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
    let frame = protocol::read_frame(stream)
        .await
        .unwrap()
        .expect("response frame");
    serde_json::from_slice(&frame).unwrap()
}

fn seed(home: &Path) {
    std::fs::write(home.join("REPOMIND.md"), "# Overlay\n\nkeep it terse\n").unwrap();
    std::fs::write(
        home.join("plans/active/ship-r3.md"),
        "---\ntitle: Ship R3\nstatus: in flight\nowner: lane-1/1\n---\n\nNext step: land the boot document\n",
    )
    .unwrap();
}

#[tokio::test]
async fn repomind_boot_regenerates_the_document_and_status_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();
    let fixture = common::Fixture::new(Store::open_in_memory().unwrap(), config, None);
    let ctx = fixture.ctx.clone();
    repomon_daemon::repomind::ensure_home(&ctx).await.unwrap();
    seed(&home);

    let sock = std::env::temp_dir().join(format!("repomon-rmb-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let booted = call(&mut stream, 1, "repomind.boot", None).await;
    assert!(booted.error.is_none(), "{:?}", booted.error);
    let result = booted.result.unwrap();

    assert_eq!(
        result["path"],
        json!(home.join(".repomind/boot.md").to_string_lossy())
    );
    assert_eq!(result["trimmed"], json!([]));
    assert!(result["bytes"].as_u64().unwrap() > 0);
    assert!(result["tokens_estimate"].as_u64().unwrap() > 0);

    let body = std::fs::read_to_string(home.join(".repomind/boot.md")).unwrap();
    assert!(body.contains("keep it terse"), "{body}");
    assert!(
        body.contains("Ship R3: in flight, owner lane-1/1"),
        "{body}"
    );
    assert!(body.contains("land the boot document"), "{body}");

    assert!(body.contains("## Fleet snapshot"), "{body}");

    let status = call(&mut stream, 2, "repomind.status", None).await;
    let boot = &status.result.unwrap()["boot"];
    assert_eq!(boot["tokens_estimate"], result["tokens_estimate"]);
    assert_eq!(boot["trimmed"], json!([]));
    assert!(boot["generated_at"].is_string(), "{boot}");

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

/// A home that has never been booted reports no boot state rather than inventing one, and
/// `repomind.status` still answers.
#[tokio::test]
async fn status_reports_no_boot_state_before_the_first_regeneration() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();
    let fixture = common::Fixture::new(Store::open_in_memory().unwrap(), config, None);
    let ctx = fixture.ctx.clone();

    let sock = std::env::temp_dir().join(format!("repomon-rmb0-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let status = call(&mut stream, 1, "repomind.status", None).await;
    let result = status.result.unwrap();
    assert_eq!(result["exists"], json!(false));
    assert_eq!(result["boot"]["generated_at"], Value::Null);
    assert_eq!(result["boot"]["tokens_estimate"], json!(0));
    assert_eq!(result["boot"]["trimmed"], json!([]));

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

/// A reduced boot budget must truncate content and report truncation in both the file and RPC
/// response.
#[tokio::test]
async fn a_tiny_budget_trims_the_document_and_the_result_names_what_went() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();
    config.repomind.boot_budget_tokens = 150;
    let fixture = common::Fixture::new(Store::open_in_memory().unwrap(), config, None);
    let ctx = fixture.ctx.clone();
    repomon_daemon::repomind::ensure_home(&ctx).await.unwrap();
    seed(&home);
    std::fs::write(
        home.join("profile/fleet.md"),
        format!("{}\n", "profile prose. ".repeat(60)),
    )
    .unwrap();

    let sock = std::env::temp_dir().join(format!("repomon-rmbt-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let result = call(&mut stream, 1, "repomind.boot", None)
        .await
        .result
        .unwrap();

    assert_eq!(result["trimmed"], json!(["profile/fleet.md"]));
    let body = std::fs::read_to_string(home.join(".repomind/boot.md")).unwrap();
    assert!(
        body.trim_end().ends_with("Trimmed: profile/fleet.md"),
        "{body}"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
}
