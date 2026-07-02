//! Event fan-out: a broadcast channel of full JSON-RPC notification objects.
//!
//! Background services (the watcher, the agent monitor) call [`crate::Ctx::broadcast`];
//! every subscribed connection forwards the notifications to its client.

use serde_json::Value;
use tokio::sync::broadcast;

pub type EventTx = broadcast::Sender<Value>;
pub type EventRx = broadcast::Receiver<Value>;

/// Notification method names (`event.<topic>`).
pub mod topic {
    pub const REPO_CHANGED: &str = "event.repo.changed";
    pub const REPO_ADDED: &str = "event.repo.added";
    pub const REPO_REMOVED: &str = "event.repo.removed";
    pub const LANE_CREATED: &str = "event.lane.created";
    pub const LANE_DELETED: &str = "event.lane.deleted";
    pub const AGENT_OUTPUT: &str = "event.agent.output";
    pub const AGENT_STATUS: &str = "event.agent.status";
    /// A custom agent was added/removed, or the default changed (config mutated).
    pub const AGENT_CHANGED: &str = "event.agent.changed";
    /// The repomind orchestrator's pane changed (streamed text capture).
    pub const ORCHESTRATOR_OUTPUT: &str = "event.orchestrator.output";
    /// The repomind orchestrator started/stopped (its `{running, agent, model, window}` status).
    pub const ORCHESTRATOR_STATUS: &str = "event.orchestrator.status";
}
