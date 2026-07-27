import { createRoot } from "solid-js";
import { describe, expect, it } from "vitest";

import type { AccountUsage, AgentSession, Lane, Repo } from "../bindings";
import {
  createFleetStore,
  laneIndicator,
  matchesLane,
  pickFocusedUsage,
  sortReposByActivity,
  withSessionKeys,
  type FleetSource,
} from "./fleet";

function lane(overrides: Partial<Lane> = {}): Lane {
  return {
    id: 7,
    repo: { id: 2, path: "/code/repomon", name: "repomon", added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false },
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

function repo(id: number, name: string, hidden = false): Repo {
  return { id, path: `/code/${name}`, name, added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden };
}

/// A store fed one fixed snapshot, started and refreshed once. `subscribe` never fires, and the
/// 2s heartbeat is stopped before the test asserts, so nothing races the assertions.
async function startedStore(repos: Repo[], lanes: Lane[], sortReposByActivity: boolean | null = false) {
  const source: FleetSource = {
    load: () => Promise.resolve({ repos, lanes, usage: [], terminals: [], sortReposByActivity }),
    subscribe: () => Promise.resolve(() => undefined),
  };
  return createRoot((dispose) => {
    const fleet = createFleetStore(source);
    fleet.start();
    return { fleet, teardown: () => { fleet.stop(); dispose(); } };
  });
}

describe("hidden repos", () => {
  it("partitions repos and takes their lanes, counts, and selection with them", async () => {
    const shown = repo(1, "visible");
    const gone = repo(2, "hidden", true);
    const lanes = [
      lane({ id: 10, repo: shown, agent_sessions: [agent({ status: "running" })] }),
      lane({ id: 20, repo: gone, agent_sessions: [agent({ status: "running" })] }),
    ];
    const { fleet, teardown } = await startedStore([shown, gone], lanes);
    await fleet.refresh();

    expect(fleet.visibleRepos().map((r) => r.id)).toEqual([1]);
    expect(fleet.hiddenRepos().map((r) => r.id)).toEqual([2]);
    expect(fleet.visibleLanes().map((l) => l.id)).toEqual([10]);
    // A running count you cannot click through to is just noise.
    expect(fleet.counts().running).toBe(1);
    // Auto-selection never lands inside a hidden repo.
    expect(fleet.selectedLaneId()).toBe(10);

    // The daemon still hands us the hidden repo and its lanes, so unhiding stays possible.
    expect(fleet.repos()).toHaveLength(2);
    expect(fleet.lanes()).toHaveLength(2);
    teardown();
  });
});

describe("sortReposByActivity", () => {
  const alpha = repo(1, "alpha");
  const beta = repo(2, "beta");
  const gamma = repo(3, "gamma");
  const at = (id: number, target: Repo, when: string) => lane({ id, repo: target, last_activity_at: when });

  it("leaves the daemon order alone when the setting is off", () => {
    const lanes = [at(10, alpha, "2026-07-20T00:00:00Z"), at(20, beta, "2026-07-27T00:00:00Z")];
    expect(sortReposByActivity([alpha, beta], lanes, false)).toEqual([alpha, beta]);
  });

  it("floats the project with the newest lane activity to the top", () => {
    const lanes = [
      at(10, alpha, "2026-07-20T00:00:00Z"),
      at(20, beta, "2026-07-27T00:00:00Z"),
      // A repo's newest lane is what counts, not its oldest.
      at(21, beta, "2026-07-01T00:00:00Z"),
    ];
    expect(sortReposByActivity([alpha, beta], lanes, true).map((r) => r.id)).toEqual([2, 1]);
  });

  it("sinks projects with no lanes and keeps ties in daemon order", () => {
    const lanes = [at(10, alpha, "2026-07-20T00:00:00Z"), at(20, beta, "2026-07-20T00:00:00Z")];
    // gamma has no lanes at all; alpha and beta tie, so they hold their incoming order.
    expect(sortReposByActivity([alpha, gamma, beta], lanes, true).map((r) => r.id)).toEqual([1, 2, 3]);
  });

  it("does not mutate the array it was given", () => {
    const input = [alpha, beta];
    sortReposByActivity(input, [at(20, beta, "2026-07-27T00:00:00Z")], true);
    expect(input).toEqual([alpha, beta]);
  });
});

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

  it("attributes usage to the focused lane's account, not the first probe", () => {
    const usage = (key: string, pct: number): AccountUsage => ({
      key,
      label: key,
      report: { windows: [{ label: "5h", pct_used: pct, reset_at: null }] },
      age_secs: 10,
    });
    const workFirst = [usage("/Users/me/.claude-work", 48), usage("default", 9)];

    // Default-account lane must show the default report even though work is first in the list.
    const onDefault = lane({ agent_sessions: [agent({ config_dir: null })] });
    expect(pickFocusedUsage(workFirst, onDefault)?.key).toBe("default");

    // A work-account lane shows work.
    const onWork = lane({ agent_sessions: [agent({ config_dir: "/Users/me/.claude-work" })] });
    expect(pickFocusedUsage(workFirst, onWork)?.key).toBe("/Users/me/.claude-work");

    // Focused account not yet probed: show nothing rather than another account's numbers.
    expect(pickFocusedUsage([usage("/Users/me/.claude-work", 48)], onDefault)).toBeNull();

    // Inferred sessions carry no account, so the real session's account wins.
    const mixed = lane({
      agent_sessions: [agent({ inferred: true, config_dir: null }), agent({ config_dir: "/Users/me/.claude-work" })],
    });
    expect(pickFocusedUsage(workFirst, mixed)?.key).toBe("/Users/me/.claude-work");

    // No agent to attribute to: fall back to the first report.
    expect(pickFocusedUsage(workFirst, lane())?.key).toBe("/Users/me/.claude-work");
    expect(pickFocusedUsage([], onDefault)).toBeNull();
  });

  it("keys codex on its own probe rather than the default Claude account", () => {
    const usage = (key: string): AccountUsage => ({
      key,
      label: key,
      report: { windows: [{ label: "5h", pct_used: 20, reset_at: null }] },
      age_secs: 10,
    });
    const reports = [usage("default"), usage("codex")];

    // A codex session also has `config_dir: null`. Keying on that alone resolved it to "default"
    // and showed Claude's numbers on a codex lane.
    const onCodex = lane({ agent_sessions: [agent({ agent: "codex", config_dir: null })] });
    expect(pickFocusedUsage(reports, onCodex)?.key).toBe("codex");

    // Claude sessions are unaffected.
    const onClaude = lane({ agent_sessions: [agent({ config_dir: null })] });
    expect(pickFocusedUsage(reports, onClaude)?.key).toBe("default");

    // Codex probed but never run here: blank, not Claude's numbers.
    expect(pickFocusedUsage([usage("default")], onCodex)).toBeNull();
  });

  it("prefers the agent in the focused pane over the lane's first session", () => {
    const usage = (key: string): AccountUsage => ({
      key,
      label: key,
      report: { windows: [{ label: "5h", pct_used: 20, reset_at: null }] },
      age_secs: 10,
    });
    const reports = [usage("default"), usage("codex")];
    const mixed = lane({
      agent_sessions: [
        agent({ session_id: "a", tmux_window: "lane-7", config_dir: null }),
        agent({ session_id: "b", agent: "codex", tmux_window: "lane-7-2", config_dir: null }),
      ],
    });

    expect(pickFocusedUsage(reports, mixed, "lane-7-2")?.key).toBe("codex");
    expect(pickFocusedUsage(reports, mixed, "lane-7")?.key).toBe("default");

    // A window that is no longer in this lane falls back to the first non-inferred session.
    expect(pickFocusedUsage(reports, mixed, "lane-9")?.key).toBe("default");
  });

  it("gives overlay sessions distinct stable reconcile keys", () => {
    // Overlay sessions all arrive with id 0; duplicate keys would make reconcile collapse
    // the lane's sessions to one (the missing-second-agent-tab bug).
    const target = lane({
      agent_sessions: [
        agent({ id: 0, session_id: "s1", tmux_window: "lane-7" }),
        agent({ id: 0, session_id: null, tmux_window: "lane-7-2" }),
      ],
    });

    const [first, second] = withSessionKeys([target])[0].agent_sessions;
    expect(first.id).not.toBe(0);
    expect(second.id).not.toBe(0);
    expect(first.id).not.toBe(second.id);
    // Stable across polls: the same identity hashes to the same key.
    expect(withSessionKeys([target])[0].agent_sessions[0].id).toBe(first.id);
    // A persisted (non-zero) id passes through untouched.
    expect(withSessionKeys([lane({ agent_sessions: [agent({ id: 42 })] })])[0].agent_sessions[0].id).toBe(42);
  });
});
