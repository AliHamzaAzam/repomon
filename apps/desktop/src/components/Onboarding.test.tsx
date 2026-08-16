import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Repo } from "../bindings";
import type { ActionsStore } from "../stores/actions";
import Onboarding from "./Onboarding";

// Mock daemonCall for SystemHealthView
vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn().mockImplementation((method: string) => {
    if (method === "system.doctor") {
      return Promise.resolve({
        tmux: { available: true, version: "tmux 3.4", source: "bundled", path: "/path/to/tmux" },
        git: { available: true, version: "git 2.44.0", path: "/usr/bin/git" },
        agents: [
          { kind: "claude-code", name: "Claude Code", command: "claude", detected: true },
        ],
      });
    }
    return Promise.resolve({});
  }),
}));

function createMockActions(repos: Repo[] = []): ActionsStore {
  return {
    fleet: {
      repos: () => repos,
      visibleRepos: () => repos,
      lanes: () => [],
      unhiddenLanes: () => [],
      selectedLaneId: () => null,
      selectedLane: () => undefined,
      selectedRepo: () => undefined,
      setSelectedLaneId: vi.fn(),
      setFocusedWindow: vi.fn(),
      toggleRepoCollapsed: vi.fn(),
      isRepoCollapsed: () => false,
      toggleLaneHidden: vi.fn(),
      toggleLanePinned: vi.fn(),
      hideAllInactive: vi.fn(),
      unhideAll: vi.fn(),
      error: () => null,
      dismissError: vi.fn(),
      refresh: vi.fn().mockResolvedValue(undefined),
      start: vi.fn(),
      stop: vi.fn(),
    } as any,
    settingsOpen: () => false,
    settingsTab: () => "general",
    openSettings: vi.fn(),
    openSettingsTab: vi.fn(),
    closeSettings: vi.fn(),
    controlOpen: () => false,
    openControl: vi.fn(),
    closeControl: vi.fn(),
    toggleControl: vi.fn(),
    spawnLane: () => null,
    spawn: vi.fn(),
    closeSpawn: vi.fn(),
    notesRepo: () => null,
    openRepoNotes: vi.fn(),
    closeRepoNotes: vi.fn(),
    newLaneOpen: () => false,
    newLaneRepoId: () => null,
    newLane: vi.fn(),
    closeNewLane: vi.fn(),
    renameTarget: () => null,
    rename: vi.fn(),
    closeRename: vi.fn(),
    confirmOptions: () => null,
    confirm: vi.fn(),
    closeConfirm: vi.fn(),
    error: () => null,
    dismissError: vi.fn(),
    reportError: vi.fn(),
    addRepo: vi.fn().mockResolvedValue(undefined),
    removeRepo: vi.fn(),
    setRepoHidden: vi.fn().mockResolvedValue(undefined),
    confirmPlaybookDelete: vi.fn(),
    pinLane: vi.fn().mockResolvedValue(undefined),
    mergeLane: vi.fn(),
    deleteLane: vi.fn(),
    stopAgent: vi.fn().mockResolvedValue(undefined),
    adoptAgent: vi.fn().mockResolvedValue(undefined),
    restoreAllAgents: vi.fn().mockResolvedValue(0),
  } as unknown as ActionsStore;
}

