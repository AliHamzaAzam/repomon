//! Selector regressions use one multi-kind lane with backend identities, including null sessions.
use super::round5_tests::{context, lane_source, user_record};
use super::*;
use crate::conn::ConnKind;
use repomon_core::SessionBackend;
use repomon_core::agent::WindowMeta;

#[tokio::test]
async fn parsed_cache_isolates_windows_with_null_provider_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let file = dir.path().join("source.jsonl");
    std::fs::write(&file, user_record(1, "codex")).unwrap();
    let base = lane_source(&ctx, dir.path(), "codex", &file, "ledger-session").await;
    let mut sources = Vec::new();
    for slot in [2, 3] {
        let window = format!("lane-{}-{slot}", base.lane_id);
        backend.metas.lock().unwrap().push(WindowMeta {
            name: window.clone(),
            wid: slot,
            session: None,
            agent_kind: Some("codex".into()),
        });
        let params: Params =
            serde_json::from_value(json!({"lane_id":base.lane_id,"window":window})).unwrap();
        let source = resolve_source(&ctx, &params, false).await.unwrap();
        assert_eq!(source.window, window);
        sources.push(source);
    }
    // Even identical inferred files/provider IDs cannot merge two live windows.
    assert_eq!(sources[0].path, sources[1].path);
    assert_eq!(sources[0].session, sources[1].session);
    let first = ctx.transcript_cache.entry(&sources[0]);
    let shared = ctx.transcript_cache.entry(&sources[0]);
    let other = ctx.transcript_cache.entry(&sources[1]);
    assert!(Arc::ptr_eq(&first, &shared));
    assert!(!Arc::ptr_eq(&first, &other));
    for source in [&sources[0], &sources[0], &sources[1]] {
        read_page(&ctx, source.clone(), None).await.unwrap();
    }
    assert_eq!(ctx.transcript_cache.scans.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn explicit_window_rejects_foreign_sessions_for_every_kind_before_fallback_or_watch_replacement()
 {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let file = dir.path().join("source.jsonl");
    std::fs::write(&file, user_record(1, "claude-code")).unwrap();
    let base = lane_source(&ctx, dir.path(), "claude-code", &file, "claude-session").await;
    let client = ctx.open_session(ConnKind::Local).await;
    for (slot, kind) in ["claude-code", "codex", "antigravity", "opencode", "hermes"]
        .into_iter()
        .enumerate()
    {
        let window = format!("lane-{}-{}", base.lane_id, slot + 2);
        let own = format!("{kind}-own");
        backend.metas.lock().unwrap().push(WindowMeta {
            name: window.clone(),
            wid: slot as u64 + 2,
            session: Some(own.clone()),
            agent_kind: Some(kind.into()),
        });
        for method in ["agent.transcript_page", "agent.transcript_watch"] {
            let error = crate::rpc::dispatch(
                &ctx,
                &client,
                method,
                Some(json!({"lane_id":base.lane_id,"window":window,"session_id":"foreign-claude"})),
            )
            .await
            .unwrap_err();
            assert_eq!(error.code, -32602, "{kind} {method}: {error:?}");
            assert!(client.transcript_watches.lock().await.is_empty());
        }
        let params: Params = serde_json::from_value(
            json!({"lane_id":base.lane_id,"window":window,"session_id":own}),
        )
        .unwrap();
        let actual = resolve_source(&ctx, &params, false).await.unwrap();
        assert_eq!(actual.kind, kind);
        assert_eq!(actual.session.as_deref(), Some(own.as_str()));
        let mut wrong_kind = params.clone();
        wrong_kind.kind = Some("other-kind".into());
        assert!(matches!(
            resolve_source(&ctx, &wrong_kind, false).await,
            Err(TranscriptError::InvalidParams(_))
        ));
        // With no stamped session, a foreign id must not select a different lane session.
        backend.metas.lock().unwrap().last_mut().unwrap().session = None;
        let mut mismatch = params.clone();
        mismatch.session_id = Some("foreign-claude".into());
        let unresolved = resolve_source(&ctx, &mismatch, false).await.unwrap();
        assert!(
            unresolved.path.is_none() && unresolved.session.is_none(),
            "{kind}"
        );
        let mut window_only = params;
        window_only.session_id = None;
        assert_eq!(
            resolve_source(&ctx, &window_only, false)
                .await
                .unwrap()
                .kind,
            kind
        );
    }
}

#[tokio::test]
async fn bound_window_session_uses_its_kind_ledger_identity_and_validates_supplied_id() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = context(dir.path()).await;
    let file = dir.path().join("agy.jsonl");
    std::fs::write(&file, "{}\n").unwrap();
    let mut p = lane_source(&ctx, dir.path(), "antigravity", &file, "agy-session").await;
    p.window = Some(TmuxRuntime::window_name(p.lane_id));
    let src = resolve_source(&ctx, &p, false).await.unwrap();
    assert_eq!(src.session.as_deref(), Some("agy-session"));
    assert_eq!(src.path, Some(file));
    p.session_id = Some("claude-session".into());
    assert!(matches!(
        resolve_source(&ctx, &p, false).await,
        Err(TranscriptError::InvalidParams(_))
    ));
    p.session_id = None;
    assert_eq!(
        resolve_source(&ctx, &p, false)
            .await
            .unwrap()
            .session
            .as_deref(),
        Some("agy-session")
    );
}

