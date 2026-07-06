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
    /// Finished its turn AND the work looks shippable (worktree clean, a fresh commit): the
    /// "review / merge?" hint. Only [`agent_attention_in`] produces this — it needs the lane.
    DoneCandidate,
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
            Attention::DoneCandidate => "done_candidate",
            Attention::Permission => "permission",
            Attention::Decision => "decision",
        }
    }
    /// Does this state want a human/orchestrator to act?
    pub fn needs_you(self) -> bool {
        !matches!(self, Attention::None)
    }
    /// Sort weight: higher = more urgent. A decision (only a human can answer) outranks a
    /// permission ask (auto-answerable) outranks a done-candidate (a concrete review/merge
    /// action) outranks a bare end-of-turn.
    pub fn priority(self) -> u8 {
        match self {
            Attention::None => 0,
            Attention::EndOfTurn => 2,
            Attention::DoneCandidate => 3,
            Attention::Permission => 4,
            Attention::Decision => 5,
        }
    }
}

/// How long before the agent's last words a commit still counts as "this turn's work".
/// (`AgentSession.started_at` mirrors the last activity, so the commit-vs-turn comparison
/// anchors on `last_activity_at` with this much slack: the commit lands minutes before the
/// final "done" message.)
const DONE_COMMIT_SLACK: chrono::Duration = chrono::Duration::minutes(30);

/// Derive an agent's attention from its status and any open dialog. Lane-blind: never
/// produces [`Attention::DoneCandidate`] — use [`agent_attention_in`] when the lane is at
/// hand.
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

/// [`agent_attention`] refined with the lane's git state: an end-of-turn whose work looks
/// shippable — worktree clean and a commit landed with this turn — reads as
/// [`Attention::DoneCandidate`] ("review?") instead of a bare end-of-turn.
pub fn agent_attention_in(lane: &Lane, s: &AgentSession) -> Attention {
    match agent_attention(s) {
        Attention::EndOfTurn if work_ready(lane, s) => Attention::DoneCandidate,
        a => a,
    }
}

/// The lane's work looks shippable: nothing uncommitted, and the last commit belongs to this
/// turn (it landed no earlier than [`DONE_COMMIT_SLACK`] before the agent's last words).
fn work_ready(lane: &Lane, s: &AgentSession) -> bool {
    lane.state.dirty.is_clean()
        && lane
            .state
            .last_commit_at
            .is_some_and(|t| t >= s.last_activity_at - DONE_COMMIT_SLACK)
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
            stale: false,
            stalled_since: None,
            ended_turn: false,
            tmux_window: Some("lane-1".into()),
            last_message: None,
            pending_prompt: pending.map(str::to_string),
            pending_dialog: None,
            config_dir: None,
            custom_label: None,
        }
    }

    /// A lane whose git state is controllable: `clean` worktree and an optional last commit
    /// time (relative to now, in minutes; negative = in the past).
    fn lane(clean: bool, commit_mins_ago: Option<i64>) -> Lane {
        let head = gix::ObjectId::null(gix::hash::Kind::Sha1);
        let mut dirty = crate::model::DirtyState::default();
        if !clean {
            dirty.unstaged = 1;
        }
        Lane {
            id: 1,
            repo: crate::model::Repo {
                id: 1,
                path: PathBuf::from("/r"),
                name: "r".into(),
                added_at: Utc::now(),
                worktree_root_template: None,
            },
            worktree: crate::model::Worktree {
                id: 1,
                repo_id: 1,
                path: PathBuf::from("/r"),
                branch: Some("main".into()),
                head,
                is_main: true,
                name: "main".into(),
            },
            state: crate::model::WorktreeState {
                worktree_id: 1,
                head,
                branch: Some("main".into()),
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty,
                last_commit_at: commit_mins_ago.map(|m| Utc::now() - chrono::Duration::minutes(m)),
                locked: false,
                prunable: false,
                last_change_at: None,
            },
            agent_sessions: vec![],
            last_activity_at: Utc::now(),
            pinned: false,
        }
    }

    #[test]
    fn done_candidate_needs_ended_turn_plus_clean_plus_fresh_commit() {
        let eot = sess(AgentStatus::Waiting, None);

        // Clean worktree + a commit from this turn → the review hint.
        assert_eq!(
            agent_attention_in(&lane(true, Some(5)), &eot),
            Attention::DoneCandidate
        );
        // Dirty worktree: work isn't shippable, stay a bare end-of-turn.
        assert_eq!(
            agent_attention_in(&lane(false, Some(5)), &eot),
            Attention::EndOfTurn
        );
        // Clean but the last commit long predates this turn (analysis-only session).
        assert_eq!(
            agent_attention_in(&lane(true, Some(600)), &eot),
            Attention::EndOfTurn
        );
        // No commit at all.
        assert_eq!(
            agent_attention_in(&lane(true, None), &eot),
            Attention::EndOfTurn
        );
        // Dialog states pass through untouched.
        assert_eq!(
            agent_attention_in(
                &lane(true, Some(5)),
                &sess(AgentStatus::Waiting, Some("Do you want to proceed?"))
            ),
            Attention::Permission
        );
        assert_eq!(
            agent_attention_in(&lane(true, Some(5)), &sess(AgentStatus::Running, None)),
            Attention::None
        );
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
    fn decision_outranks_permission_outranks_done_outranks_end_of_turn() {
        assert!(Attention::Decision.priority() > Attention::Permission.priority());
        assert!(Attention::Permission.priority() > Attention::DoneCandidate.priority());
        assert!(Attention::DoneCandidate.priority() > Attention::EndOfTurn.priority());
        assert!(Attention::EndOfTurn.priority() > Attention::None.priority());
        assert!(Attention::DoneCandidate.needs_you());
    }
}
