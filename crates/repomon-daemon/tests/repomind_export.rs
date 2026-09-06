//! Tests journal export into a temporary home and its batched commit without accessing the
//! operator’s fleet memory.

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

fn git(home: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(home)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn journal_append_then_repomind_export_writes_the_day_file_and_commits() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();
    let fixture = common::Fixture::new(Store::open_in_memory().unwrap(), config, None);
    let ctx = fixture.ctx.clone();
    repomon_daemon::repomind::ensure_home(&ctx).await.unwrap();

    let sock = std::env::temp_dir().join(format!("repomon-rmx-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let appended = call(
        &mut stream,
        1,
        "journal.append",
        Some(json!({
            "session": "it",
            "action": "spawn_agent",
            "repo": "repomon",
            "detail": "spawned lane-1-1",
        })),
    )
    .await;
    assert!(appended.error.is_none(), "{:?}", appended.error);

    let exported = call(&mut stream, 2, "repomind.export", None).await;
    assert!(exported.error.is_none(), "{:?}", exported.error);
    let result = exported.result.unwrap();
    assert_eq!(result["kinds"], json!(["journal"]));

    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let file = home.join(format!("journal/{day}.md"));
    let body = std::fs::read_to_string(&file).expect("the day file should exist");
    assert!(body.contains("spawn_agent"), "{body}");
    assert!(body.contains("spawned lane-1-1"), "{body}");

    assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(
        git(&home, &["log", "-1", "--format=%an <%ae>"]),
        "Repomind <repomind@local>"
    );
    assert_eq!(
        git(&home, &["log", "-1", "--format=%s"]),
        "chore(repomind): export journal"
    );

    // A second export with nothing new touches nothing and adds no commit.
    let again = call(&mut stream, 3, "repomind.export", None).await;
    assert_eq!(again.result.unwrap()["files"], json!([]));
    assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

/// Stores and reads repository notes in fleet/<repo>/notes.md under the home without writing the
/// app-support notes directory.
#[tokio::test]
async fn repo_notes_are_written_to_and_read_from_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    let legacy = dir.path().join("repo-notes");
    std::fs::create_dir_all(&legacy).unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&work)
            .output()
            .unwrap()
            .status
            .success()
    );

    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();
    let fixture = common::Fixture::with_paths(
        Store::open_in_memory().unwrap(),
        config,
        None,
        Some(dir.path().join("config.toml")),
        Some(legacy.clone()),
    );
    let ctx = fixture.ctx.clone();
    repomon_daemon::repomind::ensure_home(&ctx).await.unwrap();
    let repo = ctx.registry.add(&work).await.unwrap();

    let sock = std::env::temp_dir().join(format!("repomon-rmn-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let set = call(
        &mut stream,
        1,
        "repo.notes.set",
        Some(json!({ "repo_id": repo.id, "content": "Run bun test before merging.\n" })),
    )
    .await;
    assert!(set.error.is_none(), "{:?}", set.error);

    let home_file = home.join("fleet/work/notes.md");
    assert!(
        home_file.is_file(),
        "notes should live in the home, not only in {}",
        legacy.display()
    );
    assert!(
        std::fs::read_to_string(&home_file)
            .unwrap()
            .contains("Run bun test before merging.")
    );
    assert!(
        std::fs::read_dir(&legacy).unwrap().next().is_none(),
        "the app-support directory must no longer be written"
    );

    let got = call(
        &mut stream,
        2,
        "repo.notes.get",
        Some(json!({ "repo_id": repo.id })),
    )
    .await;
    let got = got.result.unwrap();
    assert_eq!(got["exists"], json!(true));
    assert_eq!(got["content"], json!("Run bun test before merging.\n"));

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

/// Playbooks are file-first: a save lands in `playbooks/drafts/` and stays invisible to search
/// until approval moves the file up into `playbooks/`.
#[tokio::test]
async fn a_playbook_draft_is_inert_until_approval_moves_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();
    let fixture = common::Fixture::new(Store::open_in_memory().unwrap(), config, None);
    let ctx = fixture.ctx.clone();
    repomon_daemon::repomind::ensure_home(&ctx).await.unwrap();

    let sock = std::env::temp_dir().join(format!("repomon-rmp-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let saved = call(
        &mut stream,
        1,
        "playbook.save",
        Some(json!({ "name": "fleet-sweep", "content": "sweep every lane\n" })),
    )
    .await;
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert_eq!(saved.result.unwrap()["status"], json!("draft"));
    assert!(home.join("playbooks/drafts/fleet-sweep.md").is_file());
    assert!(!home.join("playbooks/fleet-sweep.md").exists());

    let found = call(
        &mut stream,
        2,
        "playbook.search",
        Some(json!({ "query": "sweep" })),
    )
    .await;
    assert_eq!(
        found.result.unwrap()["playbooks"],
        json!([]),
        "a draft must never reach search"
    );

    let approved = call(
        &mut stream,
        3,
        "playbook.approve",
        Some(json!({ "name": "fleet-sweep" })),
    )
    .await;
    assert!(approved.error.is_none(), "{:?}", approved.error);
    assert_eq!(approved.result.unwrap()["status"], json!("approved"));
    assert!(home.join("playbooks/fleet-sweep.md").is_file());
    assert!(!home.join("playbooks/drafts/fleet-sweep.md").exists());

    let found = call(
        &mut stream,
        4,
        "playbook.search",
        Some(json!({ "query": "sweep" })),
    )
    .await;
    let books = found.result.unwrap();
    assert_eq!(books["playbooks"][0]["name"], json!("fleet-sweep"));

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

/// Repomind status includes export state and home counts in one response.
#[tokio::test]
async fn repomind_status_reports_the_export_state_and_home_counts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();
    let fixture = common::Fixture::new(Store::open_in_memory().unwrap(), config, None);
    let ctx = fixture.ctx.clone();
    repomon_daemon::repomind::ensure_home(&ctx).await.unwrap();
    std::fs::write(home.join("plans/active/ship-r2.md"), "goal\n").unwrap();

    let sock = std::env::temp_dir().join(format!("repomon-rms-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    call(
        &mut stream,
        1,
        "playbook.save",
        Some(json!({ "name": "fleet-sweep", "content": "sweep\n" })),
    )
    .await;

    let status = call(&mut stream, 2, "repomind.status", None).await;
    assert!(status.error.is_none(), "{:?}", status.error);
    let status = status.result.unwrap();
    assert_eq!(status["exists"], json!(true));
    assert_eq!(status["export"]["pending"], json!(true));
    assert_eq!(status["export"]["last_run"], Value::Null);
    assert_eq!(status["counts"]["active_plans"], json!(1));
    assert_eq!(status["counts"]["drafts"], json!(1));
    assert_eq!(status["counts"]["playbooks"], json!(0));
    assert_eq!(status["counts"]["standing"], json!(0));

    call(&mut stream, 3, "repomind.export", None).await;

    let status = call(&mut stream, 4, "repomind.status", None).await;
    let status = status.result.unwrap();
    assert_eq!(status["export"]["pending"], json!(false));
    assert!(status["export"]["last_run"].is_string(), "{status}");
    assert_eq!(status["export"]["last_error"], Value::Null);

    server.abort();
    let _ = std::fs::remove_file(&sock);
}

/// A daemon start must bring the home up to date, not just react to new writes. Journal rows
/// that predate the home (a fresh install, or a restart that missed a burst) are exported by the
/// start pass and land as one commit within the debounce window.
#[tokio::test]
async fn a_daemon_start_exports_journal_rows_written_before_the_home_existed() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("repomind");
    // The basic-memory registration in the start pass must never reach the operator's real
    // config; point it at a throwaway file instead.
    let basic_memory = dir.path().join("basic-memory");
    std::fs::create_dir_all(&basic_memory).unwrap();

    let store = Store::open_in_memory().unwrap();
    for action in ["spawn_agent", "merge_lane"] {
        store
            .append_journal(repomon_core::model::JournalEntry {
                id: 0,
                at: chrono::Utc::now(),
                session: "pre-existing".into(),
                action: action.into(),
                lane_id: Some(1),
                repo: Some("repomon".into()),
                params: None,
                outcome: "ok".into(),
                detail: Some(format!("{action} before the home existed")),
            })
            .await
            .unwrap();
    }

    let mut config = Config::default();
    config.repomind.home = home.to_string_lossy().into_owned();
    config.repomind.basic_memory_config = Some(
        basic_memory
            .join("config.json")
            .to_string_lossy()
            .into_owned(),
    );
    let fixture = common::Fixture::new(store, config, None);
    let ctx = fixture.ctx.clone();

    tokio::spawn(repomon_daemon::repomind::export::export_watch(ctx.clone()));
    repomon_daemon::repomind::start(&ctx).await;

    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let file = home.join(format!("journal/{day}.md"));
    // `git log` errors on a repo with no commits yet, so poll on HEAD resolving instead.
    let committed = |home: &Path| {
        std::process::Command::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(home)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    for _ in 0..60 {
        if file.exists() && committed(&home) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let body = std::fs::read_to_string(&file).expect("the start pass should write the day file");
    assert!(
        body.contains("spawn_agent before the home existed"),
        "{body}"
    );
    assert!(
        body.contains("merge_lane before the home existed"),
        "{body}"
    );
    assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(
        git(&home, &["log", "-1", "--format=%s"]),
        "chore(repomind): export journal"
    );
}
