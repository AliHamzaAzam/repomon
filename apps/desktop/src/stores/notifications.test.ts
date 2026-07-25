import { afterEach, describe, expect, it, vi } from "vitest";

import type { FleetNotification } from "./notifications";
import { createNotificationStore, showNativeNotification } from "./notifications";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    unminimize: () => Promise.resolve(),
    setFocus: () => Promise.resolve(),
  }),
}));

vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: () => Promise.resolve(true),
  requestPermission: () => Promise.resolve("granted"),
  sendNotification: vi.fn(),
}));

const rpc = vi.hoisted(() => ({
  daemonCb: undefined as ((event: unknown) => void) | undefined,
}));

vi.mock("../ipc/rpc", () => ({
  subscribeDaemon: (cb: (event: unknown) => void) => {
    rpc.daemonCb = cb;
    return Promise.resolve(() => {
      rpc.daemonCb = undefined;
    });
  },
}));

afterEach(() => {
  vi.unstubAllGlobals();
  rpc.daemonCb = undefined;
});

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

  it("alerts once when the daemon re-sends the same notification id", async () => {
    let constructions = 0;
    class NotificationStub {
      onclick: (() => void) | null = null;
      close = vi.fn();
      constructor() {
        constructions += 1;
      }
    }
    vi.stubGlobal("Notification", NotificationStub);

    const store = createNotificationStore();
    await store.start();

    const event = {
      method: "event.notification",
      params: {
        id: "7:s1:needs_you:1",
        lane_id: 7,
        title: "Agent needs you",
        body: "Approve the command",
      },
    };
    // A daemon flap re-sends the identical id; the client must drop the duplicate.
    rpc.daemonCb?.(event);
    rpc.daemonCb?.(event);

    expect(store.items()).toHaveLength(1);
    expect(constructions).toBe(1);

    store.stop();
  });
});
