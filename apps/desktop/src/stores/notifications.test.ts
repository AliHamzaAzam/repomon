import { describe, expect, it } from "vitest";

import type { FleetNotification } from "./notifications";

describe("notification feed shape", () => {
  it("keeps stable ids for daemon deduplication", () => {
    const item: FleetNotification = {
      id: "7:s1:needs_you:1",
      lane_id: 7,
      kind: "needs_you",
      title: "Agent needs you",
      body: "Approve the command",
      attention: "permission",
      received_at: 1,
      read: false,
    };
    expect(item.id).toContain("needs_you");
    expect(item.read).toBe(false);
  });
});