/// Reads only the explicitly supplied live file, never copies or ingests its directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual read-only real corpus benchmark; C1_REAL_TRANSCRIPT must be explicit"]
async fn real_corpus_first_watch_benchmark() {
    let path = PathBuf::from(std::env::var("C1_REAL_TRANSCRIPT").expect("explicit read-only file"));
    let session = path.file_stem().unwrap().to_str().unwrap();
    let snapshot = std::fs::metadata(&path).unwrap().len();
    let source = Source {
        window: "lane-1".into(),
        kind: "claude-code".into(),
        path: Some(path.clone()),
        session: Some(session.into()),
    };
    let mut scan_times = Vec::new();
    let mut watch_times = Vec::new();
    for trial in 0..6 {
        let start = std::time::Instant::now();
        let parsed = scan_page(&source, Some(snapshot)).unwrap();
        let page_ms = start.elapsed().as_secs_f64() * 1000.0;
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = context(dir.path()).await;
        let mut p = lane_source(&ctx, dir.path(), "claude-code", &path, session).await;
        p.window = Some(TmuxRuntime::window_name(p.lane_id));
        let client = ctx.open_session(ConnKind::Local).await;
        let before = std::fs::metadata(&path).unwrap().len();
        let start = std::time::Instant::now();
        let first = watch(&ctx, &client, p).await.unwrap();
        let watch_ms = start.elapsed().as_secs_f64() * 1000.0;
        let after = std::fs::metadata(&path).unwrap().len();
        println!(
            "REAL_FIRST_WATCH trial={trial} file={} snapshot_bytes={snapshot} before_bytes={before} after_bytes={after} page_ms={page_ms:.3} watch_ms={watch_ms:.3} page_start={} scanned_bytes={} items={} next_before={}",
            path.file_name().unwrap().to_string_lossy(),
            parsed.start,
            parsed.end - parsed.start,
            first["items"].as_array().unwrap().len(),
            first["next_before"]
        );
        if trial > 0 {
            scan_times.push(page_ms);
            watch_times.push(watch_ms);
        }
        unwatch_all(&ctx, &client).await;
    }
    scan_times.sort_by(f64::total_cmp);
    watch_times.sort_by(f64::total_cmp);
    println!(
        "REAL_MEDIAN bytes={snapshot} page_ms={:.3} watch_ms={:.3}",
        scan_times[2], watch_times[2]
    );
}

