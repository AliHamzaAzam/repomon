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
async fn file_read_classifies_binary_content() {
    let mut h = setup("file-binary").await;
    std::fs::write(h.root.join("blob.bin"), [b'a', b'b', 0u8, b'c']).unwrap();

    let r = call(
        &mut h.stream,
        3,
        "file.read",
        Some(json!({ "lane_id": h.lane_id, "path": "blob.bin" })),
    )
    .await;
    let res: repomon_core::model::FileReadResult =
        serde_json::from_value(r.result.expect("file.read must succeed for binary")).unwrap();
    assert_eq!(res.kind, "binary");
    assert_eq!(res.content, "");

    h.shutdown().await;
}

#[tokio::test]
async fn file_read_classifies_svg_as_text() {
    let mut h = setup("file-svg").await;
    let svg = "<svg><circle r=\"10\"/></svg>";
    std::fs::write(h.root.join("icon.svg"), svg).unwrap();

    let r = call(
        &mut h.stream,
        3,
        "file.read",
        Some(json!({ "lane_id": h.lane_id, "path": "icon.svg" })),
    )
    .await;
    let res: repomon_core::model::FileReadResult =
        serde_json::from_value(r.result.expect("file.read must succeed for svg")).unwrap();
    assert_eq!(res.kind, "text");
    assert_eq!(res.content, svg);

    h.shutdown().await;
}

