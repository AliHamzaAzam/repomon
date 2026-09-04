//! End-to-end: a journal row written over the RPC surface reaches the repomind home as a
//! markdown day file, and the export batch lands as one commit authored by Repomind.
//!
//! Every test here points `[repomind] home` at a tempdir. The operator's real `~/repomind` is
//! never created, read, or written by the suite.

use std::path::Path;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint, IpcStream};
use repomon_core::{Config, Store};
use repomon_daemon::{Ctx, serve};
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
    let ctx = Ctx::new(Store::open_in_memory().unwrap(), config, None);
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

/// Repo notes are file-first: `repo.notes.set` writes `fleet/<repo>/notes.md` in the home, and
/// `repo.notes.get` reads it back. The app-support `repo-notes/` directory is no longer written.
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
    let ctx = Ctx::new_with_paths(
        Store::open_in_memory().unwrap(),
        config,
        None,
        dir.path().join("config.toml"),
        legacy.clone(),
    );
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
