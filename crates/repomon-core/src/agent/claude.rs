//! Claude Code session monitor.
//!
//! Claude Code records each session as a JSONL transcript under
//! `~/.claude/projects/<encoded-cwd>/<session>.jsonl`, where the directory name is the
//! working directory with `/` and `.` replaced by `-`. That encoding has changed before, so
//! it's isolated in [`encode_project_dir`] and covered by a fixture test; matching also
//! falls back to reading each transcript's recorded `cwd`.
//!
//! From the transcript we derive: tool-call count, last activity, a title, and the
//! all-important status — **Waiting** (the agent finished its turn and needs you) vs
//! **Running** (mid tool-loop) vs **Idle** (gone quiet).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::model::{AgentKind, AgentSession, AgentStatus, RepoId, WorktreeId};

/// How long with no transcript activity before we consider a session idle.
const IDLE_AFTER: Duration = Duration::minutes(10);

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
    /// The agent's most recent message text — what it said (or asked) when it last ended a
    /// turn. This is the "why" behind a needs-you notification.
    pub last_message: Option<String>,
    /// The Claude config dir this session belongs to, when it isn't the default `~/.claude`
    /// (e.g. a work account run with `CLAUDE_CONFIG_DIR=~/.claude-work`). Drives adopt.
    pub config_dir: Option<PathBuf>,
    /// The session id (transcript filename stem) — lets adopt resume *this* exact session
    /// (`claude --resume <id>`) when several run in one worktree.
    pub session_id: Option<String>,
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
            status: self.status,
            external: false, // overlay flips this based on tmux ownership
            session_id: self.session_id,
            tmux_window: None, // overlay pairs managed sessions with their windows
            resume_at: None,
            inferred: false,
            config_dir: self.config_dir,
            custom_label: None, // overlay sets this from the session_labels store
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

/// All Claude config dirs to consider: the default `~/.claude`, any `~/.claude-*` that holds a
/// `projects/` dir (e.g. a separate work account run with `CLAUDE_CONFIG_DIR=~/.claude-work`),
/// and an explicit `$CLAUDE_CONFIG_DIR` if set. Each contains a `projects/` subdir.
///
/// Cached with a short TTL: this is called per-lane on every `lane.list`, and the underlying
/// `read_dir($HOME)` is the dominant cost there. The set of config dirs changes ~never, so a
/// process-global cache (re-scanned every ~45 s) turns ~10 `$HOME` scans per refresh into ~0.
pub fn config_bases() -> Vec<PathBuf> {
    use std::time::{Duration, Instant};
    // Tests mutate env / home and expect immediate results — never cache there.
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

/// Spawnable Claude agents, one per detected config dir: `(name, launch command)`. The default
/// account is `("claude-code", "claude")`; a `~/.claude-work` dir becomes
/// `("claude-work", "CLAUDE_CONFIG_DIR=/…/.claude-work claude")`.
pub fn agent_variants() -> Vec<(String, String)> {
    let default = default_config_base();
    config_bases()
        .into_iter()
        .map(|base| {
            if base == default {
                ("claude-code".to_string(), "claude".to_string())
            } else {
                let label = base
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.trim_start_matches('.').to_string())
                    .unwrap_or_else(|| "claude".to_string());
                (
                    label,
                    // Runs via `sh -c` (tmux new-window), so quote the path — a config dir with a
                    // space or shell metacharacter must not break the `VAR=val cmd` parse or inject.
                    format!(
                        "CLAUDE_CONFIG_DIR={} claude",
                        super::shell_quote(&base.display().to_string())
                    ),
                )
            }
        })
        .collect()
}

/// A stable key for a Claude account, identifying it by its config dir. The default account
/// (`~/.claude`, carried as `config_dir: None`) is `"default"`; a variant is its dir path. The
/// usage probe stores per-account usage under this key and a client matches the focused agent's
/// `AgentSession::config_dir` to it.
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

