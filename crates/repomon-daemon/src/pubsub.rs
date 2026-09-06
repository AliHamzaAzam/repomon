//! Fans out JSON-RPC notifications from background services to subscribed client connections.

use std::collections::HashSet;

use repomon_core::model::LaneId;
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
    /// Raw PTY bytes (base64) from the byte-watched pane - the embedded renderer's feed.
    pub const AGENT_BYTES: &str = "event.agent.bytes";
    /// The authoritative cell grid after a shared agent pane changes size.
    pub const AGENT_GRID: &str = "event.agent.grid";
    /// A watched pane's backend stream ended because its target window disappeared.
    pub const AGENT_STREAM_CLOSED: &str = "event.agent.stream_closed";
    pub const AGENT_STATUS: &str = "event.agent.status";
    /// A custom agent was added/removed, or the default changed (config mutated).
    pub const AGENT_CHANGED: &str = "event.agent.changed";
    /// The repomind orchestrator's pane changed (streamed text capture).
    pub const ORCHESTRATOR_OUTPUT: &str = "event.orchestrator.output";
    /// The repomind orchestrator started/stopped (its `{running, agent, model, window}` status).
    pub const ORCHESTRATOR_STATUS: &str = "event.orchestrator.status";
    /// A supervision action was evaluated and acted on (or held/skipped).
    pub const SUPERVISION_ACTED: &str = "event.supervision.acted";
    /// A supervision policy was updated.
    pub const SUPERVISION_CHANGED: &str = "event.supervision.changed";
    /// Reports completion or continued execution of a ticketed manual quota probe.
    pub const USAGE_REFRESHED: &str = "event.usage.refreshed";
    /// Signals that ledger usage or pricing changed and cached queries need refreshing.
    pub const USAGE_CHANGED: &str = "event.usage.changed";
}

pub const SUPERVISION_ACTED: &str = topic::SUPERVISION_ACTED;
pub const SUPERVISION_CHANGED: &str = topic::SUPERVISION_CHANGED;

