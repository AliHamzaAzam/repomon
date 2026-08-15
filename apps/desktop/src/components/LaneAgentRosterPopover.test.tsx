import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
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
    ended_turn: true,
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
    hidden: false,
  };
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
  };
}

describe("LaneAgentRosterPopover", () => {
  it("formats agent display names and status details accurately", () => {
    expect(agentKindDisplayName("claude-code")).toBe("Claude Code");
    expect(agentKindDisplayName("antigravity")).toBe("Antigravity");
    expect(agentKindDisplayName("codex")).toBe("Codex");
    expect(agentKindDisplayName("opencode")).toBe("OpenCode");

    const s1 = session({ agent: "claude-code", tmux_window: "lane-81-3" });
    expect(agentSessionTitle(s1)).toBe("Claude Code #3");

    const sCustom = session({ custom_label: "Review PR" });
    expect(agentSessionTitle(sCustom)).toBe("Review PR");

    const sPrompt = session({ pending_prompt: "Allow command?" });
    expect(getSessionStatusDetails(sPrompt).label).toBe("Decision");

    const sWaiting = session({ status: "waiting" });
    expect(getSessionStatusDetails(sWaiting).label).toBe("Needs attention");

    const sLimited = session({ status: "rate-limited" });
    expect(getSessionStatusDetails(sLimited).label).toBe("Rate limited");

    const sExt = session({ external: true });
    expect(getSessionStatusDetails(sExt).label).toBe("External");
  });

  it("renders a structured roster card with multiple agents on hover", () => {
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
      status: "waiting",
      last_message: "Should I proceed with the refactor?",
    });

    const lane = createLane([s1, s2]);
    const mockRect = { top: 100, right: 250, bottom: 140, left: 10, width: 240, height: 40, x: 10, y: 100, toJSON: () => ({}) } as DOMRect;

    render(() => (
      <LaneAgentRosterPopover lane={lane} anchorRect={mockRect} visible={true} />
    ));

    expect(screen.getByText("feature-roster-lane")).toBeInTheDocument();
    expect(screen.getByText("feat-roster-branch")).toBeInTheDocument();
    expect(screen.getByText("2 agents")).toBeInTheDocument();

    expect(screen.getByText("Architect")).toBeInTheDocument();
    expect(screen.getByText("Antigravity")).toBeInTheDocument();
    expect(screen.getByText("Running")).toBeInTheDocument();

    expect(screen.getByText("Claude Code #2")).toBeInTheDocument();
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
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
