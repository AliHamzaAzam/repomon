import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Lane, Repo } from "../bindings";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import FleetSidebar from "./FleetSidebar";

afterEach(() => {
  cleanup();
});

function repo(id: number, name: string, hidden = false): Repo {
  return { id, path: `/code/${name}`, name, added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden };
}

function lane(id: number, target: Repo): Lane {
  return {
    id,
    repo: target,
    worktree: { id, repo_id: target.id, path: `/code/${target.name}-wt`, branch: "main", head: "abc", is_main: true, name: "main" },
    state: { worktree_id: id, head: "abc", branch: "main", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: [],
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
  };
}

function stubs(repos: Repo[], lanes: Lane[]) {
  const setRepoHidden = vi.fn().mockResolvedValue(undefined);
  const visible = repos.filter((r) => !r.hidden);
  const fleet = {
    repos: () => repos,
    visibleRepos: () => visible,
    hiddenRepos: () => repos.filter((r) => r.hidden),
    lanes: () => lanes,
    visibleLanes: () => lanes.filter((l) => !l.repo.hidden),
    selectedLaneId: () => null,
    setSelectedLaneId: vi.fn(),
    query: () => "",
    setQuery: vi.fn(),
    urgentOnly: () => false,
    setUrgentOnly: vi.fn(),
    loading: () => false,
    counts: () => ({ urgent: 0, running: 0 }),
    focusedUsage: () => null,
  } as unknown as FleetStore;
  const actions = { setRepoHidden, removeRepo: vi.fn(), newLane: vi.fn(), addRepo: vi.fn() } as unknown as ActionsStore;
  return { fleet, actions, setRepoHidden };
}

describe("fleet sidebar hiding", () => {
  it("hides a project from its header button", () => {
    const alpha = repo(1, "alpha");
    const { fleet, actions, setRepoHidden } = stubs([alpha], [lane(10, alpha)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    fireEvent.click(screen.getByLabelText("Hide alpha"));
    expect(setRepoHidden).toHaveBeenCalledWith(alpha, true);
  });

  it("offers hidden projects a way back", () => {
    const alpha = repo(1, "alpha");
    const beta = repo(2, "beta", true);
    const { fleet, actions, setRepoHidden } = stubs([alpha, beta], [lane(10, alpha), lane(20, beta)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    // The hidden repo is out of the main list but reachable from the reveal section, which is the
    // only route back: the TUI honors the flag and has no unhide view of its own.
    expect(screen.getByText("Hidden (1)")).toBeInTheDocument();
    fireEvent.click(screen.getByTitle("Show beta again"));
    expect(setRepoHidden).toHaveBeenCalledWith(beta, false);
  });

  it("does not claim there are no repositories when they are merely hidden", () => {
    const beta = repo(2, "beta", true);
    const { fleet, actions } = stubs([beta], [lane(20, beta)]);
    render(() => <FleetSidebar fleet={fleet} actions={actions} />);

    expect(screen.queryByText("No repositories yet.")).not.toBeInTheDocument();
    expect(screen.getByText(/Every project is hidden/)).toBeInTheDocument();
  });
});