describe("Onboarding component", () => {
  afterEach(cleanup);
  it("renders Step 1 (Welcome) and walks through all 4 steps to completion", async () => {
    const actions = createMockActions();
    const onComplete = vi.fn();
    const onSkip = vi.fn();

    render(() => (
      <Onboarding
        actions={actions}
        onComplete={onComplete}
        onSkip={onSkip}
      />
    ));

    // STEP 1: Welcome
    expect(screen.getByText(/Orchestrate coding agents across git worktrees/i)).toBeInTheDocument();
    expect(screen.getByText("Isolated Worktrees")).toBeInTheDocument();
    expect(screen.getByText("Multi-Agent Runtimes")).toBeInTheDocument();

    const getStartedBtn = screen.getByRole("button", { name: "Get started with setup" });
    fireEvent.click(getStartedBtn);

    // STEP 2: System Check
    expect(await screen.findByText("System Health & Tooling")).toBeInTheDocument();
    expect(screen.getByText("Repomon Built-in")).toBeInTheDocument();

    // Click Back to verify backward navigation
    const backBtn = screen.getByRole("button", { name: /Back/i });
    fireEvent.click(backBtn);
    expect(screen.getByText(/Orchestrate coding agents across git worktrees/i)).toBeInTheDocument();

    // Go forward again to Step 2
    fireEvent.click(screen.getByRole("button", { name: "Get started with setup" }));
    expect(await screen.findByText("System Health & Tooling")).toBeInTheDocument();

    // Click Continue to Step 3
    const continueBtn = screen.getByRole("button", { name: /Continue/i });
    fireEvent.click(continueBtn);

    // STEP 3: Repository (Empty state)
    expect(screen.getByText("Add Your First Repository")).toBeInTheDocument();
    expect(screen.getByText("No repository tracked yet")).toBeInTheDocument();

    const chooseFolderBtn = screen.getByRole("button", { name: /Choose Folder…/i });
    fireEvent.click(chooseFolderBtn);
    expect(actions.addRepo).toHaveBeenCalledTimes(1);

    // Continue to Step 4
    const continueWithoutRepoBtn = screen.getByRole("button", { name: /Continue without repository/i });
    fireEvent.click(continueWithoutRepoBtn);

    // STEP 4: Ready / Done
    expect(screen.getByText(/You're ready to orchestrate!/i)).toBeInTheDocument();
    expect(screen.getByText("1. Sidebar & Lanes")).toBeInTheDocument();
    expect(screen.getByText("2. Spawn Agent")).toBeInTheDocument();
    expect(screen.getByText("3. Command Center")).toBeInTheDocument();

    const finishBtn = screen.getByRole("button", { name: "Open Mission Control" });
    fireEvent.click(finishBtn);

    expect(onComplete).toHaveBeenCalledTimes(1);
    expect(onSkip).not.toHaveBeenCalled();
  });

  it("handles Skip setup button directly from any step", () => {
    const actions = createMockActions();
    const onComplete = vi.fn();
    const onSkip = vi.fn();

    render(() => (
      <Onboarding
        actions={actions}
        onComplete={onComplete}
        onSkip={onSkip}
      />
    ));

    const skipBtn = screen.getByRole("button", { name: "Skip setup wizard" });
    fireEvent.click(skipBtn);

    expect(onSkip).toHaveBeenCalledTimes(1);
    expect(onComplete).not.toHaveBeenCalled();
  });

  it("renders tracked repositories in Step 3 when repos exist", () => {
    const mockRepo: Repo = {
      id: 1,
      name: "repomon",
      path: "/Users/dev/repomon",
      added_at: "2026-08-01T00:00:00Z",
      worktree_root_template: null,
      hidden: false,
    };
    const actions = createMockActions([mockRepo]);
    const onComplete = vi.fn();
    const onSkip = vi.fn();

    render(() => (
      <Onboarding
        actions={actions}
        onComplete={onComplete}
        onSkip={onSkip}
      />
    ));

    // Move to step 3 by clicking through
    fireEvent.click(screen.getByRole("button", { name: "Get started with setup" }));
    fireEvent.click(screen.getByRole("button", { name: /Continue/i }));

    expect(screen.getByText("Add Your First Repository")).toBeInTheDocument();
    expect(screen.getByText("repomon")).toBeInTheDocument();
    expect(screen.getByText("/Users/dev/repomon")).toBeInTheDocument();
    expect(screen.getByText("✓ 1 repo added")).toBeInTheDocument();
  });

  it("does NOT dismiss on Escape key", () => {
    const actions = createMockActions();
    const onComplete = vi.fn();
    const onSkip = vi.fn();

    render(() => (
      <Onboarding
        actions={actions}
        onComplete={onComplete}
        onSkip={onSkip}
      />
    ));

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onSkip).not.toHaveBeenCalled();
    expect(onComplete).not.toHaveBeenCalled();
  });

  it("advances with Enter key", () => {
    const actions = createMockActions();
    const onComplete = vi.fn();
    const onSkip = vi.fn();

    render(() => (
      <Onboarding
        actions={actions}
        onComplete={onComplete}
        onSkip={onSkip}
      />
    ));

    // Step 1 -> Enter -> Step 2
    fireEvent.keyDown(window, { key: "Enter" });
    expect(screen.getByText("System Health & Tooling")).toBeInTheDocument();

    // Step 2 -> Enter -> Step 3
    fireEvent.keyDown(window, { key: "Enter" });
    expect(screen.getByText("Add Your First Repository")).toBeInTheDocument();
  });
});