#[tokio::test]
async fn claude_pane_queue_tracks_each_input_then_reuses_ids_on_durable_consumption() {
    use super::round5_tests::transcript_event;
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let file = dir.path().join("queue.jsonl");
    std::fs::write(&file, "").unwrap();
    let p = lane_source(&ctx, dir.path(), "claude-code", &file, "queue-session").await;
    let window = TmuxRuntime::window_name(p.lane_id);
    let client = ctx.open_session(ConnKind::Local).await;
    let mut events = ctx.events.subscribe();
    watch(&ctx, &client, p.clone()).await.unwrap();
    let texts = [
        "First message",
        "Second message",
        "Third message",
        "Still waiting\n\nAttached file: \"/a path/file.png\"",
    ];
    for text in texts {
        crate::rpc::dispatch(
            &ctx,
            &client,
            "agent.send_input",
            Some(json!({"lane_id":p.lane_id,"window":window,"text":text,"enter":true})),
        )
        .await
        .unwrap();
    }
    *backend.pane.lock().unwrap() =
        include_str!("../../repomon-core/src/agent/fixtures/claude_queue_operator_2026_09_10.txt")
            .into();
    let queued = transcript_event(&mut events, |v| {
        v["input_states"].as_object().is_some_and(|s| {
            s.values().filter(|v| **v == "consumed").count() == 3
                && s.values().filter(|v| **v == "queued").count() == 1
        })
    })
    .await;
    assert_eq!(std::fs::metadata(&file).unwrap().len(), 0);
    let ids: Vec<_> = queued["input_states"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(ids.len(), 4);
    let items = queued["items"].as_array().unwrap();
    assert!(items.iter().all(|row| row["role"] != "assistant"
        || !row["text"].as_str().unwrap().contains("Still waiting")));
    let assistant = items.iter().find(|r| r["role"] == "assistant").unwrap()["id"]
        .as_str()
        .unwrap();
    let order = queued["order"].as_array().unwrap();
    for id in &ids {
        if queued["input_states"][id] == "consumed" {
            assert!(
                order.iter().position(|v| v == id).unwrap()
                    < order.iter().position(|v| v == assistant).unwrap()
            );
        }
    }
    let records = texts
        .iter()
        .map(|text| format!("{}\n", json!({"type":"user","message":{"content":text}})))
        .collect::<String>();
    std::fs::write(&file, records).unwrap();
    *backend.pane.lock().unwrap() = String::new();
    let consumed = transcript_event(&mut events, |v| {
        v["input_states"].as_object().is_some_and(|s| s.is_empty())
            && v["items"]
                .as_array()
                .is_some_and(|rows| rows.iter().filter(|r| r["role"] == "user").count() == 4)
    })
    .await;
    for id in ids {
        assert!(
            consumed["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["id"] == id && r["partial"] != true)
        );
    }
    unwatch_all(&ctx, &client).await;
}

#[tokio::test]
async fn invalid_selector_preserves_existing_watch_and_historical_session_only_reads() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let file = dir.path().join("source.jsonl");
    std::fs::write(&file, user_record(1, "claude-code")).unwrap();
    let mut p = lane_source(&ctx, dir.path(), "claude-code", &file, "old-session").await;
    // Historical/session-only source is still accepted without a live window selector.
    assert_eq!(
        page(&ctx, &p).await.unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    p.window = Some(TmuxRuntime::window_name(p.lane_id));
    let client = ctx.open_session(ConnKind::Local).await;
    watch(&ctx, &client, p.clone()).await.unwrap();
    let tag = backend.senders.lock().unwrap()[p.window.as_ref().unwrap()].0;
    p.session_id = Some("foreign".into());
    assert!(matches!(
        watch(&ctx, &client, p.clone()).await,
        Err(TranscriptError::InvalidParams(_))
    ));
    assert_eq!(client.transcript_watches.lock().await.len(), 1);
    assert_eq!(
        backend.senders.lock().unwrap()[p.window.as_ref().unwrap()].0,
        tag
    );
    p.session_id = Some(format!("win:{}", p.window.as_ref().unwrap()));
    assert!(resolve_source(&ctx, &p, false).await.is_ok());
    p.session_id = Some("win:lane-999".into());
    assert!(matches!(
        resolve_source(&ctx, &p, false).await,
        Err(TranscriptError::InvalidParams(_))
    ));
    unwatch_all(&ctx, &client).await;
}

#[tokio::test]
async fn old_identical_prompt_and_composer_draft_do_not_consume_new_input() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let file = dir.path().join("source.jsonl");
    std::fs::write(&file, "").unwrap();
    let p = lane_source(&ctx, dir.path(), "claude-code", &file, "session").await;
    let window = TmuxRuntime::window_name(p.lane_id);
    *backend.pane.lock().unwrap() = "❯ Repeat\n⏺ Old answer\n────\n❯ Repeat\n────".into();
    let ticket = prepare_input(&ctx, p.lane_id, &window, "Repeat")
        .await
        .unwrap();
    ctx.transcript_inputs.sent(&ctx, &window, ticket);
    let src = resolve_source(&ctx, &p, false).await.unwrap();
    let mut rows = Vec::new();
    let mut order = Vec::new();
    let states = ctx.transcript_inputs.append(
        &window,
        &src,
        &backend.pane.lock().unwrap(),
        &mut rows,
        &mut order,
    );
    let id = rows[0].id.clone().unwrap();
    assert_eq!(states[&id], "sent");
    *backend.pane.lock().unwrap() =
        "❯ Repeat\n⏺ Old answer\n❯ Repeat\n⏺ New answer\n────\n❯ \n────".into();
    let states = ctx.transcript_inputs.append(
        &window,
        &src,
        &backend.pane.lock().unwrap(),
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert_eq!(states[&id], "consumed");
    let states = ctx
        .transcript_inputs
        .append(&window, &src, "", &mut Vec::new(), &mut Vec::new());
    assert_eq!(
        states[&id], "consumed",
        "observed consumption survives scrolling off pane"
    );
}

#[tokio::test]
async fn old_stamps_and_unknown_ages_return_live_pane_without_history() {
    for kind in ["claude-code", "codex", "antigravity", "opencode"] {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, backend) = context(dir.path()).await;
        let file = dir.path().join("old.jsonl");
        std::fs::write(&file, user_record(1, kind)).unwrap();
        let mut p = lane_source(&ctx, dir.path(), kind, &file, "old-session").await;
        p.window = Some(TmuxRuntime::window_name(p.lane_id));
        *backend.pane.lock().unwrap() = "Live pane only".into();
        // The ledger's session start is older than this new window, even with a bad stamp.
        *backend.started.lock().unwrap() =
            Some(Some(chrono::Utc::now() + chrono::Duration::seconds(1)));
        let source = resolve_source(&ctx, &p, false).await.unwrap();
        assert!(source.path.is_none(), "{kind}");
        let client = ctx.open_session(ConnKind::Local).await;
        let page = watch(&ctx, &client, p.clone()).await.unwrap();
        assert!(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| i["kind"] == "terminal_block" || i["status_kind"] == "source_unavailable")
        );
        assert!(page["next_before"].is_null());
        unwatch_all(&ctx, &client).await;
        if kind == "claude-code" {
            let record = json!({"type":"user","timestamp":"2020-01-01T00:00:00Z","message":{"content":"old history"}});
            std::fs::write(&file, format!("{record}\n")).unwrap();
            assert!(resolve_source(&ctx, &p, true).await.unwrap().path.is_none());
            assert!(
                backend.metas.lock().unwrap()[0].session.is_none(),
                "bad stamp must heal"
            );
            backend
                .set_window_session(p.window.as_deref().unwrap(), "old-session")
                .unwrap();
        }
        // No creation evidence is also a successful pane fallback, even with a supplied ID.
        *backend.started.lock().unwrap() = Some(None);
        assert!(resolve_source(&ctx, &p, true).await.unwrap().path.is_none());
        // A fresh ledger row still cannot identify a window with no bound session.
        *backend.started.lock().unwrap() = Some(Some(chrono::DateTime::UNIX_EPOCH));
        backend.metas.lock().unwrap()[0].session = None;
        assert!(resolve_source(&ctx, &p, true).await.unwrap().path.is_none());
    }
}

