import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createSignal } from "solid-js";
import type { AccountUsage, AgentSession, Lane, Repo } from "../bindings";
import type { ActionsStore } from "../stores/actions";
import { controllerSummary, isControllerRepo, type FleetStore } from "../stores/fleet";
import type { RepomindStore } from "../stores/repomind";
import FleetSidebar, {
  FILTER_ROW_COMPACT_THRESHOLD_PX,
  isFilterRowCompact,
  repoDisplayName,
} from "./FleetSidebar";
import { reorderAround } from "./ordering";

vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn().mockResolvedValue({ plugins: [] }),
  subscribeDaemon: vi.fn().mockResolvedValue(() => undefined),
}));

afterEach(() => {
  cleanup();
});

function repo(id: number, name: string, hidden = false): Repo {
  return { id, path: `/code/${name}`, name, added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden, position: null, label: null };
}

function session(overrides: Partial<AgentSession> = {}): AgentSession {
  return {
    id: 9,
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
    ...overrides,
  };
}

function lane(id: number, target: Repo, sessions: AgentSession[] = [], role: string | null = null): Lane {
  return {
    id,
    repo: target,
    worktree: { id, repo_id: target.id, path: `/code/${target.name}-wt`, branch: "main", head: "abc", is_main: true, name: "main" },
    state: { worktree_id: id, head: "abc", branch: "main", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: sessions,
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
    role,
  };
}

function stubs(repos: Repo[], lanes: Lane[], sortMode = "default") {
  const setRepoHidden = vi.fn().mockResolvedValue(undefined);
  const renameRepo = vi.fn().mockResolvedValue(undefined);
  const reorderRepos = vi.fn().mockResolvedValue(undefined);
  const visible = repos.filter((r) => !r.hidden && !isControllerRepo(r.id, lanes));
  const fleet = {
    repos: () => repos,
    visibleRepos: () => visible,
    hiddenRepos: () => repos.filter((r) => r.hidden),
    lanes: () => lanes,
    visibleLanes: () => lanes.filter((l) => !l.repo.hidden && l.role !== "controller"),
    controllerLanes: () => lanes.filter((l) => l.role === "controller"),
    controller: () => controllerSummary(lanes),
    selectedLaneId: () => null,
    setSelectedLaneId: vi.fn(),
    query: () => "",
    setQuery: vi.fn(),
    urgentOnly: () => false,
    setUrgentOnly: vi.fn(),
    runningOnly: () => false,
    setRunningOnly: vi.fn(),
    loading: () => false,
    counts: () => ({ urgent: 0, running: 0, idle: 0 }),
    focusedUsage: () => null,
    sortMode: () => sortMode,
    refresh: vi.fn().mockResolvedValue(undefined),
    refreshUsage: vi.fn().mockResolvedValue(undefined),
  } as unknown as FleetStore;
  const deleteLane = vi.fn();
  const pinLane = vi.fn().mockResolvedValue(undefined);
  const actions = {
    setRepoHidden,
    renameRepo,
    reorderRepos,
    removeRepo: vi.fn(),
    newLane: vi.fn(),
    addRepo: vi.fn(),
    openRepoNotes: vi.fn(),
    deleteLane,
    pinLane,
    startRepomind: vi.fn().mockResolvedValue(undefined),
    stopRepomind: vi.fn().mockResolvedValue(undefined),
  } as unknown as ActionsStore;
  return { fleet, actions, setRepoHidden, renameRepo, reorderRepos, deleteLane, pinLane };
}

describe("fleet sidebar hiding", () => {
  it("hides a project from its header button", () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions, setRepoHidden } = stubs([alpha], [lane(10, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    fireEvent.click(screen.getByLabelText("Hide alpha"));
    expect(setRepoHidden).toHaveBeenCalledWith(alpha, true);
  });

  it("offers hidden projects a way back via expandable disclosure section", () => {
    const alpha = repo(1, "alpha");
    const beta = repo(2, "beta", true);
    const { fleet, actions, setRepoHidden } = stubs([alpha, beta], [lane(10, alpha), lane(20, beta)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    // The hidden repo section is collapsed by default showing the summary label
    expect(screen.getByText("Hidden (1)")).toBeInTheDocument();
    expect(screen.queryByTitle("Show beta again")).not.toBeInTheDocument();

    // Click to expand hidden section
    fireEvent.click(screen.getByRole("button", { name: /Hidden \(1\)/i }));
    expect(screen.getByTitle("Show beta again")).toBeInTheDocument();

    // Click unhide button
    fireEvent.click(screen.getByTitle("Show beta again"));
    expect(setRepoHidden).toHaveBeenCalledWith(beta, false);

    // Click to re-collapse hidden section
    fireEvent.click(screen.getByRole("button", { name: /Hidden \(1\)/i }));
    expect(screen.queryByTitle("Show beta again")).not.toBeInTheDocument();
  });

  it("does not claim there are no repositories when they are merely hidden", () => {
    const beta = repo(2, "beta", true);
    const { fleet, actions } = stubs([beta], [lane(20, beta)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.queryByText("No repositories yet.")).not.toBeInTheDocument();
    expect(screen.getByText(/Every project is hidden/)).toBeInTheDocument();
  });

  it("renders consistent lane row anatomy and telemetry", () => {
    const alpha = repo(1, "alpha");
    const testLane: Lane = {
      ...lane(10, alpha),
      state: {
        ...lane(10, alpha).state,
        ahead: 2,
        behind: 1,
        dirty: { staged: 1, unstaged: 2, untracked: 0 },
      },
      agent_sessions: [
        session({
          worktree_id: 10,
          tmux_window: "lane-1",
          custom_label: "Alpha Worker",
        }),
      ],
    };
    const { fleet, actions } = stubs([alpha], [testLane]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.getByText("Alpha Worker")).toBeInTheDocument();
    expect(screen.getByText("main")).toBeInTheDocument();
    expect(screen.getByTitle(/3 uncommitted files/)).toBeInTheDocument();
    expect(screen.getByTitle(/2 ahead, 1 behind upstream/)).toBeInTheDocument();
  });

  it("splits the lane row into a name-plus-pill line and a branch-plus-counts line", () => {
    // The row grammar is two lines: line 1 carries the name and the status pill, line 2 carries
    // the branch and the counts. A crowded single line was exactly the bug ("Upw... | ma... | 5 |
    // SUBAGENT RUNNING | 69") - the name and branch must never share a line with the pill again.
    const alpha = repo(1, "alpha");
    const testLane = lane(10, alpha, [session({ worktree_id: 10 })]);
    const { fleet, actions } = stubs([alpha], [testLane]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    const name = screen.getByText("main", { selector: "span.truncate" });
    const pill = screen.getByText("running");
    const branch = screen.getByText("main", { selector: "span.truncate-tail" });

    // The name and the pill share one line...
    expect(pill.parentElement).toBe(name.parentElement);
    // ...and the branch lives on a different line from both of them.
    expect(branch.parentElement).not.toBe(name.parentElement);
    // The pill never truncates - it is a fixed short word, not flexible text.
    expect(pill.className).not.toMatch(/\btruncate\b/);
    expect(pill.className).not.toMatch(/\btruncate-tail\b/);
    // The branch is the tail-preserving variant (its identifying suffix stays visible), never the
    // plain end-truncating one the name uses as its own last resort.
    const branchClasses = branch.className.split(/\s+/);
    expect(branchClasses).toContain("truncate-tail");
    expect(branchClasses).not.toContain("truncate");
  });

  it("shows the status pill in one short word, and only for a lane with agents", () => {
    const alpha = repo(1, "alpha");
    const idling = lane(10, alpha, [session({ worktree_id: 10, status: "idle" })]);
    const empty = lane(11, alpha);
    const { fleet, actions } = stubs([alpha], [idling, empty]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    // A lane with an idle agent gets the "idle" pill...
    expect(screen.getByText("idle")).toBeInTheDocument();
    // ...but an empty lane (auto-collapsed, no agents at all) never claims to be idle.
    expect(screen.queryAllByText("idle")).toHaveLength(1);
  });

  it("names what the repo header count is counting", () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha), lane(11, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    // "REPOMON 9" said nothing about what nine was.
    expect(screen.getByText("2 lanes")).toBeInTheDocument();
    expect(screen.getByTitle("2 lanes in alpha")).toBeInTheDocument();
  });

  it("rolls a project's needs-you agents up to its header", () => {
    const alpha = repo(1, "alpha");
    const waiting = lane(10, alpha, [session({ status: "waiting", worktree_id: 10 })]);
    const { fleet, actions } = stubs([alpha], [waiting, lane(11, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.getByTitle("1 agent in this project need you")).toBeInTheDocument();
  });

  it("marks a lane whose branch is already in the default branch", () => {
    const alpha = repo(1, "alpha");
    const landed = lane(10, alpha, [session({ worktree_id: 10 })]);
    landed.worktree = { ...landed.worktree, is_main: false, name: "feat-x", branch: "feat/x" };
    landed.state = { ...landed.state, branch: "feat/x", merged: true };
    const { fleet, actions } = stubs([alpha], [landed]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.getByText("merged")).toBeInTheDocument();
    // The default branch is never marked: it is not merged into itself.
    expect(screen.getByTitle(/already in the default branch/)).toBeInTheDocument();
  });

  it("offers Remove worktree from a lane row, and only for a worktree lane", () => {
    const alpha = repo(1, "alpha");
    const wt = lane(10, alpha, [session({ worktree_id: 10 })]);
    wt.worktree = { ...wt.worktree, is_main: false, name: "feat-x" };
    const { fleet, actions, deleteLane } = stubs([alpha], [wt]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    fireEvent.contextMenu(screen.getByText("feat-x"));
    fireEvent.click(screen.getByText("Remove worktree"));
    // The action itself raises the shared confirm; the row never deletes on one click.
    expect(deleteLane).toHaveBeenCalledWith(wt);
  });

  it("keeps the main lane out of the destructive menu item", () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha, [session({ worktree_id: 10 })])]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    fireEvent.contextMenu(screen.getAllByText("main")[0]);
    expect(screen.queryByText("Remove worktree")).not.toBeInTheDocument();
    expect(screen.getByText("Pin lane to top")).toBeInTheDocument();
  });

  it("offers both fleet filters as pressable toggles", () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    for (const label of ["Needs you", "Running"]) {
      const chip = screen.getByRole("button", { name: new RegExp(label) });
      expect(chip.getAttribute("aria-pressed")).toBe("false");
    }
    fireEvent.click(screen.getByRole("button", { name: /Running/ }));
    expect(fleet.setRunningOnly).toHaveBeenCalledWith(true);
  });

  it("never truncates a filter chip label, at any width", () => {
    // jsdom cannot lay out, so the width-driven compact switch is exercised directly on the pure
    // decision function (below), and this render only guards the other half of the same bug: the
    // label span must never carry a `truncate` class, since a short label plus that class was
    // exactly how "Needs attention" turned into "Need... 0".
    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    const needsYou = screen.getByText("Needs you");
    const running = screen.getByText("Running");
    expect(needsYou.className).not.toMatch(/\btruncate\b/);
    expect(running.className).not.toMatch(/\btruncate\b/);
  });

  it("switches the filter chips to icon-only below the measured compact threshold", () => {
    // Unmeasured (0, jsdom never fires the row's ResizeObserver) reads as spacious so the chips
    // never flash icon-only before layout settles.
    expect(isFilterRowCompact(0)).toBe(false);
    // Comfortably below the threshold, matching the narrow-window sidebar column (13.5rem).
    expect(isFilterRowCompact(FILTER_ROW_COMPACT_THRESHOLD_PX - 1)).toBe(true);
    // At and above the threshold, matching the default sidebar column (18rem), labels stay put.
    expect(isFilterRowCompact(FILTER_ROW_COMPACT_THRESHOLD_PX)).toBe(false);
    expect(isFilterRowCompact(420)).toBe(false);
  });

  it("renders structured usage rate limits card with clear labels", () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha)]);
    (fleet as any).focusedUsage = () => ({
      label: "claude-3-5-sonnet",
      age_secs: 15,
      report: {
        windows: [
          { label: "5h", pct_used: 12 },
          { label: "wk", pct_used: 85 },
        ],
      },
    });

    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.getByText(/Rate Limits/)).toBeInTheDocument();
    expect(screen.getByText("5-Hour Quota")).toBeInTheDocument();
    expect(screen.getByText("12%")).toBeInTheDocument();
    expect(screen.getByText("Weekly Quota")).toBeInTheDocument();
    expect(screen.getByText("85%")).toBeInTheDocument();
  });

  it("renders codex usage with monthly quota correctly", () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha)]);
    (fleet as any).focusedUsage = () => ({
      label: "codex",
      age_secs: 30,
      report: {
        windows: [
          { label: "5h", pct_used: 20 },
          { label: "mo", pct_used: 65 },
        ],
      },
    });

    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.getByText("Rate Limits (codex)")).toBeInTheDocument();
    expect(screen.getByText("5-Hour Quota")).toBeInTheDocument();
    expect(screen.getByText("20%")).toBeInTheDocument();
    expect(screen.getByText("Monthly Quota")).toBeInTheDocument();
    expect(screen.getByText("65%")).toBeInTheDocument();
  });

  it("renders antigravity usage with model groups correctly", () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha)]);
    (fleet as any).focusedUsage = () => ({
      label: "antigravity",
      age_secs: 10,
      report: {
        windows: [
          { label: "5h", pct_used: 8 },
          { label: "wk", pct_used: 29 },
          { label: "claude-5h", pct_used: 0 },
          { label: "claude-wk", pct_used: 0 },
        ],
      },
    });

    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.getByText("Rate Limits (antigravity)")).toBeInTheDocument();
    expect(screen.getByText("5-Hour Quota")).toBeInTheDocument();
    expect(screen.getByText("8%")).toBeInTheDocument();
    expect(screen.getByText("Weekly Quota")).toBeInTheDocument();
    expect(screen.getByText("29%")).toBeInTheDocument();
    expect(screen.getByText("Claude 5h Quota")).toBeInTheDocument();
    expect(screen.getByText("Claude Weekly Quota")).toBeInTheDocument();
  });

  it("auto-collapses inactive lane row by default and allows expanding/minimizing", () => {
    const alpha = repo(1, "alpha");
    const emptyLane = lane(10, alpha);
    const { fleet, actions } = stubs([alpha], [emptyLane]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.getAllByText("main")[0]).toBeInTheDocument();
    // Default is auto-collapsed for empty lane
    const expandBtn = screen.getByLabelText("Expand lane main");
    expect(expandBtn).toBeInTheDocument();
    expect(screen.queryByText("idle")).not.toBeInTheDocument();

    // Click expand
    fireEvent.click(expandBtn);
    const minimizeBtn = screen.getByLabelText("Minimize inactive lane main");
    expect(minimizeBtn).toBeInTheDocument();

    // Click minimize to collapse again
    fireEvent.click(minimizeBtn);
    expect(screen.getByLabelText("Expand lane main")).toBeInTheDocument();
  });

  it("never collapses a lane that has active agent sessions", () => {
    const alpha = repo(1, "alpha");
    const activeLane = lane(10, alpha, [
      session({ status: "running", agent: "claude-code", session_id: "s1" }),
    ]);
    const { fleet, actions } = stubs([alpha], [activeLane]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.queryByLabelText("Expand lane main")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Minimize inactive lane main")).not.toBeInTheDocument();
    expect(screen.getByText("running")).toBeInTheDocument();
  });

  it("shows a reset-time tooltip only for windows that carry reset_at (E9)", () => {
    // Pin "now" to a fixed local instant so the 5h window's reset_at (a few hours later, same
    // local day) resolves to the time-only same-day form deterministically, regardless of the
    // machine's timezone or the date this test happens to run on.
    vi.useFakeTimers();
    const now = new Date(2026, 7, 18, 12, 0, 0); // Aug 18, 2026, 12:00 local
    vi.setSystemTime(now);
    const sameDayReset = new Date(2026, 7, 18, 18, 30, 0).toISOString();

    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha)]);
    (fleet as any).focusedUsage = () => ({
      label: "claude-3-5-sonnet",
      age_secs: 15,
      report: {
        windows: [
          { label: "5h", pct_used: 12, reset_at: sameDayReset },
          { label: "wk", pct_used: 85, reset_at: null },
          { label: "mo", pct_used: 40, reset_at: "not-a-real-date" },
        ],
      },
    });

    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    // Window with a valid, same-day reset_at gets a time-only tooltip clause.
    expect(screen.getByText("12%").parentElement?.getAttribute("title")).toMatch(/resets at/);
    // Window with no reset_at omits the reset clause entirely rather than showing nothing useful.
    const weeklyTitle = screen.getByText("85%").parentElement?.getAttribute("title");
    expect(weeklyTitle).not.toMatch(/resets at/);
    expect(weeklyTitle).toMatch(/85% used$/);
    // Window with an unparsable reset_at is treated the same as absent (formatResetAt returns null).
    const monthlyTitle = screen.getByText("40%").parentElement?.getAttribute("title");
    expect(monthlyTitle).not.toMatch(/resets at/);

    vi.useRealTimers();
  });

  it("wires the Rate Limits refresh button to fleet.refresh with a spinning affordance", async () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions } = stubs([alpha], [lane(10, alpha)]);
    let resolveRefresh: () => void = () => {};
    const pending = new Promise<void>((resolve) => {
      resolveRefresh = resolve;
    });
    (fleet as any).refreshUsage = vi.fn().mockReturnValue(pending);
    (fleet as any).focusedUsage = () => ({
      label: "claude-3-5-sonnet",
      age_secs: 15,
      report: { windows: [{ label: "5h", pct_used: 12, reset_at: null }] },
    });

    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    const button = screen.getByLabelText("Refresh rate limit data");
    fireEvent.click(button);

    expect(fleet.refreshUsage).toHaveBeenCalledTimes(1);
    expect(button).toBeDisabled();
    expect(button.querySelector("svg")).toHaveClass("animate-spin");

    resolveRefresh();
    await pending;
    await Promise.resolve();

    expect(button).not.toBeDisabled();
  });
});

