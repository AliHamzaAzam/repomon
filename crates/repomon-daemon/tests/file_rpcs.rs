//! Worktree-scoped file I/O RPCs for the upcoming in-app editor (D1 `file.list`, D2
//! `file.read`/`file.write`). No tmux/agent involved — these RPCs never touch a session, so
//! (unlike `fleet_mail_*`) this harness is just `Ctx` + `serve` + a real git worktree, the same
//! shape `lane_diff_reports_commits_ahead_and_uncommitted_stat` (integration.rs) uses.

use std::path::{Path, PathBuf};
use std::process::Command;
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
    let frame = tokio::time::timeout(Duration::from_secs(10), protocol::read_frame(stream))
        .await
        .expect("timed out waiting for daemon response")
        .unwrap()
        .expect("response frame");
    serde_json::from_slice(&frame).unwrap()
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

/// A daemon serving a real git repo with one linked worktree (`root`), registered as a lane.
struct Harness {
    stream: IpcStream,
    sock: PathBuf,
    lane_id: i64,
    root: PathBuf,
    _repo_dir: tempfile::TempDir,
    _wt_parent: tempfile::TempDir,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Harness {
    async fn shutdown(self) {
        self.server.abort();
        let _ = std::fs::remove_file(&self.sock);
    }
}

async fn setup(prefix: &str) -> Harness {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-{prefix}-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "init"]);
    let r = call(
        &mut stream,
        1,
        "repo.add",
        Some(json!({ "path": repo_dir.path().to_string_lossy() })),
    )
    .await;
    assert!(r.error.is_none(), "repo.add errored: {:?}", r.error);
    let repo_id = r.result.unwrap()["id"].as_i64().unwrap();

    let wt_parent = tempfile::tempdir().unwrap();
    let wt_path = wt_parent.path().join("feat");
    let r = call(
        &mut stream,
        2,
        "lane.create",
        Some(json!({
            "repo_id": repo_id,
            "branch": "feat/thing",
            "source_branch": "main",
            "path": wt_path.to_string_lossy(),
        })),
    )
    .await;
    assert!(r.error.is_none(), "lane.create errored: {:?}", r.error);
    let lane_id = r.result.unwrap()["id"].as_i64().unwrap();

    Harness {
        stream,
        sock,
        lane_id,
        root: wt_path,
        _repo_dir: repo_dir,
        _wt_parent: wt_parent,
        server,
    }
}

