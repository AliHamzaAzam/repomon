//! End-to-end TUI test: embedded daemon -> client -> App -> rendered Fleet frame.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use repomon_core::{Config, Store};
use repomon_daemon::{serve, Ctx};
use repomon_tui::app::App;
use repomon_tui::client::DaemonClient;
use repomon_tui::render_to_string;
use serde_json::json;

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

#[tokio::test]
async fn renders_fleet_with_a_registered_repo() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-tui-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let client = DaemonClient::connect(&sock).await.expect("connect");

    // Register a repo through the daemon.
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "feat: initial commit"]);
    let repo_name = repo_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    client
        .call(
            "repo.add",
            Some(json!({ "path": repo_dir.path().to_string_lossy() })),
        )
        .await
        .expect("repo.add");

    // Build the app, refresh from the daemon, render the Fleet view.
    let mut app = App::new(client);
    app.refresh().await;
    assert_eq!(app.lanes.len(), 1, "one main-worktree lane");

    let screen = render_to_string(&app, 100, 40);
    let screen = screen.unwrap();
    assert!(screen.contains("REPOMON"), "header missing:\n{screen}");
    assert!(screen.contains("FLEET"), "fleet summary missing:\n{screen}");
    assert!(screen.contains(&repo_name), "repo name missing:\n{screen}");
    assert!(screen.contains("main"), "main worktree missing:\n{screen}");
    assert!(
        screen.contains("feat: initial commit"),
        "today commit missing:\n{screen}"
    );
    assert!(screen.contains("needs-you"), "footer missing:\n{screen}");

    server.abort();
    let _ = std::fs::remove_file(&sock);
}
