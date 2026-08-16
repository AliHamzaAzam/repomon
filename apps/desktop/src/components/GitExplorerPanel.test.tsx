import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Commit, Lane, Repo } from "../bindings";
import App from "../App";
import type { ConnectionSnapshot, ConnectionSource } from "../ipc/connection";
import type { FleetStore } from "../stores/fleet";
import GitExplorerPanel, { parseCommits } from "./GitExplorerPanel";

function sourceFor(snapshot: ConnectionSnapshot): ConnectionSource {
  return {
    current: async () => snapshot,
    subscribe: async () => () => undefined,
  };
}

const calls = vi.hoisted(() => ({ list: [] as Array<{ method: string; params: unknown }> }));
const responses = vi.hoisted(() => ({
  diff: null as unknown,
  diffError: null as string | null,
  history: [] as unknown[],
  historyError: null as string | null,
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params?: unknown) => {
    calls.list.push({ method, params });
    if (method === "lane.diff") {
      return responses.diffError ? Promise.reject(new Error(responses.diffError)) : Promise.resolve(responses.diff);
    }
    if (method === "commit.recent") {
      return responses.historyError ? Promise.reject(new Error(responses.historyError)) : Promise.resolve(responses.history);
    }
    return Promise.resolve({});
  },
  subscribeDaemon: () => Promise.resolve(() => undefined),
}));

afterEach(() => {
  cleanup();
  calls.list = [];
  responses.diff = null;
  responses.diffError = null;
  responses.history = [];
  responses.historyError = null;
  localStorage.clear();
});

function repo(): Repo {
  return { id: 2, path: "/code/repomon", name: "repomon", added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false };
}

function lane(overrides: Partial<Lane> = {}): Lane {
  return {
    id: 7,
    repo: repo(),
    worktree: { id: 3, repo_id: 2, path: "/code/repomon-wt/desktop", branch: "feat/desktop", head: "abc1234", is_main: false, name: "desktop" },
    state: {
      worktree_id: 3,
      head: "abc1234",
      branch: "feat/desktop",
      upstream: null,
      ahead: 2,
      behind: 1,
      dirty: { staged: 1, unstaged: 0, untracked: 1 },
      last_commit_at: null,
      locked: false,
      prunable: false,
      last_change_at: null,
    },
    agent_sessions: [],
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
    ...overrides,
  };
}

function commit(overrides: Partial<Commit> = {}): Commit {
  return {
    oid: "0123456789abcdef0123456789abcdef01234567",
    repo_id: 2,
    author_name: "Ada Lovelace",
    author_email: "ada@example.com",
    summary: "Add the analytical engine",
    time: "2026-08-16T11:00:00Z",
    parent_count: 1,
    ...overrides,
  };
}

function fleetWith(current: Lane | null): FleetStore {
  return { selectedLane: () => current } as unknown as FleetStore;
}

describe("parseCommits", () => {
  it("splits raw `git log --oneline` text into oid/summary rows", () => {
    expect(parseCommits("abc1234 Fix the thing\ndef5678 Add the other thing\n")).toEqual([
      { oid: "abc1234", summary: "Fix the thing" },
      { oid: "def5678", summary: "Add the other thing" },
    ]);
  });

  it("ignores blank lines and copes with a bare oid (no message)", () => {
    expect(parseCommits("abc1234 \n\ndef5678\n")).toEqual([
      { oid: "abc1234", summary: "" },
      { oid: "def5678", summary: "" },
    ]);
  });

  it("returns an empty list for empty input", () => {
    expect(parseCommits("")).toEqual([]);
  });
});

