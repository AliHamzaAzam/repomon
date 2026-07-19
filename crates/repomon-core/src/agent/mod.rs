//! Agent runtime and monitors.
//!
//! [`tmux`] provides the durable, tmux-backed runtime (spawn/capture/send/kill). The
//! [`AgentMonitor`] trait observes an agent's session for a worktree; [`ClaudeMonitor`]
//! reads Claude Code transcripts (rich status incl. "needs you"), while [`AiderMonitor`] and
//! [`CodexMonitor`] are best-effort (see `docs/agents.md`). For any repomon-spawned agent the
//! daemon also falls back to "is the tmux window alive?".

pub mod attention;
pub mod claude;
pub mod gate;
pub mod limit;
pub mod prompt;
pub mod text;
pub mod tmux;
pub mod usage;

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::model::{AgentKind, AgentStatus};

pub use claude::TranscriptSummary;
pub use limit::{LimitMenu, UsageLimit, detect_usage_limit, menu_select_keys};
pub use tmux::{TmuxRuntime, WindowMeta, shell_quote};
pub use usage::{AccountUsage, UsageReport, UsageWindow, parse_codex_status, parse_usage};

/// How recently a file must have changed for its agent to count as "running".
const ACTIVE_WINDOW: Duration = Duration::from_secs(120);

/// A read-only monitor that summarizes an agent's session for a given worktree.
pub trait AgentMonitor: Send + Sync {
    /// Which agent kind this monitor observes.
    fn kind(&self) -> AgentKind;
    /// Summarize the agent's session running in `cwd`, if any.
    fn summary_for(&self, cwd: &Path) -> Option<TranscriptSummary>;
}

/// The monitors the daemon consults, in priority order.
pub fn default_monitors() -> Vec<Box<dyn AgentMonitor>> {
    vec![
        Box::new(ClaudeMonitor),
        Box::new(AiderMonitor),
        Box::new(CodexMonitor),
    ]
}

/// Summarize a worktree's agent by trying each monitor in turn.
pub fn summary_for(cwd: &Path) -> Option<TranscriptSummary> {
    default_monitors()
        .into_iter()
        .find_map(|m| m.summary_for(cwd))
}

/// Monitors Claude Code sessions (rich status from transcripts).
#[derive(Debug, Clone, Default)]
pub struct ClaudeMonitor;

impl AgentMonitor for ClaudeMonitor {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }
    fn summary_for(&self, cwd: &Path) -> Option<TranscriptSummary> {
        claude::summary_for(cwd)
    }
}

/// Monitors Aider via its repo-local chat history file.
#[derive(Debug, Clone, Default)]
pub struct AiderMonitor;

impl AgentMonitor for AiderMonitor {
    fn kind(&self) -> AgentKind {
        AgentKind::Aider
    }
    fn summary_for(&self, cwd: &Path) -> Option<TranscriptSummary> {
        // Aider records the conversation to `.aider.chat.history.md` in the working dir.
        let history = cwd.join(".aider.chat.history.md");
        activity_summary(AgentKind::Aider, &history)
    }
}

/// Best-effort Codex monitor. Codex's on-disk session format isn't stable enough to parse
/// reliably yet, so this returns `None` and the daemon relies on the tmux-alive fallback for
/// repomon-spawned Codex agents. See `docs/agents.md`.
#[derive(Debug, Clone, Default)]
pub struct CodexMonitor;

impl AgentMonitor for CodexMonitor {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }
    fn summary_for(&self, _cwd: &Path) -> Option<TranscriptSummary> {
        None
    }
}

/// Build a coarse summary from a file's modification time: Running if it changed recently,
/// otherwise Idle. Used for agents whose detailed state we can't (yet) parse.
fn activity_summary(kind: AgentKind, manifest: &Path) -> Option<TranscriptSummary> {
    let mtime: DateTime<Utc> = std::fs::metadata(manifest)
        .and_then(|m| m.modified())
        .ok()?
        .into();
    let status = if Utc::now()
        .signed_duration_since(mtime)
        .to_std()
        .map(|d| d < ACTIVE_WINDOW)
        .unwrap_or(false)
    {
        AgentStatus::Running
    } else {
        AgentStatus::Idle
    };
    Some(TranscriptSummary {
        kind,
        manifest_path: manifest.to_path_buf(),
        cwd: None,
        last_activity: mtime,
        tool_call_count: 0,
        status,
        title: None,
        last_message: None,
        config_dir: None,
        session_id: None,
        ended_turn: false, // mtime-only monitors can't see turn boundaries
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aider_monitor_detects_recent_history() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".aider.chat.history.md"), "# chat\n").unwrap();
        let s = AiderMonitor.summary_for(dir.path()).expect("aider session");
        assert_eq!(s.kind, AgentKind::Aider);
        assert_eq!(s.status, AgentStatus::Running);
    }

    #[test]
    fn aider_monitor_none_without_history() {
        let dir = tempfile::tempdir().unwrap();
        assert!(AiderMonitor.summary_for(dir.path()).is_none());
    }
}
