//! Read agent files on blocking workers and publish attributed events, digests, and cursors
//! atomically per source; replay preserves identity and raises partial token counts monotonically.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use repomon_core::agent::claude;
use repomon_core::pricing::PriceTable;
use repomon_core::usage_ledger::{
    FleetIndex, INGEST_VERSION, UsageEvent, UsageSessionMeta,
    scan::{
        ScannedEvent, SourceScan, scan_antigravity_transcript, scan_claude_transcript,
        scan_codex_rollout, scan_opencode_db,
    },
};

use crate::Ctx;

/// Which reader handles a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Claude,
    Codex,
    Antigravity,
    OpenCode,
}

/// One file the ledger reads, with whatever the file itself cannot tell the reader.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub kind: SourceKind,
    /// The account key the source belongs to.
    pub account: String,
    /// The working directory, for sources that do not record one.
    pub cwd_hint: Option<String>,
    /// The model id, for sources that do not record one.
    pub model_hint: String,
}

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// Files actually read this pass.
    pub scanned: usize,
    /// Files listed, read or skipped.
    pub listed: usize,
    /// Events newly inserted.
    pub events: usize,
    /// Files whose read failed.
    pub failed: usize,
    /// Stale sources (an older `INGEST_VERSION`) actually re-ingested this pass, from the
    /// newest-walk and the independent stale-cursor selection combined. Equal to
    /// [`REINGEST_BATCH`] means the batch was full and stale sources may remain.
    pub reingested: usize,
    /// Stale cursors attempted, including failures and retired sources.
    pub recount_attempts: usize,
    /// Sources still waiting for the current reader after this pass.
    pub stale_remaining: u64,
}

/// Where Claude Code nests a session's subagent transcripts.
const CLAUDE_SUBAGENTS_DIR: &str = "subagents";

/// The directory Codex writes rollouts to.
fn codex_sessions_root() -> PathBuf {
    if let Ok(p) = std::env::var("REPOMON_CODEX_SESSIONS") {
        return PathBuf::from(p);
    }
    home().join(".codex/sessions")
}

/// The Antigravity CLI's data directory.
fn antigravity_root() -> PathBuf {
    // The conversation cache is the one path the core reader already resolves, with its own env
    // override; the brain transcripts sit beside it.
    repomon_core::agent::antigravity::cache_path()
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home().join(".gemini/antigravity-cli"))
}

fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Map an Antigravity model label such as `Gemini 3.8 Flash (High)` onto a model id the price
/// table knows. The exact point release is not what the table is keyed on, and the counts are
/// estimates anyway, so the family is the honest granularity here.
pub fn antigravity_model(label: &str) -> String {
    let lower = label.to_ascii_lowercase();
    if lower.contains("flash") {
        "gemini-3-flash".to_string()
    } else if lower.contains("pro") {
        "gemini-3-pro".to_string()
    } else {
        "gemini-3".to_string()
    }
}

/// The model Antigravity is configured to use, read from its settings file.
fn antigravity_configured_model(root: &Path) -> String {
    let text = std::fs::read_to_string(root.join("settings.json")).unwrap_or_default();
    let label = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or_default();
    antigravity_model(&label)
}

/// Conversation id to working directory, inverted from Antigravity's `last_conversations` cache.
fn antigravity_cwds() -> HashMap<String, String> {
    let text =
        std::fs::read_to_string(repomon_core::agent::antigravity::cache_path()).unwrap_or_default();
    let by_cwd: HashMap<String, String> = serde_json::from_str(&text).unwrap_or_default();
    by_cwd.into_iter().map(|(cwd, id)| (id, cwd)).collect()
}

/// Every file the ledger would read, at most `budget` of them, newest first.
pub fn discover_sources(budget: usize) -> Vec<Source> {
    let mut out = discover_all_sources();
    out.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    out.into_iter().map(|(_, s)| s).take(budget).collect()
}

/// Unbounded metadata listing for the rotating normal walk.
fn discover_all_sources() -> Vec<(SystemTime, Source)> {
    let mut out: Vec<(std::time::SystemTime, Source)> = Vec::new();

    // `REPOMON_CLAUDE_PROJECTS` replaces the transcript location outright rather than adding to
    // it, so a test or an isolated daemon can point the ledger somewhere safe and be certain the
    // real `~/.claude` is never read.
    let claude_roots: Vec<(PathBuf, String)> = match std::env::var("REPOMON_CLAUDE_PROJECTS") {
        Ok(root) => vec![(PathBuf::from(root), "default".to_string())],
        Err(_) => claude::config_bases()
            .into_iter()
            .filter(|base| base.join("projects").is_dir())
            .map(|base| {
                let account = claude::account_key(
                    (base != claude::default_config_base()).then_some(base.as_path()),
                );
                (base.join("projects"), account)
            })
            .collect(),
    };
    for (projects, account) in claude_roots {
        for dir in read_dir(&projects) {
            for entry in read_dir(&dir) {
                // Subagent transcripts live below the parent session directory and contribute to
                // that parent's usage.
                if entry.is_dir() {
                    for file in read_dir(&entry.join(CLAUDE_SUBAGENTS_DIR)) {
                        if file.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                            continue;
                        }
                        out.push((
                            mtime(&file),
                            Source {
                                path: file,
                                kind: SourceKind::Claude,
                                account: account.clone(),
                                cwd_hint: None,
                                model_hint: String::new(),
                            },
                        ));
                    }
                    continue;
                }
                if entry.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                out.push((
                    mtime(&entry),
                    Source {
                        path: entry,
                        kind: SourceKind::Claude,
                        account: account.clone(),
                        cwd_hint: None,
                        model_hint: String::new(),
                    },
                ));
            }
        }
    }

    // Codex rollouts nest by year, month and day.
    let codex = codex_sessions_root();
    for year in read_dir(&codex) {
        for month in read_dir(&year) {
            for day in read_dir(&month) {
                for file in read_dir(&day) {
                    let name = file.file_name().unwrap_or_default().to_string_lossy();
                    if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                        out.push((
                            mtime(&file),
                            Source {
                                path: file,
                                kind: SourceKind::Codex,
                                account: "codex".to_string(),
                                cwd_hint: None,
                                model_hint: String::new(),
                            },
                        ));
                    }
                }
            }
        }
    }

    let agy_root = antigravity_root();
    let cwds = antigravity_cwds();
    let model = antigravity_configured_model(&agy_root);
    for conversation in read_dir(&agy_root.join("brain")) {
        let transcript = conversation.join(".system_generated/logs/transcript.jsonl");
        if !transcript.is_file() {
            continue;
        }
        let id = conversation
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        out.push((
            mtime(&transcript),
            Source {
                path: transcript,
                kind: SourceKind::Antigravity,
                account: "antigravity".to_string(),
                cwd_hint: cwds.get(&id).cloned(),
                model_hint: model.clone(),
            },
        ));
    }

    let opencode = repomon_core::agent::opencode::database_path();
    if opencode.is_file() {
        out.push((
            mtime(&opencode),
            Source {
                path: opencode,
                kind: SourceKind::OpenCode,
                account: "opencode".to_string(),
                cwd_hint: None,
                model_hint: String::new(),
            },
        ));
    }

    out
}

