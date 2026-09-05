//! Ingest: turn what the agents already wrote to disk into ledger rows.
//!
//! Nothing here talks to an agent or a network. Each pass lists the source files, skips the ones
//! whose size and modification time match the cursor from last time, reads the rest from their
//! stored offset, attributes each turn to a repo and lane by its working directory, and writes
//! the events, the session digests and the new cursors. Because the unique key on
//! `(source_path, source_offset)` rejects a row it already holds, a full re-read is harmless.
//!
//! All file work runs on `spawn_blocking`, and each pass reads at most
//! [`repomon_core::config::UsageConfig::max_files_per_scan`] files, so a first run over years of
//! transcripts never holds the RPC loop or the store thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use repomon_core::agent::claude;
use repomon_core::pricing::PriceTable;
use repomon_core::usage_ledger::{
    FleetIndex, UsageEvent, UsageSessionMeta,
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
}

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

/// Every file the ledger would read, at most `budget` of them.
///
/// Newest files come first: when the budget bites on a first run, the recent history the operator
/// is actually looking at lands before the archive does, and later passes catch up.
pub fn discover_sources(budget: usize) -> Vec<Source> {
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
            for file in read_dir(&dir) {
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

    out.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    out.into_iter().map(|(_, s)| s).take(budget).collect()
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
            // A snapshot describes today's rates, so it takes effect from the epoch onward only
            // where it beats the built-in row; the longest-prefix match still prefers an exact
            // model id, which is what a snapshot always carries.
            if let Ok(rows) =
                repomon_core::pricing::parse_litellm_snapshot(&text, chrono::Utc::now())
            {
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

/// Refresh the cached price snapshot if it is missing or older than a day.
///
/// The transport is `curl`, on purpose: the ledger is offline by default and pulling an HTTP
/// stack into the daemon to serve an off-by-default flag would be a poor trade.
fn refresh_price_cache(url: &str) {
    let path = price_cache_path();
    if let Ok(meta) = std::fs::metadata(&path) {
        let fresh = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(24 * 60 * 60));
        if fresh {
            return;
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "20", url])
        .output();
    if let Ok(out) = out {
        if out.status.success() && !out.stdout.is_empty() {
            let _ = std::fs::write(&path, out.stdout);
        }
    }
}

/// Build the repo and lane index, and the lane-to-window map, from the store.
async fn fleet(ctx: &Arc<Ctx>) -> (FleetIndex, HashMap<i64, String>) {
    let repos = ctx.store.list_repos().await.unwrap_or_default();
    let lanes = ctx.store.list_lane_meta().await.unwrap_or_default();
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
    (index, windows)
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
    if config.refresh_prices {
        let url = config
            .price_url
            .clone()
            .unwrap_or_else(|| repomon_core::config::DEFAULT_USAGE_PRICE_URL.to_string());
        let _ = tokio::task::spawn_blocking(move || refresh_price_cache(&url)).await;
    }

    let budget = config.max_files_per_scan;
    let sources = tokio::task::spawn_blocking(move || discover_sources(budget))
        .await
        .map_err(|e| repomon_core::Error::Other(e.to_string()))?;
    let (index, windows) = fleet(ctx).await;

    let mut report = IngestReport {
        listed: sources.len(),
        ..Default::default()
    };
    for source in sources {
        let path = source.path.to_string_lossy().to_string();
        let cursor = ctx.store.usage_cursor(path.clone()).await?;
        let print = tokio::task::spawn_blocking({
            let p = source.path.clone();
            move || fingerprint(&p)
        })
        .await
        .map_err(|e| repomon_core::Error::Other(e.to_string()))?;
        if let Some(c) = &cursor {
            if c.mtime == print && c.error.is_none() {
                continue;
            }
        }
        let offset = cursor.map(|c| c.offset).unwrap_or(0);
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
                ctx.store
                    .set_usage_cursor(path, offset, print, Some(e.to_string()))
                    .await?;
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
                    cwd: s.cwd,
                    repo_id: a.repo_id,
                    lane_id: a.lane_id,
                    started_at: s.first_at,
                    ended_at: s.last_at,
                    turns: s.turns,
                    tool_calls: s.tool_calls,
                    retries: s.retries,
                    external: a.external,
                    source_path: Some(path.clone()),
                }
            })
            .collect();
        report.events += ctx.store.record_usage_events(events).await?;
        if !sessions.is_empty() {
            ctx.store.upsert_usage_sessions(sessions).await?;
        }
        ctx.store
            .set_usage_cursor(path, scan.next_offset, print, None)
            .await?;
    }
    Ok(report)
}

/// The periodic ingest loop. Self-gates on `[usage] enabled`, so turning the ledger off costs one
/// config read per tick and nothing else.
pub async fn ingest_watch(ctx: Arc<Ctx>) {
    loop {
        let interval = {
            let c = ctx.config.read().await;
            Duration::from_secs(c.usage.scan_interval_secs.max(30))
        };
        match ingest_once(&ctx).await {
            Ok(report) if report.events > 0 => {
                tracing::debug!(
                    events = report.events,
                    scanned = report.scanned,
                    "usage ingest pass"
                );
                ctx.broadcast(crate::pubsub::topic::USAGE_CHANGED, serde_json::json!({}));
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "usage ingest pass failed"),
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
        let store = Store::open_in_memory().unwrap();
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
        assert_eq!(claude.len(), 3);
        assert!(claude.iter().all(|r| r.lane_id.is_some()));
        assert!(claude.iter().all(|r| !r.external));
        assert_eq!(claude[0].window.as_deref(), Some("lane-1"));
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
        assert_eq!(claude.tool_calls, 1);
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
}
