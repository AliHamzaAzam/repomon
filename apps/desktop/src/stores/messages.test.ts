import { describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";

import type { FleetMessage } from "../bindings";
import { createMessageStore, mergeMessages } from "./messages";

function message(id: string, laneId = 2, read = false): FleetMessage {
  return {
    id,
    requested_to: `lane-${laneId}/1`,
    sender: { address: "operator", lane_id: null, slot: null, window: null, session_id: null, agent_kind: null },
    recipient: { address: `lane-${laneId}/1`, lane_id: laneId, slot: 1, window: `lane-${laneId}`, session_id: `s-${laneId}`, agent_kind: "claude-code" },
    body: `body ${id}`,
    thread_id: id,
    reply_to: null,
    remaining_hops: 6,
    created_at: `2026-08-13T00:00:0${id}.000Z`,
    delivered_at: null,
    read_at: read ? "2026-08-13T00:01:00.000Z" : null,
    delivery_error: null,
    delivery_state: "queued",
    read_state: read ? "read" : "unread",
  };
}

describe("message store", () => {
  it("uses message IDs as the only dedupe key", () => {
    const original = message("1");
    const updated = { ...original, body: "updated", read_state: "read" as const };
    const merged = mergeMessages([original], [updated, message("2")]);
    expect(merged).toHaveLength(2);
    expect(merged.find((item) => item.id === "1")?.body).toBe("updated");
  });

  it("tracks unread badges per recipient lane and marks before jumping", async () => {
    let listener: ((event: { jsonrpc: "2.0"; method: `event.${string}`; params: unknown }) => void) | undefined;
    const jump = vi.fn();
    const markRead = vi.fn(async (id: string) => ({
      ...message(id),
      read_at: "2026-08-13T00:01:00.000Z",
      read_state: "read" as const,
      delivery_state: "delivered" as const,
    }));
    const { store, dispose } = createRoot((dispose) => ({
      store: createMessageStore(jump, {
        list: async () => ({ messages: [message("1"), message("2", 3)], next_before: null }),
        markRead,
        subscribe: async (next) => {
          listener = next;
          return () => undefined;
        },
      }),
      dispose,
    }));
    try {
      await store.start();
      expect(store.unread()).toBe(2);
      expect(store.unreadByLane().get(2)).toBe(1);
      expect(store.unreadByLane().get(3)).toBe(1);

      listener?.({ jsonrpc: "2.0", method: "event.message.stored", params: { id: "1", message: message("1") } });
      expect(store.items()).toHaveLength(2);
      await store.open(store.items().find((item) => item.id === "1")!);
      expect(markRead).toHaveBeenCalledWith("1");
      expect(jump).toHaveBeenCalledWith(2, 1);
      expect(store.unread()).toBe(1);
    } finally {
      store.stop();
      dispose();
    }
  });
});
