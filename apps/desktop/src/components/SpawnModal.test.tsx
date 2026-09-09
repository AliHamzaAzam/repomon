import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentChoice, Lane } from "../bindings";
import SpawnModal from "./SpawnModal";

const state = vi.hoisted(() => ({
  agents: [
    { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
  ] as AgentChoice[],
  spawnWarnings: [] as string[],
  spawnError: null as string | null,
  spawnCalls: [] as Array<{ lane_id: number; agent: string; task?: string }>,
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => {
    if (method === "agent.detect") return Promise.resolve(state.agents);
    if (method === "agent.spawn") {
      state.spawnCalls.push(params as { lane_id: number; agent: string; task?: string });
      if (state.spawnError) return Promise.reject(new Error(state.spawnError));
      return Promise.resolve({ spawn_warnings: state.spawnWarnings });
    }
    return Promise.resolve(null);
  },
}));

afterEach(() => {
  cleanup();
  state.agents = [
    { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
  ];
  state.spawnError = null;
  state.spawnWarnings = [];
  state.spawnCalls = [];
});

describe("SpawnModal error rendering", () => {
  const dummyLane: Lane = {
    id: 1,
    pinned: false,
    role: null,
    last_activity_at: "2026-08-01T00:00:00Z",
    repo: { id: 1, name: "repomon", path: "/tmp/repo", added_at: "2026-08-01T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null },
    worktree: { id: 1, repo_id: 1, name: "main", branch: "main", path: "/tmp/repo", head: "abc", is_main: true },
    state: { worktree_id: 1, head: "abc", branch: "main", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: [],
  };

  it("keeps spawn warnings visible and prevents a duplicate spawn", async () => {
    state.spawnWarnings = ["Initial task delivery could not be verified. Inspect the agent before resending."];
    const onClose = vi.fn();
    const onDone = vi.fn();
    render(() => <SpawnModal lane={dummyLane} onClose={onClose} onDone={onDone} />);
    await screen.findByText("claude-code");
    fireEvent.click(screen.getByRole("button", { name: "Spawn Agent" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(state.spawnWarnings[0]);
    expect(onClose).not.toHaveBeenCalled();
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Agent started" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Agent started" }));
    expect(state.spawnCalls).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("keeps runtime selection and install help as separate keyboard controls", async () => {
    state.agents = [
      { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
      { name: "cursor", command: "cursor-agent", detected: false, default: false, custom: false },
    ];
    const onOpenSettingsTab = vi.fn();
    const { container } = render(() => <SpawnModal lane={dummyLane} onClose={vi.fn()} onDone={vi.fn()} onOpenSettingsTab={onOpenSettingsTab} />);
    const claude = await screen.findByRole("button", { name: "Select claude-code" });
    const cursor = screen.getByRole("button", { name: "Select cursor" });
    expect(claude).toHaveAttribute("aria-pressed", "true");
    expect(cursor).toHaveAttribute("aria-pressed", "false");
    expect(container.querySelector("button button")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /View install instructions for cursor/ }));
    expect(onOpenSettingsTab).toHaveBeenCalledWith("system");
    expect(claude).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(cursor);
    expect(cursor).toHaveAttribute("aria-pressed", "true");
    expect(claude).toHaveAttribute("aria-pressed", "false");
    expect(state.spawnCalls).toEqual([]);
  });

  it("renders friendly error and details when spawn fails with missing tmux", async () => {
    state.spawnError = "failed to spawn child: No such file or directory (os error 2)";

    render(() => <SpawnModal lane={dummyLane} onClose={vi.fn()} onDone={vi.fn()} />);

    await screen.findByText("claude-code");
    const spawnButton = screen.getByText("Spawn Agent");
    fireEvent.click(spawnButton);

    const friendlyMsg = await screen.findByText(
      "tmux isn't installed or couldn't be found. Repomon needs tmux to run agent sessions",
    );
    expect(friendlyMsg).toBeInTheDocument();
    expect(screen.getByText("Technical details")).toBeInTheDocument();
  });

  it("renders friendly error when custom agent command is not found", async () => {
    state.agents = [
      { name: "custom-agent", command: "custom-agent", detected: true, default: true, custom: true },
    ];
    state.spawnError = "custom-agent: command not found";

    render(() => <SpawnModal lane={dummyLane} onClose={vi.fn()} onDone={vi.fn()} />);

    await screen.findByText("custom-agent");
    const spawnButton = screen.getByText("Spawn Agent");
    fireEvent.click(spawnButton);

    const friendlyMsg = await screen.findByText("'custom-agent' isn't installed or not on PATH");
    expect(friendlyMsg).toBeInTheDocument();
  });

  it("navigates to Settings > System health when clicking missing badge on undetected agent", async () => {
    state.agents = [
      { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
      { name: "cursor", command: "cursor-agent", detected: false, default: false, custom: false },
    ];

    const onClose = vi.fn();
    const onOpenSettingsTab = vi.fn();

    render(() => (
      <SpawnModal
        lane={dummyLane}
        onClose={onClose}
        onDone={vi.fn()}
        onOpenSettingsTab={onOpenSettingsTab}
      />
    ));

    await screen.findByText("claude-code");
    expect(screen.getByText("cursor")).toBeInTheDocument();

    const missingBtn = screen.getByRole("button", { name: /View install instructions for cursor/i });
    expect(missingBtn).toBeInTheDocument();
    fireEvent.click(missingBtn);

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onOpenSettingsTab).toHaveBeenCalledWith("system");
  });
});
