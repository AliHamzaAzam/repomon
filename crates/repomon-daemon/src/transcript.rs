//! Conversation subscriptions share the terminal byte feed and reconcile pane previews with ledger
//! scanner rows. No CLI is spawned and no input is injected by this module.
use crate::{Ctx, conn::ConnSession};
use repomon_core::agent::{
    CaptureOpts, TmuxRuntime,
    conversation::{ConversationStream, pane_items},
    prompt,
};
use repomon_core::model::{LaneId, TranscriptItem};
use repomon_core::usage_ledger::{
    FleetIndex,
    scan::{
        ScanOptions, SourceScan, scan_claude_transcript, scan_claude_transcript_with_options,
        scan_codex_rollout, scan_codex_rollout_with_options,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

pub const TOPIC: &str = "event.agent.transcript";
static NEXT_WATCH: AtomicU64 = AtomicU64::new(1 << 63);
const PAGE_BYTES: u64 = 128 * 1024;

#[derive(Clone, Deserialize)]
pub struct Params {
    pub lane_id: LaneId,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub window: Option<String>,
    #[serde(default, alias = "agent")]
    pub kind: Option<String>,
    #[serde(default)]
    pub before: Option<u64>,
    #[serde(default = "watch_on")]
    pub on: bool,
}
fn watch_on() -> bool {
    true
}

pub struct Watch {
    lane: LaneId,
    reference: u64,
    task: tokio::task::AbortHandle,
}

pub fn deliver_to(value: &Value, connection: u64) -> bool {
    value["method"] != TOPIC || value["params"]["subscription_id"].as_u64() == Some(connection)
}

#[derive(Clone)]
struct Source {
    kind: String,
    path: Option<PathBuf>,
}

async fn source(ctx: &Arc<Ctx>, p: &Params) -> Result<Source, String> {
    let cwd = ctx
        .lanes
        .focus(p.lane_id)
        .await
        .map_err(|e| e.to_string())?;
    let meta = ctx
        .store
        .list_lane_meta()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|m| m.id == p.lane_id)
        .ok_or("lane not found")?;
    let window = p
        .window
        .clone()
        .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
    let backend = ctx.backend.clone();
    let (win, started) = tokio::task::spawn_blocking(move || {
        let started = backend.window_started_at(&window);
        let meta = backend
            .list_windows_meta()
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.name == window);
        (meta, started)
    })
    .await
    .map_err(|e| e.to_string())?;
    let kind = p
        .kind
        .clone()
        .or_else(|| win.as_ref().and_then(|w| w.agent_kind.clone()))
        .or(meta.agent_kind)
        .unwrap_or_else(|| "claude-code".into());
    let session = p
        .session_id
        .clone()
        .filter(|s| !s.starts_with("win:"))
        .or_else(|| win.and_then(|w| w.session));
    let started = if session.is_some() { None } else { started };
    if !matches!(kind.as_str(), "claude-code" | "codex") {
        return Ok(Source { kind, path: None });
    }
    let known = ctx
        .store
        .conversation_source(p.lane_id, kind.clone(), session.clone(), started)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(path) = known.filter(|s| PathBuf::from(s).is_file()) {
        return Ok(Source {
            kind,
            path: Some(path.into()),
        });
    }
    let scan_kind = kind.clone();
    let lane = p.lane_id;
    let path = tokio::task::spawn_blocking(move || {
        if scan_kind == "claude-code" {
            if let Some(id) = &session {
                return repomon_core::agent::claude::transcript_path_for_session(&cwd, id);
            }
            return repomon_core::agent::claude::summaries_for(
                &cwd,
                chrono::Duration::hours(24 * 30),
                1,
            )
            .iter()
            .find(|s| {
                started.is_none_or(|start| {
                    scan_claude_transcript(&s.manifest_path, 0, None)
                        .ok()
                        .and_then(|scan| scan.transcript.iter().filter_map(|r| r.item.at).min())
                        .is_some_and(|at| at >= start)
                })
            })
            .map(|s| s.manifest_path.clone());
        }
        let index = FleetIndex::new(Vec::new(), vec![(lane, meta.repo_id, cwd)]);
        crate::usage_ingest::discover_sources(512)
            .into_iter()
            .filter(|s| s.kind == crate::usage_ingest::SourceKind::Codex)
            .find_map(|s| {
                let scan = scan_codex_rollout(&s.path, 0).ok()?;
                scan.sessions
                    .iter()
                    .any(|row| {
                        index.attribute(row.cwd.as_deref()).lane_id == Some(lane)
                            && session.as_ref().is_none_or(|id| &row.session_id == id)
                            && started
                                .is_none_or(|start| row.first_at.is_some_and(|at| at >= start))
                    })
                    .then_some(s.path)
            })
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(Source { kind, path })
}

fn scan(source: &Source) -> Result<SourceScan, String> {
    let Some(path) = &source.path else {
        return Ok(SourceScan::default());
    };
    let options = ScanOptions {
        collect_transcript: true,
    };
    match source.kind.as_str() {
        "claude-code" => scan_claude_transcript_with_options(path, 0, None, options),
        "codex" => scan_codex_rollout_with_options(path, 0, options),
        _ => return Ok(SourceScan::default()),
    }
    .map_err(|e| e.to_string())
}

fn page_rows(scan: SourceScan, before: Option<u64>) -> (Vec<TranscriptItem>, Option<u64>) {
    let end = before.unwrap_or(u64::MAX);
    let last = scan
        .transcript
        .iter()
        .filter(|r| r.offset < end)
        .map(|r| r.offset)
        .max()
        .unwrap_or(0);
    let floor = last.saturating_sub(PAGE_BYTES);
    let start = scan
        .transcript
        .iter()
        .find(|r| r.offset >= floor && r.offset < end)
        .map(|r| r.offset)
        .unwrap_or(0);
    let older = scan.transcript.iter().any(|r| r.offset < start);
    let rows = scan
        .transcript
        .into_iter()
        .filter(|r| r.offset >= start && r.offset < end)
        .map(|r| r.item)
        .collect();
    (rows, older.then_some(start))
}

async fn read_page(ctx: &Arc<Ctx>, source: Source, before: Option<u64>) -> Result<Value, String> {
    let path = source.path.clone();
    let mut parsed = tokio::task::spawn_blocking(move || scan(&source))
        .await
        .map_err(|e| e.to_string())??;
    if let Some(path) = &path {
        let events = ctx
            .store
            .conversation_cost_events(path.to_string_lossy().into())
            .await
            .map_err(|e| e.to_string())?;
        let table = crate::usage_ingest::price_table(ctx).await;
        for event in events {
            if let Some(cost) = table.cost(&event.model, event.at, &event.tokens()) {
                let mut item =
                    TranscriptItem::new("status", format!("Turn cost ${cost:.4}"), Some(event.at));
                item.id = Some(format!("cost:{}", event.source_offset));
                item.status_kind = Some("turn_cost".into());
                item.cost_usd = Some(cost);
                // A billable record can contain only hidden thinking. Its cost is still a row,
                // and participates in the same byte-offset paging as visible messages.
                parsed
                    .transcript
                    .push(repomon_core::usage_ledger::scan::TranscriptEntry {
                        offset: event.source_offset as u64,
                        item,
                    });
            }
        }
        parsed.transcript.sort_by_key(|row| row.offset);
    }
    let (mut items, next_before) = page_rows(parsed, before);
    if let Some(path) = path {
        let source_key = path.to_string_lossy();
        for item in &mut items {
            item.id = item.id.take().map(|id| format!("{source_key}:{id}"));
        }
    }
    Ok(json!({ "items": items, "next_before": next_before }))
}

pub async fn page(ctx: &Arc<Ctx>, p: &Params) -> Result<Value, String> {
    let src = source(ctx, p).await?;
    if src.path.is_none() && p.before.is_none() {
        let backend = ctx.backend.clone();
        let window = p
            .window
            .clone()
            .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
        let identity = window.clone();
        let pane = tokio::task::spawn_blocking(move || {
            backend.capture_named(&window, CaptureOpts::last(100))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let mut item = TranscriptItem::new(
            "terminal_block",
            repomon_core::agent::text::strip_ansi(&pane),
            None,
        );
        item.id = Some(format!("pane:{identity}"));
        return Ok(
            json!({"items": if item.text.trim().is_empty() { Vec::new() } else { vec![item] }, "next_before":null}),
        );
    }
    read_page(ctx, src, p.before).await
}

pub async fn unwatch_all(ctx: &Ctx, sess: &ConnSession) {
    let watches = std::mem::take(&mut *sess.transcript_watches.lock().await);
    for (window, watch) in watches {
        watch.task.abort();
        crate::bytes_stream::unwatch(&ctx.backend, &ctx.bytes_watches, &window, watch.reference)
            .await;
    }
}

pub async fn watch(ctx: &Arc<Ctx>, sess: &Arc<ConnSession>, p: Params) -> Result<Value, String> {
    let window = p
        .window
        .clone()
        .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
    let mut watches = sess.transcript_watches.lock().await;
    let targets: Vec<_> = watches
        .iter()
        .filter(|(w, watch)| {
            if p.on || p.window.is_some() {
                *w == &window
            } else {
                watch.lane == p.lane_id
            }
        })
        .map(|(w, _)| w.clone())
        .collect();
    for target in targets {
        if let Some(old) = watches.remove(&target) {
            old.task.abort();
            crate::bytes_stream::unwatch(&ctx.backend, &ctx.bytes_watches, &target, old.reference)
                .await;
        }
    }
    if !p.on {
        return Ok(Value::Null);
    }
    let mut initial_source = source(ctx, &p).await?;
    let initial = if initial_source.path.is_none() {
        page(ctx, &p).await?
    } else {
        read_page(ctx, initial_source.clone(), None).await?
    };
    let response = initial.clone();
    let reference = NEXT_WATCH.fetch_add(1, Ordering::Relaxed);
    let mut events = ctx.events.subscribe();
    crate::bytes_stream::watch(
        ctx.backend.clone(),
        ctx.events.clone(),
        &ctx.bytes_watches,
        p.lane_id,
        window.clone(),
        reference,
    )
    .await?;
    let task_ctx = ctx.clone();
    let task_window = window.clone();
    let connection = sess.id;
    let initial_rows = serde_json::from_value(initial["items"].clone()).unwrap_or_default();
    let lane = p.lane_id;
    let task = tokio::spawn(async move {
        let mut state = ConversationStream::default();
        state.update(initial_rows, Vec::new(), false);
        let mut ticker = tokio::time::interval(Duration::from_millis(150));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut dirty = true;
        let mut activity = None;
        let mut cached = initial;
        let mut signature = None;
        let mut ledger_dirty = false;
        let mut stream_ended = false;
        let mut discovery = std::time::Instant::now();
        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(v) if v["params"]["window"] == task_window && v["method"] == crate::pubsub::topic::AGENT_BYTES => {
                            dirty = true;
                            stream_ended = false;
                            activity = Some(std::time::Instant::now());
                        }
                        Ok(v) if v["method"] == crate::pubsub::topic::USAGE_CHANGED => { ledger_dirty = true; }
                        Ok(v) if v["params"]["window"] == task_window && v["method"] == crate::pubsub::topic::AGENT_STREAM_CLOSED => { stream_ended = true; activity = None; dirty = true; },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { dirty = true; }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        _ => {}
                    }
                }
                _ = ticker.tick() => {
                    if discovery.elapsed() >= Duration::from_secs(2) {
                        if let Ok(src) = source(&task_ctx, &p).await { initial_source = src; }
                        discovery = std::time::Instant::now();
                    }
                    let src = initial_source.clone();
                    let kind = src.kind.clone();
                    let current = src.path.as_ref().and_then(|path| std::fs::metadata(path).ok().map(|m| (path.clone(), m.len(), m.modified().ok())));
                    if current != signature || ledger_dirty {
                        if let Ok(value) = read_page(&task_ctx, src, None).await {
                            cached = value;
                            signature = current;
                            ledger_dirty = false;
                            dirty = true;
                        }
                    }
                    if !dirty && activity.is_none() { continue; }
                    dirty = false;
                    let backend = task_ctx.backend.clone();
                    let win = task_window.clone();
                    let pane = tokio::task::spawn_blocking(move || backend.capture_named(&win, CaptureOpts::last(100))).await;
                    let Ok(Ok(pane)) = pane else { continue; };
                    let active = !stream_ended && (prompt::detect_active_spinner(&pane).is_some() || activity.is_some_and(|t| t.elapsed() < Duration::from_secs(2)));
                    if !active { activity = None; }
                    let mut live = pane_items(&kind, &pane);
                    if active && live.is_empty() {
                        let mut item = TranscriptItem::new("terminal_block", repomon_core::agent::text::strip_ansi(&pane), None);
                        item.partial = Some(true);
                        live.push(item);
                    }
                    let finals = serde_json::from_value(cached["items"].clone()).unwrap_or_default();
                    let update = state.update(finals, live, active);
                    if !update.items.is_empty() || !update.removed_ids.is_empty() {
                        task_ctx.broadcast(TOPIC, json!({ "lane_id": p.lane_id, "window": task_window,
                            "subscription_id": connection, "items": update.items, "removed_ids": update.removed_ids,
                            "next_before": cached["next_before"] }));
                    }
                }
            }
        }
    });
    watches.insert(
        window,
        Watch {
            lane,
            reference,
            task: task.abort_handle(),
        },
    );
    // The initial page is returned directly so callers need not race the first notification.
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::ConnKind;
    use repomon_core::agent::backend::{
        AttachCommand, ByteStream, OwnerState, ScrollEvent, SpawnSpec, WindowActivity,
    };
    use repomon_core::usage_ledger::UsageSessionMeta;
    use repomon_core::{ByteStreamEvent, Config, SessionBackend, Store};
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct ScriptedBackend {
        opens: AtomicU64,
        pane: StdMutex<String>,
        senders:
            StdMutex<HashMap<String, (u64, tokio::sync::mpsc::UnboundedSender<ByteStreamEvent>)>>,
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
        fn spawn(&self, _lane: LaneId, _spec: &SpawnSpec) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn spawn_named(&self, _window: &str, _spec: &SpawnSpec) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn open_named(
            &self,
            _window: &str,
            _cwd: &std::path::Path,
        ) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn capture_named(&self, _window: &str, _opts: CaptureOpts) -> repomon_core::Result<String> {
            Ok(self.pane.lock().unwrap().clone())
        }
        fn cursor_named(&self, _window: &str) -> Option<repomon_core::agent::Cursor> {
            Some(repomon_core::agent::Cursor { col: 0, row: 0 })
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
        fn scroll_wheel_named(
            &self,
            _window: &str,
            _event: ScrollEvent,
        ) -> repomon_core::Result<()> {
            Ok(())
        }
        fn send_literal_named(&self, _window: &str, _text: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn send_text_named(&self, _window: &str, _text: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn send_key_named(&self, _window: &str, _key: &str) -> repomon_core::Result<()> {
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
        fn open_byte_stream(&self, window: &str) -> repomon_core::Result<ByteStream> {
            let tag = self.opens.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            self.senders
                .lock()
                .unwrap()
                .insert(window.to_string(), (tag, tx));
            Ok(ByteStream { tag, rx })
        }
        fn close_byte_stream(&self, window: &str, tag: u64) -> repomon_core::Result<()> {
            let mut senders = self.senders.lock().unwrap();
            if senders
                .get(window)
                .is_some_and(|(current, _)| *current == tag)
            {
                senders.remove(window);
            }
            Ok(())
        }
    }

    async fn next_items(rx: &mut tokio::sync::broadcast::Receiver<Value>) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = rx.recv().await.unwrap();
                if event["method"] == TOPIC {
                    return event["params"].clone();
                }
            }
        })
        .await
        .expect("transcript push before deadline")
    }

    #[tokio::test]
    async fn real_pane_partial_precedes_delayed_final_for_both_providers_and_shares_bytes() {
        for (kind, fixture) in [
            (
                "claude-code",
                include_str!("../../repomon-core/src/agent/fixtures/claude_idle_done_spinner.txt"),
            ),
            (
                "codex",
                include_str!("../../repomon-core/src/agent/fixtures/codex_status_v0.ansi"),
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open_in_memory().unwrap();
            let repo = store
                .add_repo(dir.path().into(), "test".into(), None)
                .await
                .unwrap();
            let lane = store
                .get_or_create_lane(repo.id, dir.path().to_string_lossy().into())
                .await
                .unwrap();
            store
                .set_lane_agent_kind(lane, Some(kind.into()))
                .await
                .unwrap();
            let path = dir.path().join("transcript.jsonl");
            std::fs::write(&path, "").unwrap();
            store
                .upsert_usage_sessions(vec![UsageSessionMeta {
                    session_id: "fixture".into(),
                    agent_kind: kind.into(),
                    headline: None,
                    headline_raw: None,
                    headline_version: 1,
                    cwd: Some(dir.path().to_string_lossy().into()),
                    repo_id: Some(repo.id),
                    lane_id: Some(lane),
                    started_at: Some(chrono::Utc::now()),
                    ended_at: None,
                    turns: 0,
                    tool_calls: 0,
                    retries: 0,
                    external: false,
                    source_path: Some(path.to_string_lossy().into()),
                    counts_version: 1,
                }])
                .await
                .unwrap();
            let backend = Arc::new(ScriptedBackend::default());
            let ctx = Ctx::new_with_backend(
                store,
                Config::default(),
                None,
                dir.path().join("config.toml"),
                dir.path().join("notes"),
                backend.clone(),
            );
            let sess = ctx.open_session(ConnKind::Local).await;
            let window = TmuxRuntime::window_name(lane);
            let params: Params =
                serde_json::from_value(json!({"lane_id":lane,"kind":kind,"on":true})).unwrap();
            let mut events = ctx.events.subscribe();
            let initial = watch(&ctx, &sess, params.clone()).await.unwrap();
            assert_eq!(initial["items"], json!([]));
            // The terminal and conversation share exactly one backend stream, but own separate refs.
            crate::bytes_stream::watch(
                backend.clone(),
                ctx.events.clone(),
                &ctx.bytes_watches,
                lane,
                window.clone(),
                sess.id,
            )
            .await
            .unwrap();
            assert_eq!(backend.opens.load(Ordering::SeqCst), 1);
            *backend.pane.lock().unwrap() = fixture.into();
            backend.senders.lock().unwrap()[&window]
                .1
                .send(ByteStreamEvent::Bytes(b"fixture repaint".to_vec()))
                .unwrap();
            let partial = loop {
                let event = next_items(&mut events).await;
                if let Some(item) = event["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|i| i["partial"] == true && i["kind"] == "assistant")
                {
                    break item.clone();
                }
            };
            let partial_at = std::time::Instant::now();
            assert_eq!(
                std::fs::metadata(&path).unwrap().len(),
                0,
                "partial was delivered before any final transcript existed"
            );
            let final_text = "The transcript has now settled.";
            let row = if kind == "claude-code" {
                json!({"type":"assistant","message":{"model":"claude-sonnet-5","content":[{"type":"text","text":final_text}]}})
            } else {
                json!({"type":"event_msg","payload":{"type":"agent_message","message":final_text}})
            };
            std::fs::write(&path, format!("{row}\n")).unwrap();
            let final_row = loop {
                let event = next_items(&mut events).await;
                if let Some(item) = event["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|i| i["text"] == final_text)
                {
                    assert!(
                        !event["removed_ids"]
                            .as_array()
                            .unwrap()
                            .contains(&partial["id"])
                    );
                    break item.clone();
                }
            };
            assert_eq!(final_row["id"], partial["id"]);
            assert_ne!(final_row["partial"], true);
            assert!(partial_at.elapsed() > Duration::ZERO);
            let stop: Params = serde_json::from_value(json!({"lane_id":lane,"on":false})).unwrap();
            watch(&ctx, &sess, stop).await.unwrap();
            assert!(
                ctx.bytes_watches.lock().await.contains_key(&window),
                "stopping conversation preserves terminal ownership"
            );
            ctx.close_session(sess.id).await;
            assert!(ctx.bytes_watches.lock().await.is_empty());
            assert!(backend.senders.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn lane_view_rpc_persists_validates_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.db")).unwrap();
        let repo = store
            .add_repo(dir.path().into(), "test".into(), None)
            .await
            .unwrap();
        let lane = store
            .get_or_create_lane(repo.id, dir.path().to_string_lossy().into())
            .await
            .unwrap();
        let backend = Arc::new(ScriptedBackend::default());
        let ctx = Ctx::new_with_backend(
            store.clone(),
            Config::default(),
            None,
            dir.path().join("config.toml"),
            dir.path().join("notes"),
            backend,
        );
        let sess = ctx.open_session(ConnKind::Local).await;
        for value in [json!("conversation"), Value::Null, json!("terminal")] {
            crate::rpc::dispatch(
                &ctx,
                &sess,
                "lane.set_view",
                Some(json!({"lane_id":lane,"view_mode":value})),
            )
            .await
            .unwrap();
            let reopened = Store::open(&dir.path().join("test.db")).unwrap();
            assert_eq!(
                json!(reopened.list_lane_meta().await.unwrap()[0].view_mode),
                value
            );
        }
        assert!(
            crate::rpc::dispatch(
                &ctx,
                &sess,
                "lane.set_view",
                Some(json!({"lane_id":lane,"view_mode":"wrong"}))
            )
            .await
            .is_err()
        );
        assert_eq!(
            store.list_lane_meta().await.unwrap()[0]
                .view_mode
                .as_deref(),
            Some("terminal")
        );
    }

    #[test]
    fn transcript_events_are_connection_scoped() {
        let event = json!({"method":TOPIC,"params":{"subscription_id":4}});
        assert!(deliver_to(&event, 4));
        assert!(!deliver_to(&event, 5));
    }
    #[test]
    fn backward_pages_keep_every_message_in_order_for_both_scanners() {
        for kind in ["claude-code", "codex"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("transcript.jsonl");
            let mut body = String::new();
            for n in 0..2000 {
                let message = format!("message {n} {}", "x".repeat(80));
                let row = if kind == "claude-code" {
                    json!({"type":"user","message":{"content":message}})
                } else {
                    json!({"type":"event_msg","payload":{"type":"user_message","message":message}})
                };
                body.push_str(&format!("{row}\n"));
            }
            std::fs::write(&path, body).unwrap();
            let source = Source {
                kind: kind.into(),
                path: Some(path),
            };
            let mut before = None;
            let mut all = Vec::new();
            let mut pages = 0;
            loop {
                let (rows, next) = page_rows(scan(&source).unwrap(), before);
                pages += 1;
                let mut older: Vec<_> = rows.into_iter().map(|i| i.text).collect();
                older.append(&mut all);
                all = older;
                if next.is_none() {
                    break;
                }
                assert!(before.is_none_or(|b| next.unwrap() < b));
                before = next;
            }
            assert!(pages > 1);
            assert_eq!(all.len(), 2000);
            for (n, text) in all.iter().enumerate() {
                assert!(text.starts_with(&format!("message {n} ")));
            }
        }
    }
    #[tokio::test]
    async fn ledger_costs_attach_to_tool_only_claude_rows_and_codex_usage() {
        for (kind, fixture) in [
            ("claude-code", "claude_usage_v0.jsonl"),
            ("claude-code", "claude_multiblock_v0.jsonl"),
            ("codex", "codex_usage_v0.jsonl"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../repomon-core/src/usage_ledger/fixtures")
                .join(fixture);
            let source = Source {
                kind: kind.into(),
                path: Some(path.clone()),
            };
            let parsed = scan(&source).unwrap();
            let events = parsed
                .events
                .into_iter()
                .map(|e| repomon_core::usage_ledger::UsageEvent {
                    at: e.at,
                    agent_kind: e.agent_kind,
                    model: e.model,
                    account: e.account,
                    lane_id: None,
                    repo_id: None,
                    session_id: e.session_id,
                    window: None,
                    cwd: e.cwd,
                    input_tokens: e.tokens.input,
                    output_tokens: e.tokens.output,
                    cache_read_tokens: e.tokens.cache_read,
                    cache_write_tokens: e.tokens.cache_write,
                    thinking_tokens: e.thinking_tokens,
                    estimated: e.estimated,
                    external: false,
                    subagent: false,
                    source_path: e.source_path,
                    source_offset: e.source_offset,
                })
                .collect::<Vec<_>>();
            let expected = events.len();
            let store = Store::open_in_memory().unwrap();
            store.record_usage_events(events).await.unwrap();
            let ctx = Ctx::new_with_backend(
                store,
                Config::default(),
                None,
                dir.path().join("config.toml"),
                dir.path().join("notes"),
                Arc::new(ScriptedBackend::default()),
            );
            let page = read_page(&ctx, source, None).await.unwrap();
            let costs: Vec<_> = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|i| i["status_kind"] == "turn_cost")
                .collect();
            assert_eq!(
                costs.len(),
                expected,
                "all priced ledger events are included for {kind}"
            );
            assert!(costs.iter().all(|i| i["cost_usd"].as_f64().unwrap() > 0.0));
        }
    }
}
