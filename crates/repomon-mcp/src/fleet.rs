//! The fleet snapshot: a compact, token-economical projection of the daemon's `lane.list`,
//! refreshed by an internal poll-and-diff loop and exposed to the orchestrator's tools.
//!
//! Why poll instead of subscribe: the single most important transition — `Running → Waiting`
//! ("needs you") plus the `pending_prompt` "why" — is computed lazily inside the daemon's
//! `lane.list` overlay and is *not* pushed on the event stream (and `event.notification` only
//! fires when the remote bridge is enabled). So, exactly like the TUI, we call `lane.list` on a
//! modest cadence (~1.5s, riding the daemon's 750ms overlay cache) and diff the result. A
//! `watch` channel carries a generation counter that bumps only on a meaningful change, so
//! `wait_for_change` can sleep until a real transition instead of busy-polling.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use repomon_core::client::DaemonClient;
use repomon_core::model::{AgentSession, AgentStatus, Lane};
use repomon_core::protocol::Notification;
use serde::Serialize;
use tokio::sync::{Mutex, broadcast, watch};

// The attention taxonomy grew out of this module and now lives in core, shared with the TUI's
// badges and the daemon's notifications; re-exported so orchestrator-side callers don't churn.
pub use repomon_core::agent::attention::{
    Attention, agent_attention, agent_attention_in, primary_agent,
};

/// A single agent session, projected to the fields an orchestrator reasons about.
#[derive(Debug, Clone, Serialize)]
pub struct AgentDigest {
    pub kind: String,
    pub status: String,
    pub attention: Attention,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    pub idle_secs: i64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub external: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inferred: bool,
    /// The agent looks stuck: alive but neither pane nor transcript has moved (see
    /// `AgentSession.stale`). A watchdog flag alongside `status`, not a status of its own.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stalled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// The open dialog's summary, when one is present. Skipped from fleet digests (it lives in
    /// `headline`); kept on the struct for fingerprinting and `read_agent`.
    #[serde(skip)]
    pub pending_prompt: Option<String>,
}

/// One lane, with its primary (most-attention-worthy) agent.
#[derive(Debug, Clone, Serialize)]
pub struct LaneDigest {
    pub lane_id: i64,
    pub repo: String,
    pub branch: String,
    pub dirty: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentDigest>,
    /// How many additional agents share this lane beyond the primary (usually 0).
    #[serde(skip_serializing_if = "is_zero")]
    pub extra_agents: usize,
    /// How many of this lane's sessions are managed (have a tmux window) AND active
    /// (running/waiting/rate-limited). Drives the concurrent-agent cap, which must count every
    /// active agent, not one per lane. Distinct from `extra_agents` (a raw extra-session count
    /// that includes idle/external sessions).
    #[serde(skip_serializing_if = "is_zero")]
    pub active_agents: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl LaneDigest {
    /// The lane's rolled-up status string (the primary agent's, or `no-agent`).
    pub fn status(&self) -> &str {
        self.agent
            .as_ref()
            .map(|a| a.status.as_str())
            .unwrap_or("no-agent")
    }
    /// The lane's rolled-up attention (the primary agent's, or none).
    pub fn attention(&self) -> Attention {
        self.agent
            .as_ref()
            .map(|a| a.attention)
            .unwrap_or(Attention::None)
    }
    pub fn headline(&self) -> Option<&str> {
        self.agent.as_ref().and_then(|a| a.headline.as_deref())
    }
}

/// The current fleet state plus a generation that bumps on each meaningful change.
#[derive(Default, Clone)]
pub struct FleetSnapshot {
    pub generation: u64,
    pub stamp: Option<DateTime<Utc>>,
    pub lanes: Vec<LaneDigest>,
}

/// A handle to the live fleet: read the current snapshot, or watch for changes.
#[derive(Clone)]
pub struct Fleet {
    snapshot: Arc<Mutex<FleetSnapshot>>,
    gen_rx: watch::Receiver<u64>,
}

impl Fleet {
    /// Connect the poller and return a handle. Does one inline poll so the first `fleet_status`
    /// isn't empty, then spawns the background loop.
    pub async fn start(client: DaemonClient, socket: PathBuf) -> Fleet {
        let snapshot = Arc::new(Mutex::new(FleetSnapshot::default()));
        let (gen_tx, gen_rx) = watch::channel(0u64);

        let mut last_fp = 0u64;
        if let Ok(lanes) = client.call_typed::<Vec<Lane>>("lane.list", None).await {
            let now = Utc::now();
            let digests: Vec<LaneDigest> = lanes.iter().map(|l| project_lane(l, now)).collect();
            last_fp = fingerprint(&digests);
            let mut s = snapshot.lock().await;
            s.lanes = digests;
            s.stamp = Some(now);
            s.generation = 1;
        }

        tokio::spawn(run_poller(
            client,
            socket,
            snapshot.clone(),
            gen_tx,
            last_fp,
        ));
        Fleet { snapshot, gen_rx }
    }

