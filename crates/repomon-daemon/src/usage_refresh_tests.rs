use super::*;
use repomon_core::agent::backend::{
    AttachCommand, ByteStream, OwnerState, ScrollEvent, WindowActivity,
};
use repomon_core::protocol::{self, Request, Response};
use repomon_core::transport::{self, Endpoint};
use serde_json::json;
use std::sync::Mutex as StdMutex;

fn fixture() -> (tempfile::TempDir, Arc<Ctx>, Arc<ScriptedBackend>) {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ScriptedBackend::new());
    let mut config = repomon_core::Config {
        usage_probe: true,
        ..Default::default()
    };
    config.repomind.home = dir.path().join("home").to_string_lossy().into_owned();
    let ctx = Ctx::new_with_backend(
        repomon_core::Store::open_in_memory().unwrap(),
        config,
        None,
        dir.path().join("config.toml"),
        dir.path().join("notes"),
        backend.clone(),
    );
    (dir, ctx, backend)
}

#[tokio::test]
async fn manual_refresh_acknowledges_cached_gates_without_probing() {
    let (_dir, ctx, _) = fixture();
    ctx.config.write().await.usage_probe = false;
    assert_eq!(
        refresh(&ctx).await.reason,
        agent::UsageRefreshReason::ProbeDisabled
    );
    ctx.config.write().await.usage_probe = true;
    ctx.overlay_cache.lock().await.publish(0, vec![]);
    assert_eq!(
        refresh(&ctx).await.reason,
        agent::UsageRefreshReason::NoActiveKind
    );
    let (_dir2, ctx, _) = fixture();
    let pending = refresh(&ctx).await;
    assert_eq!(pending.reason, agent::UsageRefreshReason::Pending);
    assert!(!pending.refreshed);
    assert!(pending.request_id.is_some());
    assert_eq!(
        refresh(&ctx).await.reason,
        agent::UsageRefreshReason::Cooldown
    );
}

#[tokio::test]
async fn manual_timeout_event_keeps_round_inflight_until_final_completion() {
    let (_dir, ctx, _) = fixture();
    let mut events = ctx.events.subscribe();
    let pending = refresh_with_deadline(&ctx, Duration::from_millis(5)).await;
    let request = pending.request_id.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event["method"], "event.usage.refreshed");
    assert_eq!(event["params"]["reason"], "timeout");
    assert_eq!(event["params"]["request_id"], request);
    assert_eq!(
        refresh(&ctx).await.reason,
        agent::UsageRefreshReason::Cooldown
    );
    finish_round(&ctx, request, agent::UsageRefreshReason::Ok).await;
    assert_eq!(events.recv().await.unwrap()["params"]["reason"], "ok");
    let next = refresh(&ctx).await.request_id.unwrap();
    finish_round(&ctx, request, agent::UsageRefreshReason::Ok).await;
    assert!(
        events.try_recv().is_err(),
        "an old completion cannot settle the new round"
    );
    finish_round(&ctx, next, agent::UsageRefreshReason::Error).await;
    let event = events.recv().await.unwrap();
    assert_eq!(event["params"]["reason"], "error");
    assert!(event["params"]["snapshot"].is_array());
}

#[tokio::test]
async fn hard_deadline_releases_a_dead_watchers_ticket_without_clearing_a_new_round() {
    assert_eq!(round_ceiling(0), PROBE_TIMEOUT);
    assert_eq!(round_ceiling(2), PROBE_TIMEOUT * 2);
    let (_dir, ctx, _) = fixture();
    let request = refresh(&ctx).await.request_id.unwrap();
    refresh_deadlines(
        &ctx,
        request,
        tokio::time::Instant::now(),
        Duration::from_millis(1),
        Duration::from_millis(5),
    )
    .await;
    let next = refresh(&ctx).await;
    assert_eq!(next.reason, agent::UsageRefreshReason::Pending);
    assert_ne!(next.request_id, Some(request));
    finish_round(&ctx, request, agent::UsageRefreshReason::Ok).await;
    refresh_deadlines(
        &ctx,
        request,
        tokio::time::Instant::now(),
        Duration::ZERO,
        Duration::ZERO,
    )
    .await;
    assert_eq!(*ctx.usage_refresh_inflight.lock().await, next.request_id);
}