#[tokio::test]
async fn file_read_raw_returns_base64_for_image() {
    let mut h = setup("file-image").await;
    let png_bytes = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    std::fs::write(h.root.join("logo.png"), png_bytes).unwrap();

    let r = call(
        &mut h.stream,
        3,
        "file.read",
        Some(json!({ "lane_id": h.lane_id, "path": "logo.png" })),
    )
    .await;
    let res: repomon_core::model::FileReadResult =
        serde_json::from_value(r.result.expect("file.read must succeed for image")).unwrap();
    assert_eq!(res.kind, "image");

    let raw = call(
        &mut h.stream,
        4,
        "file.read_raw",
        Some(json!({ "lane_id": h.lane_id, "path": "logo.png" })),
    )
    .await;
    let raw_res: repomon_core::model::FileReadRawResult =
        serde_json::from_value(raw.result.expect("file.read_raw must succeed")).unwrap();
    assert_eq!(raw_res.mime, "image/png");
    assert_eq!(raw_res.size, png_bytes.len() as u64);

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

#[tokio::test]
async fn file_index_reports_worktree_files_and_caches_per_lane() {
    let mut h = setup("file-index").await;

    // Create .gitignore
    std::fs::write(h.root.join(".gitignore"), "target/\n*.log\n").unwrap();

    // Create nested directory and files
    std::fs::create_dir_all(h.root.join("src/deep")).unwrap();
    std::fs::write(h.root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(h.root.join("src/deep/file.txt"), "content\n").unwrap();

    // Create ignored files and directories
    std::fs::create_dir_all(h.root.join("target")).unwrap();
    std::fs::write(h.root.join("target/bin.exe"), "bin").unwrap();
    std::fs::write(h.root.join("test.log"), "log\n").unwrap();

    // Call file.index
    let r1 = call(
        &mut h.stream,
        2,
        "file.index",
        Some(json!({ "lane_id": h.lane_id })),
    )
    .await;
    assert!(r1.error.is_none(), "file.index failed: {:?}", r1.error);
    let res1 = r1.result.unwrap();
    let paths1: Vec<String> = res1["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let gen1 = res1["generation"].as_u64().unwrap();
    assert!(!res1["truncated"].as_bool().unwrap());

    assert!(paths1.contains(&".gitignore".to_string()));
    assert!(paths1.contains(&"src/main.rs".to_string()));
    assert!(paths1.contains(&"src/deep/file.txt".to_string()));
    assert!(!paths1.iter().any(|p| p == ".git" || p.starts_with(".git/")));
    assert!(!paths1.iter().any(|p| p.starts_with("target")));
    assert!(!paths1.contains(&"test.log".to_string()));

    // Call file.index again: verify cache hit with same generation
    let r2 = call(
        &mut h.stream,
        3,
        "file.index",
        Some(json!({ "lane_id": h.lane_id })),
    )
    .await;
    assert!(r2.error.is_none());
    let res2 = r2.result.unwrap();
    let gen2 = res2["generation"].as_u64().unwrap();
    assert_eq!(gen1, gen2, "cache hit should retain generation");

    // Write file via file.write: bumps generation
    let write_res = call(
        &mut h.stream,
        4,
        "file.write",
        Some(json!({
            "lane_id": h.lane_id,
            "path": "src/main.rs",
            "content": "fn main() { println!(\"updated\"); }\n",
        })),
    )
    .await;
    assert!(write_res.error.is_none());

    // Call file.index again: verify cache invalidation bumped generation
    let r3 = call(
        &mut h.stream,
        5,
        "file.index",
        Some(json!({ "lane_id": h.lane_id })),
    )
    .await;
    assert!(r3.error.is_none());
    let res3 = r3.result.unwrap();
    let gen3 = res3["generation"].as_u64().unwrap();
    assert!(gen3 > gen1, "generation should increase after file.write");

    h.shutdown().await;
}

#[tokio::test]
async fn worktree_watcher_lifecycle_and_events() {
    let mut h = setup("worktree-watch").await;

    let r_sub = call(&mut h.stream, 9, "subscribe", None).await;
    assert!(r_sub.error.is_none());

    // Initially, lane is not in any connection's viewport, so no watcher is running.
    // Assert viewport.set to start watcher for h.lane_id.
    let r = call(
        &mut h.stream,
        10,
        "viewport.set",
        Some(json!({ "lane_ids": [h.lane_id] })),
    )
    .await;
    assert!(r.error.is_none());
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 1. External file creation
    std::fs::write(h.root.join("new_external.txt"), "created on disk\n").unwrap();

    let mut saw_create = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Ok(Some(frame))) = tokio::time::timeout(remaining, protocol::read_frame(&mut h.stream)).await {
            if let Ok(note) = serde_json::from_slice::<protocol::Notification>(&frame) {
                if note.method == "event.file.changed"
                    && note.params["lane_id"] == json!(h.lane_id)
                    && note.params["path"] == json!("new_external.txt")
                    && note.params["op"] == json!("created")
                {
                    saw_create = true;
                    break;
                }
            }
        }
    }
    assert!(saw_create, "expected event.file.changed with op: created");
    tokio::time::sleep(Duration::from_millis(350)).await;

    // 2. External file modification (modify existing file README.md)
    std::fs::write(h.root.join("README.md"), "modified readme\n").unwrap();

    let mut saw_modify = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Ok(Some(frame))) = tokio::time::timeout(remaining, protocol::read_frame(&mut h.stream)).await {
            if let Ok(note) = serde_json::from_slice::<protocol::Notification>(&frame) {
                if note.method == "event.file.changed"
                    && note.params["lane_id"] == json!(h.lane_id)
                    && note.params["path"] == json!("README.md")
                    && note.params["op"] == json!("modified")
                {
                    saw_modify = true;
                    break;
                }
            }
        }
    }
    assert!(saw_modify, "expected event.file.changed with op: modified");
    tokio::time::sleep(Duration::from_millis(350)).await;

    // 3. Ignored file produces nothing
    std::fs::write(h.root.join(".gitignore"), "*.ignored\n").unwrap();
    // Wait for debounce on .gitignore
    tokio::time::sleep(Duration::from_millis(350)).await;

    std::fs::write(h.root.join("test.ignored"), "should be ignored\n").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(600);
    let mut saw_ignored_event = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Ok(Some(frame))) = tokio::time::timeout(remaining, protocol::read_frame(&mut h.stream)).await {
            if let Ok(note) = serde_json::from_slice::<protocol::Notification>(&frame) {
                if note.method == "event.file.changed"
                    && note.params["path"] == json!("test.ignored")
                {
                    saw_ignored_event = true;
                    break;
                }
            }
        }
    }
    assert!(!saw_ignored_event, "ignored files must not broadcast events");

    // 4. File removal
    std::fs::remove_file(h.root.join("new_external.txt")).unwrap();

    let mut saw_remove = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Ok(Some(frame))) = tokio::time::timeout(remaining, protocol::read_frame(&mut h.stream)).await {
            if let Ok(note) = serde_json::from_slice::<protocol::Notification>(&frame) {
                if note.method == "event.file.changed"
                    && note.params["lane_id"] == json!(h.lane_id)
                    && note.params["path"] == json!("new_external.txt")
                    && note.params["op"] == json!("removed")
                {
                    saw_remove = true;
                    break;
                }
            }
        }
    }
    assert!(saw_remove, "expected event.file.changed with op: removed");

    // 5. Watcher stops when lane leaves viewport
    let r = call(
        &mut h.stream,
        11,
        "viewport.set",
        Some(json!({ "lane_ids": [] })),
    )
    .await;
    assert!(r.error.is_none());
    tokio::time::sleep(Duration::from_millis(100)).await;

    std::fs::write(h.root.join("unwatched.txt"), "no watcher\n").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(600);
    let mut saw_unwatched_event = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Ok(Some(frame))) = tokio::time::timeout(remaining, protocol::read_frame(&mut h.stream)).await {
            if let Ok(note) = serde_json::from_slice::<protocol::Notification>(&frame) {
                if note.method == "event.file.changed"
                    && note.params["path"] == json!("unwatched.txt")
                {
                    saw_unwatched_event = true;
                    break;
                }
            }
        }
    }
    assert!(!saw_unwatched_event, "unwatched lane must not produce events");

    h.shutdown().await;
}

