import type { AgentSession, Lane, RepomindStatus, Repo } from "../bindings";

/// Shares a consistent home, controller lane, and status payload across control-room tests.

export const home: Repo = {
  id: 9,
  path: "/Users/pat/repomind",
  name: "repomind",
  added_at: "2026-09-04T00:00:00Z",
  worktree_root_template: null,
  hidden: false,
  position: null,
  label: null,
};

export function session(overrides: Partial<AgentSession> = {}): AgentSession {
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
    title: "Repomind",
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
    custom_label: "Primary",
    generated_label: null,
    ...overrides,
  };
}

export function controllerLane(sessions: AgentSession[]): Lane {
  return {
    id: 90,
    repo: home,
    worktree: {
      id: 90,
      repo_id: 9,
      path: "/Users/pat/repomind",
      branch: "main",
      head: "abc",
      is_main: true,
      name: "main",
    },
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

export function status(overrides: Partial<RepomindStatus> = {}): RepomindStatus {
  return {
    home: "/Users/pat/repomind",
    exists: true,
    repo_id: 9,
    lane_id: 90,
    window: "repomind-1",
    max_controllers: 2,
    export: { last_run: null, pending: false, last_error: null },
    counts: { active_plans: 1, standing: 0, playbooks: 0, drafts: 0 },
    boot: { generated_at: null, tokens_estimate: 0, trimmed: [] },
    ...overrides,
  };
}
