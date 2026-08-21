import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createRoot } from "solid-js";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import { createFleetStore, type FleetSource } from "../stores/fleet";
import { createWorkspaceStore } from "../stores/workspace";
import TerminalWorkspace from "./TerminalWorkspace";

const daemonCallMock = vi.hoisted(() => vi.fn().mockResolvedValue(null));
vi.mock("../ipc/rpc", async () => {
  const actual = await vi.importActual<typeof import("../ipc/rpc")>("../ipc/rpc");
  return { ...actual, daemonCall: (...args: unknown[]) => daemonCallMock(...args) };
});

beforeAll(() => {
  // jsdom gaps the component hits while rendering panes.
  Element.prototype.scrollIntoView = vi.fn();
  HTMLCanvasElement.prototype.getContext = vi.fn(() => null);
});

afterEach(() => {
  cleanup();
  daemonCallMock.mockClear();
});

function session(overrides: Partial<AgentSession> = {}): AgentSession {
  return {
    id: 1,
    agent: "claude-code",
    repo_id: 1,
    worktree_id: 1,
    started_at: "2026-07-20T00:00:00Z",
    last_activity_at: "2026-07-20T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 0,
    title: "Ship",
    status: "running",
    external: false,
    session_id: "s1",
    resume_at: null,
    inferred: false,
    tmux_window: "lane-10-1",
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
    ...overrides,
  };
}

function lane(sessions: AgentSession[]): Lane {
  const repo: Repo = {
    id: 1,
    name: "repomon",
    path: "/code/repomon",
    added_at: "2026-07-20T00:00:00Z",
    worktree_root_template: null,
    hidden: false,
    position: null,
    label: null,
  };
  return {
    id: 10,
    repo,
    worktree: { id: 1, repo_id: 1, path: "/code/repomon", branch: "main", head: "abc", is_main: true, name: "main" },
    state: { worktree_id: 1, head: "abc", branch: "main", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, last_change_at: null, locked: false, prunable: false },
    agent_sessions: sessions,
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
  };
}

async function mountedWorkspace(sessions: AgentSession[], terminals: Array<{ lane_id: number; id: string }> = []) {
  const source: FleetSource = {
    load: () =>
      Promise.resolve({
        repos: [sessions[0] ? lane(sessions).repo : lane([]).repo],
        lanes: [lane(sessions)],
        usage: [],
        terminals,
        sortReposByActivity: false,
        sortMode: "default",
        tabSortMode: "manual",
      }),
    refreshUsage: () => Promise.resolve(),
    subscribe: () => Promise.resolve(() => undefined),
  };
  const actions = {
    spawn: vi.fn(),
    stopAgent: vi.fn(),
    adoptAgent: vi.fn(),
    rename: vi.fn(),
    setAgentTabOrder: vi.fn().mockResolvedValue(undefined),
  };
  return createRoot(async (dispose) => {
    const fleet = createFleetStore(source);
    const workspace = createWorkspaceStore(fleet);
    fleet.start();
    await waitFor(() => expect(fleet.synced()).toBe(true));
    fleet.setSelectedLaneId(10);
    render(() => <TerminalWorkspace fleet={fleet} actions={actions as never} workspace={workspace} />);
    return { fleet, workspace, actions, dispose };
  });
}

