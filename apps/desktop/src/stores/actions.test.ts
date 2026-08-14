import { createRoot } from "solid-js";
import { describe, expect, it, vi, beforeEach } from "vitest";

import type { AgentSession, Lane } from "../bindings";
import { createActionsStore } from "./actions";
import type { FleetStore } from "./fleet";

const calls = vi.hoisted(() => ({ list: [] as Array<{ method: string; params: unknown }> }));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => {
    calls.list.push({ method, params });
    return Promise.resolve(null);
  },
}));

function lane(overrides: Partial<Lane> = {}): Lane {
  return {
    id: 7,
    repo: { id: 2, path: "/code/r", name: "r", added_at: "2026-07-26T00:00:00Z", worktree_root_template: null, hidden: false },
    worktree: { id: 3, repo_id: 2, path: "/code/r-wt", branch: "feat/x", head: "abc", is_main: false, name: "x" },
    state: { worktree_id: 3, head: "abc", branch: "feat/x", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: [],
    last_activity_at: "2026-07-26T00:00:00Z",
    pinned: false,
    ...overrides,
  };
}

function fleetStub(): FleetStore {
  return { refresh: vi.fn().mockResolvedValue(undefined) } as unknown as FleetStore;
}

beforeEach(() => {
  calls.list = [];
});

describe("lane operations", () => {
  it("pin toggles the current pinned state and refreshes", async () => {
    await createRoot(async (dispose) => {
      const fleet = fleetStub();
      const actions = createActionsStore(fleet);
      await actions.pinLane(lane({ pinned: false }));
      expect(calls.list[0]).toEqual({ method: "agent.pin", params: { lane_id: 7, pinned: true } });
      expect(fleet.refresh).toHaveBeenCalled();
      dispose();
    });
  });

  it("hides a repo immediately, without the confirm removal needs", async () => {
    await createRoot(async (dispose) => {
      const fleet = fleetStub();
      const actions = createActionsStore(fleet);
      const repo = lane().repo;

      await actions.setRepoHidden(repo, true);
      expect(calls.list[0]).toEqual({ method: "repo.set_hidden", params: { repo_id: 2, hidden: true } });
      // Hiding is reversible, so unlike removeRepo it never opens a confirm dialog.
      expect(actions.confirmOptions()).toBeNull();
      expect(fleet.refresh).toHaveBeenCalled();

      await actions.setRepoHidden(repo, false);
      expect(calls.list[1]).toEqual({ method: "repo.set_hidden", params: { repo_id: 2, hidden: false } });
      dispose();
    });
  });

  it("delete and merge go through confirm rather than firing immediately", async () => {
    await createRoot(async (dispose) => {
      const actions = createActionsStore(fleetStub());

      actions.deleteLane(lane());
      const del = actions.confirmOptions();
      expect(del?.danger).toBe(true);
      expect(calls.list).toHaveLength(0); // nothing sent until confirmed
      await del?.onConfirm();
      expect(calls.list[0].method).toBe("lane.delete");

      calls.list = [];
      actions.mergeLane(lane());
      const merge = actions.confirmOptions();
      expect(calls.list).toHaveLength(0);
      await merge?.onConfirm();
      expect(calls.list[0].method).toBe("lane.merge");
      dispose();
    });
  });

  it("delete and merge on a main worktree raise no confirm and send no RPC", async () => {
    await createRoot(async (dispose) => {
      const actions = createActionsStore(fleetStub());
      const main = lane({ worktree: { id: 3, repo_id: 2, path: "/code/r", branch: "main", head: "abc", is_main: true, name: "r" } });

      actions.deleteLane(main);
      expect(actions.confirmOptions()).toBeNull();
      expect(calls.list).toHaveLength(0);

      actions.mergeLane(main);
      expect(actions.confirmOptions()).toBeNull();
      expect(calls.list).toHaveLength(0);
      dispose();
    });
  });

  it("stop targets the agent's tmux window", async () => {
    await createRoot(async (dispose) => {
      const actions = createActionsStore(fleetStub());
      const agent = { tmux_window: "lane-7" } as AgentSession;
      actions.stopAgent(lane(), agent);
      await actions.confirmOptions()?.onConfirm();
      expect(calls.list[0]).toEqual({ method: "agent.stop", params: { lane_id: 7, window: "lane-7" } });
      dispose();
    });
  });

  it("adopt targets external session and calls agent.adopt RPC", async () => {
    await createRoot(async (dispose) => {
      const fleet = fleetStub();
      const actions = createActionsStore(fleet);
      const extSession = { session_id: "sid-123", agent: "claude-code", external: true } as AgentSession;
      await actions.adoptAgent(lane(), extSession);
      expect(calls.list[0]).toEqual({
        method: "agent.adopt",
        params: { lane_id: 7, session_id: "sid-123", agent: "claude-code" },
      });
      expect(fleet.refresh).toHaveBeenCalled();
      dispose();
    });
  });
});

describe("settings modal tab", () => {
  it("openSettingsTab opens the modal on the requested tab", () => {
    createRoot((dispose) => {
      const actions = createActionsStore(fleetStub());
      expect(actions.settingsOpen()).toBe(false);

      actions.openSettingsTab("keyboard");
      expect(actions.settingsOpen()).toBe(true);
      expect(actions.settingsTab()).toBe("keyboard");
      dispose();
    });
  });

  it("openSettings resets to the general tab even after a prior keyboard-tab open", () => {
    createRoot((dispose) => {
      const actions = createActionsStore(fleetStub());
      actions.openSettingsTab("keyboard");
      actions.closeSettings();
      expect(actions.settingsTab()).toBe("keyboard");

      actions.openSettings();
      expect(actions.settingsOpen()).toBe(true);
      expect(actions.settingsTab()).toBe("general");
      dispose();
    });
  });
});