#[tokio::test]
async fn file_create_operations_and_collisions() {
    let mut h = setup("file-create").await;

    // 1. Create directory with nested parents
    let r1 = call(
        &mut h.stream,
        10,
        "file.create",
        Some(json!({ "lane_id": h.lane_id, "path": "deep/nested/dir", "is_dir": true })),
    )
    .await;
    assert!(r1.error.is_none());
    assert!(h.root.join("deep/nested/dir").is_dir());

    // 2. Create file in existing directory
    let r2 = call(
        &mut h.stream,
        11,
        "file.create",
        Some(json!({ "lane_id": h.lane_id, "path": "deep/nested/dir/file.txt", "is_dir": false })),
    )
    .await;
    assert!(r2.error.is_none());
    assert!(h.root.join("deep/nested/dir/file.txt").is_file());

    // 3. Collision error (-32009) on already existing file
    let r3 = call(
        &mut h.stream,
        12,
        "file.create",
        Some(json!({ "lane_id": h.lane_id, "path": "deep/nested/dir/file.txt" })),
    )
    .await;
    assert_eq!(r3.error.as_ref().map(|e| e.code), Some(-32009));

    // 4. Collision error (-32009) on already existing directory
    let r4 = call(
        &mut h.stream,
        13,
        "file.create",
        Some(json!({ "lane_id": h.lane_id, "path": "deep/nested/dir", "is_dir": true })),
    )
    .await;
    assert_eq!(r4.error.as_ref().map(|e| e.code), Some(-32009));

    // 5. Parent directory auto-creation for new file
    let r5 = call(
        &mut h.stream,
        14,
        "file.create",
        Some(json!({ "lane_id": h.lane_id, "path": "auto/parent/test.txt" })),
    )
    .await;
    assert!(r5.error.is_none());
    assert!(h.root.join("auto/parent/test.txt").is_file());

    // 6. Traversal rejection
    let r6 = call(
        &mut h.stream,
        15,
        "file.create",
        Some(json!({ "lane_id": h.lane_id, "path": "../outside.txt" })),
    )
    .await;
    assert!(r6.error.is_some());

    h.shutdown().await;
}

