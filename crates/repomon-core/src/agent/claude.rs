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
            status: self.status,
            external: false, // overlay flips this based on tmux ownership
            session_id: self.session_id,
            resume_at: None,
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
pub fn config_bases() -> Vec<PathBuf> {
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
                    format!("CLAUDE_CONFIG_DIR={} claude", base.display()),
                )
            }
        })
        .collect()
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
                        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                            tool_call_count += 1;
                            has_tool = true;
                        }
                    }
                }
                last_type = Some("assistant");
                last_assistant_has_tool = has_tool;
            }
            Some("user") => {
                last_type = Some("user");
                if title.is_none() {
                    title = user_text(&v).map(|t| truncate(&t, 60));
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