/// Encode a working directory to Claude Code's project directory name.
pub fn encode_project_dir(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
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

/// Process-global memo for [`parse_transcript`], keyed by path and invalidated by file mtime.
fn cache() -> &'static Mutex<HashMap<PathBuf, (SystemTime, TranscriptSummary)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (SystemTime, TranscriptSummary)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Parse a transcript into a summary, memoised by file mtime.
///
/// `summary_for`/`summaries_for` call this on every fleet refresh (~1/s in live views). Without
/// the memo, an idle-but-recent session's whole JSONL is re-read and re-parsed each time. The
/// cache is keyed by path and invalidated when the file's mtime changes, so it stays correct
/// while making repeated refreshes of unchanged transcripts nearly free.
pub fn parse_transcript(path: &Path) -> Option<TranscriptSummary> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Some(mtime) = mtime {
        if let Ok(c) = cache().lock() {
            if let Some((cached, summary)) = c.get(path) {
                if *cached == mtime {
                    let mut s = summary.clone();
                    // `status` is the one *time*-derived field: a transcript that stops changing
                    // still decays to Idle after IDLE_AFTER even though its mtime — our cache key
                    // — never moves again. The content-derived Waiting/Running stays valid while
                    // the file is unchanged, so only the idle transition needs re-applying here.
                    if Utc::now() - s.last_activity > IDLE_AFTER {
                        s.status = AgentStatus::Idle;
                    }
                    return Some(s);
                }
            }
        }
    }
    let summary = parse_transcript_inner(path)?;
    if let Some(mtime) = mtime {
        if let Ok(mut c) = cache().lock() {
            // Bound memory: transcript paths accumulate as sessions end. A periodic clear keeps
            // the map from growing without limit — it simply re-warms on the next refresh.
            if c.len() >= 1024 {
                c.clear();
            }
            c.insert(path.to_path_buf(), (mtime, summary.clone()));
        }
    }
    Some(summary)
}

/// Parse a transcript into a summary (uncached — see [`parse_transcript`]).
fn parse_transcript_inner(path: &Path) -> Option<TranscriptSummary> {
    let text = std::fs::read_to_string(path).ok()?;
    let last_activity: DateTime<Utc> = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now());

    let mut tool_call_count = 0u32;
    let mut last_type: Option<&str> = None;
    let mut last_assistant_has_tool = false;
    let mut title: Option<String> = None;
    let mut last_message: Option<String> = None;
    let mut cwd: Option<PathBuf> = None;

    for line in text.lines() {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if cwd.is_none() {
            if let Some(c) = v.get("cwd").and_then(Value::as_str) {
                cwd = Some(PathBuf::from(c));
            }
        }
        match v.get("type").and_then(Value::as_str) {
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
                                tool_call_count += 1;
                                has_tool = true;
                            }
                            Some("text") => {
                                if let Some(t) = block.get("text").and_then(Value::as_str) {
                                    let t = t.trim();
                                    if !t.is_empty() {
                                        last_message = Some(truncate(t, 200));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                last_type = Some("assistant");
                last_assistant_has_tool = has_tool;
            }
            Some("user") => {
                last_type = Some("user");
                // Title from the first *real* prompt — skip Claude Code's injected scaffolding
                // (the local-command caveat, slash-command invocations, local-command stdout),
                // which would otherwise show up as "<local-command-caveat>Caveat: …".
                if title.is_none() {
                    if let Some(t) = user_text(&v) {
                        let t = t.trim();
                        if !t.is_empty() && !is_synthetic_user_text(t) {
                            title = Some(truncate(t, 60));
                        }
                    }
                }
            }
            Some("summary") => {
                if let Some(s) = v.get("summary").and_then(Value::as_str) {
                    title = Some(truncate(s, 60));
                }
            }
            _ => {}
        }
    }

    let status = if Utc::now() - last_activity > IDLE_AFTER {
        AgentStatus::Idle
    } else if last_type == Some("assistant") && !last_assistant_has_tool {
        // The agent spoke and issued no tool call — it's waiting on you.
        AgentStatus::Waiting
    } else {
        AgentStatus::Running
    };

    Some(TranscriptSummary {
        kind: AgentKind::ClaudeCode,
        manifest_path: path.to_path_buf(),
        cwd,
        last_activity,
        tool_call_count,
        status,
        title,
        last_message,
        config_dir: None, // set by the caller based on which config dir it came from
        session_id: path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string()),
    })
}