describe("GitExplorerPanel", () => {
  it("shows a designed empty state and fetches nothing when no lane is selected", () => {
    render(() => <GitExplorerPanel fleet={fleetWith(null)} />);

    expect(screen.getByText("No lane selected")).toBeInTheDocument();
    expect(screen.getByText(/Select a lane in the fleet/)).toBeInTheDocument();
    expect(calls.list).toHaveLength(0);
  });

  it("renders the header's branch name plus ahead/behind/dirty badges from lane.state", async () => {
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 0 };
    responses.history = [];
    render(() => <GitExplorerPanel fleet={fleetWith(lane())} />);

    expect(await screen.findByText("feat/desktop")).toBeInTheDocument();
    expect(screen.getByTitle("Git tracking: 2 ahead, 1 behind upstream")).toBeInTheDocument();
    expect(screen.getByTitle("2 uncommitted files (1 staged, 0 unstaged, 1 untracked)")).toBeInTheDocument();
  });

  it("renders the branch section from lane.diff, including the truncation note", async () => {
    responses.diff = {
      base: "main",
      merge_base: "abc0000",
      commits: "abc1234 Fix the thing\ndef5678 Add the other thing\n",
      commits_truncated: true,
      committed_stat: " 2 files changed, 10 insertions(+), 1 deletion(-)",
      uncommitted_stat: "",
      untracked: 0,
    };
    responses.history = [];
    render(() => <GitExplorerPanel fleet={fleetWith(lane())} />);

    expect(await screen.findByText("Fix the thing")).toBeInTheDocument();
    expect(screen.getByText("Add the other thing")).toBeInTheDocument();
    expect(screen.getByText("abc1234")).toBeInTheDocument();
    expect(screen.getByText(/2 commits ahead of/)).toBeInTheDocument();
    expect(screen.getByText("main", { selector: "span.font-mono" })).toBeInTheDocument();
    expect(screen.getByText("Showing the most recent 20 commits.")).toBeInTheDocument();
    expect(screen.getByText("2 files changed, 10 insertions(+), 1 deletion(-)")).toBeInTheDocument();
  });

  it("shows a 'nothing ahead' empty state when there are no commits ahead of base", async () => {
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 0 };
    responses.history = [];
    render(() => <GitExplorerPanel fleet={fleetWith(lane())} />);

    expect(await screen.findByText(/Nothing ahead of/)).toBeInTheDocument();
    expect(screen.getByText("main", { selector: "span.font-mono" })).toBeInTheDocument();
  });

  it("renders commit.recent as the History section: short oid, summary, relative time, author", async () => {
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 0 };
    responses.history = [
      commit({ oid: "0123456789abcdef0123456789abcdef01234567", summary: "Add the analytical engine", author_name: "Ada Lovelace", time: new Date(Date.now() - 5 * 60_000).toISOString() }),
    ];
    render(() => <GitExplorerPanel fleet={fleetWith(lane())} />);

    expect(await screen.findByText("Add the analytical engine")).toBeInTheDocument();
    expect(screen.getByText("0123456")).toBeInTheDocument();
    expect(screen.getByText("Ada Lovelace")).toBeInTheDocument();
    expect(screen.getByText("5m ago")).toBeInTheDocument();
  });

  it("shows an empty state for History when the lane has no recorded commits", async () => {
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 0 };
    responses.history = [];
    render(() => <GitExplorerPanel fleet={fleetWith(lane())} />);

    expect(await screen.findByText("No commits recorded for this lane yet.")).toBeInTheDocument();
  });

  it("shows a friendly error with a retry that re-fetches", async () => {
    responses.diffError = "no such file or directory (os error 2)";
    render(() => <GitExplorerPanel fleet={fleetWith(lane())} />);

    expect(await screen.findByText("Couldn't load git status")).toBeInTheDocument();
    expect(screen.getByText(/git isn't installed/)).toBeInTheDocument();
    const callsBeforeRetry = calls.list.length;

    responses.diffError = null;
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 0 };
    fireEvent.click(screen.getByText("Retry"));

    await waitFor(() => expect(calls.list.length).toBeGreaterThan(callsBeforeRetry));
    await waitFor(() => expect(screen.queryByText("Couldn't load git status")).not.toBeInTheDocument());
  });

  it("refetches when the selected lane changes, in place", async () => {
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 0 };
    responses.history = [];
    const [current, setCurrent] = createSignal<Lane>(lane({ id: 7 }));
    const fleet = { selectedLane: current } as unknown as FleetStore;
    render(() => <GitExplorerPanel fleet={fleet} />);

    await screen.findByText("feat/desktop");
    const diffCallsForFirstLane = calls.list.filter((c) => c.method === "lane.diff").length;
    expect(diffCallsForFirstLane).toBeGreaterThan(0);

    setCurrent(
      lane({
        id: 8,
        worktree: { id: 4, repo_id: 2, path: "/code/repomon-wt/other", branch: "chore/other", head: "def", is_main: false, name: "other" },
      }),
    );

    expect(await screen.findByText("chore/other")).toBeInTheDocument();
    expect(calls.list.filter((c) => c.method === "lane.diff").length).toBeGreaterThan(diffCallsForFirstLane);
  });
});

describe("panel.git keybinding (App integration)", () => {
  beforeEach(() => {
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
  });

  afterEach(() => {
    localStorage.clear();
    document.documentElement.style.removeProperty("--right-panel-width");
  });

  function renderApp() {
    return render(() => (
      <App
        connectionSource={sourceFor({
          phase: "starting",
          endpoint: "Resolving local daemon endpoint",
          message: null,
          daemon: null,
        })}
      />
    ));
  }

  it("opens the right panel already on the Git tab when closed", async () => {
    const { container } = renderApp();
    const toggle = within(container).getByRole("button", { name: "Repomind" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");

    fireEvent.keyDown(window, { key: "3", code: "Digit3", metaKey: true });

    await waitFor(() => expect(toggle).toHaveAttribute("aria-pressed", "true"));
    const tablist = within(container).getByRole("tablist", { name: "Right panel" });
    expect(within(tablist).getByRole("tab", { name: "Git" })).toHaveAttribute("aria-selected", "true");
    expect(within(tablist).getByRole("tab", { name: "Repomind" })).toHaveAttribute("aria-selected", "false");
  });

  it("closes the panel when mod+3 is pressed again while already on Git", async () => {
    const { container } = renderApp();
    const toggle = within(container).getByRole("button", { name: "Repomind" });

    fireEvent.keyDown(window, { key: "3", code: "Digit3", metaKey: true });
    await waitFor(() => expect(toggle).toHaveAttribute("aria-pressed", "true"));

    fireEvent.keyDown(window, { key: "3", code: "Digit3", metaKey: true });
    await waitFor(() => expect(toggle).toHaveAttribute("aria-pressed", "false"));
  });

  it("switches to the Git tab without closing when the panel is open on another tab", async () => {
    const { container } = renderApp();
    const toggle = within(container).getByRole("button", { name: "Repomind" });

    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toHaveAttribute("aria-pressed", "true"));
    const tablist = within(container).getByRole("tablist", { name: "Right panel" });
    expect(within(tablist).getByRole("tab", { name: "Repomind" })).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(window, { key: "3", code: "Digit3", metaKey: true });

    await waitFor(() => expect(within(tablist).getByRole("tab", { name: "Git" })).toHaveAttribute("aria-selected", "true"));
    expect(within(tablist).getByRole("tab", { name: "Repomind" })).toHaveAttribute("aria-selected", "false");
    expect(toggle).toHaveAttribute("aria-pressed", "true");
  });
});
