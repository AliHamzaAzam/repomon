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
      ws.chooseLayout("grid");
      expect(ws.layout()).toBe("grid");
      expect(localStorage.getItem("repomon.workspace.layout")).toBe("grid");
      dispose();
    });
  });

  // The fleet store attributes usage to the agent in view, and this store owns the tab state.
  // Asserted outside the `createRoot` callback on purpose: that callback runs inside one Solid
  // update, so writes made in it stay batched and no effect re-runs until it returns.
  it("mirrors the active window into the fleet store", () => {
    const setFocusedWindow = vi.fn();
    const [ws, dispose] = createRoot((dispose) =>
      [createWorkspaceStore(fleetStub({ setFocusedWindow })), dispose] as const,
    );

    ws.setActiveWindow("lane-7-2");
    expect(setFocusedWindow).toHaveBeenLastCalledWith("lane-7-2");

    ws.setActiveWindow(null);
    expect(setFocusedWindow).toHaveBeenLastCalledWith(null);
    dispose();
  });
});