#[tokio::test]
async fn file_list_is_sorted_dirs_first_and_excludes_dot_git() {
    let mut h = setup("file-list").await;

    std::fs::create_dir_all(h.root.join("src")).unwrap();
    std::fs::write(h.root.join("src/lib.rs"), "").unwrap();
    std::fs::create_dir_all(h.root.join("Assets")).unwrap();
    std::fs::write(h.root.join("a.txt"), "hello").unwrap();
    std::fs::write(h.root.join("B.txt"), "world").unwrap();

    let r = call(
        &mut h.stream,
        3,
        "file.list",
        Some(json!({ "lane_id": h.lane_id })),
    )
    .await;
    assert!(r.error.is_none(), "file.list errored: {:?}", r.error);
    let result = r.result.unwrap();
    assert_eq!(result["truncated"], json!(false));
    let entries = result["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();

    // .git (a gitlink FILE in a linked worktree, not a dir) never appears.
    assert!(!names.contains(&".git"), "names was: {names:?}");
    // README.md was checked out from the branched-off `main` commit alongside the new worktree;
    // just assert the two dirs sort first (case-insensitively), then files alphabetically.
    let dirs_first_two = &names[..2];
    assert_eq!(dirs_first_two, &["Assets", "src"], "names was: {names:?}");
    let file_entry = entries.iter().find(|e| e["name"] == "a.txt").unwrap();
    assert_eq!(file_entry["is_dir"], json!(false));
    assert_eq!(file_entry["path"], json!("a.txt"));
    assert_eq!(file_entry["size"], json!(5));
    let dir_entry = entries.iter().find(|e| e["name"] == "src").unwrap();
    assert_eq!(dir_entry["is_dir"], json!(true));
    assert!(dir_entry["size"].is_null());

    // A subdirectory listing is scoped to that one level.
    let r = call(
        &mut h.stream,
        4,
        "file.list",
        Some(json!({ "lane_id": h.lane_id, "path": "src" })),
    )
    .await;
    assert!(r.error.is_none(), "file.list src errored: {:?}", r.error);
    let sub = r.result.unwrap();
    let sub_entries = sub["entries"].as_array().unwrap();
    assert_eq!(sub_entries.len(), 1);
    assert_eq!(sub_entries[0]["path"], json!("src/lib.rs"));

    h.shutdown().await;
}

#[tokio::test]
async fn file_list_caps_entries_and_flags_truncated() {
    let mut h = setup("file-list-cap").await;
    std::fs::create_dir_all(h.root.join("many")).unwrap();
    for i in 0..2005 {
        std::fs::write(h.root.join("many").join(format!("f{i:04}.txt")), "").unwrap();
    }
    let r = call(
        &mut h.stream,
        3,
        "file.list",
        Some(json!({ "lane_id": h.lane_id, "path": "many" })),
    )
    .await;
    assert!(r.error.is_none(), "file.list errored: {:?}", r.error);
    let result = r.result.unwrap();
    assert_eq!(result["truncated"], json!(true));
    assert_eq!(result["entries"].as_array().unwrap().len(), 2000);
    h.shutdown().await;
}

#[tokio::test]
async fn file_list_flags_gitignored_entries() {
    let mut h = setup("file-list-ignore").await;
    std::fs::write(h.root.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(h.root.join("ignored.txt"), "x").unwrap();
    std::fs::write(h.root.join("kept.txt"), "y").unwrap();

    let r = call(
        &mut h.stream,
        3,
        "file.list",
        Some(json!({ "lane_id": h.lane_id })),
    )
    .await;
    assert!(r.error.is_none(), "file.list errored: {:?}", r.error);
    let entries = r.result.unwrap()["entries"].as_array().unwrap().clone();
    let ignored = entries.iter().find(|e| e["name"] == "ignored.txt").unwrap();
    assert_eq!(ignored["ignored"], json!(true));
    let kept = entries.iter().find(|e| e["name"] == "kept.txt").unwrap();
    assert_eq!(kept["ignored"], json!(false));

    h.shutdown().await;
}

#[tokio::test]
async fn unknown_lane_id_is_rejected() {
    let mut h = setup("file-unknown-lane").await;
    let r = call(
        &mut h.stream,
        3,
        "file.list",
        Some(json!({ "lane_id": h.lane_id + 999_999 })),
    )
    .await;
    assert!(r.error.is_some(), "expected an error for an unknown lane");
    h.shutdown().await;
}

#[tokio::test]
async fn path_escapes_are_hard_rejected() {
    let mut h = setup("file-escape").await;
    std::fs::write(h.root.join("inside.txt"), "safe").unwrap();

    for bad in [
        "../outside.txt",
        "../../etc/passwd",
        "/etc/passwd",
        "a/../../b",
    ] {
        let r = call(
            &mut h.stream,
            3,
            "file.read",
            Some(json!({ "lane_id": h.lane_id, "path": bad })),
        )
        .await;
        assert!(r.error.is_some(), "path {bad:?} should have been rejected");
        assert!(
            r.error.unwrap().message.contains("escapes"),
            "expected an escapes-the-root error for {bad:?}"
        );
    }

    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "shh").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), h.root.join("link")).unwrap();
        let r = call(
            &mut h.stream,
            4,
            "file.read",
            Some(json!({ "lane_id": h.lane_id, "path": "link" })),
        )
        .await;
        assert!(r.error.is_some(), "a symlink escape must be rejected");
    }

    // Write must reject the same way (outside the root, parent doesn't even matter).
    let r = call(
        &mut h.stream,
        5,
        "file.write",
        Some(json!({ "lane_id": h.lane_id, "path": "../outside.txt", "content": "x" })),
    )
    .await;
    assert!(
        r.error.is_some(),
        "file.write must reject an escaping path too"
    );

    h.shutdown().await;
}

#[tokio::test]
async fn file_read_round_trips_content_and_mtime() {
    let mut h = setup("file-read").await;
    std::fs::write(h.root.join("note.txt"), "hello\nworld\n").unwrap();

    let r = call(
        &mut h.stream,
        3,
        "file.read",
        Some(json!({ "lane_id": h.lane_id, "path": "note.txt" })),
    )
    .await;
    assert!(r.error.is_none(), "file.read errored: {:?}", r.error);
    let result = r.result.unwrap();
    assert_eq!(result["content"], json!("hello\nworld\n"));
    assert_eq!(result["size"], json!(12));
    assert_eq!(result["truncated"], json!(false));
    assert!(result["mtime_ms"].as_u64().unwrap() > 0);

    h.shutdown().await;
}

#[tokio::test]
async fn file_read_rejects_binary_content() {
    let mut h = setup("file-binary").await;
    std::fs::write(h.root.join("blob.bin"), [b'a', b'b', 0u8, b'c']).unwrap();

    let r = call(
        &mut h.stream,
        3,
        "file.read",
        Some(json!({ "lane_id": h.lane_id, "path": "blob.bin" })),
    )
    .await;
    let err = r.error.expect("binary file must be rejected");
    assert!(
        err.message.contains("binary"),
        "message was: {}",
        err.message
    );

    h.shutdown().await;
}

#[tokio::test]
async fn file_read_rejects_over_cap_size_without_truncating() {
    let mut h = setup("file-toobig").await;
    // 2MB cap: write one byte over it.
    std::fs::write(h.root.join("big.txt"), vec![b'x'; 2 * 1024 * 1024 + 1]).unwrap();

    let r = call(
        &mut h.stream,
        3,
        "file.read",
        Some(json!({ "lane_id": h.lane_id, "path": "big.txt" })),
    )
    .await;
    let err = r
        .error
        .expect("oversized file must be rejected, not truncated");
    assert!(
        err.message.contains("too large"),
        "message was: {}",
        err.message
    );

    h.shutdown().await;
}

