//! Reads Claude transcripts to derive session activity and turn status. Project-directory encoding
//! is lossy, so discovery also checks the recorded working directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::model::{AgentKind, AgentSession, AgentStatus, RepoId, WorktreeId};

/// How long with no transcript activity before we consider a session idle.
const IDLE_AFTER: Duration = Duration::minutes(2);

/// A digest of an agent session (transcript- or activity-derived).
#[derive(Debug, Clone)]
pub struct TranscriptSummary {
    pub kind: AgentKind,
    pub manifest_path: PathBuf,
    pub cwd: Option<PathBuf>,
    pub last_activity: DateTime<Utc>,
    pub tool_call_count: u32,
    pub status: AgentStatus,
    pub title: Option<String>,
    /// The agent's most recent message text - what it said (or asked) when it last ended a
    /// turn. This is the "why" behind a needs-you notification.
    pub last_message: Option<String>,
    /// The Claude config dir this session belongs to, when it isn't the default `~/.claude`
    /// (e.g. a work account run with `CLAUDE_CONFIG_DIR=~/.claude-work`). Drives adopt.
    pub config_dir: Option<PathBuf>,
    /// The session id (transcript filename stem) - lets adopt resume *this* exact session
    /// (`claude --resume <id>`) when several run in one worktree.
    pub session_id: Option<String>,
    /// Whether the last entry is the agent speaking with no tool call - it finished its turn.
    /// Unlike `status` (whose `Waiting` decays to `Idle` after [`IDLE_AFTER`]), this fact
    /// survives the decay: the stall detector needs "did it end its turn?" long after 10 min.
    pub ended_turn: bool,
}

impl TranscriptSummary {
    /// Build an [`AgentSession`] for a lane from this summary.
    pub fn into_session(self, repo_id: RepoId, worktree_id: WorktreeId) -> AgentSession {
        AgentSession {
            id: 0,
            agent: self.kind,
            repo_id,
            worktree_id: Some(worktree_id),
            started_at: self.last_activity,
            last_activity_at: self.last_activity,
            ended_at: None,
            manifest_path: self.manifest_path,
            tool_call_count: self.tool_call_count,
            title: self.title,
            last_message: self.last_message,
            pending_prompt: None, // set by the overlay's pane sniffer
            pending_dialog: None,
            status: self.status,
            external: false, // overlay flips this based on tmux ownership
            session_id: self.session_id,
            tmux_window: None, // overlay pairs managed sessions with their windows
            resume_at: None,
            inferred: false,
            stale: false, // overlaid by the daemon's stall detector
            stalled_since: None,
            subagent_running: None,
            status_reason: None,
            attention_kind: None,
            ended_turn: self.ended_turn,
            gate: None,
            config_dir: self.config_dir,
            custom_label: None,
            generated_label: None, // overlay sets this from the session_generated_labels store
        }
    }
}

fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where Claude Code stores its per-project session transcripts (the default account).
pub fn projects_root() -> PathBuf {
    if let Ok(p) = std::env::var("REPOMON_CLAUDE_PROJECTS") {
        return PathBuf::from(p);
    }
    home().join(".claude").join("projects")
}

/// The default Claude config dir (`~/.claude`).
pub fn default_config_base() -> PathBuf {
    home().join(".claude")
}

/// Returns the default, variant, and explicitly configured Claude config directories, cached
/// briefly to avoid scanning the home directory on every lane refresh.
pub fn config_bases() -> Vec<PathBuf> {
    use std::time::{Duration, Instant};
    // Tests mutate env / home and expect immediate results - never cache there.
    if cfg!(test) {
        return config_bases_uncached();
    }
    type Cache = Mutex<Option<(Instant, Vec<PathBuf>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    const TTL: Duration = Duration::from_secs(45);
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(g) = cell.lock() {
        if let Some((t, bases)) = &*g {
            if t.elapsed() < TTL {
                return bases.clone();
            }
        }
    }
    let fresh = config_bases_uncached();
    if let Ok(mut g) = cell.lock() {
        *g = Some((Instant::now(), fresh.clone()));
    }
    fresh
}

fn config_bases_uncached() -> Vec<PathBuf> {
    let mut bases = vec![default_config_base()];
    if let Ok(rd) = std::fs::read_dir(home()) {
        for e in rd.flatten() {
            let p = e.path();
            let is_variant = p
                .file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with(".claude-"))
                .unwrap_or(false);
            if is_variant && p.join("projects").is_dir() && !bases.contains(&p) {
                bases.push(p);
            }
        }
    }
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") {
        let p = PathBuf::from(d);
        if !bases.contains(&p) {
            bases.push(p);
        }
    }
    bases
}

/// Builds an account-specific launch command, unsetting inherited `CLAUDE_CONFIG_DIR` for the
/// default account because explicitly setting `~/.claude` selects a different profile location.
pub fn launch_command(base: &Path) -> String {
    if canonical(base) == canonical(&default_config_base()) {
        "env -u CLAUDE_CONFIG_DIR claude".to_string()
    } else {
        format!(
            "CLAUDE_CONFIG_DIR={} claude",
            super::shell_quote(&base.display().to_string())
        )
    }
}

/// Lists detected Claude accounts with launch commands insulated from inherited account
/// configuration.
pub fn agent_variants() -> Vec<(String, String)> {
    let default = default_config_base();
    config_bases()
        .into_iter()
        .map(|base| {
            let name = if base == default {
                "claude-code".to_string()
            } else {
                base.file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.trim_start_matches('.').to_string())
                    .unwrap_or_else(|| "claude".to_string())
            };
            (name, launch_command(&base))
        })
        .collect()
}

/// Identifies a Claude account by its config-directory path, using default when no variant is set.
pub fn account_key(config_dir: Option<&Path>) -> String {
    config_dir
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "default".to_string())
}

/// A short human label for a Claude account: `"main"` for the default `~/.claude`, otherwise the
/// dir's distinguishing suffix (`~/.claude-work` → `"work"`).
pub fn account_label(config_dir: Option<&Path>) -> String {
    match config_dir {
        None => "main".to_string(),
        Some(p) => p
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| {
                n.trim_start_matches('.')
                    .trim_start_matches("claude-")
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "claude".to_string()),
    }
}

