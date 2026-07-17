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
    /// Raw PTY bytes (base64) from the byte-watched pane — the embedded renderer's feed.
    pub const AGENT_BYTES: &str = "event.agent.bytes";
    pub const AGENT_STATUS: &str = "event.agent.status";
    /// A custom agent was added/removed, or the default changed (config mutated).
    pub const AGENT_CHANGED: &str = "event.agent.changed";
    /// The repomind orchestrator's pane changed (streamed text capture).
    pub const ORCHESTRATOR_OUTPUT: &str = "event.orchestrator.output";
    /// The repomind orchestrator started/stopped (its `{running, agent, model, window}` status).
    pub const ORCHESTRATOR_STATUS: &str = "event.orchestrator.status";
}

/// Whether a connection whose byte-watched windows are `watched` should receive `value`.
///
/// Every topic forwards unchanged EXCEPT `event.agent.bytes`, which is per-connection: the event
/// bus broadcasts one window's bytes to every subscriber, so each forwarding loop filters them to
/// the connections that actually watch that window. A bytes event forwards only when its `window`
/// param is in `watched`; a bytes event with no `window` param goes to nobody. Cheap and sync (a
/// `std::sync::Mutex` read) so it adds no `await` on the event-forward hot path.
pub fn deliver_to(value: &Value, watched: &std::collections::HashSet<String>) -> bool {
    if value.get("method").and_then(Value::as_str) != Some(topic::AGENT_BYTES) {
        return true;
    }
    match value
        .get("params")
        .and_then(|p| p.get("window"))
        .and_then(Value::as_str)
    {
        Some(window) => watched.contains(window),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn watched(windows: &[&str]) -> std::collections::HashSet<String> {
        windows.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn non_bytes_topics_always_forward() {
        let ev = json!({ "method": topic::AGENT_STATUS, "params": { "window": "lane-9" } });
        assert!(deliver_to(&ev, &watched(&[])));
        assert!(deliver_to(&ev, &watched(&["lane-1"])));
    }

    #[test]
    fn bytes_forward_only_to_watchers_of_that_window() {
        let ev = json!({ "method": topic::AGENT_BYTES, "params": { "window": "lane-1", "data": "x" } });
        assert!(deliver_to(&ev, &watched(&["lane-1"])));
        assert!(deliver_to(&ev, &watched(&["lane-1", "lane-2"])));
        assert!(!deliver_to(&ev, &watched(&["lane-2"])));
        assert!(!deliver_to(&ev, &watched(&[])));
    }

    #[test]
    fn bytes_without_a_window_go_to_nobody() {
        let ev = json!({ "method": topic::AGENT_BYTES, "params": { "data": "x" } });
        assert!(!deliver_to(&ev, &watched(&["lane-1"])));
    }
}