#[tokio::test]
async fn every_unresolved_kind_keeps_named_reason_and_live_pane_excerpt() {
    for kind in [
        "claude-code",
        "codex",
        "antigravity",
        "opencode",
        "hermes",
        "cursor",
        "aider",
        "custom-fixture",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, backend) = context(dir.path()).await;
        let path = dir.path().join("missing");
        let mut p = lane_source(&ctx, dir.path(), kind, &path, "unbound").await;
        p.window = Some(TmuxRuntime::window_name(p.lane_id));
        p.session_id = None;
        backend.metas.lock().unwrap()[0].session = None;
        *backend.started.lock().unwrap() = Some(None);
        *backend.pane.lock().unwrap() = "A live reply remains available for this agent".into();
        let page = page(&ctx, &p).await.unwrap();
        let items = page["items"].as_array().unwrap();
        assert!(
            items
                .iter()
                .any(|r| r["status_kind"] == "source_unavailable"),
            "{kind}"
        );
        assert!(
            items.iter().any(|r| r["kind"] == "terminal_block"
                && r["text"].as_str().unwrap().contains("live reply")),
            "{kind}"
        );
        let client = ctx.open_session(ConnKind::Local).await;
        let watch = watch(&ctx, &client, p).await.unwrap();
        assert!(
            watch["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["kind"] == "terminal_block"),
            "watch {kind}"
        );
        unwatch_all(&ctx, &client).await;
    }
}
#[tokio::test]
async fn delivered_mail_is_structured_and_durable_id_consumption_clears_pinned_state() {
    for kind in ["claude-code", "codex", "antigravity"] {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = context(dir.path()).await;
        let path = dir.path().join("mail.jsonl");
        std::fs::write(&path, "").unwrap();
        let p = lane_source(&ctx, dir.path(), kind, &path, "mail-session").await;
        let window = TmuxRuntime::window_name(p.lane_id);
        let envelope = "[REPOMAIL id=fixture-mail from=lane-2/1 reply_to=none] Please review this change [fixture-mail] [END REPOMAIL]";
        let ticket = prepare_input(&ctx, p.lane_id, &window, envelope)
            .await
            .unwrap();
        ctx.transcript_inputs.sent(&ctx, &window, ticket);
        let src = resolve_source(&ctx, &p, false).await.unwrap();
        let mut pending = Vec::new();
        let mut order = Vec::new();
        let states = ctx
            .transcript_inputs
            .append(&window, &src, "", &mut pending, &mut order);
        let id = pending[0].id.clone().unwrap();
        assert_eq!(pending[0].kind.as_deref(), Some("mail"));
        assert_eq!(states[&id], "sent");
        // Provider normalizes whitespace and timestamps the record before injection acknowledgement.
        let normalized = envelope.replace("Please review", "Please   review");
        let at = "2026-09-11T09:00:00Z";
        let row = match kind {
            "claude-code" => json!({"type":"user","timestamp":at,"message":{"content":normalized}}),
            "codex" => {
                json!({"type":"response_item","timestamp":at,"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":normalized}]}})
            }
            _ => {
                json!({"source":"USER_EXPLICIT","created_at":at,"type":"USER_INPUT","content":normalized})
            }
        };
        std::fs::write(&path, format!("{row}\n")).unwrap();
        let consumed = page(&ctx, &p).await.unwrap();
        assert_eq!(consumed["items"][0]["id"], id);
        assert_eq!(consumed["items"][0]["mail"]["sender"], "lane-2/1");
        let mut items = Vec::new();
        let mut order = Vec::new();
        let states = ctx
            .transcript_inputs
            .append(&window, &src, "", &mut items, &mut order);
        assert!(states.as_object().unwrap().is_empty(), "{kind}");
        assert!(items.is_empty());
    }
}