    /// The current generation and a clone of the lane digests.
    pub async fn current(&self) -> (u64, Vec<LaneDigest>) {
        let s = self.snapshot.lock().await;
        (s.generation, s.lanes.clone())
    }

    /// A receiver that fires when the generation bumps.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.gen_rx.clone()
    }
}

/// Project a live `Lane` into its compact digest.
pub fn project_lane(lane: &Lane, now: DateTime<Utc>) -> LaneDigest {
    let agent = primary_agent(lane).map(|s| project_agent(lane, s, now));
    let extra = lane.agent_sessions.len().saturating_sub(1);
    let active_agents = lane
        .agent_sessions
        .iter()
        .filter(|s| s.tmux_window.is_some() && is_active_status(&s.status))
        .count();
    LaneDigest {
        lane_id: lane.id,
        repo: lane.repo.name.clone(),
        branch: lane
            .state
            .branch
            .clone()
            .unwrap_or_else(|| "(detached)".into()),
        dirty: fmt_dirty(&lane.state.dirty),
        pinned: lane.pinned,
        agent,
        extra_agents: extra,
        active_agents,
    }
}

/// Whether a session counts toward the live agent load (mirrors the server-side `is_active`):
/// working, waiting on you, or paused on a rate limit, as opposed to idle/ended.
pub fn is_active_status(s: &AgentStatus) -> bool {
    matches!(
        s,
        AgentStatus::Running | AgentStatus::Waiting | AgentStatus::RateLimited
    )
}

fn project_agent(lane: &Lane, s: &AgentSession, now: DateTime<Utc>) -> AgentDigest {
    let attention = agent_attention_in(lane, s);
    let headline = match s.status {
        AgentStatus::Waiting => s.pending_prompt.clone().or_else(|| s.last_message.clone()),
        AgentStatus::RateLimited => Some(match s.resume_at {
            Some(t) => format!("rate-limited, resumes ~{}", t.format("%H:%M")),
            None => "rate-limited".into(),
        }),
        _ => s.last_message.clone(),
    }
    .map(|h| truncate(&h, 160));
    AgentDigest {
        kind: s.agent.short().to_string(),
        status: s.status.as_str().to_string(),
        attention,
        headline,
        idle_secs: (now - s.last_activity_at).num_seconds().max(0),
        external: s.external,
        inferred: s.inferred,
        stalled: s.stale,
        window: s.tmux_window.clone(),
        pending_prompt: s.pending_prompt.clone(),
    }
}

