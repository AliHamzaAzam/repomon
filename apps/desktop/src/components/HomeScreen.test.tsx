import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createRoot } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentChoice, Lane, Repo } from "../bindings";
import { createFleetStore, type FleetSource } from "../stores/fleet";
import HomeScreen from "./HomeScreen";

const state = vi.hoisted(() => ({
  agents: [
    { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
  ] as AgentChoice[],
  createCalls: [] as Array<{ repo_id: number; branch: string }>,
  spawnCalls: [] as Array<{ lane_id: number; agent: string; task?: string; model?: string }>,
  nextLaneId: 42,
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => {
    if (method === "agent.detect") return Promise.resolve(state.agents);
    if (method === "lane.headline") return Promise.resolve(null);
    if (method === "repo.pull_requests") return Promise.resolve([]);
    if (method === "lane.create") {
      const p = params as { repo_id: number; branch: string };
      state.createCalls.push(p);
      return Promise.resolve({
        id: state.nextLaneId,
        repo: { id: p.repo_id, name: "repomon", path: "/code/repomon", added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null },
        worktree: { id: state.nextLaneId, repo_id: p.repo_id, path: "/code/repomon-wt", branch: p.branch, head: "abc", is_main: false, name: p.branch },
        state: { worktree_id: state.nextLaneId, head: "abc", branch: p.branch, upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
        agent_sessions: [],
        last_activity_at: "2026-09-10T00:00:00Z",
        pinned: false,
        role: null,
      } satisfies Lane);
    }
    if (method === "agent.spawn") {
      state.spawnCalls.push(params as { lane_id: number; agent: string; task?: string; model?: string });
      return Promise.resolve({ lane_id: state.nextLaneId, window: "w", spawn_warnings: [] });
    }
    return Promise.resolve(null);
  },
  subscribeDaemon: () => Promise.resolve(() => undefined),
}));

afterEach(() => {
  cleanup();
  state.agents = [
    { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
  ];
  state.createCalls = [];
  state.spawnCalls = [];
  state.nextLaneId = 42;
});

function repo(id: number, name: string): Repo {
  return { id, path: `/code/${name}`, name, added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null };
}

async function mountedHomeScreen() {
  const target = repo(1, "repomon");
  const source: FleetSource = {
    load: () =>
      Promise.resolve({
        repos: [target],
        lanes: [],
        usage: [],
        terminals: [],
        sortReposByActivity: false,
        sortMode: "default",
        tabSortMode: "manual",
      }),
    refreshUsage: () => Promise.resolve(),
    subscribe: () => Promise.resolve(() => undefined),
  };
  return createRoot(async (dispose) => {
    const fleet = createFleetStore(source);
    fleet.start();
    await waitFor(() => expect(fleet.synced()).toBe(true));
    render(() => <HomeScreen fleet={fleet} />);
    return { fleet, dispose };
  });
}

describe("HomeScreen compose", () => {
  it("creates a lane, spawns the chosen agent with the task, and focuses the new lane", async () => {
    const { fleet, dispose } = await mountedHomeScreen();

    await screen.findByText("Start with a task. Repomon makes a lane in the repo you pick and hands it to the agent.");
    const input = screen.getByPlaceholderText("Describe a task") as HTMLInputElement;
    fireEvent.input(input, { target: { value: "Fix flaky login test" } });
    fireEvent.click(screen.getByRole("button", { name: "enter" }));

    await waitFor(() => expect(state.spawnCalls).toHaveLength(1));
    expect(state.createCalls).toEqual([{ repo_id: 1, branch: "fix-flaky-login-test" }]);
    expect(state.spawnCalls[0]).toMatchObject({ lane_id: 42, agent: "claude-code", task: "Fix flaky login test" });
    await waitFor(() => expect(fleet.selectedLaneId()).toBe(42));
    expect(fleet.homeSelected()).toBe(false);
    expect(input.value).toBe("");

    fleet.stop();
    dispose();
  });
});
