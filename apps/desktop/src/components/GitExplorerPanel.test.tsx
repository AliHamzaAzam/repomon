import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Commit, Lane, Repo } from "../bindings";
import App from "../App";
import type { ConnectionSnapshot, ConnectionSource } from "../ipc/connection";
import type { FleetStore } from "../stores/fleet";
import GitExplorerPanel, { parseCommits, parseStatFiles } from "./GitExplorerPanel";

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

// Sample lines are real `git diff --stat` output (captured from a scratch repo), not hand-typed
// guesses — git pads paths to the longest one in the block and uses a literal " | " separator.
describe("parseStatFiles", () => {
  const stat = [
    " src/app.tsx             | 5 +++--",
    " bin.dat                 | Bin 7 -> 15 bytes",
    " file.txt => notes.txt   | 1 +",
    " src/{old => new}/mod.rs | 0",
    " 4 files changed, 6 insertions(+), 2 deletions(-)",
  ].join("\n");

  it("parses a modified file's path and +/- bar counts", () => {
    expect(parseStatFiles(stat)[0]).toEqual({ path: "src/app.tsx", adds: 3, dels: 2, binary: false });
  });

  it("marks a `Bin ... bytes` line as binary with no adds/dels", () => {
    expect(parseStatFiles(stat)[1]).toEqual({ path: "bin.dat", adds: 0, dels: 0, binary: true });
  });

  it("parses a plain rename line (`old => new`) into path + renamedFrom", () => {
    expect(parseStatFiles(stat)[2]).toEqual({
      path: "notes.txt",
      renamedFrom: "file.txt",
      adds: 1,
      dels: 0,
      binary: false,
    });
  });

  it("expands a brace rename (`prefix/{old => new}/suffix`) to full paths", () => {
    expect(parseStatFiles(stat)[3]).toEqual({
      path: "src/new/mod.rs",
      renamedFrom: "src/old/mod.rs",
      adds: 0,
      dels: 0,
      binary: false,
    });
  });

  it("drops the trailing summary line (it has no ` | ` column and would misparse as a path)", () => {
    expect(parseStatFiles(stat)).toHaveLength(4);
  });

  it("returns an empty list for empty input", () => {
    expect(parseStatFiles("")).toEqual([]);
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

// `dirty` drives the Working tree section directly (same field the header's dirty badge reads),
// with `ahead`/`behind` zeroed so their header badge never contributes a stray digit that could
// collide with a count asserted in these tests.
function dirtyLane(dirty: { staged: number; unstaged: number; untracked: number }, overrides: Partial<Lane> = {}): Lane {
  const base = lane();
  return lane({ state: { ...base.state, ahead: 0, behind: 0, dirty }, ...overrides });
}

describe("GitExplorerPanel Working tree section", () => {
  it("shows a quiet 'Working tree clean' row when nothing is staged, unstaged, or untracked", async () => {
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 0 };
    responses.history = [];
    render(() => <GitExplorerPanel fleet={fleetWith(dirtyLane({ staged: 0, unstaged: 0, untracked: 0 }))} />);

    expect(await screen.findByText("Working tree")).toBeInTheDocument();
    expect(await screen.findByText("Working tree clean")).toBeInTheDocument();
    expect(screen.queryByText("Changes")).not.toBeInTheDocument();
    expect(screen.queryByText("Untracked")).not.toBeInTheDocument();
  });

  it("renders one honest 'Changes' group (not a staged/unstaged split) plus per-file rows, and a count-only 'Untracked' group", async () => {
    // uncommitted_stat is `git diff HEAD --stat` — it already mixes staged and unstaged hunks
    // into one line per file, so there's no way to attribute a given row to one or the other.
    responses.diff = {
      base: "main",
      merge_base: "abc0000",
      commits: "",
      committed_stat: "",
      uncommitted_stat: " apps/desktop/src/App.tsx | 5 +++--\n new-thing.md             | 2 ++\n 2 files changed, 5 insertions(+), 2 deletions(-)\n",
      untracked: 1,
    };
    responses.history = [];
    // staged+unstaged (3) deliberately differs from staged+unstaged+untracked (4, the header's
    // dirty-badge total) so every asserted digit below is unambiguous.
    render(() => <GitExplorerPanel fleet={fleetWith(dirtyLane({ staged: 1, unstaged: 2, untracked: 1 }))} />);

    // Header's top dirty badge: total from DirtyState.
    expect(await screen.findByTitle("4 uncommitted files (1 staged, 2 unstaged, 1 untracked)")).toBeInTheDocument();

    // "Changes" group: header count is DirtyState's staged+unstaged (3), same source of truth as
    // the header badge above, described honestly as "1 staged · 2 unstaged" rather than split.
    expect(await screen.findByText("Changes")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(screen.getByText("1 staged · 2 unstaged")).toBeInTheDocument();

    // Per-file rows parsed from uncommitted_stat: dir muted, basename emphasized, +/- colored.
    expect(screen.getByText("apps/desktop/src/")).toBeInTheDocument();
    expect(screen.getByText("App.tsx")).toBeInTheDocument();
    expect(screen.getByText("+3")).toBeInTheDocument();
    expect(screen.getByText("-2")).toBeInTheDocument();
    expect(screen.getByText("new-thing.md")).toBeInTheDocument();
    expect(screen.getByText("+2")).toBeInTheDocument();

    // "Untracked": count-only (no filenames available from lane.diff or the live dirty walk).
    expect(screen.getByText("Untracked")).toBeInTheDocument();
    expect(screen.getByText("1")).toBeInTheDocument();
    expect(screen.getByText("1 file not tracked by git")).toBeInTheDocument();
    expect(screen.getByText("U")).toBeInTheDocument();
  });

  it("pluralizes and omits the Changes group entirely when only untracked files are dirty", async () => {
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 2 };
    responses.history = [];
    render(() => <GitExplorerPanel fleet={fleetWith(dirtyLane({ staged: 0, unstaged: 0, untracked: 2 }))} />);

    expect(await screen.findByText("Untracked")).toBeInTheDocument();
    expect(screen.getByText("2 files not tracked by git")).toBeInTheDocument();
    expect(screen.queryByText("Changes")).not.toBeInTheDocument();
  });

  it("shows an honest fallback when DirtyState says there are changes but uncommitted_stat has no file lines for them", async () => {
    // The two counts come from different code paths (gix's live status walk vs. a shelled-out
    // `git diff --stat`), so they can legitimately disagree for one in-flight snapshot.
    responses.diff = { base: "main", merge_base: "abc0000", commits: "", committed_stat: "", uncommitted_stat: "", untracked: 0 };
    responses.history = [];
    render(() => <GitExplorerPanel fleet={fleetWith(dirtyLane({ staged: 1, unstaged: 0, untracked: 0 }))} />);

    expect(await screen.findByText("Changes")).toBeInTheDocument();
    expect(screen.getByText("No per-file details available.")).toBeInTheDocument();
  });

  it("collapses and re-expands the Changes file list on click, leaving the group's counts visible", async () => {
    responses.diff = {
      base: "main",
      merge_base: "abc0000",
      commits: "",
      committed_stat: "",
      uncommitted_stat: " App.tsx | 1 +\n 1 file changed, 1 insertion(+)\n",
      untracked: 0,
    };
    responses.history = [];
    render(() => <GitExplorerPanel fleet={fleetWith(dirtyLane({ staged: 1, unstaged: 0, untracked: 0 }))} />);

    const toggle = await screen.findByRole("button", { name: /Changes/ });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("App.tsx")).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("App.tsx")).not.toBeInTheDocument();
    expect(screen.getByText("1 staged · 0 unstaged")).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("App.tsx")).toBeInTheDocument();
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