describe("terminal workspace tab strip ordering and rename", () => {
  it("drags a pill past its neighbor's midpoint to reorder, and commits on release", async () => {
    const { actions, dispose } = await mountedWorkspace([
      session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
      session({ id: 2, agent: "claude-code", tmux_window: "lane-10-2", session_id: "s2" }),
    ]);

    // The reorderable element is the pill DIV wrapping the activate button.
    const codexPill = (await screen.findByText("codex 1")).parentElement!.parentElement!;
    const claudePill =
      screen.getAllByText("claude-code 2")[0].parentElement!.parentElement!;
    // Synthetic horizontal geometry: codex [0,100), claude-code [100,200). Dragging the second
    // pill left past the first's midpoint proves a real reorder.
    Object.defineProperty(codexPill, "getBoundingClientRect", {
      value: () => new DOMRect(0, 0, 100, 28),
      configurable: true,
    });
    Object.defineProperty(claudePill, "getBoundingClientRect", {
      value: () => new DOMRect(100, 0, 100, 28),
      configurable: true,
    });

    fireEvent.pointerDown(claudePill, { clientX: 150, clientY: 14, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 140, clientY: 14, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 40, clientY: 14, pointerId: 1 });
    await new Promise((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(resolve)),
    );
    expect(actions.setAgentTabOrder).not.toHaveBeenCalled();

    fireEvent.pointerUp(window, { clientX: 40, clientY: 14, pointerId: 1 });
    expect(actions.setAgentTabOrder).toHaveBeenCalledTimes(1);
    expect(actions.setAgentTabOrder).toHaveBeenCalledWith(10, ["s2", "s1"]);
    dispose();
  });

  it("right-clicks an agent pill to open the durable rename flow", async () => {
    const { actions, dispose } = await mountedWorkspace([
      session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
    ]);

    const pill = await screen.findByText("codex 1");
    fireEvent.contextMenu(pill.parentElement!);

    expect(actions.rename).toHaveBeenCalledWith({ sessionId: "s1", current: "Codex #1" });
    dispose();
  });

  it("leaves shell tabs out of reordering and renaming", async () => {
    const { actions, dispose } = await mountedWorkspace(
      [session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" })],
      [{ lane_id: 10, id: "term-abc" }],
    );

    const shellLabel = await screen.findByText("shell abc");
    const shellTab = shellLabel.parentElement!.parentElement!;
    // Shells get no data-reorder-id, so the pointer primitive never arms on them.
    expect(shellTab.hasAttribute("data-reorder-id")).toBe(false);
    expect(shellTab.getAttribute("draggable")).toBe(null);

    const dragged = screen.getAllByText("codex 1")[0].parentElement!;
    fireEvent.pointerDown(dragged, { clientX: 5, clientY: 5, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 400, clientY: 5, pointerId: 1 });
    await new Promise((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(resolve)),
    );
    fireEvent.pointerUp(window, { clientX: 400, clientY: 5, pointerId: 1 });
    fireEvent.contextMenu(shellTab);

    expect(actions.setAgentTabOrder).not.toHaveBeenCalled();
    expect(actions.rename).not.toHaveBeenCalled();
    dispose();
  });

  it("keeps pills fixed when the tab sort mode is activity", async () => {
    // Re-render with the same store machinery but activity mode: the strip renders from the
    // wire order and drops are ignored.
    const source: FleetSource = {
      load: () =>
        Promise.resolve({
          repos: [],
          lanes: [
            lane([
              session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
              session({ id: 2, agent: "claude-code", tmux_window: "lane-10-2", session_id: "s2" }),
            ]),
          ],
          usage: [],
          terminals: [],
          sortReposByActivity: false,
          sortMode: "default",
          tabSortMode: "activity",
        }),
      refreshUsage: () => Promise.resolve(),
      subscribe: () => Promise.resolve(() => undefined),
    };
    const actions = { spawn: vi.fn(), stopAgent: vi.fn(), adoptAgent: vi.fn(), rename: vi.fn(), setAgentTabOrder: vi.fn() };
    const { dispose } = await createRoot(async (dispose) => {
      const fleet = createFleetStore(source);
      const workspace = createWorkspaceStore(fleet);
      fleet.start();
      await waitFor(() => expect(fleet.synced()).toBe(true));
      fleet.setSelectedLaneId(10);
      render(() => <TerminalWorkspace fleet={fleet} actions={actions as never} workspace={workspace} />);
      return { dispose };
    });

    const dragged = await screen.findByText("claude-code 2");
    fireEvent.pointerDown(dragged.parentElement!, { clientX: 5, clientY: 5, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 400, clientY: 5, pointerId: 1 });
    await new Promise((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(resolve)),
    );
    fireEvent.pointerUp(window, { clientX: 400, clientY: 5, pointerId: 1 });

    expect(actions.setAgentTabOrder).not.toHaveBeenCalled();
    dispose();
  });
});