#[tokio::test]
async fn hermes_and_aider_resolve_bound_sources_and_render_real_fixture_messages() {
    for kind in ["hermes", "aider"] {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = context(dir.path()).await;
        let (path, id) = if kind == "hermes" {
            let path = dir.path().join("state.db");
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch(include_str!(
                "../../repomon-core/src/usage_ledger/fixtures/hermes_conversation_v0.sql"
            ))
            .unwrap();
            (path, "hermes-fixture".to_string())
        } else {
            let path = dir.path().join(".aider.chat.history.md");
            std::fs::write(
                &path,
                include_str!(
                    "../../repomon-core/src/usage_ledger/fixtures/aider_conversation_v0.md"
                ),
            )
            .unwrap();
            let id =
                repomon_core::usage_ledger::aider::scan(&path, None, 0, ScanOptions::default())
                    .unwrap()
                    .sessions[0]
                    .session_id
                    .clone();
            (path, id)
        };
        let p = lane_source(&ctx, dir.path(), kind, &path, &id).await;
        let src = resolve_source(&ctx, &p, false).await.unwrap();
        assert_eq!(src.path.as_ref(), Some(&path));
        let first = page(&ctx, &p).await.unwrap();
        let rows = first["items"].as_array().unwrap();
        assert!(
            rows.iter().any(|r| r["role"] == "assistant"
                && r["text"]
                    .as_str()
                    .unwrap()
                    .contains("inspection is complete")),
            "{kind}"
        );
        assert!(
            !rows
                .iter()
                .any(|r| r["text"].as_str().unwrap().contains("Unrelated")),
            "{kind}"
        );
    }
}

