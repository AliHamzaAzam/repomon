import { describe, expect, it } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import { formatStripAge, laneTitle, needsInputLanes, recentLanes, slugify, stripMark, stripStatus, uniqueBranchName } from "./home";

function repo(id: number, name: string): Repo {
  return { id, path: `/code/${name}`, name, added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null, accent: null };
}

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
    tool_call_count: 0,
    title: "Ship",
    status: "running",
    external: false,
    session_id: "s1",
    resume_at: null,
    inferred: false,
    tmux_window: "lane-1",
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
    status_reason: null,
    ...overrides,
  };
}

function lane(id: number, target: Repo, sessions: AgentSession[], lastActivityAt: string, branch: string | null = "main"): Lane {
  return {
    id,
    repo: target,
    worktree: { id, repo_id: target.id, path: `/code/${target.name}-wt`, branch, head: "abc", is_main: true, name: branch ?? `wt-${id}` },
    state: { worktree_id: id, head: "abc", branch, upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: sessions,
    last_activity_at: lastActivityAt,
    pinned: false,
    role: null,
  };
}

describe("needsInputLanes", () => {
  it("keeps only lanes whose most urgent agent needs the operator, newest first", () => {
    const repoA = repo(1, "a");
    const waiting = session({ status: "waiting", pending_prompt: "Allow Bash: bun run build?" });
    const running = session({ status: "running" });
    const older = lane(1, repoA, [waiting], "2026-09-10T10:00:00Z");
    const idle = lane(2, repoA, [running], "2026-09-10T12:00:00Z");
    const newer = lane(3, repoA, [waiting], "2026-09-10T11:00:00Z");

    const rows = needsInputLanes([older, idle, newer]);

    expect(rows.map((row) => row.lane.id)).toEqual([3, 1]);
    expect(rows[0].question).toBe("Allow Bash: bun run build?");
    expect(rows[0].isQuestion).toBe(true);
  });

  it("is empty when nothing needs the operator", () => {
    const repoA = repo(1, "a");
    const idle = lane(1, repoA, [session({ status: "running" })], "2026-09-10T10:00:00Z");
    expect(needsInputLanes([idle])).toEqual([]);
  });

  it("prefers the parsed dialog's question over the compact pending_prompt", () => {
    const repoA = repo(1, "a");
    const waiting = session({
      status: "waiting",
      pending_prompt: "Do you want to proceed?",
      pending_dialog: {
        title: "Bash command",
        question: "Do you want to proceed?",
        body: ["bun run build"],
        options: [],
        selected: null,
        context: [],
      },
    });
    const rows = needsInputLanes([lane(1, repoA, [waiting], "2026-09-10T10:00:00Z")]);
    expect(rows[0].question).toBe("Do you want to proceed?");
    expect(rows[0].isQuestion).toBe(true);
  });

  it("falls back to the daemon's status reason, unquoted, when the agent has no question", () => {
    // A daemon status_reason is a description of why the agent is waiting ("no output for
    // 4m", "pane unchanged for 4m"), never a question - it must never be quoted like one.
    const repoA = repo(1, "a");
    const stalled = session({ status: "running", stale: true, status_reason: "no output for 4m" });
    const rows = needsInputLanes([lane(1, repoA, [stalled], "2026-09-10T10:00:00Z")]);
    expect(rows[0].question).toBe("no output for 4m");
    expect(rows[0].isQuestion).toBe(false);
  });
});

describe("recentLanes", () => {
  it("excludes lanes needing input and sorts the rest by activity, newest first", () => {
    const repoA = repo(1, "a");
    const waiting = lane(1, repoA, [session({ status: "waiting" })], "2026-09-10T12:00:00Z");
    const older = lane(2, repoA, [session({ status: "ended" })], "2026-09-10T09:00:00Z");
    const newer = lane(3, repoA, [session({ status: "running" })], "2026-09-10T11:00:00Z");

    const rows = recentLanes([waiting, older, newer]);

    expect(rows.map((row) => row.id)).toEqual([3, 2]);
  });
});

describe("slugify", () => {
  it("lowercases, hyphenates and strips punctuation", () => {
    expect(slugify("Add collections to charms page!")).toBe("add-collections-to-charms-page");
  });

  it("caps length and trims a trailing hyphen left by truncation", () => {
    const long = "a".repeat(50);
    const slug = slugify(long);
    expect(slug.length).toBeLessThanOrEqual(40);
    expect(slug.endsWith("-")).toBe(false);
  });

  it("never returns empty, even with no latin or digit characters", () => {
    expect(slugify("こんにちは")).toBe("task");
    expect(slugify("   ")).toBe("task");
  });
});

describe("uniqueBranchName", () => {
  it("uses the bare slug when it is free", () => {
    expect(uniqueBranchName("Fix flaky login test", [])).toBe("fix-flaky-login-test");
  });

  it("appends the smallest free numeric suffix on collision", () => {
    const taken = ["fix-flaky-login-test", "fix-flaky-login-test-2"];
    expect(uniqueBranchName("Fix flaky login test", taken)).toBe("fix-flaky-login-test-3");
  });
});

describe("laneTitle", () => {
  const target = repo(1, "a");
  const withBranch = lane(1, target, [], "2026-09-10T10:00:00Z", "feature/x");
  const detached = lane(2, target, [], "2026-09-10T10:00:00Z", null);

  it("prefers the transcript headline when the daemon has one cached", () => {
    expect(laneTitle(withBranch, "Add collections to charms page")).toBe("Add collections to charms page");
  });

  it("falls back to the branch name while the headline is loading or absent", () => {
    expect(laneTitle(withBranch, undefined)).toBe("feature/x");
    expect(laneTitle(withBranch, null)).toBe("feature/x");
  });

  it("falls back to the worktree name for a detached lane with no headline", () => {
    expect(laneTitle(detached, null)).toBe(`wt-${detached.id}`);
  });
});

describe("formatStripAge", () => {
  const now = Date.parse("2026-09-10T12:00:00Z");

  it("formats minutes, hours and days without an 'ago' suffix", () => {
    expect(formatStripAge("2026-09-10T11:56:00Z", now)).toBe("4m");
    expect(formatStripAge("2026-09-10T10:00:00Z", now)).toBe("2h");
    expect(formatStripAge("2026-09-08T12:00:00Z", now)).toBe("2d");
  });

  it("reads under a minute as now", () => {
    expect(formatStripAge("2026-09-10T11:59:45Z", now)).toBe("now");
  });
});

describe("stripMark", () => {
  it("marks an urgent state with the attention bolt", () => {
    expect(stripMark("needs-you")).toEqual({ icon: "bolt", tone: "attention" });
    expect(stripMark("decision")).toEqual({ icon: "bolt", tone: "attention" });
  });

  it("marks running and inferred activity with the signal play mark", () => {
    expect(stripMark("running")).toEqual({ icon: "play", tone: "signal" });
    expect(stripMark("inferred")).toEqual({ icon: "play", tone: "signal" });
  });

  it("marks an exited process without claiming task completion", () => {
    expect(stripMark("exited")).toEqual({ icon: "stop", tone: "muted" });
  });

  it("distinguishes idle sessions from lanes with no agent", () => {
    expect(stripMark("idle")).toEqual({ icon: "idle", tone: "muted" });
    expect(stripMark(null)).toEqual({ icon: "branch", tone: "muted" });
  });
});


describe("ordinary fleet identity and status", () => {
  it("ignores whitespace headlines and retains detached worktree names", () => {
    expect(laneTitle(lane(1, repo(1, "repomon"), [], "", "main"), "  ")).toBe("main");
    expect(laneTitle(lane(2, repo(1, "repomon"), [], "", null), null)).toBe("wt-2");
  });

  it("requires explicit completion metadata and never treats exit as success", () => {
    const target = repo(1, "repomon");
    expect(stripStatus(lane(1, target, [session({ status: "ended" })], "")).label).toBe("Exited");
    expect(stripStatus(lane(2, target, [session({ status: "waiting" })], "")).label).toBe("Needs you");
    const completed = lane(3, target, [session({ status: "waiting", attention_kind: "end_of_turn" })], "");
    expect(stripStatus(completed).label).toBe("Turn complete");
    expect(needsInputLanes([completed])).toHaveLength(1);
    completed.agent_sessions[0].pending_prompt = "Which approach?";
    expect(stripStatus(completed).label).toBe("Needs you");
  });
});
