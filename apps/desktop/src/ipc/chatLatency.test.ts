import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";

import { markChatLatency } from "./chatLatency";

afterEach(() => clearMocks());

describe("chat-mode first-open latency breakdown (round 9)", () => {
  it("records a labeled mark with the lane/window target and an optional duration", async () => {
    const calls: unknown[] = [];
    mockIPC((command, args) => {
      calls.push({ command, args });
      return null;
    });
    markChatLatency("transcript_watch_resolved", { lane_id: 7, window: "lane-7" }, 42.5);
    await Promise.resolve();
    expect(calls).toEqual([{
      command: "record_chat_latency_event",
      args: { label: "transcript_watch_resolved", laneId: 7, window: "lane-7", durationMs: 42.5 },
    }]);
  });

  it("fills in null rather than undefined for an absent target or duration", async () => {
    const calls: unknown[] = [];
    mockIPC((command, args) => {
      calls.push({ command, args });
      return null;
    });
    markChatLatency("terminal_pane_chunk_resolved");
    await Promise.resolve();
    expect(calls).toEqual([{
      command: "record_chat_latency_event",
      args: { label: "terminal_pane_chunk_resolved", laneId: null, window: null, durationMs: null },
    }]);
  });

  it("never throws when the host rejects the call", () => {
    mockIPC(() => {
      throw new Error("no such command");
    });
    expect(() => markChatLatency("chat_clicked", { lane_id: 1, window: "lane-1" })).not.toThrow();
  });
});
