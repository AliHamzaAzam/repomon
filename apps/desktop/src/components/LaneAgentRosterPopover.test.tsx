import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import {
  LaneAgentRosterPopover,
  agentKindDisplayName,
  agentSessionTitle,
  getSessionStatusDetails,
} from "./LaneAgentRosterPopover";

afterEach(() => {
  cleanup();
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
    tool_call_count: 5,
    title: "Feature",
    status: "running",
    external: false,
    session_id: "s1",
    resume_at: null,
    inferred: false,
    tmux_window: "lane-1-2",
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

function createLane(sessions: AgentSession[] = []): Lane {
  const target: Repo = {
    id: 1,
    name: "repomon",
    path: "/code/repomon",
    added_at: "2026-07-20T00:00:00Z",
    worktree_root_template: null,
    hidden: false, position: null, label: null };
  return {
    id: 10,
    repo: target,
    worktree: {
      id: 1,
      repo_id: 1,
      path: "/code/repomon",
      branch: "feat-roster-branch",
      head: "abc",
      is_main: false,
      name: "feature-roster-lane",
    },
    state: {
      worktree_id: 1,
      head: "abc",
      branch: "feat-roster-branch",
      upstream: null,
      ahead: 0,
      behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 },
      last_commit_at: null,
      locked: false,
      prunable: false,
      last_change_at: null,
    },
    agent_sessions: sessions,
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
    role: null,
  };
}

describe("LaneAgentRosterPopover", () => {
  it("formats agent display names and status details accurately", () => {
    expect(agentKindDisplayName("claude-code")).toBe("Claude Code");
    expect(agentKindDisplayName("antigravity")).toBe("Antigravity");
    expect(agentKindDisplayName("hermes")).toBe("Hermes Agent");
    expect(agentKindDisplayName("codex")).toBe("Codex");
    expect(agentKindDisplayName("opencode")).toBe("OpenCode");

    const s1 = session({ agent: "claude-code", tmux_window: "lane-81-3" });
    expect(agentSessionTitle(s1)).toBe("Claude Code #3");

    const sCustom = session({ custom_label: "Review PR" });
    expect(agentSessionTitle(sCustom)).toBe("Review PR");

    const sPrompt = session({ pending_prompt: "Allow command?" });
    expect(getSessionStatusDetails(sPrompt).label).toBe("Decision");

    const sSubagent = session({ subagent_running: "Drafting 2026-08-api-contract.md (3m 16s)" });
    expect(getSessionStatusDetails(sSubagent).label).toBe("Subagent running");

    const sWaiting = session({ status: "waiting" });
    expect(getSessionStatusDetails(sWaiting).label).toBe("Needs attention");

    const sLimited = session({ status: "rate-limited" });
    expect(getSessionStatusDetails(sLimited).label).toBe("Rate limited");

    const sExt = session({ external: true });
    expect(getSessionStatusDetails(sExt).label).toBe("External");
  });

  it("renders a structured roster card with multiple agents and subagents on hover", () => {
    const s1 = session({
      id: 1,
      agent: "antigravity",
      tmux_window: "lane-10",
      status: "running",
      custom_label: "Architect",
    });
    const s2 = session({
      id: 2,
      agent: "claude-code",
      tmux_window: "lane-10-2",
      status: "running",
      subagent_running: "Editing interruption defaults in main.py (6m 54s) + 1 more",
    });
    const s3 = session({
      id: 3,
      agent: "claude-code",
      tmux_window: "lane-10-3",
      status: "waiting",
      last_message: "Should I proceed with the refactor?",
    });

    const lane = createLane([s1, s2, s3]);
    const mockRect = { top: 100, right: 250, bottom: 140, left: 10, width: 240, height: 40, x: 10, y: 100, toJSON: () => ({}) } as DOMRect;

    render(() => (
      <LaneAgentRosterPopover lane={lane} anchorRect={mockRect} visible={true} />
    ));

    expect(screen.getByText("feature-roster-lane")).toBeInTheDocument();
    expect(screen.getByText("feat-roster-branch")).toBeInTheDocument();
    expect(screen.getByText("3 agents")).toBeInTheDocument();

    expect(screen.getByText("Architect")).toBeInTheDocument();
    expect(screen.getByText("Antigravity")).toBeInTheDocument();
    expect(screen.getByText("Running")).toBeInTheDocument();

    expect(screen.getByText("Claude Code #2")).toBeInTheDocument();
    expect(screen.getByText("Subagent running")).toBeInTheDocument();
    expect(screen.getByText("Editing interruption defaults in main.py (6m 54s) + 1 more")).toBeInTheDocument();

    expect(screen.getByText("Claude Code #3")).toBeInTheDocument();
    expect(screen.getByText("Needs attention")).toBeInTheDocument();
    expect(screen.getByText('"Should I proceed with the refactor?"')).toBeInTheDocument();
  });

  it("triggers onSelectAgent when an agent row button is clicked", () => {
    const onSelectAgent = vi.fn();
    const s1 = session({
      id: 1,
      agent: "antigravity",
      tmux_window: "lane-10",
      custom_label: "Architect",
    });
    const lane = createLane([s1]);
    const mockRect = { top: 100, right: 250, bottom: 140, left: 10, width: 240, height: 40, x: 10, y: 100, toJSON: () => ({}) } as DOMRect;

    render(() => (
      <LaneAgentRosterPopover
        lane={lane}
        anchorRect={mockRect}
        visible={true}
        onSelectAgent={onSelectAgent}
      />
    ));

    const agentBtn = screen.getByRole("button", { name: /switch to architect terminal/i });
    fireEvent.click(agentBtn);

    expect(onSelectAgent).toHaveBeenCalledTimes(1);
    expect(onSelectAgent).toHaveBeenCalledWith(lane, s1);
  });

  it("does not render when visible is false or lane has no sessions", () => {
    const lane = createLane([]);
    const mockRect = { top: 100, right: 250, bottom: 140, left: 10, width: 240, height: 40, x: 10, y: 100, toJSON: () => ({}) } as DOMRect;

    const { unmount } = render(() => (
      <LaneAgentRosterPopover lane={lane} anchorRect={mockRect} visible={true} />
    ));
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    unmount();

    const activeLane = createLane([session()]);
    render(() => (
      <LaneAgentRosterPopover lane={activeLane} anchorRect={mockRect} visible={false} />
    ));
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });
});

describe("manual agent tab ordering and rename", () => {
  const mockRect = { top: 100, right: 250, bottom: 140, left: 10, width: 240, height: 40, x: 10, y: 100, toJSON: () => ({}) } as DOMRect;

  function twoSessionLane() {
    return createLane([
      session({ id: 1, session_id: "s1", tmux_window: "lane-1", custom_label: "Architect" }),
      session({ id: 2, session_id: "s2", tmux_window: "lane-1-2" }),
    ]);
  }

  /// Supplies row geometry for drag midpoint calculations because jsdom rectangles are zero-sized.
  function stubRowRects() {
    const rects = new Map<HTMLElement, () => DOMRect>();
    const architect = screen.getByRole("button", { name: /switch to architect terminal/i });
    const second = screen.getByRole("button", { name: /switch to claude code #2 terminal/i });
    rects.set(architect, () => new DOMRect(0, 0, 300, 50));
    rects.set(second, () => new DOMRect(0, 50, 300, 50));
    for (const [el, rect] of rects) {
      Object.defineProperty(el, "getBoundingClientRect", { value: rect, configurable: true });
    }
  }

  it("drags a row past its neighbor's midpoint to reorder, and commits on release", async () => {
    const onReorderTabs = vi.fn();
    render(() => (
      <LaneAgentRosterPopover
        lane={twoSessionLane()}
        anchorRect={mockRect}
        visible={true}
        reorderable={true}
        onReorderTabs={onReorderTabs}
      />
    ));
    stubRowRects();

    const dragged = screen.getByRole("button", { name: /switch to claude code #2 terminal/i });
    // Cross the threshold and row midpoint, flushing both animation frames before asserting.
    fireEvent.pointerDown(dragged, { button: 0, clientX: 10, clientY: 75, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 10, clientY: 70, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 10, clientY: 20, pointerId: 1 });
    await new Promise((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(resolve)),
    );

    expect(onReorderTabs).not.toHaveBeenCalled();

    fireEvent.pointerUp(window, { clientX: 10, clientY: 20, pointerId: 1 });
    expect(onReorderTabs).toHaveBeenCalledTimes(1);
    expect(onReorderTabs).toHaveBeenCalledWith(["win:lane-1-2", "win:lane-1"]);
  });

  it("does not reorder while the tab sort mode is activity (reorderable off)", async () => {
    const onReorderTabs = vi.fn();
    render(() => (
      <LaneAgentRosterPopover
        lane={twoSessionLane()}
        anchorRect={mockRect}
        visible={true}
        reorderable={false}
        onReorderTabs={onReorderTabs}
      />
    ));

    const dragged = screen.getByRole("button", { name: /switch to claude code #2 terminal/i });
    fireEvent.pointerDown(dragged, { button: 0, clientX: 10, clientY: 5, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 10, clientY: 500, pointerId: 1 });
    fireEvent.pointerUp(window, { clientX: 10, clientY: 500, pointerId: 1 });

    expect(onReorderTabs).not.toHaveBeenCalled();
  });

  it("right-clicks a tab to rename it via the durable custom label flow", () => {
    const onRenameAgent = vi.fn();
    render(() => (
      <LaneAgentRosterPopover
        lane={twoSessionLane()}
        anchorRect={mockRect}
        visible={true}
        onRenameAgent={onRenameAgent}
      />
    ));

    fireEvent.contextMenu(screen.getByRole("button", { name: /switch to architect terminal/i }));
    expect(onRenameAgent).toHaveBeenCalledTimes(1);
    expect(onRenameAgent.mock.calls[0][0].session_id).toBe("s1");
  });

  it("renames and reorders a managed session without a transcript id", async () => {
    const onRenameAgent = vi.fn();
    const onReorderTabs = vi.fn();
    const lane = createLane([
      session({ id: 1, session_id: null, tmux_window: "lane-1", custom_label: "Codex" }),
      session({ id: 2, session_id: "s2", tmux_window: "lane-1-2" }),
    ]);
    render(() => (
      <LaneAgentRosterPopover
        lane={lane}
        anchorRect={mockRect}
        visible={true}
        reorderable={true}
        onRenameAgent={onRenameAgent}
        onReorderTabs={onReorderTabs}
      />
    ));

    const codex = screen.getByRole("button", { name: /switch to codex terminal/i });
    expect(codex.getAttribute("data-reorder-id")).toBe("win:lane-1");
    fireEvent.contextMenu(codex);
    expect(onRenameAgent).toHaveBeenCalledWith(lane.agent_sessions[0]);
  });

  it("drops a stale optimistic order when the backend order changes externally", async () => {
    const onReorderTabs = vi.fn();
    const initialLane = createLane([
      session({ id: 1, session_id: "s1", tmux_window: "lane-1", custom_label: "Architect" }),
      session({ id: 2, session_id: "s2", tmux_window: "lane-1-2" }),
      session({ id: 3, session_id: "s3", tmux_window: "lane-1-3" }),
    ]);
    const [currentLane, setCurrentLane] = createSignal(initialLane);
    render(() => (
      <LaneAgentRosterPopover
        lane={currentLane()}
        anchorRect={mockRect}
        visible={true}
        reorderable={true}
        onReorderTabs={onReorderTabs}
      />
    ));
    stubRowRects();

    const dragged = screen.getByRole("button", { name: /switch to claude code #2 terminal/i });
    fireEvent.pointerDown(dragged, { button: 0, clientX: 10, clientY: 75, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 10, clientY: 70, pointerId: 1 });
    fireEvent.pointerMove(window, { clientX: 10, clientY: 20, pointerId: 1 });
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    fireEvent.pointerUp(window, { clientX: 10, clientY: 20, pointerId: 1 });
    expect(onReorderTabs).toHaveBeenCalledWith([
      "win:lane-1-2",
      "win:lane-1",
      "win:lane-1-3",
    ]);

    // An authoritative order from another surface must invalidate this surface’s optimistic order.
    setCurrentLane(createLane([
      session({ id: 3, session_id: "s3", tmux_window: "lane-1-3" }),
      session({ id: 1, session_id: "s1", tmux_window: "lane-1", custom_label: "Architect" }),
      session({ id: 2, session_id: "s2", tmux_window: "lane-1-2" }),
    ]));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: /switch to /i })[0]).toHaveAccessibleName(
        "Switch to Claude Code #3 terminal",
      );
    });
  });
});
