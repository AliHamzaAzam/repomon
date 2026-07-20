//! The standing-orchestration machinery: the bounded headless runner and the scheduler tick.
//! Runs use a fake agent command (a plain `echo`), which shares the custom-agent code path
//! with real runs; the claude-flag composition is unit-tested in the daemon crate.

use std::time::Duration;

use repomon_core::{Config, Store};
use repomon_daemon::{Ctx, standing};

#[tokio::test]
async fn run_bounded_captures_stdout_and_stderr() {
    let (ok, out) = standing::run_bounded(
        "echo visible-stdout && echo visible-stderr >&2",
        Duration::from_secs(5),
    )
    .await;
    assert!(ok);
    assert!(out.contains("visible-stdout"), "out: {out}");
    assert!(out.contains("visible-stderr"), "out: {out}");
}

#[tokio::test]
async fn run_bounded_kills_on_wall_clock() {
    let (ok, out) = standing::run_bounded("sleep 30", Duration::from_secs(1)).await;
    assert!(!ok);
    assert!(out.contains("timed out"), "out: {out}");
}

#[tokio::test]
async fn run_bounded_reports_failure_exit() {
    let (ok, out) = standing::run_bounded("echo boom && exit 3", Duration::from_secs(5)).await;
    assert!(!ok);
    assert!(out.contains("boom"), "out: {out}");
}

#[tokio::test]
async fn scheduler_fires_due_schedules_once_and_journals() {
    let store = Store::open_in_memory().unwrap();
    let mut config = Config {
        orchestrator_agent: Some("noop".to_string()),
        ..Default::default()
    };
    config
        .agents
        .insert("noop".to_string(), "echo BRIEFING: all quiet".to_string());
    let state_dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::new_with_paths(
        store,
        config,
        None,
        state_dir.path().join("config.toml"),
        state_dir.path().join("repo-notes"),
    );

    let sched = ctx
        .store
        .add_schedule("every 30m".into(), "morning fleet briefing".into(), 10)
        .await
        .unwrap();

    // Not due yet: created just now, interval 30m.
    standing::scheduler_tick(&ctx, chrono::Local::now()).await;
    assert!(
        ctx.store.recent_journal(10).await.unwrap().is_empty(),
        "nothing should fire before the interval elapses"
    );

    // 31 minutes later: due. The run executes the fake agent and journals its output.
    let later = chrono::Local::now() + chrono::Duration::minutes(31);
    standing::scheduler_tick(&ctx, later).await;
    let entries = ctx.store.recent_journal(10).await.unwrap();
    assert_eq!(entries.len(), 1, "entries: {entries:?}");
    assert_eq!(entries[0].action, "standing_run");
    assert_eq!(entries[0].outcome, "ok");
    assert!(
        entries[0]
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("BRIEFING"),
        "journal must carry the run output: {entries:?}"
    );

    // last_run_at was stamped, so the same instant does not double-fire.
    standing::scheduler_tick(&ctx, later).await;
    assert_eq!(ctx.store.recent_journal(10).await.unwrap().len(), 1);
    let scheds = ctx.store.list_schedules().await.unwrap();
    assert_eq!(scheds[0].id, sched.id);
    assert!(scheds[0].last_run_at.is_some());
}
