import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentChoice, Lane } from "../bindings";
import SpawnModal from "./SpawnModal";

const state = vi.hoisted(() => ({
  agents: [
    { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
  ] as AgentChoice[],
  spawnError: null as string | null,
  spawnCalls: [] as Array<{ lane_id: number; agent: string; task?: string }>,
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => {
    if (method === "agent.detect") return Promise.resolve(state.agents);
    if (method === "agent.spawn") {
      state.spawnCalls.push(params as { lane_id: number; agent: string; task?: string });
      if (state.spawnError) return Promise.reject(new Error(state.spawnError));
      return Promise.resolve(null);
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
  state.spawnCalls = [];
});

describe("SpawnModal error rendering", () => {
  const dummyLane: Lane = {
    id: 1,
    repo_id: 1,
    path: "/tmp/repo",
    created_at: "2026-08-01T00:00:00Z",
    pinned: false,
    repo: { id: 1, name: "repomon", root: "/tmp/repo", main_branch: "main", hidden: false },
    worktree: { id: 1, repo_id: 1, name: "main", branch: "main", path: "/tmp/repo", is_main: true },
    agent_sessions: [],
  };

  it("renders friendly error and details when spawn fails with missing tmux", async () => {
    state.spawnError = "failed to spawn child: No such file or directory (os error 2)";

    render(() => <SpawnModal lane={dummyLane} onClose={vi.fn()} onDone={vi.fn()} />);

    await screen.findByText("claude-code");
    const spawnButton = screen.getByText("Spawn Agent");
    fireEvent.click(spawnButton);

    const friendlyMsg = await screen.findByText(
      "tmux isn't installed or couldn't be found — Repomon needs tmux to run agent sessions",
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