#[test]
fn pane_mail_is_structured_without_losing_surrounding_excerpt() {
    let items = fallback_items(
        "cursor",
        "Before the mail\n[REPOMAIL id=receipt from=operator reply_to=none] Review the changes [receipt] [END REPOMAIL]\nAfter the mail",
        "lane-1",
    );
    assert!(
        items
            .iter()
            .any(|r| r.mail.as_ref().is_some_and(|m| m.sender == "operator")
                && r.text == "Review the changes")
    );
    assert!(
        items
            .iter()
            .any(|r| r.kind.as_deref() == Some("terminal_block")
                && r.text.contains("Before the mail"))
    );
    assert!(
        items
            .iter()
            .any(|r| r.kind.as_deref() == Some("terminal_block")
                && r.text.contains("After the mail"))
    );
    assert!(!items.iter().any(|r| r.text.contains("[REPOMAIL")));
}

/// Opt-in read-only provider audit. Only synthetic in-memory lane attribution is constructed.
#[test]
#[ignore = "manual local provider-format measurement"]
fn real_provider_chat_audit() {
    use repomon_core::usage_ledger::{FleetIndex, scan::scan_codex_rollout};
    let home = directories::BaseDirs::new().unwrap();
    let root = home.home_dir().join(".codex/sessions/2026/09");
    let mut files = 0;
    let mut attributed = 0;
    let mut messages = 0;
    for day in std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().as_ref() >= "09")
    {
        for file in std::fs::read_dir(day.path())
            .unwrap()
            .flatten()
            .filter(|f| f.path().extension().is_some_and(|e| e == "jsonl"))
        {
            let scan = scan_codex_rollout(&file.path(), 0).unwrap();
            let Some(session) = scan.sessions.first() else {
                continue;
            };
            let Some(cwd) = &session.cwd else {
                continue;
            };
            let index = FleetIndex::new(
                vec![(1, PathBuf::from(cwd))],
                vec![(7, 1, PathBuf::from(cwd))],
            );
            attributed += usize::from(index.attribute(Some(cwd)).lane_id == Some(7));
            files += 1;
            let parsed = scan_page(
                &Source {
                    window: "fixture-window".into(),
                    kind: "codex".into(),
                    path: Some(file.path()),
                    session: Some(session.session_id.clone()),
                },
                None,
            )
            .unwrap();
            messages += parsed
                .scan
                .transcript
                .iter()
                .filter(|r| matches!(r.item.kind.as_deref(), Some("user" | "assistant")))
                .count();
        }
    }
    println!(
        "CODEX files={files} cwd_attributed={attributed} latest_page_prose_rows={messages} monitor_identity=None (baseline CodexMonitor)"
    );
    let agy = home.home_dir().join(".gemini/antigravity-cli/brain");
    let mut files = 0;
    let mut messages = 0;
    for dir in std::fs::read_dir(agy).unwrap().flatten() {
        let path = dir.path().join(".system_generated/logs/transcript.jsonl");
        if !path.is_file() {
            continue;
        }
        let src = Source {
            window: "fixture-window".into(),
            kind: "antigravity".into(),
            path: Some(path),
            session: Some(dir.file_name().to_string_lossy().into()),
        };
        if let Ok(page) = scan_page(&src, None) {
            files += 1;
            messages += page
                .scan
                .transcript
                .iter()
                .filter(|r| matches!(r.item.kind.as_deref(), Some("user" | "assistant")))
                .count();
        }
    }
    println!("ANTIGRAVITY exported_files={files} latest_page_prose_rows={messages}");
    let path = repomon_core::usage_ledger::hermes::database_path();
    let sessions = repomon_core::usage_ledger::hermes::sessions(&path).unwrap();
    let mut rows = 0;
    for session in &sessions {
        rows += repomon_core::usage_ledger::hermes::scan(
            &path,
            0,
            &session.session_id,
            ScanOptions {
                collect_transcript: true,
                before_offset: None,
            },
        )
        .unwrap()
        .transcript
        .len();
    }
    println!(
        "HERMES sessions={} cwd_missing={} structured_rows={rows}",
        sessions.len(),
        sessions.iter().filter(|s| s.cwd.is_none()).count()
    );
    assert!(files > 0 && messages > 0 && rows > 0);
}

