//! Round 5 regression tests use isolated stores, provider fixtures, and the production watch path.
use super::tests::{ScriptedBackend, next_items};
use super::*;
use crate::conn::ConnKind;
use repomon_core::usage_ledger::UsageSessionMeta;
use repomon_core::{ByteStreamEvent, Config, Store};
use std::io::Write;
use std::path::Path;

pub(super) async fn context(dir: &Path) -> (Arc<Ctx>, Arc<ScriptedBackend>) {
    let mut config = Config::default();
    config.usage.refresh_prices = false;
    config.repomind.home = dir.join("repomind").to_string_lossy().into_owned();
    let backend = Arc::new(ScriptedBackend::default());
    let ctx = Ctx::new_with_backend(
        Store::open_in_memory().unwrap(),
        config,
        None,
        dir.join("config.toml"),
        dir.join("notes"),
        backend.clone(),
    );
    (ctx, backend)
}
pub(super) async fn lane_source(
    ctx: &Arc<Ctx>,
    dir: &Path,
    kind: &str,
    path: &Path,
    session: &str,
) -> Params {
    let repo = ctx
        .store
        .add_repo(dir.into(), session.into(), None)
        .await
        .unwrap();
    let lane = ctx
        .store
        .get_or_create_lane(repo.id, dir.to_string_lossy().into())
        .await
        .unwrap();
    ctx.store
        .set_lane_agent_kind(lane, Some(kind.into()))
        .await
        .unwrap();
    ctx.store
        .upsert_usage_sessions(vec![UsageSessionMeta {
            session_id: session.into(),
            agent_kind: kind.into(),
            headline: None,
            headline_raw: None,
            headline_version: 5,
            cwd: Some(dir.to_string_lossy().into()),
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
    ctx.backend
        .set_window_agent_kind(&TmuxRuntime::window_name(lane), kind)
        .unwrap();
    serde_json::from_value(json!({"lane_id":lane,"session_id":session,"kind":kind})).unwrap()
}
pub(super) fn user_record(n: usize, kind: &str) -> String {
    let text = format!("message {n} {} ü", "x".repeat(100));
    let row = if kind == "claude-code" {
        json!({"type":"user","message":{"content":text}})
    } else {
        json!({"type":"event_msg","payload":{"type":"user_message","message":text}})
    };
    format!(
        "{row}
"
    )
}
#[tokio::test]
async fn cache_is_shared_and_fingerprint_invalidates_append_replacement_and_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = context(dir.path()).await;
    let path = dir.path().join("source.jsonl");
    std::fs::write(&path, user_record(1, "codex")).unwrap();
    let source = Source {
        window: "lane-1".into(),
        kind: "codex".into(),
        path: Some(path.clone()),
        session: None,
    };
    let (a, b) = tokio::join!(
        read_page(&ctx, source.clone(), None),
        read_page(&ctx, source.clone(), None)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(ctx.transcript_cache.scans.load(Ordering::Relaxed), 1);
    ctx.transcript_cache.usage_changed("/another/session.jsonl");
    assert_eq!(ctx.transcript_cache.cost_revision(&source), 0);
    ctx.transcript_cache.usage_changed(&path.to_string_lossy());
    assert_eq!(ctx.transcript_cache.cost_revision(&source), 1);
    read_page(&ctx, source.clone(), None).await.unwrap();
    assert_eq!(
        ctx.transcript_cache.scans.load(Ordering::Relaxed),
        1,
        "usage alone must not parse"
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(user_record(2, "codex").as_bytes())
        .unwrap();
    assert_eq!(
        read_page(&ctx, source.clone(), None).await.unwrap()["page_count"],
        2
    );
    let replacement = dir.path().join("replacement");
    std::fs::write(&replacement, user_record(3, "codex")).unwrap();
    std::fs::rename(replacement, &path).unwrap();
    let value = read_page(&ctx, source.clone(), None).await.unwrap();
    assert!(
        value["items"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("message 3")
    );
    std::fs::write(&path, "").unwrap();
    assert_eq!(
        read_page(&ctx, source, None).await.unwrap()["page_count"],
        0
    );
    assert_eq!(ctx.transcript_cache.scans.load(Ordering::Relaxed), 4);
}
#[tokio::test]
async fn bounded_pages_seek_and_preserve_every_user_record() {
    for kind in ["claude-code", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = context(dir.path()).await;
        let path = dir.path().join("source.jsonl");
        std::fs::write(
            &path,
            (0..2500).map(|n| user_record(n, kind)).collect::<String>(),
        )
        .unwrap();
        let source = Source {
            window: "lane-1".into(),
            kind: kind.into(),
            path: Some(path),
            session: None,
        };
        let first = scan_page(&source, None).unwrap();
        assert!(first.start > 0);
        assert!(first.end - first.start <= PAGE_BYTES);
        let mut before = None;
        let mut all = Vec::new();
        loop {
            let page = read_page(&ctx, source.clone(), before).await.unwrap();
            let items = page["items"].as_array().unwrap();
            all.splice(
                0..0,
                items
                    .iter()
                    .map(|v| v["text"].as_str().unwrap().to_string()),
            );
            let next = page["next_before"].as_u64();
            if next.is_none() {
                break;
            }
            assert!(before.is_none_or(|b| next.unwrap() < b));
            before = next;
        }
        assert_eq!(all.len(), 2500);
        for (n, text) in all.iter().enumerate() {
            assert!(text.starts_with(&format!("message {n} ")));
        }
    }
}
#[tokio::test]
async fn antigravity_and_opencode_resolve_through_the_existing_scanners() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = context(dir.path()).await;
    let path = dir.path().join("agy.jsonl");
    std::fs::write(
        &path,
        include_str!("../../repomon-core/src/usage_ledger/fixtures/antigravity_usage_v0.jsonl"),
    )
    .unwrap();
    let params = lane_source(&ctx, dir.path(), "antigravity", &path, "agy").await;
    let value = page(&ctx, &params).await.unwrap();
    assert!(
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["kind"] == "user")
    );
    assert!(
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["kind"] == "assistant")
    );
    assert!(
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["kind"] == "tool_call")
    );
    let db = dir.path().join("opencode.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(include_str!(
        "../../repomon-core/src/usage_ledger/fixtures/opencode_conversation_v0.sql"
    ))
    .unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    let oc_dir = dir.path().join("oc");
    std::fs::create_dir(&oc_dir).unwrap();
    let params = lane_source(&ctx, &oc_dir, "opencode", &db, "first").await;
    let first = page(&ctx, &params).await.unwrap();
    assert!(!first.to_string().contains("DO NOT MIX"));
    assert_eq!(first["older_message_count"], 0);
    assert!(
        first["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["text"] == "Fix the preview")
    );
    assert!(
        first["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["result_summary"] == "file content" && v["status"] == "ok")
    );
    let source = source(&ctx, &params).await.unwrap();
    let stamp = fingerprint(&source);
    conn.execute(
        "UPDATE part SET data=?1 WHERE id='prose'",
        [r#"{"type":"text","text":"WAL update"}"#],
    )
    .unwrap();
    assert_ne!(fingerprint(&source), stamp);
    assert!(
        page(&ctx, &params)
            .await
            .unwrap()
            .to_string()
            .contains("WAL update")
    );
}
#[tokio::test]
async fn opencode_pages_keep_timestamp_ties_and_filter_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = context(dir.path()).await;
    let path = dir.path().join("db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!(
        "../../repomon-core/src/usage_ledger/fixtures/opencode_conversation_v0.sql"
    ))
    .unwrap();
    for n in 0..450 {
        let id = format!("msg-{n}");
        conn.execute(
            "INSERT INTO message VALUES(?1,'first',?2,?3)",
            rusqlite::params![id, 2000 + n / 3, r#"{"role":"user"}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES(?1,?1,'first',?2,?3)",
            rusqlite::params![
                id,
                2000 + n / 3,
                json!({"type":"text","text":format!("extra {n}")}).to_string()
            ],
        )
        .unwrap();
    }
    let src = Source {
        window: "lane-1".into(),
        kind: "opencode".into(),
        path: Some(path),
        session: Some("first".into()),
    };
    let mut before = None;
    let mut ids = std::collections::HashSet::new();
    loop {
        let p = read_page(&ctx, src.clone(), before).await.unwrap();
        for item in p["items"].as_array().unwrap() {
            assert!(
                ids.insert(item["id"].as_str().unwrap().to_owned()),
                "duplicate {}",
                item["id"]
            );
            assert!(!item["text"].as_str().unwrap().contains("DO NOT MIX"));
        }
        before = p["next_before"].as_u64();
        if before.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 454);
}
#[tokio::test]
async fn hermes_idle_capture_is_a_stable_terminal_excerpt() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let params = lane_source(
        &ctx,
        dir.path(),
        "hermes",
        &dir.path().join("absent"),
        "hermes",
    )
    .await;
    *backend.pane.lock().unwrap() = "Hermes is ready.".into();
    let initial = page(&ctx, &params).await.unwrap();
    assert_eq!(initial["items"][0]["kind"], "terminal_block");
    let session = ctx.open_session(ConnKind::Local).await;
    watch(&ctx, &session, params.clone()).await.unwrap();
    let mut events = ctx.events.subscribe();
    *backend.pane.lock().unwrap() = "Hermes has a result.".into();
    let window = TmuxRuntime::window_name(params.lane_id);
    backend.senders.lock().unwrap()[&window]
        .1
        .send(ByteStreamEvent::Bytes(b"update".to_vec()))
        .unwrap();
    let event = next_items(&mut events).await;
    let item = event["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == "terminal_block")
        .unwrap();
    assert_eq!(item["id"], initial["items"][0]["id"]);
    assert_eq!(item["text"], "Hermes has a result.");
    unwatch_all(&ctx, &session).await;
}
#[tokio::test]
#[allow(clippy::await_holding_lock)] // Intentional barrier: prove delivery before releasing parse lock.
async fn pane_preview_and_terminal_bytes_do_not_wait_for_a_blocked_history_read() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let path = dir.path().join("codex.jsonl");
    std::fs::write(&path, "").unwrap();
    let params = lane_source(&ctx, dir.path(), "codex", &path, "fixture").await;
    let session = ctx.open_session(ConnKind::Local).await;
    watch(&ctx, &session, params.clone()).await.unwrap();
    let src = source(&ctx, &params).await.unwrap();
    let entry = ctx
        .transcript_cache
        .entries
        .lock()
        .unwrap()
        .get(&src)
        .unwrap()
        .clone();
    // Hold the parse worker at a deterministic barrier. The preview must arrive before release.
    let barrier = entry.parsed.lock().unwrap();
    std::fs::write(&path, user_record(1, "codex")).unwrap();
    let captured = repomon_core::agent::text::strip_ansi(include_str!(
        "../../repomon-core/src/agent/fixtures/codex_working_footer_2026_09_10.ansi"
    ));
    let indicator = captured
        .lines()
        .find(|line| line.contains("esc to interrupt"))
        .unwrap();
    *backend.pane.lock().unwrap() = format!("• New preview before history finishes\n{indicator}");
    let window = TmuxRuntime::window_name(params.lane_id);
    let mut events = ctx.events.subscribe();
    backend.senders.lock().unwrap()[&window]
        .1
        .send(ByteStreamEvent::Bytes(b"new byte".to_vec()))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut byte = false;
        let mut preview = false;
        while !byte || !preview {
            let event = events.recv().await.unwrap();
            byte |= event["method"] == crate::pubsub::topic::AGENT_BYTES;
            preview |= event["method"] == TOPIC
                && event["params"]["items"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|v| v["text"] == "New preview before history finishes")
                });
        }
    })
    .await
    .expect("bytes and preview delivered with history still blocked");
    drop(barrier);
    unwatch_all(&ctx, &session).await;
}