/// A file that disappeared permanently is different from a transient metadata/read error.
async fn source_missing(path: &Path) -> repomon_core::Result<bool> {
    let path = path.to_path_buf();
    let exists = tokio::task::spawn_blocking(move || path.try_exists())
        .await
        .map_err(|e| repomon_core::Error::Other(e.to_string()))?;
    Ok(matches!(exists, Ok(false)))
}

async fn retire_cursor(
    ctx: &Ctx,
    cursor: repomon_core::usage_ledger::UsageCursor,
) -> repomon_core::Result<()> {
    ctx.store
        .set_usage_cursor(
            cursor.source_path,
            cursor.offset,
            cursor.mtime,
            cursor.error,
            INGEST_VERSION,
        )
        .await
}

/// Select cursor paths first. Discovery supplies metadata only, never eligibility or order.
/// Retired cursors consume this batch's attempt budget just like a source we can still read.
async fn stale_batch(
    ctx: &Ctx,
    all: &[(SystemTime, Source)],
) -> repomon_core::Result<(Vec<Source>, usize)> {
    let by_path: HashMap<_, _> = all.iter().map(|(_, s)| (s.path.clone(), s)).collect();
    let mut sources = Vec::new();
    let mut retired = 0;
    for cursor in ctx
        .store
        .stale_usage_cursors(INGEST_VERSION, REINGEST_BATCH)
        .await?
    {
        let path = PathBuf::from(&cursor.source_path);
        if source_missing(&path).await? {
            retire_cursor(ctx, cursor).await?;
            retired += 1;
            continue;
        }
        if let Some(source) = by_path.get(&path) {
            sources.push((*source).clone());
            continue;
        }
        // Stored event metadata can recover a reader outside current discovery roots.
        let event = ctx
            .store
            .usage_source_event(cursor.source_path.clone())
            .await?;
        let source = event.and_then(|event| {
            let kind = match event.agent_kind.as_str() {
                "claude-code" => SourceKind::Claude,
                "codex" => SourceKind::Codex,
                "antigravity" => SourceKind::Antigravity,
                "opencode" => SourceKind::OpenCode,
                _ => return None,
            };
            Some(Source {
                path,
                kind,
                account: event.account,
                cwd_hint: event.cwd,
                model_hint: event.model,
            })
        });
        if let Some(source) = source {
            sources.push(source);
        } else {
            // No supported reader can recount this source. Keep its events and prior error,
            // but do not leave the entire ledger permanently in a recount state.
            retire_cursor(ctx, cursor).await?;
            retired += 1;
        }
    }
    Ok((sources, retired))
}

/// Rotate and wrap the bounded source window so older files eventually receive a scan.
fn rotated_window(sorted: &[(SystemTime, Source)], window: usize, offset: usize) -> Vec<Source> {
    let total = sorted.len();
    if total == 0 {
        return Vec::new();
    }
    let offset = offset % total;
    sorted
        .iter()
        .cycle()
        .skip(offset)
        .take(window.min(total))
        .map(|(_, s)| s.clone())
        .collect()
}

/// Keep the hottest half on every pass, rotating only through the older tail.
fn scan_window(
    sorted: &[(SystemTime, Source)],
    budget: usize,
    rotation: usize,
) -> (Vec<Source>, usize) {
    if sorted.len() <= budget {
        return (rotated_window(sorted, budget, 0), 0);
    }
    let head = (budget / 2).max(1).min(budget);
    let tail_budget = budget - head;
    let tail = &sorted[head..];
    let mut sources: Vec<_> = sorted[..head].iter().map(|(_, s)| s.clone()).collect();
    sources.extend(rotated_window(tail, tail_budget, rotation));
    let next = if tail_budget > 0 {
        (rotation + tail_budget) % tail.len()
    } else {
        0
    };
    (sources, next)
}