#[tokio::test]
async fn old_hard_deadline_cannot_release_a_replacement_round() {
    let (_dir, ctx, _) = fixture();
    let mut events = ctx.events.subscribe();
    let request = refresh(&ctx).await.request_id.unwrap();
    let timer = tokio::spawn({
        let ctx = ctx.clone();
        async move {
            refresh_deadlines(
                &ctx,
                request,
                tokio::time::Instant::now(),
                Duration::ZERO,
                Duration::from_millis(30),
            )
            .await;
        }
    });
    events.recv().await.unwrap();
    finish_round(&ctx, request, agent::UsageRefreshReason::Ok).await;
    let next = refresh(&ctx).await.request_id;
    timer.await.unwrap();
    assert_eq!(*ctx.usage_refresh_inflight.lock().await, next);
}

#[tokio::test]
async fn completion_preserves_gate_timeout_and_error_outcomes() {
    for (outcome, reason, detail) in [
        (
            agent::UsageRefreshReason::NoActiveKind,
            "no_active_kind",
            "No agent running to probe",
        ),
        (
            agent::UsageRefreshReason::ProbeDisabled,
            "probe_disabled",
            "Usage probe is off in Settings",
        ),
        (
            agent::UsageRefreshReason::Timeout,
            "timeout",
            "Usage probe timed out; try again",
        ),
        (
            agent::UsageRefreshReason::Error,
            "error",
            "Usage probe failed; try again",
        ),
    ] {
        let (_dir, ctx, _) = fixture();
        let mut events = ctx.events.subscribe();
        let request = refresh(&ctx).await.request_id.unwrap();
        finish_round(&ctx, request, outcome).await;
        let event = events.recv().await.unwrap();
        assert_eq!(event["params"]["reason"], reason);
        assert_eq!(event["params"]["detail"], detail);
        assert_eq!(event["params"]["request_id"], request);
        assert!(ctx.usage_refresh_inflight.lock().await.is_none());
    }
}