/// Encodes a working directory into Claude’s project-directory name, replacing separators, dots,
/// and Windows drive colons with dashes.
pub fn encode_project_dir(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| match c {
            '/' | '.' | '\\' | ':' => '-',
            c => c,
        })
        .collect()
}

/// The newest `*.jsonl` transcript in a directory, by modification time.
pub fn newest_transcript_in(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(mtime) = mtime {
            if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                best = Some((mtime, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// The mtime and byte length jointly detect same-timestamp appends; the sequence stamp bounds the
/// cache through approximate LRU eviction.
#[derive(Clone)]
struct CacheEntry {
    key: (SystemTime, u64),
    /// Whether `key`'s stamp had settled when this entry was built. An entry built while the stamp
    /// could still absorb a same-length rewrite is never served as a finished summary. It may still
    /// seed an incremental parse, but only against [`CacheEntry::prefix_hash`], which is what
    /// actually establishes that resuming is sound.
    servable: bool,
    /// Hash of the `offset` bytes already folded into `state`, or `None` if they could not be read.
    /// Resuming asserts those bytes are unchanged, and no combination of mtime, length and identity
    /// can establish that: a rewrite of the folded bytes followed by an append is byte-for-byte
    /// indistinguishable in metadata from a plain append. Re-reading them is the only proof.
    prefix_hash: Option<u64>,
    seq: u64,
    summary: TranscriptSummary,
    state: SummaryState,
    offset: u64,
    identity: FileIdentity,
}

#[cfg(unix)]
type FileIdentity = (u64, u64);
#[cfg(not(unix))]
type FileIdentity = Option<SystemTime>;

fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        metadata.created().ok()
    }
}

/// Process-global memo for [`parse_transcript`], keyed by path and invalidated by file mtime+len.
fn cache() -> &'static Mutex<HashMap<PathBuf, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Monotonic counter handing out the `seq` access stamps for LRU eviction.
fn cache_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// The cache's soft capacity. Past this we evict the least-recently-used entry on each insert
/// rather than clearing everything - so a fleet with more than this many transcripts doesn't
/// re-parse the whole set on every refresh.
const CACHE_CAP: usize = 1024;

/// Hash the first `upto` bytes of `path`, or `None` if they cannot all be read.
///
/// This is what an incremental resume pays for, and it is deliberately not a heuristic over part of
/// the file: the question "are the bytes I already folded still the same bytes" has no metadata
/// answer, so the bytes are read. At 2.7 GB/s this is about 4x cheaper than re-parsing them.
fn prefix_hash(path: &Path, upto: u64) -> Option<u64> {
    use std::hash::Hasher;
    use std::io::Read;
    if upto == 0 {
        return Some(0);
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut remaining = upto;
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        match file.read(&mut buf[..want]) {
            // A file that ended early is shorter than what we folded, so it is not an extension.
            Ok(0) => return None,
            Ok(read) => {
                hasher.write(&buf[..read]);
                remaining -= read as u64;
            }
            Err(_) => return None,
        }
    }
    Some(hasher.finish())
}

/// Summarize complete JSONL records once, then fold appends from the saved offset.
/// Replacement, truncation, and same-length rewrites reset the aggregate.
pub fn parse_transcript(path: &Path) -> Option<TranscriptSummary> {
    let metadata = std::fs::metadata(path).ok()?;
    let key = (metadata.modified().ok()?, metadata.len());
    // Read before the scan below, never after: a rewrite landing during the scan would otherwise be
    // excused by an instant that has already made its stamp look settled.
    let observed_at = SystemTime::now();
    let identity = file_identity(&metadata);
    // Serialize updates so concurrent overlays cannot both reread the same append. The cache
    // holds only summary state, never the transcript body.
    let mut c = cache().lock().ok()?;
    if let Some(entry) = c.get_mut(path) {
        if entry.servable && entry.key == key && entry.identity == identity {
            entry.seq = cache_seq();
            let mut summary = entry.summary.clone();
            if Utc::now() - summary.last_activity > IDLE_AFTER {
                summary.status = AgentStatus::Idle;
            }
            return Some(summary);
        }
    }
    let prior = c
        .get(path)
        .filter(|e| e.identity == identity && key.1 > e.key.1);
    // A longer file is not necessarily the same file with more on the end. Resume only against
    // bytes that still hash to what was folded; anything else rebuilds the aggregate from zero.
    let (mut state, offset) = prior
        .filter(|e| e.prefix_hash.is_some() && prefix_hash(path, e.offset) == e.prefix_hash)
        .map(|e| (e.state.clone(), e.offset))
        .unwrap_or_default();
    let offset =
        crate::usage_ledger::scan::for_each_line_until(path, offset, Some(key.1), |v, _| {
            state.observe(v);
        })
        .ok()?;
    let summary = state.summary(path, key.0.into());
    if c.len() >= CACHE_CAP && !c.contains_key(path) {
        if let Some(oldest) = c.iter().min_by_key(|(_, e)| e.seq).map(|(p, _)| p.clone()) {
            c.remove(&oldest);
        }
    }
    c.insert(
        path.into(),
        CacheEntry {
            key,
            servable: crate::fs_stamp::is_settled(key.0, observed_at),
            prefix_hash: prefix_hash(path, offset),
            seq: cache_seq(),
            summary: summary.clone(),
            state,
            offset,
            identity,
        },
    );
    Some(summary)
}

/// Last timestamped conversation activity, excluding filesystem-only edits and trailers.
pub fn transcript_activity(path: &Path) -> Option<DateTime<Utc>> {
    parse_transcript(path)?;
    cache().lock().ok()?.get(path)?.state.last_msg_activity
}

#[derive(Clone, Default)]
struct SummaryState {
    tool_call_count: u32,
    last_type: Option<&'static str>,
    last_assistant_has_tool: bool,
    title: Option<String>,
    last_message: Option<String>,
    cwd: Option<PathBuf>,
    last_msg_activity: Option<DateTime<Utc>>,
}

impl SummaryState {
    fn observe(&mut self, v: &Value) {
        if self.cwd.is_none() {
            if let Some(c) = v.get("cwd").and_then(Value::as_str) {
                self.cwd = Some(PathBuf::from(c));
            }
        }
        let entry_type = v.get("type").and_then(Value::as_str);
        // Count only real conversation turns as activity - not the untimestamped trailer
        // (last-prompt/ai-title/…) or a pr-link refresh, which bump mtime without new work.
        if matches!(entry_type, Some("assistant") | Some("user")) {
            if let Some(ts) = v
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
            {
                self.last_msg_activity = Some(self.last_msg_activity.map_or(ts, |p| p.max(ts)));
            }
        }
        match entry_type {
            Some("assistant") => {
                let mut has_tool = false;
                if let Some(arr) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_array)
                {
                    for block in arr {
                        match block.get("type").and_then(Value::as_str) {
                            Some("tool_use") => {
                                self.tool_call_count += 1;
                                has_tool = true;
                            }
                            Some("text") => {
                                if let Some(t) = block.get("text").and_then(Value::as_str) {
                                    let t = t.trim();
                                    if !t.is_empty() {
                                        self.last_message = Some(truncate(t, 200));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                self.last_type = Some("assistant");
                self.last_assistant_has_tool = has_tool;
            }
            Some("user") => {
                self.last_type = Some("user");
                // Title from the first *real* prompt - skip Claude Code's injected scaffolding
                // (the local-command caveat, slash-command invocations, local-command stdout),
                // which would otherwise show up as "<local-command-caveat>Caveat: …".
                if self.title.is_none() {
                    if let Some(t) = user_text(v) {
                        let t = t.trim();
                        if !t.is_empty() && !is_synthetic_user_text(t) {
                            self.title = Some(truncate(t, 60));
                        }
                    }
                }
            }
            Some("summary") => {
                if let Some(s) = v.get("summary").and_then(Value::as_str) {
                    self.title = Some(truncate(s, 60));
                }
            }
            _ => {}
        }
    }
    fn summary(&self, path: &Path, mtime: DateTime<Utc>) -> TranscriptSummary {
        let last_activity = self.last_msg_activity.unwrap_or(mtime);
        let status = if Utc::now() - last_activity > IDLE_AFTER {
            AgentStatus::Idle
        } else if self.last_type == Some("assistant") && !self.last_assistant_has_tool {
            // The agent spoke and issued no tool call - it's waiting on you.
            AgentStatus::Waiting
        } else {
            AgentStatus::Running
        };

        TranscriptSummary {
            kind: AgentKind::ClaudeCode,
            manifest_path: path.to_path_buf(),
            cwd: self.cwd.clone(),
            last_activity,
            tool_call_count: self.tool_call_count,
            status,
            title: self.title.clone(),
            last_message: self.last_message.clone(),
            config_dir: None, // set by the caller based on which config dir it came from
            session_id: path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string()),
            ended_turn: self.last_type == Some("assistant") && !self.last_assistant_has_tool,
        }
    }
}

/// Find and summarize the Claude session for `cwd` under `root`.
pub fn summary_for_root(root: &Path, cwd: &Path) -> Option<TranscriptSummary> {
    let encoded = root.join(encode_project_dir(cwd));
    if encoded.is_dir() {
        if let Some(t) = newest_transcript_in(&encoded) {
            if let Some(s) = parse_transcript(&t) {
                return Some(s);
            }
        }
    }

    rescan_by_cwd(root, cwd)
}

/// Scan recorded working directories when the lossy encoded path lookup cannot identify the
/// session.
fn rescan_by_cwd(root: &Path, cwd: &Path) -> Option<TranscriptSummary> {
    let want = canonical(cwd);
    let mut best: Option<TranscriptSummary> = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(t) = newest_transcript_in(&entry.path()) else {
            continue;
        };
        if let Some(s) = parse_transcript(&t) {
            if s.cwd.as_deref().map(canonical) == Some(want.clone())
                && best
                    .as_ref()
                    .map(|b| s.last_activity > b.last_activity)
                    .unwrap_or(true)
            {
                best = Some(s);
            }
        }
    }
    best
}

/// Summarizes the Claude session for `cwd`, falling back to recorded working directories when the
/// lossy encoded directory lookup finds no transcript.
pub fn summary_for(cwd: &Path) -> Option<TranscriptSummary> {
    let encoded = encode_project_dir(cwd);

    // Test override: a single projects dir, treated as the default account.
    if let Ok(p) = std::env::var("REPOMON_CLAUDE_PROJECTS") {
        let root = PathBuf::from(p);
        let dir = root.join(&encoded);
        if let Some(s) = newest_transcript_in(&dir).and_then(|t| parse_transcript(&t)) {
            return Some(s);
        }
        // Encoding drift: the encoded dir is absent/empty - match by recorded cwd instead.
        return rescan_by_cwd(&root, cwd);
    }

    // Scan every config dir's encoded project subdir (usually 1-2), keeping the most recent -
    // so a work-account session in `~/.claude-work` is detected alongside the default account.
    let default = default_config_base();
    let mut best: Option<TranscriptSummary> = None;
    for base in config_bases() {
        let root = base.join("projects");
        let dir = root.join(&encoded);
        // The encoded dir might be missing (encoding drift) or present-but-empty; either way fall
        // through to the recorded-cwd rescan under this base so a live session is still found.
        let s = newest_transcript_in(&dir)
            .and_then(|t| parse_transcript(&t))
            .or_else(|| rescan_by_cwd(&root, cwd));
        if let Some(mut s) = s {
            s.config_dir = (base != default).then(|| base.clone());
            if best
                .as_ref()
                .map(|b| s.last_activity > b.last_activity)
                .unwrap_or(true)
            {
                best = Some(s);
            }
        }
    }
    best
}

/// Returns recent Claude summaries across config directories newest-first, capped at max and
/// filtered by transcript modification age.
pub fn summaries_for(cwd: &Path, within: Duration, max: usize) -> Vec<TranscriptSummary> {
    let encoded = encode_project_dir(cwd);
    let cutoff = Utc::now() - within;

    // (config_dir for that base, the encoded project dir under it)
    let dirs: Vec<(Option<PathBuf>, PathBuf)> =
        if let Ok(p) = std::env::var("REPOMON_CLAUDE_PROJECTS") {
            vec![(None, PathBuf::from(p).join(&encoded))]
        } else {
            let default = default_config_base();
            config_bases()
                .into_iter()
                .map(|base| {
                    let cfg = (base != default).then(|| base.clone());
                    (cfg, base.join("projects").join(&encoded))
                })
                .collect()
        };

    let mut out: Vec<TranscriptSummary> = Vec::new();
    for (config_dir, dir) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            // Cheap mtime gate before parsing the whole transcript.
            let recent = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| DateTime::<Utc>::from(t) >= cutoff)
                .unwrap_or(false);
            if !recent {
                continue;
            }
            if let Some(mut s) = parse_transcript(&path) {
                s.config_dir = config_dir.clone();
                out.push(s);
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.last_activity));
    out.truncate(max);
    out
}

/// Which config dir holds `session_id`'s transcript for `cwd` (so adopt can resume it against
/// the right account). `Some(None)` = the default `~/.claude`; `Some(Some(dir))` = a variant.
pub fn config_base_for_session(cwd: &Path, session_id: &str) -> Option<Option<PathBuf>> {
    transcript_location(cwd, session_id).map(|(_, config_dir)| config_dir)
}

fn transcript_location(cwd: &Path, session_id: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    if !valid_session_id(session_id) {
        return None;
    }
    let encoded = encode_project_dir(cwd);
    let file = format!("{session_id}.jsonl");

    // Test override: a single projects dir, treated as the default account.
    if let Ok(projects) = std::env::var("REPOMON_CLAUDE_PROJECTS") {
        let path = PathBuf::from(projects).join(&encoded).join(&file);
        return path.is_file().then_some((path, None));
    }

    let default = default_config_base();
    for base in config_bases() {
        let path = base.join("projects").join(&encoded).join(&file);
        if path.is_file() {
            let config_dir = (base != default).then_some(base);
            return Some((path, config_dir));
        }
    }
    None
}

/// Locate a known session's JSONL file without parsing the whole transcript.
pub fn transcript_path_for_session(cwd: &Path, session_id: &str) -> Option<PathBuf> {
    transcript_location(cwd, session_id).map(|(path, _)| path)
}

/// Looks up a specific session by ID without substituting a more recently active session.
pub fn transcript_for_session(cwd: &Path, session_id: &str) -> Option<TranscriptSummary> {
    let (path, config_dir) = transcript_location(cwd, session_id)?;
    let mut summary = parse_transcript(&path)?;
    summary.config_dir = config_dir;
    Some(summary)
}

fn valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id != "."
        && session_id != ".."
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Whether a user message is Claude Code's injected scaffolding rather than a real prompt - the
/// local-command caveat, a slash-command invocation, or local-command stdout. Such messages must
/// not become the session title/summary.
fn is_synthetic_user_text(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with("Caveat:")
        || t.starts_with("<local-command-caveat>")
        || t.starts_with("<command-name>")
        || t.starts_with("<command-message>")
        || t.starts_with("<command-args>")
        || t.starts_with("<local-command-stdout>")
}

fn user_text(v: &Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        for block in arr {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// How much of a transcript's tail to read for the chat view - bounds the cost of polling a
/// long session (this file can be many MB).
const TAIL_BYTES: u64 = 512 * 1024;
/// Cap per-item text so payloads stay bounded (full messages, not titles).
const ITEM_TEXT_MAX: usize = 4000;
/// One backwards history page. A partial JSONL line at the front is deferred to the next page.
const TRANSCRIPT_PAGE_BYTES: u64 = 128 * 1024;

fn transcript_items<'a>(
    lines: impl Iterator<Item = &'a str>,
    text_limit: Option<usize>,
) -> Vec<crate::model::TranscriptItem> {
    use crate::model::TranscriptItem;

    let mut items: Vec<TranscriptItem> = Vec::new();
    // Tool calls accumulated since the last message, in first-use order.
    let mut tools: Vec<(String, u32)> = Vec::new();
    let mut tools_at: Option<DateTime<Utc>> = None;

    fn flush_tools(
        items: &mut Vec<crate::model::TranscriptItem>,
        tools: &mut Vec<(String, u32)>,
        at: &mut Option<DateTime<Utc>>,
    ) {
        if tools.is_empty() {
            return;
        }
        let text = tools
            .iter()
            .map(|(name, n)| {
                if *n > 1 {
                    format!("{name} ×{n}")
                } else {
                    name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" · ");
        items.push(crate::model::TranscriptItem {
            role: "tools".into(),
            text,
            at: at.take(),
            ..Default::default()
        });
        tools.clear();
    }

    for line in lines {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let at = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let Some(arr) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                // Blocks in order: a text block flushes pending tools first (they happened
                // before it), tool_use blocks accumulate.
                for block in arr {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                let t = t.trim();
                                if !t.is_empty() {
                                    flush_tools(&mut items, &mut tools, &mut tools_at);
                                    items.push(TranscriptItem {
                                        role: "assistant".into(),
                                        text: text_limit
                                            .map(|limit| truncate(t, limit))
                                            .unwrap_or_else(|| t.to_string()),
                                        at,
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                        Some("tool_use") => {
                            let name = block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_string();
                            match tools.iter_mut().find(|(n, _)| *n == name) {
                                Some((_, n)) => *n += 1,
                                None => tools.push((name, 1)),
                            }
                            tools_at = at;
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                // Real user text only - tool_result carriers return None here.
                if let Some(t) = user_text(&v) {
                    let t = t.trim().to_string();
                    if !t.is_empty() {
                        flush_tools(&mut items, &mut tools, &mut tools_at);
                        items.push(TranscriptItem {
                            role: "user".into(),
                            text: text_limit.map(|limit| truncate(&t, limit)).unwrap_or(t),
                            at,
                            ..Default::default()
                        });
                    }
                }
            }
            _ => {}
        }
    }
    flush_tools(&mut items, &mut tools, &mut tools_at);
    items
}

/// Reads the final conversation items from a transcript tail, preserving message text and
/// aggregating intervening tool calls.
pub fn transcript_tail(path: &Path, max_items: usize) -> Vec<crate::model::TranscriptItem> {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_BYTES);
    if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if f.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        lines.next();
    }
    let mut items = transcript_items(lines, Some(ITEM_TEXT_MAX));

    if items.len() > max_items {
        items.drain(..items.len() - max_items);
    }
    items
}

/// One bounded, backwards-readable page of a Claude transcript.
pub struct TranscriptPage {
    pub items: Vec<crate::model::TranscriptItem>,
    pub next_before: Option<u64>,
}

fn transcript_page_with_bytes(path: &Path, before: Option<u64>, page_bytes: u64) -> TranscriptPage {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return TranscriptPage {
            items: Vec::new(),
            next_before: None,
        };
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let end = before.unwrap_or(len).min(len);
    if end == 0 {
        return TranscriptPage {
            items: Vec::new(),
            next_before: None,
        };
    }

    let mut requested = page_bytes.max(1).min(end);
    loop {
        let start = end.saturating_sub(requested);
        let read_start = start.saturating_sub(1);
        if file.seek(SeekFrom::Start(read_start)).is_err() {
            return TranscriptPage {
                items: Vec::new(),
                next_before: None,
            };
        }
        let mut bytes = vec![0; (end - read_start) as usize];
        if file.read_exact(&mut bytes).is_err() {
            return TranscriptPage {
                items: Vec::new(),
                next_before: None,
            };
        }

        let (actual_start, body) = if start == 0 {
            (0, bytes.as_slice())
        } else if bytes.first() == Some(&b'\n') {
            (start, &bytes[1..])
        } else if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            (read_start + newline as u64 + 1, &bytes[newline + 1..])
        } else {
            requested = requested.saturating_mul(2).min(end);
            continue;
        };

        let text = String::from_utf8_lossy(body);
        return TranscriptPage {
            items: transcript_items(text.lines(), None),
            next_before: (actual_start > 0).then_some(actual_start),
        };
    }
}

/// Read the newest page when `before` is `None`, then progressively older pages by passing the
/// returned `next_before`. Pages meet on JSONL line boundaries, so no message is skipped.
pub fn transcript_page(path: &Path, before: Option<u64>) -> TranscriptPage {
    transcript_page_with_bytes(path, before, TRANSCRIPT_PAGE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_variant_unsets_config_dir_against_env_leak() {
        // The default profile requires an unset CLAUDE_CONFIG_DIR; setting ~/.claude selects a
        // different profile file.
        let variants = agent_variants();
        let (_, cmd) = variants
            .iter()
            .find(|(n, _)| n == "claude-code")
            .expect("default claude-code variant is always present");
        assert_ne!(
            cmd, "claude",
            "bare `claude` would inherit a leaked CLAUDE_CONFIG_DIR"
        );
        assert!(
            !cmd.contains("CLAUDE_CONFIG_DIR="),
            "default must not PIN a config dir (that reads the ~/.claude stub), got: {cmd}"
        );
        assert!(
            cmd.contains("env -u CLAUDE_CONFIG_DIR"),
            "default must UNSET CLAUDE_CONFIG_DIR so it reads ~/.claude.json, got: {cmd}"
        );
        assert!(cmd.ends_with("claude"), "still launches claude, got: {cmd}");
    }

    #[test]
    fn encodes_cwd_like_claude_code() {
        assert_eq!(
            encode_project_dir(Path::new("/Users/azaleas/Developer/Claude/repomon")),
            "-Users-azaleas-Developer-Claude-repomon"
        );

        assert_eq!(
            encode_project_dir(Path::new("/Users/x/.config/app")),
            "-Users-x--config-app"
        );
        // Windows: the drive colon and backslashes map to dashes (verified against a real
        // Claude Code projects dir: C:\Users\me\Documents\Dev → C--Users-me-Documents-Dev).
        assert_eq!(
            encode_project_dir(Path::new(r"C:\Users\me\Documents\Dev")),
            "C--Users-me-Documents-Dev"
        );
    }

    fn write_transcript(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    fn write_complete_transcript(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = write_transcript(dir, name, lines);
        // Incremental summaries commit only newline-terminated records, like the ledger scanner.
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        path
    }

    #[test]
    fn parse_transcript_memoises_by_mtime() {
        let root = tempfile::tempdir().unwrap();
        let line = r#"{"type":"user","cwd":"/code/x","message":{"content":"hello"}}"#;
        let path = write_complete_transcript(root.path(), "sess.jsonl", &[line]);

        let s1 = parse_transcript(&path).expect("parses");

        // Poison the cached summary, then parse again with the file unchanged: a cache hit must
        // return the poisoned value (proving it did not re-read the file). The entry is marked
        // servable because the transcript here was written microseconds ago; a real transcript
        // reaches that state on its own once its stamp settles.
        {
            let mut c = cache().lock().unwrap();
            let entry = c.get_mut(&path).unwrap();
            entry.summary.title = Some("SENTINEL".into());
            entry.servable = true;
        }
        let s2 = parse_transcript(&path).expect("parses");
        assert_eq!(
            s2.title.as_deref(),
            Some("SENTINEL"),
            "should be a cache hit"
        );

        // Staling the stored mtime forces a miss: the real content (not the sentinel) comes back.
        {
            let mut c = cache().lock().unwrap();
            c.get_mut(&path).unwrap().key.0 = SystemTime::UNIX_EPOCH;
        }
        let s3 = parse_transcript(&path).expect("parses");
        assert_ne!(
            s3.title.as_deref(),
            Some("SENTINEL"),
            "stale mtime re-parses"
        );
        assert_eq!(s3.title, s1.title);
    }

    #[test]
    fn cache_hit_still_decays_to_idle() {
        let root = tempfile::tempdir().unwrap();
        // An assistant turn with no tool call → Waiting (needs you); freshly written → not idle.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#;
        let path = write_complete_transcript(root.path(), "idle.jsonl", &[line]);
        assert_eq!(
            parse_transcript(&path).unwrap().status,
            AgentStatus::Waiting
        );

        // Backdate the cached summary's last_activity past IDLE_AFTER *without* touching the file
        // (so its mtime - our cache key - is unchanged). The next call is a cache hit that must
        // still report Idle: status decays by the clock, not by a file change.
        {
            let mut c = cache().lock().unwrap();
            let entry = c.get_mut(&path).unwrap();
            entry.summary.last_activity = Utc::now() - Duration::minutes(20);
            entry.servable = true;
        }
        assert_eq!(
            parse_transcript(&path).unwrap().status,
            AgentStatus::Idle,
            "a frozen transcript still decays to Idle on a cache hit"
        );
    }

    /// Resuming an incremental parse asserts that the bytes already folded are unchanged. A rewrite
    /// of those bytes followed by an append leaves exactly the `(mtime, len)` a plain append would,
    /// so metadata cannot tell the two apart and the aggregate must be rebuilt from zero.
    #[test]
    fn a_rewritten_prefix_is_not_extended_by_a_later_append() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("rewrite.jsonl");
        // Two records of identical byte length, so only their content differs.
        let record = |cwd: &str, text: &str| {
            format!(r#"{{"type":"user","cwd":"{cwd}","message":{{"content":"{text}"}}}}"#) + "\n"
        };
        let first = record("/code/aaa", "hello one");
        let rewritten = record("/code/bbb", "hello two");
        assert_eq!(
            first.len(),
            rewritten.len(),
            "the rewrite must not change the length, or length alone would catch it"
        );

        std::fs::write(&path, &first).unwrap();
        assert_eq!(
            parse_transcript(&path).unwrap().cwd,
            Some(PathBuf::from("/code/aaa"))
        );

        // Rewrite what was already folded, then append, with no read in between. The cached entry
        // is a shorter prefix of a now-longer file, which is exactly what a plain append looks like.
        std::fs::write(&path, rewritten + &record("/code/ccc", "hello two")).unwrap();
        assert_eq!(
            parse_transcript(&path).unwrap().cwd,
            Some(PathBuf::from("/code/bbb")),
            "the rewritten prefix must be re-read, not skipped by resuming from the stale offset"
        );
    }

    /// Length catches appends and truncations, but a rewrite to the same length inside one
    /// timestamp tick moves neither length nor stamp. Until the stamp settles the summary is
    /// rebuilt rather than served, so such a rewrite can never be masked.
    #[test]
    fn a_transcript_written_this_instant_is_not_served_from_cache() {
        let root = tempfile::tempdir().unwrap();
        let line = r#"{"type":"user","cwd":"/code/x","message":{"content":"hello"}}"#;
        let path = write_complete_transcript(root.path(), "hot.jsonl", &[line]);

        parse_transcript(&path).expect("parses");
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(
            !crate::fs_stamp::is_settled(stamp, SystemTime::now()),
            "a transcript written microseconds ago cannot be settled"
        );
        {
            let c = cache().lock().unwrap();
            assert!(
                !c.get(&path).unwrap().servable,
                "an entry built while the stamp is unsettled must never be served"
            );
        }

        // Poisoning the cached summary now changes nothing: the file is re-read.
        {
            let mut c = cache().lock().unwrap();
            c.get_mut(&path).unwrap().summary.title = Some("SENTINEL".into());
        }
        assert_ne!(
            parse_transcript(&path).expect("parses").title.as_deref(),
            Some("SENTINEL"),
            "an unsettled stamp must force a re-read"
        );
    }

    #[test]
    fn synthetic_user_text_detected() {
        assert!(is_synthetic_user_text(
            "Caveat: The messages below were generated while running local commands"
        ));
        assert!(is_synthetic_user_text("<local-command-caveat>Caveat: …"));
        assert!(is_synthetic_user_text("<command-name>/foo</command-name>"));
        assert!(is_synthetic_user_text("  <local-command-stdout>out"));
        assert!(!is_synthetic_user_text("Refactor the parser to stream"));
    }

    #[test]
    fn title_skips_local_command_scaffolding() {
        // The first user message is Claude Code's injected caveat; the title must be the next,
        // real prompt - not "<local-command-caveat>Caveat: …".
        let root = tempfile::tempdir().unwrap();
        let caveat = r#"{"type":"user","message":{"content":"<local-command-caveat>Caveat: generated while running local commands"}}"#;
        let real = r#"{"type":"user","message":{"content":"Refactor the parser to stream"}}"#;
        let path = write_complete_transcript(root.path(), "caveat.jsonl", &[caveat, real]);
        assert_eq!(
            parse_transcript(&path).unwrap().title.as_deref(),
            Some("Refactor the parser to stream")
        );
    }

    #[test]
    fn summaries_for_lists_every_recent_session() {
        let root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/code/multi");
        let dir = root.path().join(encode_project_dir(cwd));
        let line = r#"{"type":"user","cwd":"/code/multi","message":{"content":"hi"}}"#;
        for id in ["aaaa1111", "bbbb2222", "cccc3333"] {
            write_complete_transcript(&dir, &format!("{id}.jsonl"), &[line]);
        }

        // SAFETY: single-threaded test; nothing else reads the environment here.
        unsafe { std::env::set_var("REPOMON_CLAUDE_PROJECTS", root.path()) };
        let all = summaries_for(cwd, Duration::hours(6), 8);
        let capped = summaries_for(cwd, Duration::hours(6), 2);
        let single = summary_for(cwd);
        // SAFETY: single-threaded test; nothing else reads the environment here.
        unsafe { std::env::remove_var("REPOMON_CLAUDE_PROJECTS") };

        // Each concurrent session surfaces as its own entry, keyed by session id.
        assert_eq!(all.len(), 3, "all three sessions surface");
        let ids: std::collections::HashSet<String> =
            all.iter().filter_map(|s| s.session_id.clone()).collect();
        assert!(ids.contains("aaaa1111"));
        assert!(ids.contains("bbbb2222"));
        assert!(ids.contains("cccc3333"));
        // The cap is honored, and the single-session helper still works.
        assert_eq!(capped.len(), 2);
        assert!(single.is_some());
    }

    #[test]
    fn transcript_for_session_finds_its_file_regardless_of_recency() {
        let root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/code/pinned");
        let dir = root.path().join(encode_project_dir(cwd));
        let line = r#"{"type":"user","cwd":"/code/pinned","message":{"content":"hi"}}"#;
        // Write the "pinned" session first (older mtime), then an unrelated one that touches its
        // transcript later (newer mtime) - the scenario that misattributes under a
        // newest-transcript heuristic but must not under a direct id lookup.
        write_complete_transcript(&dir, "pinned-session-id.jsonl", &[line]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_complete_transcript(&dir, "unrelated-newer.jsonl", &[line]);

        // SAFETY: single-threaded test; nothing else reads the environment here.
        unsafe { std::env::set_var("REPOMON_CLAUDE_PROJECTS", root.path()) };
        let found = transcript_for_session(cwd, "pinned-session-id");
        let missing = transcript_for_session(cwd, "no-such-session");
        // SAFETY: single-threaded test; nothing else reads the environment here.
        unsafe { std::env::remove_var("REPOMON_CLAUDE_PROJECTS") };

        let found = found.expect("the pinned session's transcript is found by id");
        assert_eq!(found.session_id.as_deref(), Some("pinned-session-id"));
        assert!(
            missing.is_none(),
            "an id with no matching file must not fall back to some other transcript"
        );
    }

    #[test]
    fn transcript_session_lookup_rejects_path_components() {
        let cwd = Path::new("/code/pinned");
        for invalid in ["", ".", "..", "../secret", r"..\secret", "/tmp/secret"] {
            assert!(transcript_for_session(cwd, invalid).is_none());
            assert!(config_base_for_session(cwd, invalid).is_none());
        }
    }

    #[test]
    fn waiting_when_last_entry_is_assistant_text() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = "/code/proj";
        let lines = [
            r#"{"type":"user","cwd":"/code/proj","message":{"content":"add tests"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done — want me to also..."}]}}"#,
        ];
        let path = write_complete_transcript(dir.path(), "s.jsonl", &lines);
        let s = parse_transcript(&path).unwrap();
        assert_eq!(s.tool_call_count, 1);
        assert_eq!(s.status, AgentStatus::Waiting);
        assert!(s.status.needs_you());
        assert_eq!(s.cwd.as_deref(), Some(Path::new(cwd)));
        assert_eq!(s.title.as_deref(), Some("add tests"));
        // The "why" behind a needs-you alert: the agent's final message text.
        assert_eq!(s.last_message.as_deref(), Some("Done — want me to also..."));
    }

    #[test]
    fn stale_message_decays_to_idle_despite_fresh_mtime() {
        // Metadata rewrites must not make an old message look freshly active.
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"type":"user","timestamp":"2020-01-01T00:00:00Z","message":{"content":"go"}}"#,
            r#"{"type":"assistant","timestamp":"2020-01-01T00:00:05Z","message":{"content":[{"type":"text","text":"Done — need you."}]}}"#,
        ];
        // write_transcript creates the file now, so its mtime is fresh (the "metadata touch").
        let path = write_complete_transcript(dir.path(), "s.jsonl", &lines);
        let s = parse_transcript(&path).unwrap();
        assert_eq!(
            s.status,
            AgentStatus::Idle,
            "an old last message with a freshly-touched mtime must read Idle, not Waiting"
        );
    }

    #[test]
    fn transcript_tail_builds_chat_items() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"type":"user","timestamp":"2026-06-12T10:00:00Z","message":{"content":"add tests"}}"#,
            // Text before tools within one entry keeps its position.
            r#"{"type":"assistant","timestamp":"2026-06-12T10:00:05Z","message":{"content":[{"type":"text","text":"On it."},{"type":"tool_use","name":"Bash"}]}}"#,
            // Tool-result carrier - not a user message.
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-12T10:00:10Z","message":{"content":[{"type":"tool_use","name":"Bash"},{"type":"tool_use","name":"Edit"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-12T10:01:00Z","message":{"content":[{"type":"text","text":"Done — tests pass."}]}}"#,
        ];
        let path = write_transcript(dir.path(), "s.jsonl", &lines);
        let items = transcript_tail(&path, 50);
        let view: Vec<(&str, &str)> = items
            .iter()
            .map(|i| (i.role.as_str(), i.text.as_str()))
            .collect();
        assert_eq!(
            view,
            vec![
                ("user", "add tests"),
                ("assistant", "On it."),
                ("tools", "Bash ×2 · Edit"),
                ("assistant", "Done — tests pass."),
            ]
        );
        assert!(items[0].at.is_some());
        assert!(
            items[2].at.is_some(),
            "tools item carries the last tool's timestamp"
        );

        let last_two = transcript_tail(&path, 2);
        assert_eq!(last_two.len(), 2);
        assert_eq!(last_two[1].text, "Done — tests pass.");

        assert!(transcript_tail(&dir.path().join("nope.jsonl"), 10).is_empty());
    }

    #[test]
    fn transcript_pages_walk_to_the_first_message_without_truncating() {
        let dir = tempfile::tempdir().unwrap();
        let long = "x".repeat(5000);
        let lines = [
            r#"{"type":"user","message":{"content":"first"}}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"second"}]}}"#
                .to_string(),
            r#"{"type":"user","message":{"content":"third"}}"#.to_string(),
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{long}"}}]}}}}"#
            ),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let path = write_transcript(dir.path(), "paged.jsonl", &refs);

        let mut before = None;
        let mut collected = Vec::new();
        loop {
            let page = transcript_page_with_bytes(&path, before, 96);
            let mut items = page.items;
            items.append(&mut collected);
            collected = items;
            match page.next_before {
                Some(next) => {
                    if let Some(previous) = before {
                        assert!(next < previous, "the history cursor always moves backwards");
                    }
                    before = Some(next);
                }
                None => break,
            }
        }

        assert_eq!(collected.len(), 4);
        assert_eq!(collected[0].text, "first");
        assert_eq!(collected[1].text, "second");
        assert_eq!(collected[2].text, "third");
        assert_eq!(collected[3].text, long);
    }

    #[test]
    fn transcript_page_keeps_a_line_when_the_window_starts_at_its_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"type":"user","message":{"content":"first"}}"#,
            r#"{"type":"user","message":{"content":"second"}}"#,
            r#"{"type":"user","message":{"content":"third"}}"#,
        ];
        let path = write_transcript(dir.path(), "boundary.jsonl", &lines);
        let boundary = (lines[0].len() + 1) as u64;
        let page = transcript_page_with_bytes(
            &path,
            None,
            std::fs::metadata(&path).unwrap().len() - boundary,
        );

        assert_eq!(
            page.items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["second", "third"]
        );
        assert_eq!(page.next_before, Some(boundary));
    }

    #[test]
    fn last_message_is_truncated_and_survives_tool_turns() {
        let dir = tempfile::tempdir().unwrap();
        let long = "x".repeat(300);
        let lines = [
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{long}"}}]}}}}"#
            ),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#
                .to_string(),
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let path = write_complete_transcript(dir.path(), "s.jsonl", &refs);
        let s = parse_transcript(&path).unwrap();
        let msg = s.last_message.unwrap();
        assert_eq!(msg.chars().count(), 201);
        assert!(msg.starts_with("xxx") && msg.ends_with('…'));
    }

    #[test]
    fn running_when_mid_tool_loop() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"type":"user","cwd":"/code/proj","message":{"content":"go"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
        ];
        let path = write_complete_transcript(dir.path(), "s.jsonl", &lines);
        let s = parse_transcript(&path).unwrap();
        assert_eq!(s.status, AgentStatus::Running);
        assert!(!s.status.needs_you());
    }

    #[test]
    fn ended_turn_survives_the_idle_decay() {
        // The stall detector must distinguish "Idle because the turn ended long ago" (fine)
        // from "Idle because it froze mid-work" (stale) - `status` alone can't, since both
        // decay to Idle after IDLE_AFTER. `ended_turn` carries the fact through the decay.
        let dir = tempfile::tempdir().unwrap();

        // Old turn that ENDED (assistant text, no tool): Idle + ended_turn.
        let ended = [
            r#"{"type":"user","timestamp":"2020-01-01T00:00:00Z","message":{"content":"go"}}"#,
            r#"{"type":"assistant","timestamp":"2020-01-01T00:00:05Z","message":{"content":[{"type":"text","text":"All done."}]}}"#,
        ];
        let s =
            parse_transcript(&write_complete_transcript(dir.path(), "a.jsonl", &ended)).unwrap();
        assert_eq!(s.status, AgentStatus::Idle);
        assert!(s.ended_turn, "a finished turn must survive the Idle decay");

        // Old transcript frozen MID-TOOL-CALL: Idle + NOT ended_turn (the stale shape).
        let frozen = [
            r#"{"type":"user","timestamp":"2020-01-01T00:00:00Z","message":{"content":"go"}}"#,
            r#"{"type":"assistant","timestamp":"2020-01-01T00:00:05Z","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
        ];
        let s =
            parse_transcript(&write_complete_transcript(dir.path(), "b.jsonl", &frozen)).unwrap();
        assert_eq!(s.status, AgentStatus::Idle);
        assert!(!s.ended_turn, "frozen mid-tool is not a finished turn");

        let fresh = [
            r#"{"type":"user","message":{"content":"go"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}"#,
        ];
        let s =
            parse_transcript(&write_complete_transcript(dir.path(), "c.jsonl", &fresh)).unwrap();
        assert_eq!(s.status, AgentStatus::Waiting);
        assert!(s.ended_turn);
    }

    #[test]
    fn summary_for_root_finds_encoded_dir() {
        let root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/code/pos-saas");
        let enc = root.path().join(encode_project_dir(cwd));
        write_complete_transcript(
            &enc,
            "sess.jsonl",
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#],
        );
        let s = summary_for_root(root.path(), cwd).unwrap();
        assert_eq!(s.status, AgentStatus::Waiting);
    }

    #[test]
    fn summary_for_root_falls_back_to_cwd_match() {
        let root = tempfile::tempdir().unwrap();
        // A dir name that does NOT match our encoding, but whose transcript records the cwd.
        write_complete_transcript(
            &root.path().join("weird-legacy-name"),
            "sess.jsonl",
            &[r#"{"type":"user","cwd":"/code/montage","message":{"content":"x"}}"#],
        );
        let s = summary_for_root(root.path(), Path::new("/code/montage"));
        assert!(
            s.is_some(),
            "should match by recorded cwd when the dir name differs"
        );
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn append_resumes_after_complete_prefix_and_defers_partial_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let first = "{\"type\":\"assistant\",\"timestamp\":\"2026-09-01T00:00:00Z\",\"message\":{\"content\":[{\"type\":\"tool_use\"}]}}\n";
        std::fs::write(&path, first).unwrap();
        assert_eq!(parse_transcript(&path).unwrap().tool_call_count, 1);
        // Poison the aggregate rather than modifying the file: a full reread would restore 1.
        cache()
            .lock()
            .unwrap()
            .get_mut(&path)
            .unwrap()
            .state
            .tool_call_count = 100;
        let partial =
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\"}]}}";
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(partial.as_bytes()).unwrap();
        assert_eq!(parse_transcript(&path).unwrap().tool_call_count, 100);
        assert_eq!(
            cache().lock().unwrap().get(&path).unwrap().offset,
            first.len() as u64
        );
        file.write_all(b"\n").unwrap();
        assert_eq!(parse_transcript(&path).unwrap().tool_call_count, 101);
        assert_eq!(
            transcript_activity(&path).unwrap().to_rfc3339(),
            "2026-09-01T00:00:00+00:00"
        );
        std::fs::write(&path, first).unwrap();
        assert_eq!(parse_transcript(&path).unwrap().tool_call_count, 1);
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, format!("{first}{first}")).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(parse_transcript(&path).unwrap().tool_call_count, 2);
    }
}
