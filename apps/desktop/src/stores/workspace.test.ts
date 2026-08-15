import { createRoot } from "solid-js";
import { describe, expect, it, vi } from "vitest";

import type { PaneTarget } from "../components/terminalTargets";
import { createWorkspaceStore } from "./workspace";
import type { FleetStore } from "./fleet";

vi.mock("../ipc/rpc", () => ({ daemonCall: vi.fn().mockResolvedValue({ id: "term-1" }) }));

function target(window: string): PaneTarget {
  return { laneId: 7, window, label: window, shell: false, sessionId: null };
}

function fleetStub(overrides: Partial<FleetStore> = {}): FleetStore {
  return {
    refresh: vi.fn().mockResolvedValue(undefined),
    selectedLaneId: () => 7,
    lanes: () => [],
    terminals: () => [],
    setFocusedWindow: vi.fn(),
    ...overrides,
  } as unknown as FleetStore;
}

describe("workspace store", () => {
  it("cycles tabs with wraparound in both directions", () => {
    createRoot((dispose) => {
      const ws = createWorkspaceStore(fleetStub());
      const tabs = [target("a"), target("b"), target("c")];

      ws.setActiveWindow("a");
      ws.cycleTab(1, tabs);
      expect(ws.activeWindow()).toBe("b");

      ws.cycleTab(-1, tabs);
      expect(ws.activeWindow()).toBe("a");

      // Wraps backwards off the front, and forwards off the end.
      ws.cycleTab(-1, tabs);
      expect(ws.activeWindow()).toBe("c");
      ws.cycleTab(1, tabs);
      expect(ws.activeWindow()).toBe("a");
      dispose();
    });
  });

  it("cycling is inert with no tabs and starts at the first when nothing is active", () => {
    createRoot((dispose) => {
      const ws = createWorkspaceStore(fleetStub());
      ws.cycleTab(1, []);
      expect(ws.activeWindow()).toBeNull();

      ws.cycleTab(1, [target("a"), target("b")]);
      expect(ws.activeWindow()).toBe("a");
      dispose();
    });
  });

  it("layout persists to localStorage", () => {
    createRoot((dispose) => {
      const ws = createWorkspaceStore(fleetStub());
      expect(ws.layout()).toBe("auto");
      ws.chooseLayout("grid");
      expect(ws.layout()).toBe("grid");
      expect(localStorage.getItem("repomon.workspace.layout")).toBe("grid");
      ws.chooseLayout("auto");
      expect(ws.layout()).toBe("auto");
      expect(localStorage.getItem("repomon.workspace.layout")).toBe("auto");
      dispose();
    });
  });

  it("tracks closing windows and shifts active window away when closing active tab", () => {
    createRoot((dispose) => {
      const ws = createWorkspaceStore(fleetStub({
        lanes: () => [{
          id: 7,
          worktree: { name: "main", branch: "main", root: "/tmp", clean: true },
          state: "idle",
          agent_sessions: [
            { tmux_window: "lane-7-1", agent: "claude-code", session_id: "s1" },
            { tmux_window: "lane-7-2", agent: "claude-code", session_id: "s2" },
          ],
        }] as unknown as import("../bindings").Lane[],
      }));

      expect(ws.isClosing("lane-7-1")).toBe(false);
      ws.setActiveWindow("lane-7-1");
      expect(ws.activeWindow()).toBe("lane-7-1");

      ws.markClosing("lane-7-1");
      expect(ws.isClosing("lane-7-1")).toBe(true);
      // Active window automatically switched away to the remaining sibling tab
      expect(ws.activeWindow()).toBe("lane-7-2");

      ws.unmarkClosing("lane-7-1");
      expect(ws.isClosing("lane-7-1")).toBe(false);
      dispose();
    });
  });
});