#[tokio::test]
async fn file_rename_operations_and_traversal() {
    let mut h = setup("file-rename").await;

    std::fs::write(h.root.join("source.txt"), "rename me\n").unwrap();
    std::fs::create_dir_all(h.root.join("target_dir")).unwrap();
    std::fs::write(h.root.join("target_dir/existing.txt"), "already here\n").unwrap();

    // 1. Cross-directory rename
    let r1 = call(
        &mut h.stream,
        10,
        "file.rename",
        Some(json!({ "lane_id": h.lane_id, "from": "source.txt", "to": "target_dir/moved.txt" })),
    )
    .await;
    assert!(r1.error.is_none());
    assert!(!h.root.join("source.txt").exists());
    assert!(h.root.join("target_dir/moved.txt").is_file());

    // 2. Collision error (-32009) when destination exists
    let r2 = call(
        &mut h.stream,
        11,
        "file.rename",
        Some(json!({ "lane_id": h.lane_id, "from": "target_dir/moved.txt", "to": "target_dir/existing.txt" })),
    )
    .await;
    assert_eq!(r2.error.as_ref().map(|e| e.code), Some(-32009));

    // 3. Traversal rejection
    let r3 = call(
        &mut h.stream,
        12,
        "file.rename",
        Some(json!({ "lane_id": h.lane_id, "from": "target_dir/moved.txt", "to": "../escaped.txt" })),
    )
    .await;
    assert!(r3.error.is_some());

    h.shutdown().await;
}

#[tokio::test]
async fn file_delete_operations_and_guards() {
    let mut h = setup("file-delete").await;

    std::fs::write(h.root.join("delete_me.txt"), "remove\n").unwrap();
    std::fs::create_dir_all(h.root.join("empty_dir")).unwrap();
    std::fs::create_dir_all(h.root.join("non_empty_dir/sub")).unwrap();
    std::fs::write(h.root.join("non_empty_dir/sub/item.txt"), "nested\n").unwrap();

    // 1. Delete plain file
    let r1 = call(
        &mut h.stream,
        10,
        "file.delete",
        Some(json!({ "lane_id": h.lane_id, "path": "delete_me.txt" })),
    )
    .await;
    assert!(r1.error.is_none());
    assert!(!h.root.join("delete_me.txt").exists());

    // 2. Delete empty directory without recursive flag
    let r2 = call(
        &mut h.stream,
        11,
        "file.delete",
        Some(json!({ "lane_id": h.lane_id, "path": "empty_dir" })),
    )
    .await;
    assert!(r2.error.is_none());
    assert!(!h.root.join("empty_dir").exists());

    // 3. Delete non-empty directory without recursive flag: rejected with -32008
    let r3 = call(
        &mut h.stream,
        12,
        "file.delete",
        Some(json!({ "lane_id": h.lane_id, "path": "non_empty_dir", "recursive": false })),
    )
    .await;
    assert_eq!(r3.error.as_ref().map(|e| e.code), Some(-32008));
    assert!(h.root.join("non_empty_dir").exists());

    // 4. Delete non-empty directory with recursive: true succeeds
    let r4 = call(
        &mut h.stream,
        13,
        "file.delete",
        Some(json!({ "lane_id": h.lane_id, "path": "non_empty_dir", "recursive": true })),
    )
    .await;
    assert!(r4.error.is_none());
    assert!(!h.root.join("non_empty_dir").exists());

    // 5. Delete .git is rejected
    let r5 = call(
        &mut h.stream,
        14,
        "file.delete",
        Some(json!({ "lane_id": h.lane_id, "path": ".git", "recursive": true })),
    )
    .await;
    assert!(r5.error.is_some());

    // 6. Delete root is rejected
    let r6 = call(
        &mut h.stream,
        15,
        "file.delete",
        Some(json!({ "lane_id": h.lane_id, "path": "", "recursive": true })),
    )
    .await;
    assert!(r6.error.is_some());

    h.shutdown().await;
}

