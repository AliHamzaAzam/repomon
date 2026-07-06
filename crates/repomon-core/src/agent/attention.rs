//! What an agent needs from a human or orchestrator, derived from its status and any open
//! dialog. One shared taxonomy: the TUI's badges, the daemon's notifications, and the MCP
//! orchestrator's fleet digests all classify a `Waiting` session the same way.

use serde::{Deserialize, Serialize};

use crate::agent::prompt::{PromptClass, classify_prompt};
use crate::model::{AgentSession, AgentStatus, Lane};

/// What an agent needs from a human/orchestrator, derived from status + `pending_prompt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attention {
    /// Nothing — running, rate-limited (auto-continue handles it), idle, or ended.
    None,
    /// Finished its turn with no open dialog; awaiting the next instruction.
    EndOfTurn,
    /// Sitting on a routine permission dialog it raised about its own next tool call.
    Permission,
    /// Sitting on a real decision-question it is deferring to a human.
    Decision,
}

impl Attention {
    pub fn as_str(self) -> &'static str {
        match self {
            Attention::None => "none",
            Attention::EndOfTurn => "end_of_turn",
            Attention::Permission => "permission",
            Attention::Decision => "decision",
        }
    }
    /// Does this state want a human/orchestrator to act?
    pub fn needs_you(self) -> bool {
        !matches!(self, Attention::None)
    }
    /// Sort weight: higher = more urgent. A decision (only a human can answer) outranks a
    /// permission ask (auto-answerable) outranks a bare end-of-turn.
    pub fn priority(self) -> u8 {
        match self {
            Attention::None => 0,
            Attention::EndOfTurn => 2,
            Attention::Permission => 3,
            Attention::Decision => 4,
        }
    }
}

/// Derive an agent's attention from its status and any open dialog.
pub fn agent_attention(s: &AgentSession) -> Attention {
    match s.status {
        AgentStatus::Waiting => match s.pending_prompt.as_deref() {
            Some(p) => match classify_prompt(p) {
                PromptClass::Permission => Attention::Permission,
                PromptClass::Decision => Attention::Decision,
            },
            None => Attention::EndOfTurn,
        },
        _ => Attention::None,
    }
}

/// Pick the agent that most wants attention (waiting/decision first, then running, then idle),
/// preferring managed sessions when otherwise tied.
pub fn primary_agent(lane: &Lane) -> Option<&AgentSession> {
    lane.agent_sessions.iter().max_by_key(|s| agent_rank(s))
}

fn agent_rank(s: &AgentSession) -> (u8, u8, bool) {
    let status_rank = match s.status {
        AgentStatus::Waiting => 3,
        AgentStatus::Running => 2,
        AgentStatus::RateLimited => 1,
        AgentStatus::Idle | AgentStatus::Ended => 0,
    };
    (
        agent_attention(s).priority(),
        status_rank,
        s.tmux_window.is_some(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentKind;
    use chrono::Utc;
    use std::path::PathBuf;

    fn sess(status: AgentStatus, pending: Option<&str>) -> AgentSession {
        AgentSession {
            id: 1,
            agent: AgentKind::ClaudeCode,
            repo_id: 1,
            worktree_id: Some(1),
            started_at: Utc::now(),
            last_activity_at: Utc::now(),
            ended_at: None,
            manifest_path: PathBuf::from("/tmp/x.jsonl"),
            tool_call_count: 0,
            title: None,
            status,
            external: false,
            session_id: None,
            resume_at: None,
            inferred: false,
            tmux_window: Some("lane-1".into()),
            last_message: None,
            pending_prompt: pending.map(str::to_string),
            pending_dialog: None,
            config_dir: None,
            custom_label: None,
        }
    }

    #[test]
    fn attention_reflects_status_and_prompt() {
        assert_eq!(
            agent_attention(&sess(AgentStatus::Running, None)),
            Attention::None
        );
        assert_eq!(
            agent_attention(&sess(AgentStatus::RateLimited, None)),
            Attention::None
        );
        // Waiting with no dialog = ended its turn, awaiting next instruction.
        assert_eq!(
            agent_attention(&sess(AgentStatus::Waiting, None)),
            Attention::EndOfTurn
        );
        // Waiting on a permission dialog = auto-answerable.
        assert_eq!(
            agent_attention(&sess(
                AgentStatus::Waiting,
                Some("Bash command — Do you want to proceed?")
            )),
            Attention::Permission
        );
        // Waiting on a real question = must escalate.
        assert_eq!(
            agent_attention(&sess(
                AgentStatus::Waiting,
                Some("Which auth method should we use?")
            )),
            Attention::Decision
        );
    }

    #[test]
    fn decision_outranks_permission_outranks_end_of_turn() {
        assert!(Attention::Decision.priority() > Attention::Permission.priority());
        assert!(Attention::Permission.priority() > Attention::EndOfTurn.priority());
        assert!(Attention::EndOfTurn.priority() > Attention::None.priority());
    }
}