/// Filters terminal events by the connection’s byte watches or requested viewport lane/window,
/// preserving lane subscriptions across window resolution changes and forwarding other topics
/// unchanged.
pub fn deliver_to(
    value: &Value,
    watched: &HashSet<String>,
    output_lanes: &HashSet<LaneId>,
    output_windows: &HashSet<String>,
) -> bool {
    let method = value.get("method").and_then(Value::as_str);
    let params = value.get("params");
    match method {
        Some(topic::AGENT_BYTES) | Some(topic::AGENT_GRID) | Some(topic::AGENT_STREAM_CLOSED) => {
            match params.and_then(|p| p.get("window")).and_then(Value::as_str) {
                Some(window) => watched.contains(window),
                None => false,
            }
        }
        Some(topic::AGENT_OUTPUT) => {
            let lane_hit = params
                .and_then(|p| p.get("lane_id"))
                .and_then(Value::as_i64)
                .is_some_and(|lane| output_lanes.contains(&lane));
            let window_hit = params
                .and_then(|p| p.get("window"))
                .and_then(Value::as_str)
                .is_some_and(|window| output_windows.contains(window));
            lane_hit || window_hit
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn windows(names: &[&str]) -> HashSet<String> {
        names.iter().map(|w| w.to_string()).collect()
    }

    fn lanes(ids: &[LaneId]) -> HashSet<LaneId> {
        ids.iter().copied().collect()
    }

    /// A connection subscribed to nothing: no watched bytes windows, no viewport lanes/windows.
    fn nothing() -> (HashSet<String>, HashSet<LaneId>, HashSet<String>) {
        (HashSet::new(), HashSet::new(), HashSet::new())
    }

    #[test]
    fn non_filtered_topics_always_forward() {
        let ev = json!({ "method": topic::AGENT_STATUS, "params": { "window": "lane-9" } });
        let (w, ol, ow) = nothing();
        assert!(deliver_to(&ev, &w, &ol, &ow));
        assert!(deliver_to(
            &ev,
            &windows(&["lane-1"]),
            &lanes(&[1]),
            &windows(&["term-2-1"])
        ));
    }

    #[test]
    fn bytes_forward_only_to_watchers_of_that_window() {
        let ev =
            json!({ "method": topic::AGENT_BYTES, "params": { "window": "lane-1", "data": "x" } });
        let (_, ol, ow) = nothing();
        assert!(deliver_to(&ev, &windows(&["lane-1"]), &ol, &ow));
        assert!(deliver_to(&ev, &windows(&["lane-1", "lane-2"]), &ol, &ow));
        assert!(!deliver_to(&ev, &windows(&["lane-2"]), &ol, &ow));
        assert!(!deliver_to(&ev, &windows(&[]), &ol, &ow));
    }

    #[test]
    fn bytes_without_a_window_go_to_nobody() {
        let ev = json!({ "method": topic::AGENT_BYTES, "params": { "data": "x" } });
        let (_, ol, ow) = nothing();
        assert!(!deliver_to(&ev, &windows(&["lane-1"]), &ol, &ow));
    }

    #[test]
    fn grid_changes_forward_only_to_watchers_of_that_window() {
        let ev = json!({
            "method": topic::AGENT_GRID,
            "params": { "lane_id": 1, "window": "lane-1", "cols": 120, "rows": 40 }
        });
        let (_, ol, ow) = nothing();
        assert!(deliver_to(&ev, &windows(&["lane-1"]), &ol, &ow));
        assert!(!deliver_to(&ev, &windows(&["lane-2"]), &ol, &ow));
        assert!(!deliver_to(&ev, &windows(&[]), &ol, &ow));
    }

    #[test]
    fn stream_close_forwards_only_to_watchers_of_that_window() {
        let ev = json!({
            "method": topic::AGENT_STREAM_CLOSED,
            "params": { "lane_id": 1, "window": "lane-1", "generation": 7 }
        });
        let (_, ol, ow) = nothing();
        assert!(deliver_to(&ev, &windows(&["lane-1"]), &ol, &ow));
        assert!(!deliver_to(&ev, &windows(&["lane-2"]), &ol, &ow));
        assert!(!deliver_to(&ev, &windows(&[]), &ol, &ow));
    }

    #[test]
    fn output_forwards_when_lane_is_in_the_viewport() {
        let ev = json!({ "method": topic::AGENT_OUTPUT, "params": { "lane_id": 7, "window": "lane-7", "content": "hi" } });
        let (w, _, ow) = nothing();
        assert!(deliver_to(&ev, &w, &lanes(&[7]), &ow));
        assert!(deliver_to(&ev, &w, &lanes(&[7, 9]), &ow));
        assert!(!deliver_to(&ev, &w, &lanes(&[9]), &ow));
    }

    #[test]
    fn output_forwards_when_window_is_a_viewport_terminal_tile() {
        // A plain terminal tile the client put in its viewport, whose lane the client is NOT
        // otherwise subscribed to: the window match alone delivers it.
        let ev = json!({ "method": topic::AGENT_OUTPUT, "params": { "lane_id": 3, "window": "term-3-1", "content": "hi" } });
        let (w, _, _) = nothing();
        assert!(deliver_to(&ev, &w, &lanes(&[]), &windows(&["term-3-1"])));
        assert!(!deliver_to(&ev, &w, &lanes(&[]), &windows(&["term-3-2"])));
    }

    #[test]
    fn output_goes_to_nobody_without_a_viewport() {
        // A connection without a viewport must receive no capture output.
        let ev = json!({ "method": topic::AGENT_OUTPUT, "params": { "lane_id": 7, "window": "lane-7", "content": "hi" } });
        let (w, ol, ow) = nothing();
        assert!(!deliver_to(&ev, &w, &ol, &ow));
    }
}
