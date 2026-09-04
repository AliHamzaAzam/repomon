import { cleanup, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import { controllerSummary } from "../stores/fleet";
import { RepomindStateDot } from "./RepomindRow";

afterEach(() => {
  cleanup();
});

const home: Repo = {
  id: 9,
  path: "/Users/pat/repomind",
  name: "repomind",
  added_at: "2026-09-04T00:00:00Z",
  worktree_root_template: null,
  hidden: false,
  position: null,
  label: null,
};

function session(overrides: Partial<AgentSession> = {}): AgentSession {
  return {
    id: 1,
    agent: "claude-code",
    repo_id: 9,
    worktree_id: 9,
    started_at: "2026-09-04T00:00:00Z",
    last_activity_at: "2026-09-04T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 0,
    title: null,
    status: "running",
    external: false,
    session_id: "c1",
    resume_at: null,
    inferred: false,
    tmux_window: "repomind-1",
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

function controllerLane(sessions: AgentSession[]): Lane {
  return {
    id: 90,
    repo: home,
    worktree: { id: 90, repo_id: 9, path: "/Users/pat/repomind", branch: "main", head: "abc", is_main: true, name: "main" },
    state: {
      worktree_id: 90,
      head: "abc",
      branch: "main",
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
    last_activity_at: "2026-09-04T00:00:00Z",
    pinned: false,
    role: "controller",
  };
}

describe("the Repomind toolbar state dot", () => {
  it("shows nothing at all when the home is off", () => {
    render(() => <RepomindStateDot controller={controllerSummary([controllerLane([])])} />);
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("shows a signal dot while a controller runs", () => {
    render(() => <RepomindStateDot controller={controllerSummary([controllerLane([session()])])} />);
    const dot = screen.getByRole("img", { name: "Repomind: running" });
    expect(dot.className).toContain("bg-signal");
  });

  it("switches to the attention color when a controller wants the operator", () => {
    const lane = controllerLane([session(), session({ id: 2, session_id: "c2", status: "waiting" })]);
    render(() => <RepomindStateDot controller={controllerSummary([lane])} />);
    const dot = screen.getByRole("img", { name: "Repomind: needs you" });
    expect(dot.className).toContain("bg-attention");
  });

  it("uses the fault color for a stalled controller rather than flattening it to attention", () => {
    const lane = controllerLane([session({ stale: true })]);
    render(() => <RepomindStateDot controller={controllerSummary([lane])} />);
    const dot = screen.getByRole("img", { name: "Repomind: stalled" });
    expect(dot.className).toContain("bg-fault");
  });
});
