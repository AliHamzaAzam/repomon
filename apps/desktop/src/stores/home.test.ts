import { describe, expect, it } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import { needsInputLanes, recentLanes, slugify, uniqueBranchName } from "./home";

function repo(id: number, name: string): Repo {
  return { id, path: `/code/${name}`, name, added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null };
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
    const waiting = session({ status: "waiting", status_reason: `"Allow Bash: bun run build?"` });
    const running = session({ status: "running" });
    const older = lane(1, repoA, [waiting], "2026-09-10T10:00:00Z");
    const idle = lane(2, repoA, [running], "2026-09-10T12:00:00Z");
    const newer = lane(3, repoA, [waiting], "2026-09-10T11:00:00Z");

    const rows = needsInputLanes([older, idle, newer]);

    expect(rows.map((row) => row.lane.id)).toEqual([3, 1]);
    expect(rows[0].question).toBe(`"Allow Bash: bun run build?"`);
  });

  it("is empty when nothing needs the operator", () => {
    const repoA = repo(1, "a");
    const idle = lane(1, repoA, [session({ status: "running" })], "2026-09-10T10:00:00Z");
    expect(needsInputLanes([idle])).toEqual([]);
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