fn fmt_dirty(d: &repomon_core::model::DirtyState) -> String {
    if d.is_clean() {
        "clean".into()
    } else {
        format!("+{} ~{} ?{}", d.staged, d.unstaged, d.untracked)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Hash only the fields that represent a *meaningful* transition: lane set, primary agent kind,
/// status, attention, the stall flag, and the open-dialog summary. Deliberately excludes
/// churning fields (headline text, idle seconds, dirty counts) so `wait_for_change` wakes on
/// real edges, not on every streamed token or file save.
fn fingerprint(lanes: &[LaneDigest]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut ordered: Vec<&LaneDigest> = lanes.iter().collect();
    ordered.sort_by_key(|d| d.lane_id);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for d in ordered {
        d.lane_id.hash(&mut h);
        match &d.agent {
            Some(a) => {
                1u8.hash(&mut h);
                a.kind.hash(&mut h);
                a.status.hash(&mut h);
                a.attention.as_str().hash(&mut h);
                a.stalled.hash(&mut h);
                a.pending_prompt.as_deref().unwrap_or("").hash(&mut h);
            }
            None => 0u8.hash(&mut h),
        }
    }
    h.finish()
}

async fn run_poller(
    mut client: DaemonClient,
    socket: PathBuf,
    snapshot: Arc<Mutex<FleetSnapshot>>,
    gen_tx: watch::Sender<u64>,
    mut last_fp: u64,
) {
    let _ = client.call("subscribe", None).await;
    let mut events = client.subscribe();
    let mut interval = tokio::time::interval(Duration::from_millis(1500));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        match client.call_typed::<Vec<Lane>>("lane.list", None).await {
            Ok(lanes) => {
                let now = Utc::now();
                let digests: Vec<LaneDigest> = lanes.iter().map(|l| project_lane(l, now)).collect();
                let fp = fingerprint(&digests);
                let changed = fp != last_fp;
                let g = {
                    let mut s = snapshot.lock().await;
                    if changed {
                        last_fp = fp;
                        s.generation += 1;
                    }
                    s.lanes = digests;
                    s.stamp = Some(now);
                    s.generation
                };
                if changed {
                    let _ = gen_tx.send(g);
                }
            }
            Err(e) => {
                tracing::warn!("mcp poller: lane.list failed: {e}; attempting reconnect");
                if let Ok(c) = DaemonClient::connect(&socket).await {
                    let _ = c.call("subscribe", None).await;
                    events = c.subscribe();
                    client = c;
                } else {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }

        // Re-poll on the cadence, or immediately when a lane is created/deleted (structural
        // changes the orchestrator wants to see fast). Non-structural events (agent output) are
        // ignored here so they don't drive a tight poll loop.
        tokio::select! {
            _ = interval.tick() => {}
            _ = wait_structural(&mut events) => {}
        }
    }
}

async fn wait_structural(events: &mut broadcast::Receiver<Notification>) {
    loop {
        match events.recv().await {
            Ok(n) if n.method.ends_with("lane.created") || n.method.ends_with("lane.deleted") => {
                return;
            }
            Ok(_) => continue,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            // The client died; let the interval drive polling (the poll error path reconnects).
            Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repomon_core::model::AgentKind;
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
            stale: false,
            stalled_since: None,
            ended_turn: false,
            config_dir: None,
            custom_label: None,
        }
    }

    // `agent_attention`'s own unit tests moved to core with the taxonomy
    // (repomon_core::agent::attention); the tests here cover the fleet projection on top.

    #[test]
    fn permission_dialog_outranks_a_running_sibling() {
        // primary_agent should surface the one that needs a human.
        let mut lane = Lane {
            id: 1,
            repo: repomon_core::model::Repo {
                id: 1,
                path: PathBuf::from("/r"),
                name: "r".into(),
                added_at: Utc::now(),
                worktree_root_template: None,
            },
            worktree: repomon_core::model::Worktree {
                id: 1,
                repo_id: 1,
                path: PathBuf::from("/r"),
                branch: Some("main".into()),
                head: "0".repeat(40).parse().unwrap(),
                is_main: true,
                name: "main".into(),
            },
            state: repomon_core::model::WorktreeState {
                worktree_id: 1,
                head: "0".repeat(40).parse().unwrap(),
                branch: Some("main".into()),
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: Default::default(),
                last_commit_at: None,
                locked: false,
                prunable: false,
                last_change_at: None,
            },
            agent_sessions: vec![
                sess(AgentStatus::Running, None),
                sess(AgentStatus::Waiting, Some("Do you want to proceed?")),
            ],
            last_activity_at: Utc::now(),
            pinned: false,
        };
        let digest = project_lane(&lane, Utc::now());
        assert_eq!(digest.attention(), Attention::Permission);
        assert_eq!(digest.extra_agents, 1);

        // With only a running agent, attention is none.
        lane.agent_sessions = vec![sess(AgentStatus::Running, None)];
        assert_eq!(project_lane(&lane, Utc::now()).attention(), Attention::None);
    }

    fn lane_with(sessions: Vec<AgentSession>) -> Lane {
        Lane {
            id: 1,
            repo: repomon_core::model::Repo {
                id: 1,
                path: PathBuf::from("/r"),
                name: "r".into(),
                added_at: Utc::now(),
                worktree_root_template: None,
            },
            worktree: repomon_core::model::Worktree {
                id: 1,
                repo_id: 1,
                path: PathBuf::from("/r"),
                branch: Some("main".into()),
                head: "0".repeat(40).parse().unwrap(),
                is_main: true,
                name: "main".into(),
            },
            state: repomon_core::model::WorktreeState {
                worktree_id: 1,
                head: "0".repeat(40).parse().unwrap(),
                branch: Some("main".into()),
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: Default::default(),
                last_commit_at: None,
                locked: false,
                prunable: false,
                last_change_at: None,
            },
            agent_sessions: sessions,
            last_activity_at: Utc::now(),
            pinned: false,
        }
    }

    #[test]
    fn active_agents_counts_managed_active_sessions_not_lanes() {
        let windowless = {
            let mut s = sess(AgentStatus::Running, None);
            s.tmux_window = None; // active but unmanaged -> excluded
            s
        };
        let lane = lane_with(vec![
            sess(AgentStatus::Running, None),             // managed + active
            sess(AgentStatus::Waiting, Some("proceed?")), // managed + active
            sess(AgentStatus::Idle, None),                // managed but idle -> excluded
            windowless,
        ]);
        // Three-plus agents in ONE lane: the cap must see 2 active, not 1-per-lane.
        assert_eq!(project_lane(&lane, Utc::now()).active_agents, 2);

        let idle_only = lane_with(vec![sess(AgentStatus::Idle, None)]);
        assert_eq!(project_lane(&idle_only, Utc::now()).active_agents, 0);
    }
}