#[tokio::test]
async fn pane_echo_does_not_consume_pending_mail_without_provider_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, backend) = context(dir.path()).await;
    let p = lane_source(
        &ctx,
        dir.path(),
        "hermes",
        &dir.path().join("missing"),
        "missing",
    )
    .await;
    *backend.started.lock().unwrap() = Some(None);
    let window = TmuxRuntime::window_name(p.lane_id);
    let envelope =
        "[REPOMAIL id=waiting from=operator reply_to=none] Still waiting [waiting] [END REPOMAIL]";
    let ticket = prepare_input(&ctx, p.lane_id, &window, envelope)
        .await
        .unwrap();
    ctx.transcript_inputs.sent(&ctx, &window, ticket);
    let src = resolve_source(&ctx, &p, false).await.unwrap();
    let mut rows = fallback_items("hermes", envelope, &window);
    ctx.transcript_inputs.reconcile(&window, &src, &mut rows);
    assert!(!rows.iter().any(|row| row.mail.is_some()));
    let mut order = Vec::new();
    let states = ctx
        .transcript_inputs
        .append(&window, &src, envelope, &mut rows, &mut order);
    assert_eq!(states.as_object().unwrap().values().next().unwrap(), "sent");
    assert_eq!(rows.iter().filter(|row| row.mail.is_some()).count(), 1);
}
