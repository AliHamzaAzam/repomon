//! Event fan-out: a broadcast channel of full JSON-RPC notification objects.
//!
//! Background services (the watcher, the agent monitor) call [`crate::Ctx::broadcast`];
//! every subscribed connection forwards the notifications to its client.

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

/// Whether a connection with the given per-connection filter state should receive `value`.
///
/// Two topics are per-connection; every other topic forwards unchanged. The event bus broadcasts
/// each event to every subscriber, so the forwarding loops narrow the two streaming topics to the
/// connections that actually asked for them. All state read here lives behind `std::sync::Mutex`es,
/// so the check adds no `await` on the event-forward hot path.
///
/// - `event.agent.bytes` (A4): forwards only when its `window` param is in `watched`; a bytes event
///   with no `window` goes to nobody.
/// - `event.agent.output` (A5): forwards when its `lane_id` is in the connection's viewport
///   `output_lanes` OR its `window` is in `output_windows` — i.e. lane-membership or the terminal
///   window the client put in its viewport. This is what the client literally requested via
///   `viewport.set`, NOT the resolved stream-target window for each lane: window resolution for a
///   lane can change between `viewport.set` calls, so a name-resolved cache would go stale and
///   silently drop output for a lane the session is still subscribed to. Lane-membership cannot go
///   stale. Over-delivery within a subscribed lane that has multiple windows is acceptable by
///   design. An event missing both a matching `lane_id` and `window` goes to nobody, so a
///   connection that never called `viewport.set` (empty sets) receives no output events at all.
pub fn deliver_to(
    value: &Value,
    watched: &HashSet<String>,
    output_lanes: &HashSet<LaneId>,
    output_windows: &HashSet<String>,
) -> bool {
    let method = value.get("method").and_then(Value::as_str);
    let params = value.get("params");
    match method {
        Some(topic::AGENT_BYTES) => {
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
        let ev = json!({ "method": topic::AGENT_BYTES, "params": { "window": "lane-1", "data": "x" } });
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
        // The core A5 guarantee: a connection that never called `viewport.set` has empty sets, so
        // it receives no output events at all. TODAY's shipping iPhone app never calls viewport.set
        // and never consumes event.agent.output (it polls agent.capture), so nothing it relies on
        // goes missing while the iPad's tiles stop wasting its bandwidth.
        let ev = json!({ "method": topic::AGENT_OUTPUT, "params": { "lane_id": 7, "window": "lane-7", "content": "hi" } });
        let (w, ol, ow) = nothing();
        assert!(!deliver_to(&ev, &w, &ol, &ow));
    }
}
