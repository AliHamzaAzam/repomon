import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import FleetSidebar from "./FleetSidebar";

afterEach(() => {
  cleanup();
});

function repo(id: number, name: string, hidden = false): Repo {
  return { id, path: `/code/${name}`, name, added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden };
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

function lane(id: number, target: Repo, sessions: AgentSession[] = []): Lane {
  return {
    id,
    repo: target,
    worktree: { id, repo_id: target.id, path: `/code/${target.name}-wt`, branch: "main", head: "abc", is_main: true, name: "main" },
    state: { worktree_id: id, head: "abc", branch: "main", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: sessions,
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
    runningOnly: () => false,
    setRunningOnly: vi.fn(),
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
});