describe("repo display labels", () => {
  it("falls back to the folder name when no label is set", () => {
    expect(repoDisplayName({ name: "repomon", label: null })).toBe("repomon");
    // A whitespace-only label is treated as unset rather than blanking the row.
    expect(repoDisplayName({ name: "repomon", label: "   " })).toBe("repomon");
  });

  it("shows the custom label instead of the folder name", () => {
    const alpha = { ...repo(1, "alpha"), label: "Client Portal" };
    const { fleet, actions } = stubs([alpha], [lane(10, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.getByText("Client Portal")).toBeInTheDocument();
    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
    // The real identity stays reachable via the tooltip.
    expect(screen.getByTitle(/repository: alpha/i)).toBeInTheDocument();
  });
});

describe("repo rename from the context menu", () => {
  it("opens the rename modal and calls repo.rename with the entered label", async () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions, renameRepo } = stubs([alpha], [lane(10, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    fireEvent.contextMenu(screen.getByText("alpha"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Rename…" }));

    const input = await screen.findByPlaceholderText("alpha");
    fireEvent.input(input, { target: { value: "Client Portal" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await vi.waitFor(() => {
      expect(renameRepo).toHaveBeenCalledWith(alpha, "Client Portal");
    });
  });

  it("passes an empty label through so clearing falls back to the folder name", async () => {
    const alpha = { ...repo(1, "alpha"), label: "Client Portal" };
    const { fleet, actions, renameRepo } = stubs([alpha], [lane(10, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    fireEvent.contextMenu(screen.getByText("Client Portal"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Rename…" }));

    const input = await screen.findByPlaceholderText("alpha");
    fireEvent.input(input, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await vi.waitFor(() => {
      expect(renameRepo).toHaveBeenCalledWith(alpha, "");
    });
  });
});

describe("manual repo reordering", () => {
  it("reorderAround moves a repo before or after its drop target", () => {
    expect(reorderAround([1, 2, 3], 3, 1, false)).toEqual([3, 1, 2]);
    expect(reorderAround([1, 2, 3], 1, 3, true)).toEqual([2, 3, 1]);
    // Dropping onto itself is a no-op.
    expect(reorderAround([1, 2, 3], 2, 2, false)).toBeNull();
    // A target that is not in the list leaves the order untouched.
    expect(reorderAround([1, 2, 3], 1, 99, true)).toBeNull();
  });

  it("drops a dragged header on another repo and persists the new order", () => {
    const alpha = repo(1, "alpha");
    const beta = repo(2, "beta");
    const { fleet, actions, reorderRepos } = stubs([alpha, beta], [lane(10, alpha), lane(20, beta)], "manual");
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    const alphaHeader = screen.getByText("alpha").parentElement as HTMLElement;
    const betaHeader = screen.getByText("beta").parentElement as HTMLElement;
    // jsdom rects are zero-height, so the pointer counts as the upper half: insert before.
    fireEvent.dragStart(betaHeader);
    fireEvent.dragOver(alphaHeader);
    fireEvent.drop(alphaHeader);

    expect(reorderRepos).toHaveBeenCalledWith([2, 1]);
  });

  it("does not reorder outside manual mode", () => {
    const alpha = repo(1, "alpha");
    const beta = repo(2, "beta");
    const { fleet, actions, reorderRepos } = stubs([alpha, beta], [lane(10, alpha), lane(20, beta)], "activity");
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    const alphaHeader = screen.getByText("alpha").parentElement as HTMLElement;
    const betaHeader = screen.getByText("beta").parentElement as HTMLElement;
    fireEvent.dragStart(alphaHeader);
    fireEvent.dragOver(betaHeader);
    fireEvent.drop(betaHeader);

    expect(reorderRepos).not.toHaveBeenCalled();
  });
});

describe("the pinned Repomind row", () => {
  const home = repo(9, "repomind");
  const project = repo(1, "alpha");

  function repomindStub(activePlans: number, homePath: string) {
    return {
      status: () => ({
        home: homePath,
        exists: true,
        repo_id: 9,
        lane_id: 90,
        window: "repomind-1",
        max_controllers: 2,
        export: { last_run: null, pending: false, last_error: null },
        counts: { active_plans: activePlans, standing: 0, playbooks: 0, drafts: 0 },
        boot: { generated_at: null, tokens_estimate: 0, trimmed: [] },
      }),
    } as unknown as RepomindStore;
  }

  it("states a live controller in the fleet's own one-word vocabulary", () => {
    const controller = lane(90, home, [session({ status: "waiting", tmux_window: "repomind-1" })], "controller");
    const { fleet, actions } = stubs([project, home], [lane(10, project), controller]);
    render(() => (
      <FleetSidebar fleet={fleet} actions={actions} repomind={repomindStub(3, "/Users/pat/repomind")} />
    ));

    const row = screen.getByRole("button", { name: /Repomind/ });
    expect(row.textContent).toContain("Repomind");
    expect(row.textContent).toContain("needs you");
    expect(row.textContent).toContain("3 goals");
    expect(row.textContent).toContain("/Users/pat/repomind");
    // The home never also appears as a repo group.
    expect(screen.queryByLabelText("repomind")).toBeNull();
  });

  it("reads off when the home has no controller running", () => {
    const { fleet, actions } = stubs([project, home], [lane(10, project), lane(90, home, [], "controller")]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} repomind={repomindStub(0, "/Users/pat/repomind")} />);

    const row = screen.getByRole("button", { name: /Repomind/ });
    expect(row.textContent).toContain("off");
    expect(row.textContent).toContain("0 goals");
  });

  it("is absent entirely when no controller lane exists", () => {
    const { fleet, actions } = stubs([project], [lane(10, project)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);
    expect(screen.queryByRole("button", { name: /^Repomind/ })).toBeNull();
  });

  it("selects the controller lane when clicked, like any lane row", () => {
    const controller = lane(90, home, [session({ tmux_window: "repomind-1" })], "controller");
    const { fleet, actions } = stubs([project, home], [lane(10, project), controller]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} repomind={repomindStub(1, "/home")} />);

    fireEvent.click(screen.getByRole("button", { name: /Repomind/ }));
    expect(fleet.setSelectedLaneId).toHaveBeenCalledWith(90);
  });

  it("offers start, the panel, and the home in its context menu when off", () => {
    const onOpenRepomindPanel = vi.fn();
    const onOpenEditor = vi.fn();
    const { fleet, actions } = stubs([project, home], [lane(10, project), lane(90, home, [], "controller")]);
    render(() => (
      <FleetSidebar
        fleet={fleet}
        actions={actions}
        repomind={repomindStub(0, "/home")}
        onOpenRepomindPanel={onOpenRepomindPanel}
        onOpenEditor={onOpenEditor}
      />
    ));

    fireEvent.contextMenu(screen.getByRole("button", { name: /Repomind/ }));
    expect(screen.queryByText("Stop Repomind")).toBeNull();

    fireEvent.click(screen.getByText("Start Repomind"));
    expect(actions.startRepomind).toHaveBeenCalled();

    fireEvent.contextMenu(screen.getByRole("button", { name: /Repomind/ }));
    fireEvent.click(screen.getByText("Open panel"));
    expect(onOpenRepomindPanel).toHaveBeenCalled();

    fireEvent.contextMenu(screen.getByRole("button", { name: /Repomind/ }));
    fireEvent.click(screen.getByText("Open home in editor"));
    // Opening the home in the editor selects its lane first, so the editor targets the home.
    expect(fleet.setSelectedLaneId).toHaveBeenCalledWith(90);
    expect(onOpenEditor).toHaveBeenCalled();
  });

  it("offers stop instead of start once a controller is running", () => {
    const controller = lane(90, home, [session({ tmux_window: "repomind-1" })], "controller");
    const { fleet, actions } = stubs([project, home], [lane(10, project), controller]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} repomind={repomindStub(0, "/home")} />);

    fireEvent.contextMenu(screen.getByRole("button", { name: /Repomind/ }));
    expect(screen.queryByText("Start Repomind")).toBeNull();
    fireEvent.click(screen.getByText("Stop Repomind"));
    expect(actions.stopRepomind).toHaveBeenCalled();
  });

  it("puts the row ahead of every repo group in the document order", () => {
    const controller = lane(90, home, [session({ tmux_window: "repomind-1" })], "controller");
    const { fleet, actions } = stubs([project, home], [lane(10, project), controller]);
    const { container } = render(() => (
      <FleetSidebar fleet={fleet} actions={actions} repomind={repomindStub(0, "/home")} />
    ));

    const row = screen.getByRole("button", { name: /Repomind/ });
    const group = container.querySelector('section[aria-label="alpha"]');
    expect(group).not.toBeNull();
    // The row is the sidebar's first stop; `fleet.moveSelection` walks the same order.
    expect(row.compareDocumentPosition(group as Node) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});


describe("manual usage outcomes", () => {
  it("shows a short inline notice when the probe is disabled", async () => {
    const { fleet, actions } = stubs([], []);
    const [usage, setUsage] = createSignal<AccountUsage | null>({ key: "default", label: "main", age_secs: 90, report: { windows: [] } });
    fleet.focusedUsage = usage;
    fleet.refreshUsage = vi.fn().mockImplementation(async () => {
      setUsage(null);
      return { refreshed: false, reason: "probe_disabled", detail: null, snapshot: [] };
    });
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);
    fireEvent.click(screen.getByLabelText("Refresh rate limit data"));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Usage probe is off in Settings"));
    expect(screen.getByLabelText("Refresh rate limit data")).not.toBeDisabled();
  });
});


describe("sidebar cost visibility", () => {
  it("defaults on, hides the Today row, and persists across a remount", () => {
    localStorage.removeItem("repomon:sidebar-show-today-cost");
    const { fleet, actions } = stubs([], []);
    fleet.focusedUsage = () => ({ key: "default", label: "main", age_secs: 0, report: { windows: [] } });
    fleet.costToday = () => 12;
    const first = render(() => <FleetSidebar fleet={fleet} actions={actions} />);
    expect(screen.getByText("Today")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Hide today's cost in the sidebar"));
    expect(screen.queryByText("Today")).not.toBeInTheDocument();
    first.unmount();
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);
    expect(screen.queryByText("Today")).not.toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Show today's cost in the sidebar"));
    expect(screen.getByText("Today")).toBeInTheDocument();
    localStorage.removeItem("repomon:sidebar-show-today-cost");
  });
});


describe("long probe feedback", () => {
  it.each([
    ["timeout", "Still probing, this can take a moment"],
    ["error", "Usage probe failed; try again"],
  ])("uses accurate copy for a %s event", async (reason, notice) => {
    const { fleet, actions } = stubs([], []);
    fleet.focusedUsage = () => ({ key: "default", label: "main", age_secs: 0, report: { windows: [] } });
    fleet.refreshUsage = vi.fn().mockResolvedValue({ refreshed: false, reason, detail: null, snapshot: [] });
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);
    fireEvent.click(screen.getByLabelText("Refresh rate limit data"));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent(notice));
    expect(screen.queryByText("Probe timed out")).not.toBeInTheDocument();
  });
});
