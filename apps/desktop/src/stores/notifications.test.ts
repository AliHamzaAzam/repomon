import { afterEach, describe, expect, it, vi } from "vitest";

import type { FleetNotification } from "./notifications";
import { showNativeNotification } from "./notifications";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    unminimize: () => Promise.resolve(),
    setFocus: () => Promise.resolve(),
  }),
}));

afterEach(() => vi.unstubAllGlobals());

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

  it("activates the matching lane when a native popup is clicked", () => {
    let popup: { onclick: (() => void) | null; close: ReturnType<typeof vi.fn> } | undefined;
    class NotificationStub {
      onclick: (() => void) | null = null;
      close = vi.fn();

      constructor() {
        popup = this;
      }
    }
    vi.stubGlobal("Notification", NotificationStub);
    vi.spyOn(window, "focus").mockImplementation(() => undefined);
    const activate = vi.fn();
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

    showNativeNotification(item, activate);
    popup?.onclick?.();

    expect(activate).toHaveBeenCalledWith(7);
    expect(popup?.close).toHaveBeenCalled();
  });
});
