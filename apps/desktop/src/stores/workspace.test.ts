import { createRoot, createSignal } from "solid-js";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { PaneTarget } from "../components/terminalTargets";
import { createWorkspaceStore } from "./workspace";
import type { FleetStore } from "./fleet";

// `subscribeDaemon` is here because the workspace store reads `isControllerLane` from the fleet
// store, whose module imports it; nothing in this file subscribes to anything.
vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn().mockResolvedValue({ id: "term-1" }),
  subscribeDaemon: vi.fn().mockResolvedValue(() => undefined),
}));

function target(window: string): PaneTarget {
  return { laneId: 7, window, label: window, shell: false, sessionId: null, targetId: null };
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

function lane(id: number, windows: string[]): import("../bindings").Lane {
  return {
    id,
    repo: { id, name: `repo-${id}`, path: `/repo-${id}`, added_at: "", worktree_root_template: null, hidden: false, position: null, label: null },
    worktree: { id, repo_id: id, name: `lane-${id}`, branch: `lane-${id}`, path: `/repo-${id}/lane`, head: "abc", is_main: false },
    state: { worktree_id: id, head: "abc", branch: `lane-${id}`, upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, last_change_at: null, locked: false, prunable: false },
    agent_sessions: windows.map((window, index) => ({
      id: id * 10 + index,
      agent: "codex",
      repo_id: id,
      worktree_id: id,
      started_at: "",
      last_activity_at: "",
      ended_at: null,
      manifest_path: "",
      tool_call_count: 0,
      title: window,
      status: "running",
      external: false,
      session_id: `session-${window}`,
      resume_at: null,
      inferred: false,
      tmux_window: window,
      last_message: null,
      pending_prompt: null,
      pending_dialog: null,
      stale: false,
      stalled_since: null,
      subagent_running: null,
      gate: null,
      config_dir: null,
      custom_label: null,
      generated_label: null,
    })),
    last_activity_at: "",
    pinned: false,
    role: null,
  };
}

beforeEach(() => {
  localStorage.removeItem("repomon.workspace.lane-panes.v1");
  localStorage.removeItem("repomon.workspace.multitask-panes.v1");
  localStorage.removeItem("repomon.workspace.multitask-spans.v1");
});

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
          repo: { id: 7, name: "repo", label: null },
          worktree: { name: "main", branch: "main" },
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

  it("keeps lane pane selections lane-scoped and persists them", () => {
    createRoot((dispose) => {
      const [selectedLaneId] = createSignal<number | null>(7);
      const ws = createWorkspaceStore(fleetStub({
        selectedLaneId,
        lanes: () => [lane(7, ["a", "b", "new"]), lane(8, ["other"])],
      }));

      ws.setLanePaneSelection(7, ["b", "a"]);
      expect(ws.selectedLaneTargets().map((item) => item.window)).toEqual(["b", "a"]);
      expect(ws.selectedLaneTargets().some((item) => item.window === "other")).toBe(false);
      expect(localStorage.getItem("repomon.workspace.lane-panes.v1")).toBe('{"7":["b","a"]}');
      dispose();
    });
  });

  it("restores the last viewed live agent independently for each lane", async () => {
    await createRoot(async (dispose) => {
      const [selectedLaneId, setSelectedLaneId] = createSignal<number | null>(7);
      const ws = createWorkspaceStore(fleetStub({
        selectedLaneId,
        lanes: () => [lane(7, ["a1", "a2"]), lane(8, ["b1", "b2"])],
      }));

      expect(ws.activeWindow()).toBe("a1");
      ws.setActiveWindow("a2");
      setSelectedLaneId(8);
      await Promise.resolve();
      expect(ws.activeWindow()).toBe("b1");
      ws.setActiveWindow("b2");
      setSelectedLaneId(7);
      await Promise.resolve();
      expect(ws.activeWindow()).toBe("a2");
      setSelectedLaneId(8);
      await Promise.resolve();
      expect(ws.activeWindow()).toBe("b2");
      dispose();
    });
  });

  it("persists fleet-wide multitasking order and pane footprints", () => {
    createRoot((dispose) => {
      const ws = createWorkspaceStore(fleetStub({
        lanes: () => [lane(7, ["a", "b"]), lane(8, ["c"])],
      }));

      ws.setMultitaskPaneSelection(["c", "a"]);
      ws.setMultitaskSpan("c", { columns: 2, rows: 2 });
      expect(ws.multitaskTargets().map((item) => item.window)).toEqual(["c", "a"]);
      expect(ws.multitaskSpans().c).toEqual({ columns: 2, rows: 2 });
      expect(localStorage.getItem("repomon.workspace.multitask-panes.v1")).toBe('["c","a"]');
      dispose();
    });
  });

  it("keeps the unconfigured multitasking fallback stable across activity reordering", () => {
    createRoot((dispose) => {
      const first = lane(7, ["a", "b"]);
      const second = lane(8, ["c", "d"]);
      const [lanes, setLanes] = createSignal([first, second]);
      const ws = createWorkspaceStore(fleetStub({ lanes }));

      expect(ws.multitaskTargets().map((item) => item.window)).toEqual(["a", "b", "c", "d"]);
      // Activity sorting in the fleet can reverse lane order between polls. The default grid
      // should keep its established window positions until the user explicitly reorders it.
      setLanes([second, first]);
      expect(ws.multitaskTargets().map((item) => item.window)).toEqual(["a", "b", "c", "d"]);
      dispose();
    });
  });
});
