import { describe, expect, it } from "vitest";

import type { AgentSession } from "../bindings";
import { agentLabel, primarySession, slotOf } from "./agentLabel";

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
    title: null,
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

describe("slotOf", () => {
  it("reads the first slot from a bare lane window", () => {
    expect(slotOf("lane-7")).toBe(1);
  });

  it("reads a later slot from a suffixed lane window", () => {
    expect(slotOf("lane-7-3")).toBe(3);
  });

  it("returns null for a GUI shell window", () => {
    expect(slotOf("shell-7-2")).toBeNull();
  });

  it("returns null when the session has no window", () => {
    expect(slotOf(null)).toBeNull();
  });
});

describe("agentLabel", () => {
  it("prefers a user-set label", () => {
    expect(agentLabel(agent({ custom_label: "reviewer", title: "Ship desktop" }))).toBe("reviewer");
  });

  it("falls back to the session title before the numbered form", () => {
    expect(agentLabel(agent({ title: "Ship desktop" }))).toBe("Ship desktop");
  });

  it("numbers by window slot when there is no title", () => {
    expect(agentLabel(agent({ agent: "codex", tmux_window: "lane-7-2" }))).toBe("codex 2");
  });

  it("numbers a windowless session as the first slot", () => {
    expect(agentLabel(agent({ agent: "codex", tmux_window: null }))).toBe("codex 1");
  });

  // The regression: the daemon hands sessions over newest-transcript-first, so the array reorders
  // itself whenever the lane's agents take turns. A label must depend only on the session.
  it("gives a window the same label regardless of array order", () => {
    const first = agent({ session_id: "a", tmux_window: "lane-7" });
    const second = agent({ session_id: "b", tmux_window: "lane-7-2" });

    const forward = [first, second].map(agentLabel);
    const reversed = [second, first].map(agentLabel);

    expect(forward).toEqual(["claude-code 1", "claude-code 2"]);
    expect(reversed).toEqual(["claude-code 2", "claude-code 1"]);
  });
});

describe("primarySession", () => {
  it("picks the lowest window slot regardless of array order", () => {
    const first = agent({ session_id: "a", tmux_window: "lane-7", title: "first" });
    const second = agent({ session_id: "b", tmux_window: "lane-7-2", title: "second" });

    expect(primarySession([second, first])?.title).toBe("first");
    expect(primarySession([first, second])?.title).toBe("first");
  });

  it("falls back to the first session when none has a lane window", () => {
    const external = agent({ tmux_window: null, title: "external" });
    expect(primarySession([external])?.title).toBe("external");
  });

  it("returns undefined for a lane with no sessions", () => {
    expect(primarySession([])).toBeUndefined();
  });
});
