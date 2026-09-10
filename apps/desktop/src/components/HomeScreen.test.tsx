import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createRoot } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentChoice, AgentSession, Lane, Repo } from "../bindings";
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

async function mountedHomeScreen(lanes: Lane[] = []) {
  const target = repo(1, "repomon");
  const source: FleetSource = {
    load: () =>
      Promise.resolve({
        repos: lanes.length ? lanes.map((lane) => lane.repo) : [target],
        lanes,
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


function ordinaryLane(id: number, name: string, status?: AgentSession["status"]): Lane {
  const target = repo(id, name);
  return {
    id, repo: target,
    worktree: { id, repo_id: id, path: `/code/${name}`, branch: "main", head: "abc", is_main: true, name: "main" },
    state: { worktree_id: id, head: "abc", branch: "main", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null }, pinned: false, role: null, last_activity_at: "2026-09-10T10:00:00Z",
    agent_sessions: status ? [{
      id, repo_id: id, worktree_id: id, agent: "codex", status, title: null,
      started_at: "2026-09-10T00:00:00Z", last_activity_at: "2026-09-10T10:00:00Z",
      ended_at: status === "ended" ? "2026-09-10T10:00:00Z" : null,
      manifest_path: "", tool_call_count: 0, external: false, session_id: `s${id}`,
      resume_at: null, inferred: false, tmux_window: `lane-${id}`, last_message: null,
      pending_prompt: null, pending_dialog: null, stale: false, stalled_since: null,
      subagent_running: null, gate: null, config_dir: null, custom_label: null,
      generated_label: null, status_reason: null,
    }] : [],
  };
}

describe("HomeScreen ordinary fleet", () => {
  it("identifies repeated main lanes by repo, exposes distinct states, and opens the right lane by keyboard", async () => {
    const { fleet, dispose } = await mountedHomeScreen([
      ordinaryLane(1, "repomon", "running"), ordinaryLane(2, "Mira", "idle"),
      ordinaryLane(3, "SAAS", "ended"), ordinaryLane(4, "portfolio"),
    ]);
    expect(screen.getByText("Nothing needs you")).toBeTruthy();
    expect(screen.getByText("1 running")).toBeTruthy();
    const first = screen.getByRole("button", { name: "repomon: main, Running" });
    const second = screen.getByRole("button", { name: "Mira: main, Idle" });
    expect(screen.getByRole("button", { name: "SAAS: main, Exited" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "portfolio: main, No agent" })).toBeTruthy();
    expect(second.textContent).toContain("Mira");
    screen.getAllByRole("button").filter((node) => node.classList.contains("home-strip")).forEach((node, index) => {
      const rect = new DOMRect(0, index * 40, 600, 40);
      vi.spyOn(node, "getClientRects").mockReturnValue([rect] as unknown as DOMRectList);
      vi.spyOn(node, "getBoundingClientRect").mockReturnValue(rect);
    });
    first.focus();
    fireEvent.keyDown(first, { key: "ArrowDown" });
    expect(document.activeElement).toBe(second);
    fireEvent.click(second);
    expect(fleet.selectedLaneId()).toBe(2);
    fleet.stop();
    dispose();
  });

  it("adds the lane number only when repo and title still collide", async () => {
    const first = ordinaryLane(1, "repomon");
    const second = ordinaryLane(2, "repomon");
    second.repo = first.repo;
    second.worktree.repo_id = first.repo.id;
    const { fleet, dispose } = await mountedHomeScreen([first, second]);
    expect(screen.getByRole("button", { name: "repomon: main, lane 1, No agent" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "repomon: main, lane 2, No agent" }));
    expect(fleet.selectedLaneId()).toBe(2);
    fleet.stop();
    dispose();
  });

});

it("moves spatially through the wide board's two columns", async () => {
  const {fleet,dispose} = await mountedHomeScreen([ordinaryLane(1,"one"),ordinaryLane(2,"two"),ordinaryLane(3,"three"),ordinaryLane(4,"four")]);
  const rows = screen.getAllByRole("button").filter((node) => node.classList.contains("home-strip"));
  rows.forEach((node,index) => {
    const rect = new DOMRect(index % 2 * 400, Math.floor(index / 2) * 48, 400, 48);
    vi.spyOn(node,"getClientRects").mockReturnValue([rect] as unknown as DOMRectList);
    vi.spyOn(node,"getBoundingClientRect").mockReturnValue(rect);
  });
  rows[0].focus();
  fireEvent.keyDown(rows[0],{key:"ArrowDown"}); expect(document.activeElement).toBe(rows[2]);
  fireEvent.keyDown(rows[2],{key:"ArrowRight"}); expect(document.activeElement).toBe(rows[3]);
  fireEvent.keyDown(rows[3],{key:"ArrowUp"}); expect(document.activeElement).toBe(rows[1]);
  fireEvent.keyDown(rows[1],{key:"ArrowLeft"}); expect(document.activeElement).toBe(rows[0]);
  fleet.stop(); dispose();
});