#[tokio::test]
async fn file_search_features_and_truncation() {
    let mut h = setup("file-search").await;

    std::fs::create_dir_all(h.root.join("src")).unwrap();
    std::fs::write(
        h.root.join("src/lib.rs"),
        "pub fn calculate_score() -> i32 {\n    let score = 42;\n    score\n}\n",
    )
    .unwrap();
    std::fs::write(
        h.root.join("src/main.rs"),
        "fn main() {\n    println!(\"Score: 100\");\n    calculate_score();\n}\n",
    )
    .unwrap();
    // Binary file: should be skipped
    std::fs::write(h.root.join("src/blob.bin"), [0u8, 1, 2, b's', b'c', b'o', b'r', b'e']).unwrap();
    // Ignored file: should be skipped
    std::fs::write(h.root.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(h.root.join("audit.log"), "score in log\n").unwrap();

    // 1. Plain substring search with 1-based line and column
    let r1 = call(
        &mut h.stream,
        10,
        "file.search",
        Some(json!({
            "lane_id": h.lane_id,
            "query": "calculate_score",
        })),
    )
    .await;
    assert!(r1.error.is_none());
    let res1: repomon_core::model::FileSearchResult = serde_json::from_value(r1.result.unwrap()).unwrap();
    assert_eq!(res1.hits.len(), 2);
    let hit_lib = res1.hits.iter().find(|hit| hit.path == "src/lib.rs").unwrap();
    assert_eq!(hit_lib.line, 1);
    assert_eq!(hit_lib.column, 8); // "pub fn " is 7 chars, so column is 8!
    assert!(hit_lib.preview.contains("calculate_score"));

    // 2. Case sensitive search
    let r2 = call(
        &mut h.stream,
        11,
        "file.search",
        Some(json!({
            "lane_id": h.lane_id,
            "query": "score",
            "case_sensitive": true,
        })),
    )
    .await;
    let res2: repomon_core::model::FileSearchResult = serde_json::from_value(r2.result.unwrap()).unwrap();
    assert!(!res2.hits.iter().any(|hit| hit.preview.contains("println!(\"Score:")));

    // Case insensitive search
    let r2_ci = call(
        &mut h.stream,
        12,
        "file.search",
        Some(json!({
            "lane_id": h.lane_id,
            "query": "score",
            "case_sensitive": false,
        })),
    )
    .await;
    let res2_ci: repomon_core::model::FileSearchResult = serde_json::from_value(r2_ci.result.unwrap()).unwrap();
    assert!(res2_ci.hits.iter().any(|hit| hit.preview.contains("println!(\"Score:")));

    // 3. Regex mode
    let r3 = call(
        &mut h.stream,
        13,
        "file.search",
        Some(json!({
            "lane_id": h.lane_id,
            "query": r"score\s*=\s*\d+",
            "regex": true,
        })),
    )
    .await;
    let res3: repomon_core::model::FileSearchResult = serde_json::from_value(r3.result.unwrap()).unwrap();
    assert_eq!(res3.hits.len(), 1);
    assert_eq!(res3.hits[0].path, "src/lib.rs");
    assert_eq!(res3.hits[0].line, 2);

    // 4. Glob filter restricts to matching paths
    let r4 = call(
        &mut h.stream,
        14,
        "file.search",
        Some(json!({
            "lane_id": h.lane_id,
            "query": "score",
            "glob": "*main.rs",
        })),
    )
    .await;
    let res4: repomon_core::model::FileSearchResult = serde_json::from_value(r4.result.unwrap()).unwrap();
    assert!(!res4.hits.is_empty());
    assert!(res4.hits.iter().all(|hit| hit.path == "src/main.rs"));

    // 5. Binary file and ignored file are excluded
    assert!(!res2_ci.hits.iter().any(|hit| hit.path == "src/blob.bin"));
    assert!(!res2_ci.hits.iter().any(|hit| hit.path == "audit.log"));

    // 6. Cap truncation
    let r6 = call(
        &mut h.stream,
        15,
        "file.search",
        Some(json!({
            "lane_id": h.lane_id,
            "query": "score",
            "max_results": 1,
        })),
    )
    .await;
    let res6: repomon_core::model::FileSearchResult = serde_json::from_value(r6.result.unwrap()).unwrap();
    assert_eq!(res6.hits.len(), 1);
    assert!(res6.truncated);

    h.shutdown().await;
}