/// Find and summarize the Claude session for `cwd` under `root`.
pub fn summary_for_root(root: &Path, cwd: &Path) -> Option<TranscriptSummary> {
    // Primary: the encoded project directory.
    let encoded = root.join(encode_project_dir(cwd));
    if encoded.is_dir() {
        if let Some(t) = newest_transcript_in(&encoded) {
            if let Some(s) = parse_transcript(&t) {
                return Some(s);
            }
        }
    }
    // Fallback (encoding drift): scan every project dir and match the recorded cwd.
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

/// Summarize the Claude session for `cwd` — the hot path, used per-lane on every refresh.
///
/// Only the encoded project directory is consulted (an O(1) lookup), so a worktree with no
/// Claude session doesn't trigger an expensive scan of every project dir. For the
/// encoding-drift fallback, call [`summary_for_root`] explicitly.
pub fn summary_for(cwd: &Path) -> Option<TranscriptSummary> {
    let encoded = encode_project_dir(cwd);

    // Test override: a single projects dir, treated as the default account.
    if let Ok(p) = std::env::var("REPOMON_CLAUDE_PROJECTS") {
        let dir = PathBuf::from(p).join(&encoded);
        return newest_transcript_in(&dir).and_then(|t| parse_transcript(&t));
    }

    // Scan every config dir's encoded project subdir (usually 1-2), keeping the most recent —
    // so a work-account session in `~/.claude-work` is detected alongside the default account.
    let default = default_config_base();
    let mut best: Option<TranscriptSummary> = None;
    for base in config_bases() {
        let dir = base.join("projects").join(&encoded);
        if !dir.is_dir() {
            continue;
        }
        if let Some(t) = newest_transcript_in(&dir) {
            if let Some(mut s) = parse_transcript(&t) {
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
    }
    best
}

/// Summarize *every* recently-active Claude session for `cwd` — one per transcript — across
/// all config dirs, newest first, capped at `max`. This is what lets repomon show several
/// concurrent agents in one worktree (each is a distinct `<session-id>.jsonl`) rather than
/// only the newest. "Recently active" means the transcript changed within `within`.
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
    let encoded = encode_project_dir(cwd);
    let file = format!("{session_id}.jsonl");
    let default = default_config_base();
    for base in config_bases() {
        if base.join("projects").join(&encoded).join(&file).is_file() {
            return Some((base != default).then_some(base));
        }
    }
    None
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Whether a user message is Claude Code's injected scaffolding rather than a real prompt — the
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

/// How much of a transcript's tail to read for the chat view — bounds the cost of polling a
/// long session (this file can be many MB).
const TAIL_BYTES: u64 = 512 * 1024;
/// Cap per-item text so payloads stay bounded (full messages, not titles).
const ITEM_TEXT_MAX: usize = 4000;

/// The last `max_items` conversation items from a transcript: user/assistant messages with
/// their full unwrapped text, tool calls between messages aggregated into one "tools" item
/// ("Bash ×2 · Edit"). The mobile client renders these natively instead of a desktop-width
/// pane capture. Only the file tail is read.
pub fn transcript_tail(path: &Path, max_items: usize) -> Vec<crate::model::TranscriptItem> {
    use crate::model::TranscriptItem;
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
        lines.next(); // the seek likely landed mid-line — drop the partial one
    }

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
                                        text: truncate(t, ITEM_TEXT_MAX),
                                        at,
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
                // Real user text only — tool_result carriers return None here.
                if let Some(t) = user_text(&v) {
                    let t = t.trim().to_string();
                    if !t.is_empty() {
                        flush_tools(&mut items, &mut tools, &mut tools_at);
                        items.push(TranscriptItem {
                            role: "user".into(),
                            text: truncate(&t, ITEM_TEXT_MAX),
                            at,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    flush_tools(&mut items, &mut tools, &mut tools_at);

    if items.len() > max_items {
        items.drain(..items.len() - max_items);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_cwd_like_claude_code() {
        assert_eq!(
            encode_project_dir(Path::new("/Users/azaleas/Developer/Claude/repomon")),
            "-Users-azaleas-Developer-Claude-repomon"
        );
        // Dots become dashes too.
        assert_eq!(
            encode_project_dir(Path::new("/Users/x/.config/app")),
            "-Users-x--config-app"
        );
    }

    fn write_transcript(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    #[test]
    fn parse_transcript_memoises_by_mtime() {
        let root = tempfile::tempdir().unwrap();
        let line = r#"{"type":"user","cwd":"/code/x","message":{"content":"hello"}}"#;
        let path = write_transcript(root.path(), "sess.jsonl", &[line]);

        // First parse populates the cache.
        let s1 = parse_transcript(&path).expect("parses");

        // Poison the cached summary, then parse again with the file unchanged: a cache hit must
        // return the poisoned value (proving it did not re-read the file).
        {
            let mut c = cache().lock().unwrap();
            c.get_mut(&path).unwrap().1.title = Some("SENTINEL".into());
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
            c.get_mut(&path).unwrap().0 = SystemTime::UNIX_EPOCH;
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
        let path = write_transcript(root.path(), "idle.jsonl", &[line]);
        assert_eq!(
            parse_transcript(&path).unwrap().status,
            AgentStatus::Waiting
        );

        // Backdate the cached summary's last_activity past IDLE_AFTER *without* touching the file
        // (so its mtime — our cache key — is unchanged). The next call is a cache hit that must
        // still report Idle: status decays by the clock, not by a file change.
        {
            let mut c = cache().lock().unwrap();
            c.get_mut(&path).unwrap().1.last_activity = Utc::now() - Duration::minutes(20);
        }
        assert_eq!(
            parse_transcript(&path).unwrap().status,
            AgentStatus::Idle,
            "a frozen transcript still decays to Idle on a cache hit"
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
        // real prompt — not "<local-command-caveat>Caveat: …".
        let root = tempfile::tempdir().unwrap();
        let caveat = r#"{"type":"user","message":{"content":"<local-command-caveat>Caveat: generated while running local commands"}}"#;
        let real = r#"{"type":"user","message":{"content":"Refactor the parser to stream"}}"#;
        let path = write_transcript(root.path(), "caveat.jsonl", &[caveat, real]);
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
            write_transcript(&dir, &format!("{id}.jsonl"), &[line]);
        }

        std::env::set_var("REPOMON_CLAUDE_PROJECTS", root.path());
        let all = summaries_for(cwd, Duration::hours(6), 8);
        let capped = summaries_for(cwd, Duration::hours(6), 2);
        let single = summary_for(cwd);
        std::env::remove_var("REPOMON_CLAUDE_PROJECTS");

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
    fn waiting_when_last_entry_is_assistant_text() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = "/code/proj";
        let lines = [
            r#"{"type":"user","cwd":"/code/proj","message":{"content":"add tests"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done — want me to also..."}]}}"#,
        ];
        let path = write_transcript(dir.path(), "s.jsonl", &lines);
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
    fn transcript_tail_builds_chat_items() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"type":"user","timestamp":"2026-06-12T10:00:00Z","message":{"content":"add tests"}}"#,
            // Text before tools within one entry keeps its position.
            r#"{"type":"assistant","timestamp":"2026-06-12T10:00:05Z","message":{"content":[{"type":"text","text":"On it."},{"type":"tool_use","name":"Bash"}]}}"#,
            // Tool-result carrier — not a user message.
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

        // The limit keeps the newest items.
        let last_two = transcript_tail(&path, 2);
        assert_eq!(last_two.len(), 2);
        assert_eq!(last_two[1].text, "Done — tests pass.");

        // Missing file → empty, not an error.
        assert!(transcript_tail(&dir.path().join("nope.jsonl"), 10).is_empty());
    }

    #[test]
    fn last_message_is_truncated_and_survives_tool_turns() {
        let dir = tempfile::tempdir().unwrap();
        let long = "x".repeat(300);
        let lines = [
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{long}"}}]}}}}"#
            ),
            // A later tool-only turn must not erase the question text.
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#
                .to_string(),
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let path = write_transcript(dir.path(), "s.jsonl", &refs);
        let s = parse_transcript(&path).unwrap();
        let msg = s.last_message.unwrap();
        assert_eq!(msg.chars().count(), 201); // 200 kept + ellipsis
        assert!(msg.starts_with("xxx") && msg.ends_with('…'));
    }

    #[test]
    fn running_when_mid_tool_loop() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"type":"user","cwd":"/code/proj","message":{"content":"go"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
        ];
        let path = write_transcript(dir.path(), "s.jsonl", &lines);
        let s = parse_transcript(&path).unwrap();
        assert_eq!(s.status, AgentStatus::Running);
        assert!(!s.status.needs_you());
    }

    #[test]
    fn summary_for_root_finds_encoded_dir() {
        let root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/code/pos-saas");
        let enc = root.path().join(encode_project_dir(cwd));
        write_transcript(
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
        write_transcript(
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