/// Four synthetic lanes (Claude/Codex at 10/50 MiB), the same terminal streams in both runs,
/// and two conversation subscriptions per lane in the open case. Measures backend enqueue to
/// daemon AGENT_BYTES notification, not client IPC/rendering or a real terminal's paint latency.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual latency benchmark; needs synthetic C1_BENCH_DIR"]
async fn terminal_byte_latency_open_versus_closed() {
    let root = PathBuf::from(std::env::var("C1_BENCH_DIR").expect("synthetic fixtures"));
    for open in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, backend) = context(dir.path()).await;
        ctx.config.write().await.usage.refresh_prices =
            std::env::var("C1_BENCH_PRICES").as_deref() == Ok("1");
        let clients = [
            ctx.open_session(ConnKind::Local).await,
            ctx.open_session(ConnKind::Local).await,
        ];
        let mut lanes = Vec::new();
        for (index, (kind, mib)) in [("claude", 10), ("claude", 50), ("codex", 10), ("codex", 50)]
            .into_iter()
            .enumerate()
        {
            let cwd = dir.path().join(format!("lane-{index}"));
            std::fs::create_dir(&cwd).unwrap();
            let path = cwd.join("source.jsonl");
            std::fs::copy(root.join(format!("{kind}-{mib}.jsonl")), &path).unwrap();
            let kind = if kind == "claude" {
                "claude-code"
            } else {
                kind
            };
            let params = lane_source(&ctx, &cwd, kind, &path, &format!("session-{index}")).await;
            let window = TmuxRuntime::window_name(params.lane_id);
            crate::bytes_stream::watch(
                backend.clone(),
                ctx.events.clone(),
                &ctx.bytes_watches,
                params.lane_id,
                window.clone(),
                999,
            )
            .await
            .unwrap();
            if open {
                for client in &clients {
                    watch(&ctx, client, params.clone()).await.unwrap();
                }
            }
            lanes.push((window, kind, path));
        }
        let mut events = ctx.events.subscribe();
        let mut micros = Vec::new();
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let producer_times = sent.clone();
        let producers: Vec<_> = lanes
            .iter()
            .map(|(window, kind, path)| {
                (
                    backend.senders.lock().unwrap()[window].1.clone(),
                    kind.to_string(),
                    path.clone(),
                )
            })
            .collect();
        let producer_ctx = ctx.clone();
        // A real PTY can produce bytes while all async workers are busy. A separate OS thread
        // preserves that property instead of silently delaying the benchmark's own send clock.
        let producer = std::thread::spawn(move || {
            for pulse in 0..200usize {
                if pulse % 10 == 0 {
                    for (_, kind, path) in &producers {
                        std::fs::OpenOptions::new()
                            .append(true)
                            .open(path)
                            .unwrap()
                            .write_all(user_record(50000 + pulse, kind).as_bytes())
                            .unwrap();
                    }
                    producer_ctx.broadcast(crate::pubsub::topic::USAGE_CHANGED, json!({}));
                }
                producer_times
                    .lock()
                    .unwrap()
                    .push(std::time::Instant::now());
                for (sender, _, _) in &producers {
                    sender
                        .send(ByteStreamEvent::Bytes(
                            (pulse as u64).to_le_bytes().to_vec(),
                        ))
                        .unwrap();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let mut received = 0;
        while received < 200 * lanes.len() {
            let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .unwrap()
                .unwrap();
            if event["method"] == crate::pubsub::topic::AGENT_BYTES {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(event["params"]["data"].as_str().unwrap())
                    .unwrap();
                let pulse = u64::from_le_bytes(bytes.try_into().unwrap()) as usize;
                if pulse >= 20 {
                    micros.push(sent.lock().unwrap()[pulse].elapsed().as_secs_f64() * 1_000_000.0);
                }
                received += 1;
            }
        }
        producer.join().unwrap();
        micros.sort_by(f64::total_cmp);
        println!(
            "BYTE_LATENCY views={} lanes=4 subscriptions={} samples={} p50_us={:.1} p95_us={:.1} p99_us={:.1} max_us={:.1}",
            if open { "open" } else { "closed" },
            if open { 8 } else { 0 },
            micros.len(),
            micros[micros.len() / 2],
            micros[micros.len() * 95 / 100],
            micros[micros.len() * 99 / 100],
            micros.last().unwrap()
        );
        for client in &clients {
            unwatch_all(&ctx, client).await;
        }
        for (window, _, _) in &lanes {
            crate::bytes_stream::unwatch(&ctx.backend, &ctx.bytes_watches, window, 999).await;
        }
    }
}

pub(super) async fn transcript_event(
    events: &mut tokio::sync::broadcast::Receiver<Value>,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["method"] == TOPIC && predicate(&event["params"]) {
                return event["params"].clone();
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn queued_input_is_visible_before_consumption_then_finalizes_under_the_same_id() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let path = dir.path().join("source.jsonl");
    std::fs::write(&path, "").unwrap();
    let params = lane_source(&ctx, dir.path(), "codex", &path, "session").await;
    let window = TmuxRuntime::window_name(params.lane_id);
    *backend.pane.lock().unwrap() =
        "• Answer already in flight.\n• Working (2s • esc to interrupt)".into();
    let client = ctx.open_session(ConnKind::Local).await;
    let mut events = ctx.events.subscribe();
    watch(&ctx, &client, params.clone()).await.unwrap();
    let first = transcript_event(&mut events, |v| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["partial"] == true)
    })
    .await;
    let answer_id = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["partial"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    *backend.pane.lock().unwrap() = "• Answer already in flight.\n• Working (3s • esc to interrupt)\nQueued follow-up inputs, 2 questions".into();
    crate::rpc::dispatch(&ctx, &client, "agent.send_input", Some(json!({"lane_id":params.lane_id,"window":window,"text":"Please answer the follow-up","enter":true}))).await.unwrap();
    let queued = transcript_event(&mut events, |v| {
        v["input_states"]
            .as_object()
            .is_some_and(|states| states.values().any(|v| v == "queued"))
    })
    .await;
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        0,
        "queued input must precede transcript consumption"
    );
    let id = queued["input_states"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let order = queued["order"].as_array().unwrap();
    assert!(
        order.iter().position(|v| v == &answer_id).unwrap()
            < order.iter().position(|v| v == &id).unwrap()
    );
    assert!(
        queued["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == id && r["role"] == "user" && r["partial"] == true)
    );
    std::fs::write(&path, format!("{}\n{}\n{}\n",
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Answer already in flight."}]}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"Please answer the follow-up"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Here is the follow-up answer."}]}})
    )).unwrap();
    *backend.pane.lock().unwrap() = String::new();
    let consumed = transcript_event(&mut events, |v| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == id && r["partial"] != true)
    })
    .await;
    assert!(consumed["input_states"].as_object().unwrap().is_empty());
    assert!(
        !consumed["removed_ids"]
            .as_array()
            .unwrap()
            .contains(&json!(id))
    );
    let order = consumed["order"].as_array().unwrap();
    assert!(
        order.iter().position(|v| v == &answer_id).unwrap()
            < order.iter().position(|v| v == &id).unwrap()
    );
    let next_answer = consumed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["text"] == "Here is the follow-up answer.")
        .unwrap()["id"]
        .clone();
    assert!(
        order.iter().position(|v| v == &id).unwrap()
            < order.iter().position(|v| v == &next_answer).unwrap()
    );
    let reopened = page(&ctx, &params).await.unwrap();
    assert!(
        reopened["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == id)
    );
    unwatch_all(&ctx, &client).await;
}