#[tokio::test]
async fn two_account_round_reports_progress_then_success_without_false_failure() {
    let (_dir, ctx, _) = fixture();
    let mut events = ctx.events.subscribe();
    let request = refresh_with_deadline(&ctx, Duration::from_millis(10))
        .await
        .request_id
        .unwrap();
    ctx.usage.lock().await.insert(
        "first".into(),
        UsageEntry {
            report: UsageReport::default(),
            label: "first".into(),
            fetched_at: Instant::now(),
        },
    );
    let waiting = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(waiting["params"]["reason"], "timeout");
    assert_eq!(
        waiting["params"]["detail"],
        "Still probing, this can take a moment"
    );
    assert_eq!(waiting["params"]["snapshot"].as_array().unwrap().len(), 1);
    ctx.usage.lock().await.insert(
        "second".into(),
        UsageEntry {
            report: UsageReport::default(),
            label: "second".into(),
            fetched_at: Instant::now(),
        },
    );
    finish_round(&ctx, request, agent::UsageRefreshReason::Ok).await;
    let done = events.recv().await.unwrap();
    assert_eq!(done["params"]["reason"], "ok");
    assert_eq!(done["params"]["snapshot"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn socket_keystroke_completes_while_a_slow_probe_is_pending() {
    let (dir, ctx, backend) = fixture();
    let socket = dir.path().join("refresh.sock");
    let server = tokio::spawn({
        let ctx = ctx.clone();
        let socket = socket.clone();
        async move {
            crate::serve(ctx, &socket).await.unwrap();
        }
    });
    let mut stream = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(stream) = transport::connect(&Endpoint::from_path(&socket)).await {
                break stream;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture socket should bind");
    protocol::write_message(&mut stream, &Request::new(1, "subscribe", None))
        .await
        .unwrap();
    protocol::read_frame(&mut stream).await.unwrap().unwrap();
    let (release, blocked) = tokio::sync::oneshot::channel::<()>();
    let slow_probe = tokio::spawn({
        let ctx = ctx.clone();
        async move {
            ctx.usage_refresh.notified().await;
            blocked.await.unwrap();
            let request = ctx.usage_refresh_request.load(Ordering::Relaxed);
            finish_round(&ctx, request, agent::UsageRefreshReason::Ok).await;
        }
    });
    protocol::write_message(&mut stream, &Request::new(2, "usage.refresh", None))
        .await
        .unwrap();
    protocol::write_message(
        &mut stream,
        &Request::new(
            3,
            "agent.send_input",
            Some(json!({ "lane_id": 1, "window": "lane-1", "text": "x", "enter": false })),
        ),
    )
    .await
    .unwrap();
    let responses = tokio::time::timeout(Duration::from_secs(1), async {
        let a: Response =
            serde_json::from_slice(&protocol::read_frame(&mut stream).await.unwrap().unwrap())
                .unwrap();
        let b: Response =
            serde_json::from_slice(&protocol::read_frame(&mut stream).await.unwrap().unwrap())
                .unwrap();
        (a, b)
    })
    .await
    .expect("probe completion must not block the connection's next RPC");
    assert_eq!(responses.0.result.unwrap()["reason"], "pending");
    assert!(responses.1.error.is_none());
    assert_eq!(
        backend.sent_text.lock().unwrap().as_slice(),
        &[("lane-1".into(), "x".into())]
    );
    assert!(!slow_probe.is_finished());
    release.send(()).unwrap();
    let event: serde_json::Value =
        serde_json::from_slice(&protocol::read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
    assert_eq!(event["method"], "event.usage.refreshed");
    assert_eq!(event["params"]["reason"], "ok");
    slow_probe.await.unwrap();
    drop(stream);
    ctx.request_shutdown();
    server.await.unwrap();
}

struct ScriptedBackend {
    sent_keys: StdMutex<Vec<(String, String)>>,
    sent_text: StdMutex<Vec<(String, String)>>,
}

impl ScriptedBackend {
    fn new() -> Self {
        Self {
            sent_keys: StdMutex::new(Vec::new()),
            sent_text: StdMutex::new(Vec::new()),
        }
    }
}

impl SessionBackend for ScriptedBackend {
    fn available(&self) -> bool {
        true
    }
    fn label(&self) -> String {
        "scripted".to_string()
    }
    fn session_exists(&self) -> bool {
        true
    }
    fn claim_or_verify_owner(&self, _me: &str) -> OwnerState {
        OwnerState::Owned
    }
    fn list_windows(&self) -> repomon_core::Result<Vec<String>> {
        Ok(vec![])
    }
    fn list_windows_with_activity(&self) -> repomon_core::Result<Vec<WindowActivity>> {
        Ok(vec![])
    }
    fn spawn(
        &self,
        _lane: repomon_core::model::LaneId,
        _spec: &SpawnSpec,
    ) -> repomon_core::Result<String> {
        Ok("target".into())
    }
    fn spawn_named(&self, _window: &str, _spec: &SpawnSpec) -> repomon_core::Result<String> {
        Ok("target".into())
    }
    fn open_named(&self, _window: &str, _cwd: &std::path::Path) -> repomon_core::Result<String> {
        Ok("target".into())
    }
    fn capture_named(&self, _window: &str, _opts: CaptureOpts) -> repomon_core::Result<String> {
        Ok(String::new())
    }
    fn cursor_named(&self, _window: &str) -> Option<repomon_core::agent::Cursor> {
        Some(repomon_core::agent::Cursor { col: 2, row: 0 })
    }
    fn size_named(&self, _window: &str) -> Option<(u16, u16)> {
        Some((80, 24))
    }
    fn resize_named(&self, _window: &str, _cols: u16, _rows: u16) -> repomon_core::Result<()> {
        Ok(())
    }
    fn follow_client_named(&self, _window: &str) -> repomon_core::Result<()> {
        Ok(())
    }
    fn alternate_on_named(&self, _window: &str) -> bool {
        false
    }
    fn scroll_wheel_named(&self, _window: &str, _event: ScrollEvent) -> repomon_core::Result<()> {
        Ok(())
    }
    fn send_literal_named(&self, window: &str, text: &str) -> repomon_core::Result<()> {
        self.sent_text
            .lock()
            .unwrap()
            .push((window.into(), text.into()));
        Ok(())
    }
    fn send_text_named(&self, window: &str, text: &str) -> repomon_core::Result<()> {
        self.sent_text
            .lock()
            .unwrap()
            .push((window.to_string(), text.to_string()));
        Ok(())
    }
    fn send_key_named(&self, window: &str, key: &str) -> repomon_core::Result<()> {
        self.sent_keys
            .lock()
            .unwrap()
            .push((window.to_string(), key.to_string()));
        Ok(())
    }
    fn kill_named(&self, _window: &str) -> repomon_core::Result<()> {
        Ok(())
    }
    fn configure(&self) {}
    fn target_named(&self, window: &str) -> String {
        window.to_string()
    }
    fn exact_target_named(&self, window: &str) -> String {
        window.to_string()
    }
    fn attach_command(&self, target: &str) -> AttachCommand {
        AttachCommand {
            program: "tmux".into(),
            args: vec!["attach".into(), "-t".into(), target.into()],
        }
    }
    fn open_byte_stream(&self, _window: &str) -> repomon_core::Result<ByteStream> {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Ok(ByteStream { rx })
    }
    fn close_byte_stream(&self, _window: &str) -> repomon_core::Result<()> {
        Ok(())
    }
}