#[tokio::test]
async fn file_write_creates_new_file_atomically_with_no_conflict_check() {
    let mut h = setup("file-write-new").await;

    let r = call(
        &mut h.stream,
        3,
        "file.write",
        Some(json!({ "lane_id": h.lane_id, "path": "brand-new.txt", "content": "fresh\n" })),
    )
    .await;
    assert!(r.error.is_none(), "file.write errored: {:?}", r.error);
    let result = r.result.unwrap();
    assert_eq!(result["size"], json!(6));
    assert!(result["mtime_ms"].as_u64().unwrap() > 0);

    assert_eq!(
        std::fs::read_to_string(h.root.join("brand-new.txt")).unwrap(),
        "fresh\n"
    );
    // Atomic write: no leftover temp file.
    assert!(!h.root.join("brand-new.txt.repomon-tmp").exists());

    h.shutdown().await;
}

#[tokio::test]
async fn file_write_rejects_when_parent_dir_is_missing() {
    let mut h = setup("file-write-noparent").await;
    let r = call(
        &mut h.stream,
        3,
        "file.write",
        Some(json!({ "lane_id": h.lane_id, "path": "nope/nested.txt", "content": "x" })),
    )
    .await;
    assert!(r.error.is_some(), "missing parent dir must be rejected");
    h.shutdown().await;
}

#[tokio::test]
async fn file_write_rejects_stale_mtime_and_broadcasts_event_on_success() {
    let mut h = setup("file-write-conflict").await;
    std::fs::write(h.root.join("shared.txt"), "v1").unwrap();

    // Subscribe before the write we expect to succeed, so the broadcast is observable.
    let r = call(
        &mut h.stream,
        3,
        "subscribe",
        Some(json!({ "topics": ["*"] })),
    )
    .await;
    assert!(r.error.is_none(), "subscribe errored: {:?}", r.error);

    let read = call(
        &mut h.stream,
        4,
        "file.read",
        Some(json!({ "lane_id": h.lane_id, "path": "shared.txt" })),
    )
    .await
    .result
    .unwrap();
    let stale_mtime = read["mtime_ms"].as_u64().unwrap().saturating_sub(1);

    // A change lands on disk out from under the editor.
    tokio::time::sleep(Duration::from_millis(20)).await;
    std::fs::write(h.root.join("shared.txt"), "v2-from-elsewhere").unwrap();

    let r = call(
        &mut h.stream,
        5,
        "file.write",
        Some(json!({
            "lane_id": h.lane_id,
            "path": "shared.txt",
            "content": "v3-from-editor",
            "expected_mtime_ms": stale_mtime,
        })),
    )
    .await;
    let err = r.error.expect("stale expected_mtime_ms must be rejected");
    assert_eq!(err.code, -32011);
    assert!(
        err.message.contains("conflict"),
        "message was: {}",
        err.message
    );
    let data = err.data.expect("conflict error must carry data");
    assert!(data.get("expected_mtime_ms").is_some());
    assert!(data.get("actual_mtime_ms").is_some());
    // Rejected write must not have touched the file.
    assert_eq!(
        std::fs::read_to_string(h.root.join("shared.txt")).unwrap(),
        "v2-from-elsewhere"
    );

    // A write with the CURRENT mtime succeeds and broadcasts event.file.changed.
    let current_read = call(
        &mut h.stream,
        6,
        "file.read",
        Some(json!({ "lane_id": h.lane_id, "path": "shared.txt" })),
    )
    .await
    .result
    .unwrap();
    let current_mtime = current_read["mtime_ms"].as_u64().unwrap();

    let r = call(
        &mut h.stream,
        7,
        "file.write",
        Some(json!({
            "lane_id": h.lane_id,
            "path": "shared.txt",
            "content": "v3-from-editor",
            "expected_mtime_ms": current_mtime,
        })),
    )
    .await;
    assert!(r.error.is_none(), "file.write errored: {:?}", r.error);

    let mut saw_event = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Ok(Some(frame))) = tokio::time::timeout(
            Duration::from_millis(500),
            protocol::read_frame(&mut h.stream),
        )
        .await
        {
            if let Ok(note) = serde_json::from_slice::<protocol::Notification>(&frame) {
                if note.method == "event.file.changed"
                    && note.params["lane_id"] == json!(h.lane_id)
                    && note.params["path"] == json!("shared.txt")
                {
                    saw_event = true;
                    break;
                }
            }
        }
    }
    assert!(
        saw_event,
        "expected an event.file.changed broadcast after a successful write"
    );
    assert_eq!(
        std::fs::read_to_string(h.root.join("shared.txt")).unwrap(),
        "v3-from-editor"
    );

    h.shutdown().await;
}