#[tokio::test]
async fn activity_changes_and_clears_without_creating_a_transcript_row() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let path = dir.path().join("source.jsonl");
    std::fs::write(&path, "").unwrap();
    let params = lane_source(&ctx, dir.path(), "claude-code", &path, "session").await;
    *backend.pane.lock().unwrap() = "+ Vibing... (22m 42s, 100.1k tokens, thought for 1s)".into();
    let client = ctx.open_session(ConnKind::Local).await;
    let mut events = ctx.events.subscribe();
    let initial = watch(&ctx, &client, params.clone()).await.unwrap();
    assert_eq!(initial["activity"]["elapsed_seconds"], 1362.0);
    assert!(initial["items"].as_array().unwrap().is_empty());
    let window = TmuxRuntime::window_name(params.lane_id);
    *backend.pane.lock().unwrap() = "+ Pondering... (23m 2s, 102k tokens, thought for 3s)".into();
    backend.senders.lock().unwrap()[&window]
        .1
        .send(ByteStreamEvent::Bytes(vec![b'x']))
        .unwrap();
    let changed = transcript_event(&mut events, |v| v["activity"]["verb"] == "Pondering").await;
    assert_eq!(changed["activity"]["token_count"], 102000);
    assert!(
        changed["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["kind"] == "status")
    );
    *backend.pane.lock().unwrap() = String::new();
    backend.senders.lock().unwrap()[&window]
        .1
        .send(ByteStreamEvent::Bytes(vec![b'x']))
        .unwrap();
    let cleared = transcript_event(&mut events, |v| v["activity"].is_null()).await;
    assert!(cleared["activity"].is_null());
    unwatch_all(&ctx, &client).await;
}

