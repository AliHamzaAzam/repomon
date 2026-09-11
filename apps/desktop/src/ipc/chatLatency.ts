import { invoke } from "@tauri-apps/api/core";

// Chat-mode first-open latency breakdown (round 9). Always on, no flag to remember: cheap
// (a handful of marks per chat open, never a trace of every RPC) and persisted to
// <daemon data dir>/logs/desktop-chat-latency.jsonl by the Rust side
// (src-tauri/src/diagnostics.rs), since the operator will not have devtools open when the
// 30-second delay recurs. Fire-and-forget: recording must never perturb what it measures, and a
// failure to write is not worth surfacing.
export function markChatLatency(
  label: string,
  target?: { lane_id?: number; window?: string },
  durationMs?: number,
): void {
  void invoke("record_chat_latency_event", {
    label,
    laneId: target?.lane_id ?? null,
    window: target?.window ?? null,
    durationMs: durationMs ?? null,
  }).catch(() => undefined);
}
