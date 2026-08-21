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
  it("drops a dragged agent pill on another and persists the new session order", async () => {
    const { actions, dispose } = await mountedWorkspace([
      session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
      session({ id: 2, agent: "claude-code", tmux_window: "lane-10-2", session_id: "s2" }),
    ]);

    const dragged = await screen.findByText("codex 1");
    const target = screen.getAllByText("claude-code 2")[0];
    // jsdom rects are zero-height, so the pointer counts as the upper half: insert before.
    // Dragging the SECOND pill onto the FIRST proves a real reorder happened.
    fireEvent.dragStart(target.parentElement!);
    fireEvent.dragOver(dragged.parentElement!);
    fireEvent.drop(dragged.parentElement!);

    await waitFor(() => {
      expect(actions.setAgentTabOrder).toHaveBeenCalledWith(10, ["s2", "s1"]);
    });
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
    expect(shellTab.getAttribute("draggable")).toBe("false");

    const dragged = screen.getAllByText("codex 1")[0].parentElement!;
    fireEvent.dragStart(dragged);
    fireEvent.dragOver(shellTab);
    fireEvent.drop(shellTab);
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
    const target = screen.getAllByText("codex 1")[0];
    fireEvent.dragStart(dragged.parentElement!);
    fireEvent.dragOver(target.parentElement!);
    fireEvent.drop(target.parentElement!);

    expect(actions.setAgentTabOrder).not.toHaveBeenCalled();
    dispose();
  });
});