#[tokio::test]
async fn pending_input_does_not_consume_an_older_identical_message_and_is_shared_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = context(dir.path()).await;
    let path = dir.path().join("source.jsonl");
    let row = json!({"type":"user","message":{"content":"Try again"}});
    std::fs::write(&path, format!("{row}\n")).unwrap();
    let params = lane_source(&ctx, dir.path(), "claude-code", &path, "session").await;
    let window = TmuxRuntime::window_name(params.lane_id);
    let client = ctx.open_session(ConnKind::Local).await;
    crate::rpc::dispatch(
        &ctx,
        &client,
        "agent.send_input",
        Some(json!({"lane_id":params.lane_id,"text":"Try again","enter":true})),
    )
    .await
    .unwrap();
    let initial = watch(&ctx, &client, params.clone()).await.unwrap();
    let id = initial["input_states"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    assert_eq!(initial["input_states"][&id], "sent");
    assert_eq!(
        initial["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["role"] == "user")
            .count(),
        2
    );
    let second = ctx.open_session(ConnKind::Local).await;
    let reopened = watch(&ctx, &second, params.clone()).await.unwrap();
    assert_eq!(reopened["input_states"], initial["input_states"]);
    // Literal composer edits are not successful submissions.
    crate::rpc::dispatch(
        &ctx,
        &client,
        "agent.send_input",
        Some(json!({"lane_id":params.lane_id,"text":"not submitted","enter":false})),
    )
    .await
    .unwrap();
    let src = source(&ctx, &params).await.unwrap();
    let mut rows = Vec::new();
    let mut order = Vec::new();
    let states = ctx
        .transcript_inputs
        .append(&window, &src, "", &mut rows, &mut order);
    assert_eq!(states.as_object().unwrap().len(), 1);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(format!("{row}\n").as_bytes())
        .unwrap();
    let consumed = page(&ctx, &params).await.unwrap();
    assert_eq!(consumed["items"][1]["id"], id);
    assert_ne!(consumed["items"][0]["id"], id);
    unwatch_all(&ctx, &client).await;
    unwatch_all(&ctx, &second).await;
}

#[tokio::test]
async fn conversation_price_table_is_shared_until_configuration_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = context(dir.path()).await;
    let first = ctx.transcript_cache.price_table(&ctx).await.unwrap();
    let second = ctx.transcript_cache.price_table(&ctx).await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    ctx.config.write().await.usage.refresh_prices = true;
    let changed = ctx.transcript_cache.price_table(&ctx).await.unwrap();
    assert!(!Arc::ptr_eq(&first, &changed));
}

