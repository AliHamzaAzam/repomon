//! Conversation subscriptions share the terminal byte feed and reconcile pane previews with ledger
//! scanner rows. No CLI is spawned and no input is injected by this module.
use crate::{Ctx, conn::ConnSession};
use repomon_core::agent::{
    CaptureOpts, TmuxRuntime,
    conversation::{ConversationStream, pane_content, pane_items},
    conversation_activity::pane_activity,
    prompt,
};
use repomon_core::model::{LaneId, TranscriptItem};
use repomon_core::usage_ledger::{
    FleetIndex,
    scan::{
        ScanOptions, SourceScan, scan_antigravity_transcript_with_options, scan_claude_transcript,
        scan_claude_transcript_with_options, scan_codex_rollout_with_options,
        scan_opencode_db_with_options,
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

#[path = "transcript_inputs.rs"]
mod inputs;
pub use inputs::{Inputs, prepare_input};

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Source {
    kind: String,
    path: Option<PathBuf>,
    session: Option<String>,
}

/// One metadata identity for ordinary files or a SQLite database plus its WAL.
type FileStamp = Option<(u64, Option<std::time::SystemTime>, u64, u64)>;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint(Vec<FileStamp>);
fn fingerprint(source: &Source) -> Fingerprint {
    let mut paths: Vec<_> = source.path.iter().cloned().collect();
    if source.kind == "opencode" {
        if let Some(path) = &source.path {
            paths.push(PathBuf::from(format!("{}-wal", path.display())));
        }
    }
    Fingerprint(
        paths
            .iter()
            .map(|path| {
                std::fs::metadata(path).ok().map(|meta| {
                    #[cfg(unix)]
                    let identity = {
                        use std::os::unix::fs::MetadataExt;
                        (meta.dev(), meta.ino())
                    };
                    #[cfg(not(unix))]
                    let identity = (0, 0);
                    (meta.len(), meta.modified().ok(), identity.0, identity.1)
                })
            })
            .collect(),
    )
}

#[derive(Default)]
struct CachedSource {
    stamp: Option<Fingerprint>,
    pages: std::collections::HashMap<Option<u64>, Arc<ParsedPage>>,
}
struct ParsedPage {
    scan: SourceScan,
    start: u64,
    end: u64,
    next_before: Option<u64>,
    older_message_count: Option<u64>,
}
#[derive(Default)]
struct CacheEntry {
    parsed: std::sync::Mutex<CachedSource>,
    cost_revision: AtomicU64,
}
#[derive(PartialEq)]
struct PriceKey {
    refresh: bool,
    overrides: std::collections::HashMap<String, repomon_core::pricing::PriceOverride>,
    stamp: Option<(u64, Option<std::time::SystemTime>)>,
}
type CachedPrices = Option<(PriceKey, Arc<repomon_core::pricing::PriceTable>)>;
/// Shared across connections, single-flight per source, with bounded source retention.
#[derive(Default)]
pub struct Cache {
    entries: std::sync::Mutex<std::collections::HashMap<Source, Arc<CacheEntry>>>,
    prices: tokio::sync::Mutex<CachedPrices>,
    #[cfg(test)]
    scans: AtomicU64,
}
impl Cache {
    async fn price_table(
        &self,
        ctx: &Arc<Ctx>,
    ) -> Result<Arc<repomon_core::pricing::PriceTable>, String> {
        let config = ctx.config.read().await.usage.clone();
        let key = PriceKey {
            refresh: config.refresh_prices,
            overrides: config.price_overrides.clone(),
            stamp: config
                .refresh_prices
                .then(|| std::fs::metadata(crate::usage_ingest::price_cache_path()).ok())
                .flatten()
                .map(|m| (m.len(), m.modified().ok())),
        };
        let mut cached = self.prices.lock().await;
        if let Some((previous, table)) = &*cached {
            if previous == &key {
                return Ok(table.clone());
            }
        }
        let table = Arc::new(
            tokio::task::spawn_blocking(move || crate::usage_ingest::build_price_table(config))
                .await
                .map_err(|e| e.to_string())?,
        );
        *cached = Some((key, table.clone()));
        Ok(table)
    }
    fn entry(&self, source: &Source) -> Arc<CacheEntry> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= 16 && !entries.contains_key(source) {
            // Open watches pin their entry, so late ingestion/pricing revisions remain observable.
            entries.retain(|_, entry| Arc::strong_count(entry) > 1);
        }
        entries.entry(source.clone()).or_default().clone()
    }
    /// Ingestion touches only retained conversation sources, with no parsing or row construction.
    pub fn usage_changed(&self, path: &str) {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        for (source, entry) in entries.iter() {
            if source
                .path
                .as_ref()
                .is_some_and(|p| p.to_string_lossy() == path)
            {
                entry.cost_revision.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    pub fn prices_changed(&self) {
        for entry in self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            entry.cost_revision.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn cost_revision(&self, source: &Source) -> u64 {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(source)
            .map_or(0, |entry| entry.cost_revision.load(Ordering::Relaxed))
    }
    fn complete_tools(
        &self,
        source: &Source,
        rows: &mut [repomon_core::usage_ledger::scan::TranscriptEntry],
    ) {
        let entry = self.entry(source);
        let entry = entry.parsed.lock().unwrap_or_else(|e| e.into_inner());
        for row in rows
            .iter_mut()
            .filter(|r| r.item.kind.as_deref() == Some("tool_call"))
        {
            if let Some(settled) = entry
                .pages
                .values()
                .flat_map(|page| &page.scan.transcript)
                .find(|other| {
                    other.item.id == row.item.id
                        && matches!(
                            other.item.status,
                            Some(
                                repomon_core::model::ToolCallStatus::Ok
                                    | repomon_core::model::ToolCallStatus::Error
                            )
                        )
                })
            {
                row.item.status = settled.item.status;
                row.item.result_summary = settled.item.result_summary.clone();
            }
        }
    }
    fn get(&self, source: &Source, before: Option<u64>) -> Result<Arc<ParsedPage>, String> {
        let entry = self.entry(source);
        let mut entry = entry.parsed.lock().unwrap_or_else(|e| e.into_inner());
        let stamp = fingerprint(source);
        if entry.stamp.as_ref() != Some(&stamp) {
            entry.pages.clear();
            entry.stamp = None;
        }
        if let Some(value) = entry.pages.get(&before) {
            return Ok(value.clone());
        }
        #[cfg(test)]
        self.scans.fetch_add(1, Ordering::Relaxed);
        let value = Arc::new(scan_page(source, before)?);
        if fingerprint(source) == stamp {
            if entry.pages.len() >= 8 {
                entry.pages.clear();
            }
            // Do not retain pathological single-record pages beyond the ordinary cache budget.
            if value
                .scan
                .transcript
                .iter()
                .map(|r| r.item.text.len() + r.item.result_summary.as_ref().map_or(0, String::len))
                .sum::<usize>()
                <= 2 * 1024 * 1024
            {
                entry.pages.insert(before, value.clone());
                entry.stamp = Some(stamp);
            }
        }
        Ok(value)
    }
}

async fn source(ctx: &Arc<Ctx>, p: &Params) -> Result<Source, String> {
    resolve_source(ctx, p, true).await
}
async fn resolve_source(ctx: &Ctx, p: &Params, discover: bool) -> Result<Source, String> {
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
    if !matches!(
        kind.as_str(),
        "claude-code" | "codex" | "antigravity" | "opencode"
    ) {
        return Ok(Source {
            kind,
            path: None,
            session,
        });
    }
    let known = ctx
        .store
        .conversation_source(p.lane_id, kind.clone(), session.clone(), started)
        .await
        .map_err(|e| e.to_string())?;
    if let Some((path, session)) = known.filter(|(path, _)| PathBuf::from(path).is_file()) {
        return Ok(Source {
            kind,
            path: Some(path.into()),
            session: Some(session),
        });
    }
    if !discover {
        return Ok(Source {
            kind,
            path: None,
            session,
        });
    }
    let scan_kind = kind.clone();
    let lane = p.lane_id;
    let selected_session = session.clone();
    let found = tokio::task::spawn_blocking(move || {
        if scan_kind == "claude-code" {
            if let Some(id) = &session {
                return repomon_core::agent::claude::transcript_path_for_session(&cwd, id)
                    .map(|p| (p, Some(id.clone())));
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
                        .and_then(|scan| scan.sessions.iter().filter_map(|r| r.first_at).min())
                        .is_some_and(|at| at >= start)
                })
            })
            .map(|s| (s.manifest_path.clone(), session.clone()));
        }
        let index = FleetIndex::new(Vec::new(), vec![(lane, meta.repo_id, cwd)]);
        crate::usage_ingest::discover_sources(512)
            .into_iter()
            .filter(|s| {
                matches!(
                    (scan_kind.as_str(), s.kind),
                    ("codex", crate::usage_ingest::SourceKind::Codex)
                        | ("antigravity", crate::usage_ingest::SourceKind::Antigravity)
                        | ("opencode", crate::usage_ingest::SourceKind::OpenCode)
                )
            })
            .find_map(|s| {
                let scan = crate::usage_ingest::scan_source(&s, 0).ok()?;
                scan.sessions
                    .iter()
                    .filter(|row| {
                        index.attribute(row.cwd.as_deref()).lane_id == Some(lane)
                            && session.as_ref().is_none_or(|id| &row.session_id == id)
                            && started
                                .is_none_or(|start| row.first_at.is_some_and(|at| at >= start))
                    })
                    .max_by_key(|row| row.last_at)
                    .map(|row| (s.path, Some(row.session_id.clone())))
            })
    })
    .await
    .map_err(|e| e.to_string())?;
    let (path, session) = found.map_or((None, selected_session), |(path, session)| {
        (Some(path), session)
    });
    Ok(Source {
        kind,
        path,
        session,
    })
}

#[cfg(test)]
fn scan(source: &Source) -> Result<SourceScan, String> {
    scan_range(source, 0, None)
}

fn scan_range(source: &Source, from: u64, before: Option<u64>) -> Result<SourceScan, String> {
    let Some(path) = &source.path else {
        return Ok(SourceScan::default());
    };
    let options = ScanOptions {
        collect_transcript: true,
        before_offset: before,
    };
    match source.kind.as_str() {
        "claude-code" => scan_claude_transcript_with_options(path, from, None, options),
        "codex" => scan_codex_rollout_with_options(path, from, options),
        "antigravity" => {
            scan_antigravity_transcript_with_options(path, from, "gemini-3", None, options)
        }
        "opencode" => scan_opencode_db_with_options(path, from, source.session.as_deref(), options),
        _ => return Ok(SourceScan::default()),
    }
    .map_err(|e| e.to_string())
}

fn scan_page(source: &Source, before: Option<u64>) -> Result<ParsedPage, String> {
    use repomon_core::usage_ledger::scan::{jsonl_page_start, opencode_page_bounds};
    let Some(path) = &source.path else {
        return Ok(ParsedPage {
            scan: SourceScan::default(),
            start: 0,
            end: 0,
            next_before: None,
            older_message_count: None,
        });
    };
    if source.kind == "opencode" {
        let session = source
            .session
            .as_deref()
            .ok_or("OpenCode conversation requires a session")?;
        let end = before.unwrap_or(i64::MAX as u64);
        let (start, older) =
            opencode_page_bounds(path, session, end, 200).map_err(|e| e.to_string())?;
        return Ok(ParsedPage {
            scan: scan_range(source, start.saturating_sub(1), Some(end))?,
            start,
            end,
            next_before: (older > 0).then_some(start),
            older_message_count: Some(older),
        });
    }
    let end = before
        .unwrap_or(u64::MAX)
        .min(std::fs::metadata(path).map_err(|e| e.to_string())?.len());
    let mut scan_end = end;
    loop {
        let start = jsonl_page_start(path, scan_end, PAGE_BYTES).map_err(|e| e.to_string())?;
        let scan = scan_range(source, start, Some(scan_end))?;
        if !scan.transcript.is_empty() || !scan.events.is_empty() || start == 0 {
            return Ok(ParsedPage {
                scan,
                start,
                end,
                next_before: (start > 0).then_some(start),
                older_message_count: None,
            });
        }
        scan_end = start;
    }
}

#[cfg(test)]
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
    let session = source.session.clone();
    let cache_ctx = ctx.clone();
    let (parsed, mut rows) = tokio::task::spawn_blocking(move || {
        let parsed = cache_ctx.transcript_cache.get(&source, before)?;
        let mut rows = parsed.scan.transcript.clone();
        cache_ctx
            .transcript_cache
            .complete_tools(&source, &mut rows);
        Ok::<_, String>((parsed, rows))
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Some(path) = &path {
        let events = ctx
            .store
            .conversation_cost_events(
                path.to_string_lossy().into(),
                session,
                parsed.start,
                parsed.end,
            )
            .await
            .map_err(|e| e.to_string())?;
        let table = if events.is_empty() {
            None
        } else {
            Some(ctx.transcript_cache.price_table(ctx).await?)
        };
        for event in events {
            if let Some(cost) = table
                .as_ref()
                .and_then(|table| table.cost(&event.model, event.at, &event.tokens()))
            {
                let mut item =
                    TranscriptItem::new("status", format!("Turn cost ${cost:.4}"), Some(event.at));
                item.id = Some(format!("cost:{}", event.source_offset));
                item.status_kind = Some("turn_cost".into());
                item.cost_usd = Some(cost);
                // A billable record can contain only hidden thinking. Its cost is still a row,
                // and participates in the same byte-offset paging as visible messages.
                rows.push(repomon_core::usage_ledger::scan::TranscriptEntry {
                    offset: event.source_offset as u64,
                    item,
                });
            }
        }
        rows.sort_by_key(|row| row.offset);
    }
    let mut items: Vec<_> = rows.into_iter().map(|row| row.item).collect();
    let next_before = parsed.next_before;
    if let Some(path) = path {
        let source_key = path.to_string_lossy();
        for item in &mut items {
            item.id = item.id.take().map(|id| format!("{source_key}:{id}"));
        }
    }
    Ok(
        json!({ "page_count": items.len(), "items": items, "next_before": next_before, "older_message_count": parsed.older_message_count }),
    )
}

pub async fn page(ctx: &Arc<Ctx>, p: &Params) -> Result<Value, String> {
    let src = source(ctx, p).await?;
    if src.path.is_none() && p.before.is_none() {
        let mut p = p.clone();
        p.kind = Some(src.kind);
        return capture_page(ctx, &p).await;
    }
    let mut value = read_page(ctx, src.clone(), p.before).await?;
    let window = p
        .window
        .clone()
        .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
    let mut rows: Vec<TranscriptItem> =
        serde_json::from_value(value["items"].clone()).unwrap_or_default();
    let replaced = ctx.transcript_inputs.reconcile(&window, &src, &mut rows);
    value["removed_ids"] = json!(replaced);
    value["order"] = json!(rows.iter().filter_map(|r| r.id.clone()).collect::<Vec<_>>());
    value["items"] = json!(rows);
    Ok(value)
}

async fn capture_page(ctx: &Arc<Ctx>, p: &Params) -> Result<Value, String> {
    let backend = ctx.backend.clone();
    let window = p
        .window
        .clone()
        .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
    let identity = window.clone();
    let pane =
        tokio::task::spawn_blocking(move || backend.capture_named(&window, CaptureOpts::last(100)))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    let mut item = TranscriptItem::new(
        "terminal_block",
        pane_content(p.kind.as_deref().unwrap_or("other"), &pane),
        None,
    );
    item.id = Some(format!("pane:{identity}"));
    let page_count = usize::from(!item.text.trim().is_empty());
    Ok(
        json!({"items": if item.text.trim().is_empty() { Vec::new() } else { vec![item] }, "next_before":null, "page_count": page_count, "older_message_count": null}),
    )
}

// A second subscription may resolve a shared input before this watch's history worker catches
// up. Keep its existing pending row until this watch can upsert the durable alias, with no gap.
fn retain_pending_until_consumed(
    previous: &[TranscriptItem],
    update: &mut repomon_core::agent::conversation::Update,
    states: &mut Value,
) {
    for item in previous {
        let Some(id) = &item.id else {
            continue;
        };
        if !update.order.contains(id) {
            update.order.push(id.clone());
            update.items.push(item.clone());
            states[id] = json!("sent");
        }
    }
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
    let mut initial_source = resolve_source(ctx, &p, false).await?;
    let mut source_cache = ctx.transcript_cache.entry(&initial_source);
    let initial_signature = fingerprint(&initial_source);
    let initial_cost_revision = source_cache.cost_revision.load(Ordering::Relaxed);
    let mut initial = if initial_source.path.is_none() {
        let mut capture_params = p.clone();
        capture_params.kind = Some(initial_source.kind.clone());
        capture_page(ctx, &capture_params).await?
    } else {
        read_page(ctx, initial_source.clone(), None).await?
    };
    let capture_backend = ctx.backend.clone();
    let capture_window = window.clone();
    let initial_pane = tokio::task::spawn_blocking(move || {
        capture_backend.capture_named(&capture_window, CaptureOpts::last(100))
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default();
    let initial_activity = pane_activity(&initial_source.kind, &initial_pane);
    let mut initial_rows: Vec<TranscriptItem> =
        serde_json::from_value(initial["items"].clone()).unwrap_or_default();
    ctx.transcript_inputs
        .reconcile(&window, &initial_source, &mut initial_rows);
    initial["items"] = json!(initial_rows);
    let mut response = initial.clone();
    let mut response_rows = initial_rows.clone();
    let mut order: Vec<_> = response_rows.iter().filter_map(|r| r.id.clone()).collect();
    response["input_states"] = ctx.transcript_inputs.append(
        &window,
        &initial_source,
        &initial_pane,
        &mut response_rows,
        &mut order,
    );
    response["items"] = json!(response_rows);
    response["page_count"] = json!(response_rows.len());
    response["order"] = json!(order);
    response["activity"] = json!(initial_activity);
    let initial_pending: Vec<_> = response_rows
        .iter()
        .filter(|r| r.role == "user" && r.partial == Some(true))
        .cloned()
        .collect();
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
    let lane = p.lane_id;
    let task = tokio::spawn(async move {
        let mut state = ConversationStream::default();
        state.update(initial_rows, Vec::new(), false);
        let mut ticker = tokio::time::interval(Duration::from_millis(150));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut dirty = true;
        let mut activity = None;
        let mut cached = initial;
        let mut signature = Some(initial_signature);
        let mut cost_revision = initial_cost_revision;
        let mut ledger_dirty = source_cache.cost_revision.load(Ordering::Relaxed) != cost_revision;
        let mut stream_ended = false;
        let mut discovery = std::time::Instant::now() - Duration::from_secs(2);
        let mut reads = tokio::task::JoinSet::new();
        let mut discoveries = tokio::task::JoinSet::new();
        let mut previous_activity = initial_activity;
        let mut previous_order = Vec::new();
        let mut previous_inputs = Value::Null;
        let mut previous_pending = initial_pending;
        let mut input_source = initial_source.clone();
        loop {
            tokio::select! {
                Some(result) = reads.join_next(), if !reads.is_empty() => {
                    if let Ok((src, stamp, revision, Ok(value))) = result {
                        if src == initial_source {
                            cached = value;
                            signature = Some(stamp);
                            cost_revision = revision;
                            ledger_dirty = task_ctx.transcript_cache.cost_revision(&initial_source) != cost_revision;
                            dirty = true;
                        }
                    }
                }
                Some(result) = discoveries.join_next(), if !discoveries.is_empty() => {
                    if let Ok(Ok(src)) = result {
                        if src != initial_source {
                            initial_source = src;
                            source_cache = task_ctx.transcript_cache.entry(&initial_source);
                            signature = None;
                            dirty = true;
                        }
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(v) if v["params"]["window"] == task_window && v["method"] == crate::pubsub::topic::AGENT_BYTES => {
                            dirty = true;
                            stream_ended = false;
                            activity = Some(std::time::Instant::now());
                        }
                        Ok(v) if v["method"] == inputs::INPUT_SENT && v["params"]["window"] == task_window => { dirty = true; },
                        Ok(v) if v["method"] == crate::pubsub::topic::USAGE_CHANGED => {
                            ledger_dirty |= source_cache.cost_revision.load(Ordering::Relaxed) != cost_revision;
                        }
                        Ok(v) if v["params"]["window"] == task_window && v["method"] == crate::pubsub::topic::AGENT_STREAM_CLOSED => { stream_ended = true; activity = None; dirty = true; },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { dirty = true; }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        _ => {}
                    }
                }
                _ = ticker.tick() => {
                    if discovery.elapsed() >= Duration::from_secs(2) && discoveries.is_empty() {
                        let ctx = task_ctx.clone();
                        let params = p.clone();
                        discoveries.spawn(async move { source(&ctx, &params).await });
                        discovery = std::time::Instant::now();
                    }
                    let src = initial_source.clone();
                    let kind = src.kind.clone();
                    let current = Some(fingerprint(&src));
                    if (current != signature || ledger_dirty) && reads.is_empty() {
                        let ctx = task_ctx.clone();
                        let revision = ctx.transcript_cache.cost_revision(&src);
                        let stamp = current.clone().unwrap();
                        reads.spawn(async move {
                            let result = read_page(&ctx, src.clone(), None).await;
                            (src, stamp, revision, result)
                        });
                    }
                    if !dirty && activity.is_none() { continue; }
                    dirty = false;
                    let backend = task_ctx.backend.clone();
                    let win = task_window.clone();
                    let pane = tokio::task::spawn_blocking(move || backend.capture_named(&win, CaptureOpts::last(100))).await;
                    let Ok(Ok(pane)) = pane else { continue; };
                    let structured_activity = if stream_ended { None } else { pane_activity(&kind, &pane) };
                    let active = !stream_ended && (structured_activity.as_ref().is_some_and(|a| a.verb.is_some()) || prompt::detect_active_spinner(&pane).is_some() || activity.is_some_and(|t| t.elapsed() < Duration::from_secs(2)));
                    if !active { activity = None; }
                    let mut live = pane_items(&kind, &pane);
                    if active && live.is_empty() && !matches!(kind.as_str(), "claude-code" | "codex") {
                        let mut item = TranscriptItem::new("terminal_block", pane_content(&kind, &pane), None);
                        item.partial = Some(true);
                        live.push(item);
                    }
                    let mut finals = serde_json::from_value(cached["items"].clone()).unwrap_or_default();
                    if initial_source.path.is_none() {
                        let mut excerpt = TranscriptItem::new("terminal_block", pane_content(&kind, &pane), None);
                        excerpt.id = Some(format!("pane:{task_window}"));
                        finals = if excerpt.text.trim().is_empty() { Vec::new() } else { vec![excerpt] };
                        live.retain(|i| matches!(i.kind.as_deref(), Some("dialog" | "status")));
                    }
                    let replaced = task_ctx.transcript_inputs.reconcile(&task_window, &initial_source, &mut finals);
                    let replaced: Vec<_> = replaced.into_iter().filter(|id| state.forget_replaced_user(id)).collect();
                    let mut update = state.update(finals, live, active);
                    update.removed_ids.extend(replaced);
                    let mut input_states = task_ctx.transcript_inputs.append(&task_window, &initial_source, &pane, &mut update.items, &mut update.order);
                    if input_source == initial_source {
                        retain_pending_until_consumed(&previous_pending, &mut update, &mut input_states);
                    }
                    previous_pending = update.items.iter().filter(|r| r.role == "user" && r.partial == Some(true)).cloned().collect();
                    input_source = initial_source.clone();
                    if let Some(previous) = previous_inputs.as_object() {
                        for id in previous.keys() {
                            if !update.order.contains(id) { update.removed_ids.push(id.clone()); }
                        }
                    }
                    if !update.items.is_empty() || !update.removed_ids.is_empty() || update.order != previous_order || structured_activity != previous_activity || input_states != previous_inputs {
                        task_ctx.broadcast(TOPIC, json!({ "lane_id": p.lane_id, "window": task_window,
                            "subscription_id": connection, "items": update.items, "removed_ids": update.removed_ids,
                            "next_before": cached["next_before"], "older_message_count": cached["older_message_count"],
                            "page_count": update.items.len(), "order": update.order,
                            "input_states": input_states, "activity": structured_activity }));
                        previous_order = update.order;
                        previous_activity = structured_activity;
                        previous_inputs = input_states;
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
    pub(super) struct ScriptedBackend {
        opens: AtomicU64,
        pub(super) pane: StdMutex<String>,
        pub(super) senders:
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

    pub(super) async fn next_items(rx: &mut tokio::sync::broadcast::Receiver<Value>) -> Value {
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
            // No daemon input or queue exists in this case. Both source records become visible
            // after the partial, reproducing delayed ingestion independently of follow-up queues.
            let user = if kind == "claude-code" {
                json!({"type":"user","message":{"content":"Please settle the transcript."}})
            } else {
                json!({"type":"event_msg","payload":{"type":"user_message","message":"Please settle the transcript."}})
            };
            std::fs::write(&path, format!("{user}\n{row}\n")).unwrap();
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
                    let user = event["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["role"] == "user")
                        .unwrap();
                    let order = event["order"].as_array().unwrap();
                    assert!(
                        order.iter().position(|id| id == &user["id"]).unwrap()
                            < order.iter().position(|id| id == &partial["id"]).unwrap()
                    );
                    assert!(event["input_states"].as_object().unwrap().is_empty());
                    println!(
                        "ORDER kind={kind} queue=none partial_before_source=true final_delay_ms={:.3} order=user,assistant same_id=true",
                        partial_at.elapsed().as_secs_f64() * 1000.0
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
                session: None,
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
                session: None,
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

#[cfg(test)]
mod benchmarks {
    use super::*;
    use repomon_core::{Config, Store};
    use std::time::Instant;

    /// Run with C1_BENCH_DIR pointing to synthetic fixtures generated by qa/c1-round5-generate.py.
    #[tokio::test]
    #[ignore = "manual warm-cache benchmark; needs synthetic C1_BENCH_DIR"]
    async fn conversation_warm_cache_benchmark() {
        let root =
            PathBuf::from(std::env::var("C1_BENCH_DIR").expect("synthetic fixture directory"));
        for kind in ["claude", "codex"] {
            for mib in [1, 10, 50] {
                let dir = tempfile::tempdir().unwrap();
                let ctx = Ctx::new_with_backend(
                    Store::open_in_memory().unwrap(),
                    Config::default(),
                    None,
                    dir.path().join("config"),
                    dir.path().join("notes"),
                    Arc::new(super::tests::ScriptedBackend::default()),
                );
                let source = Source {
                    kind: if kind == "claude" {
                        "claude-code"
                    } else {
                        "codex"
                    }
                    .into(),
                    path: Some(root.join(format!("{kind}-{mib}.jsonl"))),
                    session: None,
                };
                let mut initial_ms = Vec::new();
                let mut latest_ms = Vec::new();
                let mut earlier_ms = Vec::new();
                // One warmup then five measured repeats. Fresh source scans and repeated page
                // reads share warmed OS buffers; page timings include store access and JSON.
                for repeat in 0..6 {
                    let t = Instant::now();
                    drop(scan(&source).unwrap());
                    let full = t.elapsed().as_secs_f64() * 1000.0;
                    let t = Instant::now();
                    let page = read_page(&ctx, source.clone(), None).await.unwrap();
                    let latest = t.elapsed().as_secs_f64() * 1000.0;
                    let before = page["next_before"].as_u64().expect("history");
                    let t = Instant::now();
                    let older = read_page(&ctx, source.clone(), Some(before)).await.unwrap();
                    let earlier = t.elapsed().as_secs_f64() * 1000.0;
                    assert!(!older["items"].as_array().unwrap().is_empty());
                    if repeat > 0 {
                        initial_ms.push(full);
                        latest_ms.push(latest);
                        earlier_ms.push(earlier);
                    }
                }
                let median = |mut values: Vec<f64>| {
                    values.sort_by(f64::total_cmp);
                    values[2]
                };
                println!(
                    "BENCH kind={kind} mib={mib} full_ms={:.3} latest_ms={:.3} earlier_ms={:.3}",
                    median(initial_ms),
                    median(latest_ms),
                    median(earlier_ms)
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod round5_tests;
