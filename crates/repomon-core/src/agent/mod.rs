//! Agent runtime and monitors.
//!
//! [`tmux`] provides the durable, tmux-backed runtime (spawn/capture/send/kill). [`claude`]
//! monitors Claude Code session transcripts to derive status and "needs you". Both Codex and
//! Aider (M10) implement the same [`AgentMonitor`] trait.

pub mod claude;
pub mod tmux;

use std::path::Path;

use crate::model::AgentKind;

pub use claude::TranscriptSummary;
pub use tmux::{shell_quote, TmuxRuntime};

/// A read-only monitor that summarizes an agent's session for a given worktree.
pub trait AgentMonitor: Send + Sync {
    /// Which agent kind this monitor observes.
    fn kind(&self) -> AgentKind;
    /// Summarize the agent's session running in `cwd`, if any.
    fn summary_for(&self, cwd: &Path) -> Option<TranscriptSummary>;
}

/// Monitors Claude Code sessions.
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
