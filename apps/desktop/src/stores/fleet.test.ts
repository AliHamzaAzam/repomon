import { describe, expect, it } from "vitest";

import type { AgentSession, Lane } from "../bindings";
import { laneIndicator, matchesLane } from "./fleet";

function lane(overrides: Partial<Lane> = {}): Lane {
  return {
    id: 7,
    repo: { id: 2, path: "/code/repomon", name: "repomon", added_at: "2026-07-20T00:00:00Z", worktree_root_template: null },
    worktree: { id: 3, repo_id: 2, path: "/code/repomon-wt/desktop", branch: "feat/desktop", head: "abc", is_main: false, name: "desktop" },
    state: { worktree_id: 3, head: "abc", branch: "feat/desktop", upstream: null, ahead: 2, behind: 0, dirty: { staged: 0, unstaged: 1, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: [],
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
    ...overrides,
  };
}

function agent(overrides: Partial<AgentSession> = {}): AgentSession {
  return {
    id: 9,
    agent: "claude-code",
    repo_id: 2,
    worktree_id: 3,
    started_at: "2026-07-20T00:00:00Z",
    last_activity_at: "2026-07-20T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 0,
    title: "Ship desktop",
    status: "waiting",
    external: false,
    session_id: "s1",
    resume_at: null,
    inferred: false,
    tmux_window: "lane-7",
    last_message: null,
    pending_prompt: null,
    pending_dialog: null,
    stale: false,
    stalled_since: null,
    gate: null,
    config_dir: null,
    custom_label: null,
    ...overrides,
  };
}

describe("fleet presentation", () => {
  it("prioritizes live dialogs as urgent decisions", () => {
    const target = lane({
      agent_sessions: [agent({
        pending_prompt: "Run tests?",
        pending_dialog: { title: "Bash", question: "Run tests?", body: [], options: [], selected: null },
      })],
    });

    expect(laneIndicator(target)).toEqual({ label: "decision", tone: "attention", urgent: true });
  });

  it("shows blocked gates and inferred activity without making them actionable", () => {
    const blocked = lane({
      agent_sessions: [agent({
        status: "running",
        gate: { allowed: false, net_new_findings: 3, at: "2026-07-20T00:00:00Z", session_id: "s1" },
      })],
    });
    const inferred = lane({
      agent_sessions: [agent({ status: "running", inferred: true, tmux_window: null, session_id: null })],
    });

    expect(laneIndicator(blocked)).toEqual({ label: "running · gate 3", tone: "signal", urgent: false });
    expect(laneIndicator(inferred)).toEqual({ label: "active · inferred", tone: "signal", urgent: false });
  });

  it("fuzzy matches repo, branch, and agent text", () => {
    const target = lane();
    expect(matchesLane(target, "rpmndsk")).toBe(true);
    expect(matchesLane(target, "featdesktop")).toBe(true);
    expect(matchesLane(target, "unrelated")).toBe(false);
  });
});