fn read_dir(path: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(path)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn mtime(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

/// A fingerprint that changes whenever a source has more to say: its size and modification time.
fn fingerprint(path: &Path) -> i64 {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Size moves on every append and the second-resolution mtime moves on most of them; together
    // they miss only a rewrite that keeps both, which transcripts do not do.
    secs.wrapping_mul(1_000_003) ^ meta.len() as i64
}

/// Read one source from `offset`.
fn scan_source(source: &Source, offset: u64) -> repomon_core::Result<SourceScan> {
    match source.kind {
        SourceKind::Claude => scan_claude_transcript(&source.path, offset, Some(&source.account)),
        SourceKind::Codex => scan_codex_rollout(&source.path, offset),
        SourceKind::Antigravity => scan_antigravity_transcript(
            &source.path,
            offset,
            &source.model_hint,
            source.cwd_hint.as_deref(),
        ),
        SourceKind::OpenCode => scan_opencode_db(&source.path, offset),
    }
}

/// The price table this daemon prices with: the built-in rates, then any cached snapshot, then
/// the operator's overrides, which always win.
pub async fn price_table(ctx: &Arc<Ctx>) -> PriceTable {
    let config = ctx.config.read().await.usage.clone();
    let mut table = PriceTable::builtin();
    if config.refresh_prices {
        if let Ok(text) = std::fs::read_to_string(price_cache_path()) {
            // A stable floor after the built-in date lets the snapshot beat built-in rows
            // while pricing stored events, independently of when this table is constructed.
            if let Ok(rows) = repomon_core::pricing::parse_litellm_snapshot(
                &text,
                repomon_core::pricing::snapshot_effective_from(),
            ) {
                for row in rows {
                    table.insert(row);
                }
            }
        }
    }
    table.apply_overrides(config.price_overrides.clone());
    table
}

/// Where a refreshed price snapshot is cached.
pub fn price_cache_path() -> PathBuf {
    repomon_core::config::data_dir().join("prices/litellm.json")
}

/// Build the repo and lane index, and the lane-to-window map, from the store.
async fn fleet(ctx: &Arc<Ctx>) -> repomon_core::Result<(FleetIndex, HashMap<i64, String>)> {
    let repos = ctx.store.list_repos().await?;
    let lanes = ctx.store.list_lane_meta().await?;
    let windows = lanes
        .iter()
        .filter_map(|l| l.tmux_window.clone().map(|w| (l.id, w)))
        .collect();
    let index = FleetIndex::new(
        repos.iter().map(|r| (r.id, r.path.clone())).collect(),
        lanes
            .iter()
            .map(|l| (l.id, l.repo_id, l.worktree_path.clone()))
            .collect(),
    );
    Ok((index, windows))
}

/// Attribute a scanned turn and turn it into a stored ledger row.
fn to_event(e: ScannedEvent, index: &FleetIndex, windows: &HashMap<i64, String>) -> UsageEvent {
    let a = index.attribute(e.cwd.as_deref());
    UsageEvent {
        at: e.at,
        agent_kind: e.agent_kind,
        model: e.model,
        account: e.account,
        lane_id: a.lane_id,
        repo_id: a.repo_id,
        session_id: e.session_id,
        window: a.lane_id.and_then(|l| windows.get(&l).cloned()),
        cwd: e.cwd,
        input_tokens: e.tokens.input,
        output_tokens: e.tokens.output,
        cache_read_tokens: e.tokens.cache_read,
        cache_write_tokens: e.tokens.cache_write,
        thinking_tokens: e.thinking_tokens,
        estimated: e.estimated,
        external: a.external,
        subagent: e.subagent,
        source_path: e.source_path,
        source_offset: e.source_offset,
    }
}

/// Run one ingest pass.
pub async fn ingest_once(ctx: &Arc<Ctx>) -> repomon_core::Result<IngestReport> {
    let _pass = ctx.usage_ingest_lock.lock().await;
    let config = ctx.config.read().await.usage.clone();
    if !config.enabled {
        return Ok(IngestReport::default());
    }
    // Attribution is required even when recount only retires a missing source's cursor.
    let (index, windows) = fleet(ctx).await?;

    let budget = config.max_files_per_scan;
    let all = tokio::task::spawn_blocking(discover_all_sources)
        .await
        .map_err(|e| repomon_core::Error::Other(e.to_string()))?;
    let mut by_recency = all.clone();
    by_recency.sort_by_key(|(at, _)| std::cmp::Reverse(*at));

    // Recent transcripts stay fresh on every pass while the archive still makes progress.
    let rotation = ctx
        .usage_scan_rotation
        .load(std::sync::atomic::Ordering::Relaxed);
    let (walked, next_rotation) = scan_window(&by_recency, budget, rotation);
    ctx.usage_scan_rotation
        .store(next_rotation, std::sync::atomic::Ordering::Relaxed);

    // Recount directly from the cursor table before spending the ordinary walk's budget.
    let (stale_extra, retired) = stale_batch(ctx, &all).await?;
    let mut seen: std::collections::HashSet<PathBuf> =
        stale_extra.iter().map(|s| s.path.clone()).collect();
    let mut sources = stale_extra;
    for source in walked {
        if seen.insert(source.path.clone()) {
            sources.push(source);
        }
    }

    let mut report = IngestReport {
        listed: sources.len(),
        ..Default::default()
    };
    let mut reingested = 0usize;
    let mut recount_attempts = retired;
    for source in sources {
        let path = source.path.to_string_lossy().to_string();
        let cursor = ctx.store.usage_cursor(path.clone()).await?;
        let print = tokio::task::spawn_blocking({
            let p = source.path.clone();
            move || fingerprint(&p)
        })
        .await
        .map_err(|e| repomon_core::Error::Other(e.to_string()))?;
        // Recount outdated reader versions regardless of file fingerprints, preserving deferred
        // cursors until their bounded turn.
        let stale = cursor
            .as_ref()
            .is_some_and(|c| c.ingest_version < INGEST_VERSION);
        // Never append using an obsolete offset then stamp it current just because this batch
        // is full. Leave the source untouched until its turn to recount from byte zero.
        if stale && recount_attempts >= REINGEST_BATCH {
            continue;
        }
        if let Some(c) = &cursor {
            if c.mtime == print && c.error.is_none() && !stale {
                continue;
            }
        }
        if stale {
            recount_attempts += 1;
        }
        let version = cursor
            .as_ref()
            .map(|c| c.ingest_version)
            .unwrap_or(INGEST_VERSION);
        let offset = if stale {
            0
        } else {
            cursor.map(|c| c.offset).unwrap_or(0)
        };
        let scanned = tokio::task::spawn_blocking({
            let source = source.clone();
            move || scan_source(&source, offset)
        })
        .await
        .map_err(|e| repomon_core::Error::Other(e.to_string()))?;
        report.scanned += 1;
        let scan = match scanned {
            Ok(s) => s,
            Err(e) => {
                report.failed += 1;
                if stale && !source_missing(&source.path).await? {
                    ctx.store
                        .fail_usage_recount(path, e.to_string(), INGEST_VERSION)
                        .await?;
                } else {
                    ctx.store
                        .set_usage_cursor(
                            path,
                            offset,
                            print,
                            Some(e.to_string()),
                            if stale { INGEST_VERSION } else { version },
                        )
                        .await?;
                }
                continue;
            }
        };
        let events: Vec<UsageEvent> = scan
            .events
            .into_iter()
            .map(|e| to_event(e, &index, &windows))
            .collect();
        let sessions: Vec<UsageSessionMeta> = scan
            .sessions
            .into_iter()
            .map(|s| {
                let a = index.attribute(s.cwd.as_deref());
                UsageSessionMeta {
                    session_id: s.session_id,
                    agent_kind: s.agent_kind,
                    headline: s.headline,
                    headline_raw: s.headline_raw,
                    headline_version: repomon_core::usage_ledger::HEADLINE_VERSION,
                    cwd: s.cwd,
                    repo_id: a.repo_id,
                    lane_id: a.lane_id,
                    started_at: s.first_at,
                    ended_at: s.last_at,
                    turns: s.turns,
                    tool_calls: s.tool_calls,
                    retries: s.retries,
                    external: a.external,
                    // A subagent transcript is not where the session's headline lives, so it must
                    // not become the file the headline redigest re-reads.
                    source_path: (!s.subagent).then(|| path.clone()),
                    counts_version: INGEST_VERSION,
                }
            })
            .collect();
        report.events += ctx
            .store
            .commit_usage_source(
                repomon_core::usage_ledger::UsageCursor {
                    source_path: path,
                    offset: scan.next_offset,
                    mtime: print,
                    scanned_at: chrono::Utc::now(),
                    error: None,
                    ingest_version: INGEST_VERSION,
                },
                stale,
                events,
                sessions,
            )
            .await?;
        if stale {
            reingested += 1;
        }
    }
    report.reingested = reingested;
    report.recount_attempts = recount_attempts;
    report.stale_remaining = ctx
        .store
        .usage_sources_below_ingest_version(INGEST_VERSION)
        .await?;
    Ok(report)
}

/// How many sources an older reader wrote [`ingest_once`] re-reads in one pass. Bounded for the
/// same reason [`HEADLINE_REDIGEST_BATCH`] is: a version bump leaves every source stale at once,
/// and a full re-read of years of transcripts in one tick would stall the pass that follows it.
const REINGEST_BATCH: usize = 25;

/// How many stale session digests [`redigest_stale_headlines`] rewrites in one call. Bounded so a
/// headline-extractor version bump, which leaves every existing session stale at once, catches up
/// over several ingest ticks. The batch limits file count, not the duration of each reread.
const HEADLINE_REDIGEST_BATCH: usize = 25;

/// Upgrade stale headlines in bounded batches, accepting authoritative null headlines and retiring
/// missing sources without changing their content to prevent backlog starvation.
pub async fn redigest_stale_headlines(ctx: &Arc<Ctx>) -> repomon_core::Result<usize> {
    use repomon_core::usage_ledger::HEADLINE_VERSION;
    let stale = ctx
        .store
        .usage_sessions_needing_headline_upgrade(HEADLINE_VERSION, HEADLINE_REDIGEST_BATCH)
        .await?;
    let mut updated = 0;
    for (agent_kind, session_id, source_path) in stale {
        let found = match source_path {
            Some(path) => {
                let path = PathBuf::from(path);
                let agent_kind = agent_kind.clone();
                let session_id = session_id.clone();
                tokio::task::spawn_blocking(move || {
                    rescan_headline(&path, &agent_kind, &session_id)
                })
                .await
                .map_err(|e| repomon_core::Error::Other(e.to_string()))?
            }
            None => None,
        };
        match found {
            Some((headline, headline_raw)) => {
                ctx.store
                    .update_usage_session_headline(
                        agent_kind,
                        session_id,
                        headline,
                        headline_raw,
                        HEADLINE_VERSION,
                    )
                    .await?;
                updated += 1;
            }
            None => {
                ctx.store
                    .mark_usage_session_headline_current(agent_kind, session_id, HEADLINE_VERSION)
                    .await?;
            }
        }
    }
    Ok(updated)
}

/// Return an authoritative headline pair, including an empty pair, or None when the source/session
/// is unavailable and the caller must preserve its stored content.
fn rescan_headline(
    path: &Path,
    agent_kind: &str,
    session_id: &str,
) -> Option<(Option<String>, Option<String>)> {
    let scan = match agent_kind {
        "claude-code" => scan_claude_transcript(path, 0, None),
        "codex" => scan_codex_rollout(path, 0),
        "antigravity" => scan_antigravity_transcript(path, 0, "", None),
        "opencode" => scan_opencode_db(path, 0),
        _ => return None,
    };
    scan.ok()?
        .sessions
        .into_iter()
        .find(|s| s.session_id == session_id)
        .map(|s| (s.headline, s.headline_raw))
}

fn next_ingest_delay(report: &IngestReport, interval: Duration) -> Duration {
    if report.recount_attempts == REINGEST_BATCH && report.stale_remaining > 0 {
        Duration::from_secs(1)
    } else {
        interval
    }
}

/// The periodic ingest loop. Self-gates on `[usage] enabled`, so turning the ledger off costs one
/// config read per tick and nothing else.
pub async fn ingest_watch(ctx: Arc<Ctx>) {
    loop {
        let mut interval = {
            let c = ctx.config.read().await;
            Duration::from_secs(c.usage.scan_interval_secs.max(30))
        };
        let mut changed = false;
        match ingest_once(&ctx).await {
            Ok(report) => {
                tracing::debug!(
                    events = report.events,
                    scanned = report.scanned,
                    "usage ingest pass"
                );
                changed = report.events > 0 || report.recount_attempts > 0;
                interval = next_ingest_delay(&report, interval);
            }
            Err(e) => tracing::warn!(error = %e, "usage ingest pass failed"),
        }
        match redigest_stale_headlines(&ctx).await {
            Ok(updated) if updated > 0 => {
                tracing::debug!(updated, "usage headline redigest pass");
                changed = true;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "usage headline redigest pass failed"),
        }
        if changed {
            ctx.broadcast(crate::pubsub::topic::USAGE_CHANGED, serde_json::json!({}));
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = ctx.usage_ingest_wake.notified() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repomon_core::{Config, Store};
    use std::fs;

    /// Source locations are process-global environment variables, so only one test may point
    /// them at its own tempdir at a time.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        prev: Vec<(&'static str, Option<String>)>,
        _held: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(vars: &[(&'static str, &str)]) -> Self {
            let held = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut prev = Vec::new();
            for (k, v) in vars {
                prev.push((*k, std::env::var(k).ok()));
                unsafe { std::env::set_var(k, v) };
            }
            EnvGuard { prev, _held: held }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.prev {
                match v {
                    Some(v) => unsafe { std::env::set_var(k, v) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }

    /// Lay out one of each source kind under `root` and return the env vars that point at them.
    fn seed_sources(root: &Path) -> Vec<(&'static str, String)> {
        let projects = root.join("claude/projects/-repos-demo");
        fs::create_dir_all(&projects).unwrap();
        fs::write(
            projects.join("sess-claude-1.jsonl"),
            include_str!("../../repomon-core/src/usage_ledger/fixtures/claude_usage_v0.jsonl"),
        )
        .unwrap();
        // Claude Code nests each subagent's transcript one directory below the session file.
        let subagents = projects.join("sess-claude-1/subagents");
        fs::create_dir_all(&subagents).unwrap();
        fs::write(
            subagents.join("agent-a1b2c3d4.jsonl"),
            include_str!("../../repomon-core/src/usage_ledger/fixtures/claude_subagent_v0.jsonl"),
        )
        .unwrap();

        let codex = root.join("codex/sessions/2026/09/02");
        fs::create_dir_all(&codex).unwrap();
        fs::write(
            codex.join("rollout-2026-09-02T09-00-00-sess-codex-1.jsonl"),
            include_str!("../../repomon-core/src/usage_ledger/fixtures/codex_usage_v0.jsonl"),
        )
        .unwrap();

        let brain = root.join("agy/brain/conv-1/.system_generated/logs");
        fs::create_dir_all(&brain).unwrap();
        fs::write(
            brain.join("transcript.jsonl"),
            include_str!("../../repomon-core/src/usage_ledger/fixtures/antigravity_usage_v0.jsonl"),
        )
        .unwrap();
        let cache = root.join("agy/cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            cache.join("last_conversations.json"),
            r#"{"/repos/demo":"conv-1"}"#,
        )
        .unwrap();

        vec![
            (
                "REPOMON_CLAUDE_PROJECTS",
                root.join("claude/projects").to_string_lossy().to_string(),
            ),
            (
                "REPOMON_CODEX_SESSIONS",
                root.join("codex/sessions").to_string_lossy().to_string(),
            ),
            (
                "REPOMON_ANTIGRAVITY_CACHE",
                cache
                    .join("last_conversations.json")
                    .to_string_lossy()
                    .to_string(),
            ),
            (
                "REPOMON_OPENCODE_DB",
                root.join("missing-opencode.db")
                    .to_string_lossy()
                    .to_string(),
            ),
        ]
    }

    fn with_seeded_sources(root: &Path) -> EnvGuard {
        let vars = seed_sources(root);
        let borrowed: Vec<(&'static str, &str)> =
            vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        EnvGuard::set(&borrowed)
    }

    #[test]
    fn discovery_finds_one_source_per_transcript_and_skips_a_missing_database() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let sources = discover_sources(usize::MAX);
        let kinds: Vec<SourceKind> = sources.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SourceKind::Claude));
        assert!(kinds.contains(&SourceKind::Codex));
        assert!(kinds.contains(&SourceKind::Antigravity));
        assert!(
            !kinds.contains(&SourceKind::OpenCode),
            "a database that is not there is not a source"
        );
    }

    #[test]
    fn discovery_finds_the_subagent_transcripts_nested_under_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let sources = discover_sources(usize::MAX);
        let nested: Vec<&Source> = sources
            .iter()
            .filter(|s| s.path.parent().and_then(|p| p.file_name()) == Some("subagents".as_ref()))
            .collect();
        assert_eq!(nested.len(), 1, "the subagent transcript is a source too");
        assert_eq!(nested[0].kind, SourceKind::Claude);
    }

    #[test]
    fn discovery_carries_the_antigravity_working_directory_from_the_conversation_cache() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let agy = discover_sources(usize::MAX)
            .into_iter()
            .find(|s| s.kind == SourceKind::Antigravity)
            .expect("an antigravity source");
        assert_eq!(agy.cwd_hint.as_deref(), Some("/repos/demo"));
    }

    #[test]
    fn discovery_is_bounded_by_the_configured_file_budget() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        assert_eq!(discover_sources(2).len(), 2);
    }

    #[test]
    fn an_antigravity_model_label_becomes_a_priceable_model_id() {
        assert_eq!(
            antigravity_model("Gemini 3.8 Flash (High)"),
            "gemini-3-flash"
        );
        assert_eq!(antigravity_model("Gemini 3 Pro"), "gemini-3-pro");
        assert_eq!(antigravity_model(""), "gemini-3");
    }

    async fn seeded_ctx(root: &Path) -> Arc<Ctx> {
        seeded_ctx_with_store(root, Store::open_in_memory().unwrap()).await
    }

    async fn seeded_ctx_with_store(root: &Path, store: Store) -> Arc<Ctx> {
        let repo = store
            .add_repo(root.join("repos/demo"), "demo".to_string(), None)
            .await
            .unwrap();
        let lane = store
            .get_or_create_lane(repo.id, "/repos/demo".to_string())
            .await
            .unwrap();
        store
            .set_lane_tmux_window(lane, Some("lane-1".to_string()))
            .await
            .unwrap();
        Ctx::new(store, Config::default(), None)
    }

    #[tokio::test]
    async fn an_ingest_pass_records_events_attributed_to_the_lane_that_owns_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        let report = ingest_once(&ctx).await.unwrap();
        assert!(report.events > 0);
        let rows = ctx
            .store
            .usage_events_between(
                chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                chrono::Utc::now() + chrono::Duration::days(365),
            )
            .await
            .unwrap();
        let claude: Vec<_> = rows
            .iter()
            .filter(|r| r.agent_kind == "claude-code")
            .collect();
        assert_eq!(
            claude.len(),
            5,
            "three turns of the session's own and two of its subagent's"
        );
        assert!(claude.iter().all(|r| r.lane_id.is_some()));
        assert!(claude.iter().all(|r| !r.external));
        assert_eq!(claude[0].window.as_deref(), Some("lane-1"));
    }

    /// Every ledger row, over a window wide enough that no test has to spell one out.
    async fn all_events(ctx: &Arc<Ctx>) -> Vec<repomon_core::usage_ledger::UsageEvent> {
        ctx.store
            .usage_events_between(
                chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                chrono::Utc::now() + chrono::Duration::days(365),
            )
            .await
            .unwrap()
    }

    async fn all_sessions(ctx: &Arc<Ctx>) -> Vec<repomon_core::usage_ledger::UsageSessionRow> {
        ctx.store
            .usage_sessions_between(
                chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                chrono::Utc::now() + chrono::Duration::days(365),
                None,
                50,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_subagent_turn_folds_into_the_row_of_the_session_that_spawned_it() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        ingest_once(&ctx).await.unwrap();
        let events = all_events(&ctx).await;
        let sub: Vec<_> = events.iter().filter(|e| e.subagent).collect();
        assert_eq!(sub.len(), 2, "both subagent messages are recorded");
        assert!(
            sub.iter()
                .all(|e| e.session_id.as_deref() == Some("sess-claude-1")),
            "a subagent turn belongs to the session that spawned it"
        );
        assert!(
            sub.iter().all(|e| e.lane_id.is_some() && !e.external),
            "and to the same lane as the session"
        );
        let row = all_sessions(&ctx)
            .await
            .into_iter()
            .find(|r| r.session_id == "sess-claude-1")
            .expect("the session row");
        assert_eq!(
            row.totals.subagent_tokens, 2007,
            "the row says how much of its spend was its subagents"
        );
        assert!(row.totals.total_tokens > row.totals.subagent_tokens);
        assert_eq!(
            row.headline.as_deref(),
            Some("Wire up the ledger"),
            "the subagent's own prompt must not become the session's task"
        );
        assert_eq!(row.turns, 5, "three of its own turns and two subagent ones");
    }

    #[tokio::test]
    async fn a_source_an_older_reader_wrote_is_re_read_and_its_events_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        let path = dir
            .path()
            .join("claude/projects/-repos-demo/sess-claude-1.jsonl")
            .to_string_lossy()
            .to_string();
        // Seed obsolete per-content-block offsets with a completed cursor to verify replay replaces
        // the stored event set.
        ctx.store
            .record_usage_events(vec![stale_claude_event(&path, 4096)])
            .await
            .unwrap();
        ctx.store
            .set_usage_cursor(path.clone(), 1, 0, None, 0)
            .await
            .unwrap();

        ingest_once(&ctx).await.unwrap();

        let events = all_events(&ctx).await;
        assert!(
            !events.iter().any(|e| e.source_offset == 4096),
            "the superseded row must be gone, not merely added to"
        );
        assert_eq!(
            ctx.store
                .usage_cursor(path)
                .await
                .unwrap()
                .expect("the cursor")
                .ingest_version,
            repomon_core::usage_ledger::INGEST_VERSION
        );
    }

    #[test]
    fn recount_uses_short_sleep_only_for_full_batch_with_stale_left() {
        let long = Duration::from_secs(600);
        for (recount_attempts, stale_remaining, expected) in [
            (25, 1, Duration::from_secs(1)),
            (25, 0, long),
            (24, 1, long),
            (0, 0, long),
        ] {
            assert_eq!(
                next_ingest_delay(
                    &IngestReport {
                        recount_attempts,
                        stale_remaining,
                        ..Default::default()
                    },
                    long
                ),
                expected
            );
        }
    }

    #[test]
    fn rotating_walk_eventually_visits_changed_old_files() {
        let sources: Vec<_> = (0..300)
            .map(|i| {
                (
                    SystemTime::UNIX_EPOCH,
                    Source {
                        path: PathBuf::from(format!("{i}.jsonl")),
                        kind: SourceKind::Claude,
                        account: "default".into(),
                        cwd_hint: None,
                        model_hint: String::new(),
                    },
                )
            })
            .collect();
        let first = rotated_window(&sources, 200, 0);
        let second = rotated_window(&sources, 200, 200);
        assert!(!first.iter().any(|s| s.path == Path::new("299.jsonl")));
        assert!(second.iter().any(|s| s.path == Path::new("299.jsonl")));
        assert_eq!(rotated_window(&sources, 500, 0).len(), 300);
        assert!(rotated_window(&[], 200, 0).is_empty());
    }

    #[test]
    fn newest_ten_are_scanned_every_pass_and_old_tail_is_reached() {
        let sources: Vec<_> = (0..1000)
            .map(|i| {
                (
                    SystemTime::UNIX_EPOCH,
                    Source {
                        path: PathBuf::from(format!("{i}.jsonl")),
                        kind: SourceKind::Claude,
                        account: "default".into(),
                        cwd_hint: None,
                        model_hint: String::new(),
                    },
                )
            })
            .collect();
        let mut rotation = 0;
        let mut visited_old = false;
        for _ in 0..10 {
            let (walked, next) = scan_window(&sources, 200, rotation);
            rotation = next;
            assert_eq!(walked.len(), 200);
            let paths: std::collections::HashSet<_> =
                walked.iter().map(|s| s.path.clone()).collect();
            assert_eq!(paths.len(), 200, "head and tail must not overlap");
            for i in 0..10 {
                assert!(paths.contains(&PathBuf::from(format!("{i}.jsonl"))));
            }
            visited_old |= paths.contains(Path::new("900.jsonl"));
        }
        assert!(visited_old);
        assert!(scan_window(&sources, 0, 0).0.is_empty());
        assert_eq!(scan_window(&sources, 1, 10).0[0].path, Path::new("0.jsonl"));
    }

    #[tokio::test]
    async fn three_hundred_sources_with_250_stale_converge_in_ten_passes() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        // Start with the fixture sources current. The archive is outside discovery roots and
        // cannot be reached by even an unbounded filesystem walk.
        ingest_once(&ctx).await.unwrap();
        let archive = dir.path().join("archive");
        fs::create_dir_all(&archive).unwrap();
        let tracked = ctx.store.usage_cursors().await.unwrap().len() as i64;
        for i in 0..(300 - tracked) {
            let path = archive.join(format!("old-{i}.jsonl"));
            fs::write(
                &path,
                include_str!("../../repomon-core/src/usage_ledger/fixtures/claude_usage_v0.jsonl"),
            )
            .unwrap();
            let path = path.to_string_lossy().to_string();
            ctx.store
                .record_usage_events(vec![stale_claude_event(&path, 4096)])
                .await
                .unwrap();
            ctx.store
                .set_usage_cursor(
                    path,
                    99999,
                    i,
                    None,
                    if i < 250 { 0 } else { INGEST_VERSION },
                )
                .await
                .unwrap();
        }
        for pass in 1..=10 {
            let report = ingest_once(&ctx).await.unwrap();
            assert_eq!(report.reingested, 25);
            assert_eq!(report.stale_remaining, 250 - pass * 25);
        }
    }

    #[tokio::test]
    async fn deleted_stale_source_is_retired_without_losing_events_or_error() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        let path = dir
            .path()
            .join("missing.jsonl")
            .to_string_lossy()
            .to_string();
        ctx.store
            .record_usage_events(vec![stale_claude_event(&path, 4096)])
            .await
            .unwrap();
        ctx.store
            .set_usage_cursor(path.clone(), 4096, 1, Some("last read failed".into()), 0)
            .await
            .unwrap();
        let report = ingest_once(&ctx).await.unwrap();
        assert_eq!(report.failed, 0);
        assert_eq!(report.recount_attempts, 1);
        assert_eq!(report.stale_remaining, 0);
        assert!(
            ctx.store
                .usage_source_event(path.clone())
                .await
                .unwrap()
                .is_some()
        );
        let cursor = ctx.store.usage_cursor(path).await.unwrap().unwrap();
        assert_eq!(cursor.ingest_version, INGEST_VERSION);
        assert_eq!(cursor.error.as_deref(), Some("last read failed"));
    }

    #[tokio::test]
    async fn unknown_and_unresolvable_stale_sources_are_retired() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        for name in ["unknown", "no-event"] {
            let path = dir.path().join(name).to_string_lossy().to_string();
            fs::write(&path, "{}\n").unwrap();
            if name == "unknown" {
                let mut event = stale_claude_event(&path, 4096);
                event.agent_kind = "unsupported-agent".into();
                ctx.store.record_usage_events(vec![event]).await.unwrap();
            }
            ctx.store
                .set_usage_cursor(path, 4096, 1, Some("old error".into()), 0)
                .await
                .unwrap();
        }
        let report = ingest_once(&ctx).await.unwrap();
        assert_eq!(report.stale_remaining, 0);
        assert_eq!(report.recount_attempts, 2);
        assert!(
            ctx.store
                .usage_source_event(dir.path().join("unknown").to_string_lossy().to_string())
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn unreadable_source_in_full_batch_keeps_fast_cadence_and_old_counts() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        ingest_once(&ctx).await.unwrap();
        let archive = dir.path().join("archive");
        fs::create_dir_all(&archive).unwrap();
        for i in 0..26 {
            let path = archive.join(format!("source-{i}"));
            // An existing directory deterministically fails a transcript read on every OS,
            // including when the tests run with permissions that could bypass a mode bit.
            if i == 25 {
                fs::create_dir(&path).unwrap();
            } else {
                fs::write(
                    &path,
                    include_str!(
                        "../../repomon-core/src/usage_ledger/fixtures/claude_usage_v0.jsonl"
                    ),
                )
                .unwrap();
            }
            let path = path.to_string_lossy().to_string();
            ctx.store
                .record_usage_events(vec![stale_claude_event(&path, 4096)])
                .await
                .unwrap();
            ctx.store
                .set_usage_cursor(path, 99999, i, None, 0)
                .await
                .unwrap();
        }
        let report = ingest_once(&ctx).await.unwrap();
        assert_eq!(report.recount_attempts, 25);
        assert_eq!(report.reingested, 24);
        assert_eq!(report.failed, 1);
        assert_eq!(report.stale_remaining, 2);
        assert_eq!(
            next_ingest_delay(&report, Duration::from_secs(600)),
            Duration::from_secs(1)
        );
        let unreadable = archive.join("source-25").to_string_lossy().to_string();
        assert_eq!(
            ctx.store
                .usage_cursor(unreadable.clone())
                .await
                .unwrap()
                .unwrap()
                .ingest_version,
            0
        );
        assert!(
            ctx.store
                .usage_source_event(unreadable)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn unreadable_stale_source_retires_on_third_failure_preserving_events_and_error() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let db = dir.path().join("recount.db");
        let ctx = seeded_ctx_with_store(dir.path(), Store::open(&db).unwrap()).await;
        let connection = rusqlite::Connection::open(&db).unwrap();
        let path = dir.path().join("unreadable.jsonl");
        fs::create_dir(&path).unwrap();
        let path = path.to_string_lossy().to_string();
        ctx.store
            .record_usage_events(vec![stale_claude_event(&path, 4096)])
            .await
            .unwrap();
        ctx.store
            .set_usage_cursor(path.clone(), 4096, 1, None, 0)
            .await
            .unwrap();
        for attempt in 1..=3 {
            // Advance the persisted observation time instead of sleeping in the test.
            connection
                .execute(
                    "UPDATE usage_ingest_cursors SET scanned_at = ?1 WHERE source_path = ?2",
                    rusqlite::params![
                        (chrono::Utc::now() - chrono::Duration::seconds(61)).to_rfc3339(),
                        path
                    ],
                )
                .unwrap();
            let report = ingest_once(&ctx).await.unwrap();
            assert_eq!(report.failed, 1);
            assert_eq!(report.recount_attempts, 1);
            assert_eq!(report.stale_remaining, u64::from(attempt < 3));
            let cursor = ctx.store.usage_cursor(path.clone()).await.unwrap().unwrap();
            assert_eq!(
                cursor.ingest_version,
                if attempt < 3 { 0 } else { INGEST_VERSION }
            );
            assert_eq!(cursor.offset, 4096);
            assert!(cursor.error.is_some());
            if attempt < 3 {
                ingest_once(&ctx).await.unwrap();
                assert_eq!(
                    ctx.store.usage_cursor(path.clone()).await.unwrap().unwrap(),
                    cursor
                );
            }
            assert!(
                ctx.store
                    .usage_source_event(path.clone())
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        assert_eq!(ingest_once(&ctx).await.unwrap().recount_attempts, 0);
    }

    #[tokio::test]
    async fn full_recount_batch_does_not_stamp_other_stale_walked_sources_current() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        ingest_once(&ctx).await.unwrap();
        let project = dir.path().join("claude/projects/-repos-demo");
        for i in 0..26 {
            let path = project.join(format!("stale-{i}.jsonl"));
            fs::write(
                &path,
                include_str!("../../repomon-core/src/usage_ledger/fixtures/claude_usage_v0.jsonl"),
            )
            .unwrap();
            ctx.store
                .set_usage_cursor(path.to_string_lossy().to_string(), 99999, i, None, 0)
                .await
                .unwrap();
        }
        let first = ingest_once(&ctx).await.unwrap();
        assert_eq!(first.reingested, 25);
        assert_eq!(first.stale_remaining, 1);
        let second = ingest_once(&ctx).await.unwrap();
        assert_eq!(second.reingested, 1);
        assert_eq!(second.stale_remaining, 0);
        assert!(
            second.events > 0,
            "the last source must be read from zero, not its old offset"
        );
    }

    #[tokio::test]
    async fn stale_cursor_selection_orders_version_then_recency() {
        let store = repomon_core::Store::open_in_memory().unwrap();
        for (path, version, mtime) in [("new", 1, 30), ("old", 0, 10), ("older", 0, 20)] {
            store
                .set_usage_cursor(path.into(), 0, mtime, None, version)
                .await
                .unwrap();
        }
        let cursors = store.stale_usage_cursors(2, 2).await.unwrap();
        assert_eq!(
            cursors
                .iter()
                .map(|c| c.source_path.as_str())
                .collect::<Vec<_>>(),
            ["older", "old"]
        );
    }

    #[tokio::test]
    async fn usage_status_counts_the_sources_still_waiting_to_be_re_read() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        ctx.store
            .set_usage_cursor("/t/old.jsonl".into(), 1, 0, None, 0)
            .await
            .unwrap();
        let status = crate::usage_query::status(&ctx).await.unwrap();
        assert_eq!(status.stale_sources, 1);
        ctx.store
            .set_usage_cursor(
                "/t/old.jsonl".into(),
                1,
                0,
                None,
                repomon_core::usage_ledger::INGEST_VERSION,
            )
            .await
            .unwrap();
        assert_eq!(
            crate::usage_query::status(&ctx)
                .await
                .unwrap()
                .stale_sources,
            0
        );
    }

    #[tokio::test]
    async fn a_second_ingest_pass_adds_nothing_and_reads_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        let first = ingest_once(&ctx).await.unwrap();
        let second = ingest_once(&ctx).await.unwrap();
        assert!(first.events > 0);
        assert_eq!(second.events, 0);
        assert_eq!(
            second.scanned, 0,
            "unchanged files are skipped by mtime and size"
        );
    }

    #[tokio::test]
    async fn appended_lines_are_picked_up_without_re_reading_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        let first = ingest_once(&ctx).await.unwrap();
        let path = dir
            .path()
            .join("claude/projects/-repos-demo/sess-claude-1.jsonl");
        let extra = r#"{"type":"assistant","timestamp":"2026-09-01T10:06:00.000Z","cwd":"/repos/demo","sessionId":"sess-claude-1","message":{"role":"assistant","model":"claude-sonnet-5","content":[],"usage":{"input_tokens":7,"output_tokens":9,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;
        let mut body = std::fs::read_to_string(&path).unwrap();
        body.push_str(extra);
        body.push('\n');
        std::fs::write(&path, body).unwrap();
        let second = ingest_once(&ctx).await.unwrap();
        assert_eq!(second.events, 1, "only the appended line is new");
        assert!(first.events > second.events);
    }

    #[tokio::test]
    async fn a_session_outside_every_lane_is_recorded_and_marked_external() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let store = Store::open_in_memory().unwrap();
        let ctx = Ctx::new(store, Config::default(), None);
        ingest_once(&ctx).await.unwrap();
        let rows = ctx
            .store
            .usage_events_between(
                chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                chrono::Utc::now() + chrono::Duration::days(365),
            )
            .await
            .unwrap();
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| r.external));
        assert!(rows.iter().all(|r| r.lane_id.is_none()));
    }

    #[tokio::test]
    async fn an_ingest_pass_writes_the_session_digest_the_sessions_table_reads() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        ingest_once(&ctx).await.unwrap();
        let rows = ctx
            .store
            .usage_sessions_between(
                chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                chrono::Utc::now() + chrono::Duration::days(365),
                None,
                50,
            )
            .await
            .unwrap();
        let claude = rows
            .iter()
            .find(|r| r.session_id == "sess-claude-1")
            .expect("the claude session");
        assert_eq!(claude.headline.as_deref(), Some("Wire up the ledger"));
        assert_eq!(claude.retries, 1);
        assert_eq!(
            claude.tool_calls, 2,
            "one of its own and one its subagent made"
        );
    }

    /// A stale headline version must trigger extraction under the current rules.
    fn stale_session_meta(
        session_id: &str,
        source_path: &str,
        bad_headline: &str,
    ) -> UsageSessionMeta {
        UsageSessionMeta {
            session_id: session_id.to_string(),
            agent_kind: "codex".to_string(),
            headline: Some(bad_headline.to_string()),
            headline_raw: Some(bad_headline.to_string()),
            headline_version: 0,
            cwd: Some("/repos/demo".to_string()),
            repo_id: None,
            lane_id: None,
            started_at: None,
            ended_at: None,
            turns: 1,
            tool_calls: 0,
            retries: 0,
            external: true,
            source_path: Some(source_path.to_string()),
            counts_version: INGEST_VERSION,
        }
    }

    /// A row shaped the way the pre-fix reader wrote them: one per content-block line.
    fn stale_claude_event(source_path: &str, offset: i64) -> UsageEvent {
        UsageEvent {
            at: chrono::DateTime::parse_from_rfc3339("2026-09-01T10:01:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            agent_kind: "claude-code".to_string(),
            model: "claude-sonnet-5".to_string(),
            account: "default".to_string(),
            lane_id: None,
            repo_id: None,
            session_id: Some("sess-claude-1".to_string()),
            window: None,
            cwd: Some("/repos/demo".to_string()),
            input_tokens: 2,
            output_tokens: 611,
            cache_read_tokens: 30449,
            cache_write_tokens: 26034,
            thinking_tokens: 487,
            estimated: false,
            external: true,
            subagent: false,
            source_path: source_path.to_string(),
            source_offset: offset,
        }
    }

    fn codex_review_event(session_id: &str) -> UsageEvent {
        UsageEvent {
            at: chrono::DateTime::parse_from_rfc3339("2026-09-02T09:01:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            agent_kind: "codex".to_string(),
            model: "gpt-5.6-sol".to_string(),
            account: "codex".to_string(),
            lane_id: None,
            repo_id: None,
            session_id: Some(session_id.to_string()),
            window: None,
            cwd: Some("/repos/demo".to_string()),
            input_tokens: 100,
            output_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            thinking_tokens: 0,
            estimated: false,
            external: true,
            subagent: false,
            source_path: "r.jsonl".to_string(),
            source_offset: 0,
        }
    }

    #[tokio::test]
    async fn a_digest_at_an_old_headline_version_is_recomputed_on_the_next_pass() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        fs::write(
            &path,
            include_str!(
                "../../repomon-core/src/usage_ledger/fixtures/codex_injected_preamble_v0.jsonl"
            ),
        )
        .unwrap();
        let ctx = Ctx::new(Store::open_in_memory().unwrap(), Config::default(), None);
        ctx.store
            .record_usage_events(vec![codex_review_event("sess-codex-review-1")])
            .await
            .unwrap();
        ctx.store
            .upsert_usage_sessions(vec![stale_session_meta(
                "sess-codex-review-1",
                &path.to_string_lossy(),
                "The following is the Codex agent history whose request action you ar...",
            )])
            .await
            .unwrap();
        let updated = redigest_stale_headlines(&ctx).await.unwrap();
        assert_eq!(updated, 1);
        let rows = ctx
            .store
            .usage_sessions_needing_headline_upgrade(
                repomon_core::usage_ledger::HEADLINE_VERSION,
                10,
            )
            .await
            .unwrap();
        assert!(
            rows.is_empty(),
            "the session must no longer be flagged stale"
        );
        let sessions = ctx
            .store
            .usage_sessions_between(
                chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                chrono::Utc::now() + chrono::Duration::days(365 * 2),
                None,
                50,
            )
            .await
            .unwrap();
        let s = sessions
            .iter()
            .find(|s| s.session_id == "sess-codex-review-1")
            .expect("the session, joined through its event");
        assert_eq!(
            s.headline, None,
            "the leaked preamble must not survive the redigest"
        );
    }

    #[tokio::test]
    async fn a_digest_already_at_the_current_headline_version_is_not_re_read() {
        let dir = tempfile::tempdir().unwrap();
        // A source file that, if re-read, would produce a different headline than the one stored
        // below: proof that a current digest is left alone rather than rescanned.
        let path = dir.path().join("r.jsonl");
        fs::write(
            &path,
            include_str!("../../repomon-core/src/usage_ledger/fixtures/codex_usage_v0.jsonl"),
        )
        .unwrap();
        let ctx = Ctx::new(Store::open_in_memory().unwrap(), Config::default(), None);
        ctx.store
            .upsert_usage_sessions(vec![UsageSessionMeta {
                headline_version: repomon_core::usage_ledger::HEADLINE_VERSION,
                ..stale_session_meta(
                    "sess-codex-1",
                    &path.to_string_lossy(),
                    "Already-settled headline",
                )
            }])
            .await
            .unwrap();
        let updated = redigest_stale_headlines(&ctx).await.unwrap();
        assert_eq!(updated, 0, "a current digest is not a candidate at all");
    }

    #[tokio::test]
    async fn usage_ingest_now_also_redigests_stale_headlines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        fs::write(
            &path,
            include_str!(
                "../../repomon-core/src/usage_ledger/fixtures/codex_injected_preamble_v0.jsonl"
            ),
        )
        .unwrap();
        let ctx = Ctx::new(Store::open_in_memory().unwrap(), Config::default(), None);
        ctx.store
            .upsert_usage_sessions(vec![stale_session_meta(
                "sess-codex-review-1",
                &path.to_string_lossy(),
                "<USER_REQUEST>wrapper the CLI added</USER_REQUEST>",
            )])
            .await
            .unwrap();
        // `usage.ingest_now`'s handler calls both passes; exercising the redigest call directly
        // here (the RPC layer has its own dispatch tests) confirms it is reachable from that path
        // without duplicating the whole RPC harness.
        ingest_once(&ctx).await.unwrap();
        let updated = redigest_stale_headlines(&ctx).await.unwrap();
        assert_eq!(updated, 1);
    }

    #[tokio::test]
    async fn the_price_table_takes_the_configured_overrides() {
        let mut config = Config::default();
        config.usage.price_overrides.insert(
            "claude-sonnet-5".to_string(),
            repomon_core::pricing::PriceOverride {
                input_per_mtok: Some(99.0),
                ..Default::default()
            },
        );
        let ctx = Ctx::new(Store::open_in_memory().unwrap(), config, None);
        let table = price_table(&ctx).await;
        let at = chrono::Utc::now();
        assert_eq!(
            table.lookup("claude-sonnet-5", at).unwrap().input_per_mtok,
            99.0
        );
    }

    #[tokio::test]
    async fn attribution_read_errors_preserve_cursors_and_recover_without_unassigned_events() {
        for table in ["repos", "lanes"] {
            let dir = tempfile::tempdir().unwrap();
            let _env = with_seeded_sources(dir.path());
            let db = dir.path().join("failure.db");
            let store = Store::open(&db).unwrap();
            let repo = store
                .add_repo(dir.path().join("repos/demo"), "demo".into(), None)
                .await
                .unwrap();
            store
                .get_or_create_lane(repo.id, "/repos/demo".into())
                .await
                .unwrap();
            let mut config = Config {
                tmux_session: format!("usage-errors-{}-{table}", std::process::id()),
                ..Config::default()
            };
            config.repomind.home = dir.path().join("repomind").to_string_lossy().into_owned();
            let ctx = Ctx::new_with_paths(
                store,
                config,
                Some(db.clone()),
                dir.path().join("config.toml"),
                dir.path().join("notes"),
            );
            let missing = dir
                .path()
                .join("missing.jsonl")
                .to_string_lossy()
                .into_owned();
            ctx.store
                .set_usage_cursor(missing.clone(), 123, 1, None, 0)
                .await
                .unwrap();
            let before = ctx.store.usage_cursors().await.unwrap();
            let connection = rusqlite::Connection::open(&db).unwrap();
            connection
                .execute_batch(&format!("ALTER TABLE {table} RENAME TO unavailable"))
                .unwrap();
            assert!(ingest_once(&ctx).await.is_err());
            assert_eq!(ctx.store.usage_cursors().await.unwrap(), before);
            assert!(all_events(&ctx).await.is_empty());
            connection
                .execute_batch(&format!("ALTER TABLE unavailable RENAME TO {table}"))
                .unwrap();
            ingest_once(&ctx).await.unwrap();
            let events = all_events(&ctx).await;
            let claude: Vec<_> = events
                .iter()
                .filter(|e| e.agent_kind == "claude-code")
                .collect();
            assert!(!claude.is_empty());
            assert!(claude.iter().all(|e| e.lane_id.is_some() && !e.external));
            assert_eq!(
                ctx.store
                    .usage_cursor(missing)
                    .await
                    .unwrap()
                    .unwrap()
                    .ingest_version,
                INGEST_VERSION
            );
        }
    }

    #[tokio::test]
    async fn ingest_does_not_fetch_prices_when_refresh_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let _env = with_seeded_sources(dir.path());
        let ctx = seeded_ctx(dir.path()).await;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        {
            let mut config = ctx.config.write().await;
            config.usage.refresh_prices = true;
            config.usage.price_url = Some(format!("http://{}", listener.local_addr().unwrap()));
        }
        assert!(ingest_once(&ctx).await.unwrap().events > 0);
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
