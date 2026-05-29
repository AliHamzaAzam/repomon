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

use std::path::{Path, PathBuf};

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

/// Parse a transcript into a summary.
pub fn parse_transcript(path: &Path) -> Option<TranscriptSummary> {
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