#[tokio::test]
async fn one_subscription_consuming_input_does_not_remove_it_from_a_lagging_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = context(dir.path()).await;
    let path = dir.path().join("source.jsonl");
    std::fs::write(&path, "").unwrap();
    let params = lane_source(&ctx, dir.path(), "claude-code", &path, "session").await;
    let window = TmuxRuntime::window_name(params.lane_id);
    let source = source(&ctx, &params).await.unwrap();
    let ticket = prepare_input(&ctx, params.lane_id, &window, "Question")
        .await
        .unwrap();
    ctx.transcript_inputs.sent(&ctx, &window, ticket);
    let mut pending = Vec::new();
    let mut order = Vec::new();
    ctx.transcript_inputs
        .append(&window, &source, "", &mut pending, &mut order);
    let id = pending[0].id.clone().unwrap();
    std::fs::write(
        &path,
        format!(
            "{}\n",
            json!({"type":"user","message":{"content":"Question"}})
        ),
    )
    .unwrap();
    let fast = page(&ctx, &params).await.unwrap();
    assert_eq!(fast["items"][0]["id"], id);
    // The second subscription has not received its history worker result yet.
    let mut slow = repomon_core::agent::conversation::Update::default();
    let mut states =
        ctx.transcript_inputs
            .append(&window, &source, "", &mut slow.items, &mut slow.order);
    assert!(slow.items.is_empty());
    retain_pending_until_consumed(&pending, &Value::Null, &mut slow, &mut states);
    assert_eq!(slow.items[0].id.as_deref(), Some(id.as_str()));
    assert_eq!(states[&id], "sent");
    // Once its durable upsert arrives, no pending duplicate remains.
    slow.items = serde_json::from_value(fast["items"].clone()).unwrap();
    slow.order = vec![id.clone()];
    let mut states = json!({});
    retain_pending_until_consumed(&pending, &Value::Null, &mut slow, &mut states);
    assert_eq!(slow.items.len(), 1);
    assert_ne!(slow.items[0].partial, Some(true));
    assert!(states.as_object().unwrap().is_empty());
}
