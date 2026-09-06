import { createRoot } from "solid-js";
import { describe, expect, it, vi } from "vitest";

import type { AccountUsage, AgentSession, Lane, Repo, UsageRefreshResult } from "../bindings";
import {
  agentState,
  controllerAgentState,
  controllerSummary,
  createFleetStore,
  fleetCounts,
  isControllerRepo,
  laneIndicator,
  laneIndicatorTitle,
  matchesLane,
  orderRepos,
  pickFocusedUsage,
  sortReposByActivity,
  withSessionKeys,
  type FleetSource,
} from "./fleet";

function lane(overrides: Partial<Lane> = {}): Lane {
  return {
    id: 7,
    repo: { id: 2, path: "/code/repomon", name: "repomon", added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null },
    worktree: { id: 3, repo_id: 2, path: "/code/repomon-wt/desktop", branch: "feat/desktop", head: "abc", is_main: false, name: "desktop" },
    state: { worktree_id: 3, head: "abc", branch: "feat/desktop", upstream: null, ahead: 2, behind: 0, dirty: { staged: 0, unstaged: 1, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: [],
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
    role: null,
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
    generated_label: null,
    ...overrides,
  };
}

function repo(id: number, name: string, hidden = false): Repo {
  return { id, path: `/code/${name}`, name, added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden, position: null, label: null };
}

/// A store fed one fixed snapshot, started and refreshed once. `subscribe` never fires, and the
/// 2s heartbeat is stopped before the test asserts, so nothing races the assertions.
async function startedStore(repos: Repo[], lanes: Lane[], sortReposByActivity: boolean | null = false) {
  const source: FleetSource = {
    load: () => Promise.resolve({ repos, lanes, usage: [], terminals: [], sortReposByActivity, sortMode: sortReposByActivity === true ? "activity" : "default", tabSortMode: null }),
    refreshUsage: () => Promise.resolve(),
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

describe("orderRepos", () => {
  const alpha = repo(1, "alpha");
  const beta = repo(2, "beta");
  const at = (id: number, target: Repo, when: string) => lane({ id, repo: target, last_activity_at: when });

  it("keeps the daemon order in default mode", () => {
    expect(orderRepos([beta, alpha], [], "default")).toEqual([beta, alpha]);
  });

  it("sorts by activity in activity mode", () => {
    const lanes = [at(10, alpha, "2026-07-20T00:00:00Z"), at(20, beta, "2026-07-27T00:00:00Z")];
    expect(orderRepos([alpha, beta], lanes, "activity").map((r) => r.id)).toEqual([2, 1]);
  });

  it("takes the daemon's persisted manual order as-is in manual mode", () => {
    // repo.list already sorts by position; re-sorting here would fight the user's drags.
    const lanes = [at(10, alpha, "2026-07-27T00:00:00Z"), at(20, beta, "2026-07-20T00:00:00Z")];
    expect(orderRepos([beta, alpha], lanes, "manual")).toEqual([beta, alpha]);
  });

  it("treats an unknown mode like default rather than throwing", () => {
    expect(orderRepos([beta, alpha], [], "something-new")).toEqual([beta, alpha]);
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
    expect(laneIndicator(inferred)).toEqual({ label: "inferred", tone: "signal", urgent: false });
    expect(laneIndicatorTitle(inferred)).toBe(
      "the worktree is changing but the agent behind it could not be identified",
    );
    expect(laneIndicator(lane({ agent_sessions: [] }))).toEqual({ label: "", tone: "muted", urgent: false });
    expect(laneIndicator(lane({ agent_sessions: [agent({ status: "idle" })] }))).toEqual({ label: "idle", tone: "muted", urgent: false });
  });

  it("keeps the pill to one short word regardless of how many agents share the state", () => {
    // A lane with 3 agents where only 1 is running must say "running", not "3 running" - the
    // label never carries the agent count. That count moves to the tooltip instead.
    const mostlyIdle = lane({
      agent_sessions: [
        agent({ status: "running" }),
        agent({ status: "idle" }),
        agent({ status: "idle" }),
      ],
    });
    expect(laneIndicator(mostlyIdle)).toEqual({ label: "running", tone: "signal", urgent: false });
    expect(laneIndicatorTitle(mostlyIdle)).toBeUndefined();

    const twoRunning = lane({
      agent_sessions: [
        agent({ status: "running" }),
        agent({ status: "running" }),
        agent({ status: "idle" }),
      ],
    });
    expect(laneIndicator(twoRunning)).toEqual({ label: "running", tone: "signal", urgent: false });
    expect(laneIndicatorTitle(twoRunning)).toBe("2 running");

    const subRunning = lane({
      agent_sessions: [
        agent({ status: "running", subagent_running: "Drafting contract (3m 16s)" }),
        agent({ status: "idle" }),
      ],
    });
    expect(laneIndicator(subRunning)).toEqual({ label: "running", tone: "signal", urgent: false });
    expect(laneIndicatorTitle(subRunning)).toBe("subagent running");
  });

  it("counts agents rather than lanes, so a chip can never undercount its own rows", async () => {
    // The operator's report: one lane's pill read "2 RUNNING" under a "Running 1" chip, because
    // the pill counted agents and the chip counted lanes.
    const only = repo(1, "upwork");
    const busy = lane({
      id: 10,
      repo: only,
      agent_sessions: [agent({ status: "running" }), agent({ status: "running" })],
    });
    expect(laneIndicator(busy).label).toBe("running");
    expect(laneIndicatorTitle(busy)).toBe("2 running");

    const { fleet, teardown } = await startedStore([only], [busy]);
    await fleet.refresh();
    expect(fleet.counts().running).toBe(2);
    teardown();
  });

  it("counts the same agents the pills do, inferred sessions included nowhere", () => {
    // The chip used a bare `status === "running"` test while the pill filtered inferred
    // sessions out, so an inferred lane inflated the chip past what any row claimed.
    const inferredOnly = lane({
      id: 11,
      agent_sessions: [agent({ status: "running", inferred: true, tmux_window: null })],
    });
    expect(laneIndicator(inferredOnly).label).toBe("inferred");
    expect(fleetCounts([inferredOnly])).toEqual({ urgent: 0, running: 0, idle: 0 });
  });

  it("counts needs-you and stalled agents as needing attention, and idle agents apart", () => {
    const mixed = [
      lane({ id: 12, agent_sessions: [agent({ status: "waiting" }), agent({ status: "idle" })] }),
      lane({ id: 13, agent_sessions: [agent({ status: "running", stale: true })] }),
      lane({ id: 14, agent_sessions: [agent({ status: "running" }), agent({ status: "idle" })] }),
    ];
    expect(fleetCounts(mixed)).toEqual({ urgent: 2, running: 1, idle: 2 });
  });

  it("gives every agent exactly one state, in urgency order", () => {
    expect(agentState(agent({ status: "waiting", pending_dialog: { question: "Run?", body: [], options: [], selected: null } }))).toBe("decision");
    expect(agentState(agent({ status: "running", stale: true }))).toBe("stalled");
    expect(agentState(agent({ status: "rate-limited" }))).toBe("limited");
    expect(agentState(agent({ status: "waiting" }))).toBe("needs-you");
    expect(agentState(agent({ status: "running", external: true }))).toBe("external");
    expect(agentState(agent({ status: "running" }))).toBe("running");
    expect(agentState(agent({ status: "running", inferred: true }))).toBe("inferred");
    expect(agentState(agent({ status: "idle" }))).toBe("idle");
    expect(agentState(agent({ status: "ended" }))).toBe("exited");
    // A stalled external session is not "stalled": the daemon never watches its pane.
    expect(agentState(agent({ status: "running", stale: true, external: true }))).toBe("external");
  });

  it("reads a controller that just ended its turn as idle, not as needing you", () => {
    // A controller is a standing coordinator: it sits at end-of-turn between instructions, and
    // the operator does not owe it an answer for that. A worker in a project lane still does.
    const ended = agent({ status: "waiting", attention_kind: "end_of_turn" });
    expect(controllerAgentState(ended)).toBe("idle");
    expect(agentState(ended)).toBe("needs-you");

    const controllerLane = lane({ id: 90, role: "controller", agent_sessions: [ended] });
    expect(laneIndicator(controllerLane)).toEqual({ label: "idle", tone: "muted", urgent: false });
    expect(controllerSummary([controllerLane])).toMatchObject({ state: "idle", urgent: 0 });
    expect(fleetCounts([controllerLane])).toEqual({ urgent: 0, running: 0, idle: 1 });
  });

  it("keeps needs you for a controller sitting on a dialog or an explicit question", () => {
    const dialog = agent({
      status: "waiting",
      attention_kind: "permission",
      pending_prompt: "Bash: rm -rf build",
      pending_dialog: { question: "Run?", body: [], options: [], selected: null },
    });
    expect(controllerAgentState(dialog)).toBe("decision");

    const question = agent({
      status: "waiting",
      attention_kind: "decision",
      pending_prompt: "Which auth method should we use?",
    });
    expect(controllerAgentState(question)).toBe("needs-you");

    const controllerLane = lane({ id: 90, role: "controller", agent_sessions: [question] });
    expect(controllerSummary([controllerLane])).toMatchObject({ state: "needs-you", urgent: 1 });
  });

  it("leaves a worker's mapping untouched, whatever the attention word says", () => {
    // Same payload, an ordinary lane: an ended turn there means work is waiting to be picked up.
    const ended = agent({ status: "waiting", attention_kind: "end_of_turn" });
    const workerLane = lane({ agent_sessions: [ended] });
    expect(laneIndicator(workerLane).label).toBe("needs you");
    expect(fleetCounts([workerLane])).toEqual({ urgent: 1, running: 0, idle: 0 });
  });

  it("keeps a controller on needs you when the daemon sent no attention word", () => {
    // An older daemon says nothing about why the session waits, so the reading does not soften.
    expect(controllerAgentState(agent({ status: "waiting" }))).toBe("needs-you");
  });

  it("marks an ended session exited rather than idle, with its own pill", () => {
    const exited = lane({ agent_sessions: [agent({ status: "ended" })] });
    expect(laneIndicator(exited)).toEqual({ label: "exited", tone: "muted", urgent: false });
  });

  it("explains a status from the daemon's reason instead of guessing at one", () => {
    const explained = lane({
      agent_sessions: [
        agent({ status: "running", status_reason: "spinner on screen: Thinking (2m 14s)" }),
        agent({ status: "idle", status_reason: "no output for 41m" }),
      ],
    });
    expect(laneIndicatorTitle(explained)).toBe("spinner on screen: Thinking (2m 14s)");
    expect(laneIndicatorTitle(lane({ agent_sessions: [agent({ status: "idle" })] }))).toBeUndefined();
  });

  it("filters to the same lanes the chips count", async () => {
    const only = repo(1, "repomon");
    const lanes = [
      lane({ id: 20, repo: only, agent_sessions: [agent({ status: "running" })] }),
      lane({ id: 21, repo: only, agent_sessions: [agent({ status: "idle" })] }),
      lane({ id: 22, repo: only, agent_sessions: [agent({ status: "waiting" })] }),
    ];
    const { fleet, teardown } = await startedStore([only], lanes);
    await fleet.refresh();

    fleet.setRunningOnly(true);
    expect(fleet.visibleLanes().map((l) => l.id)).toEqual([20]);
    expect(fleet.counts().running).toBe(1);
    fleet.setRunningOnly(false);

    fleet.setUrgentOnly(true);
    expect(fleet.visibleLanes().map((l) => l.id)).toEqual([22]);
    expect(fleet.counts().urgent).toBe(1);
    teardown();
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

  it("laneIndicator respects running status for stalled indicators", () => {
    // Running + stale -> stalled (tone: fault)
    const runningStale = lane({ agent_sessions: [agent({ status: "running", stale: true })] });
    expect(laneIndicator(runningStale)).toEqual({ label: "stalled", tone: "fault", urgent: true });

    // Idle + stale -> idle (not stalled)
    const idleStale = lane({ agent_sessions: [agent({ status: "idle", stale: true })] });
    expect(laneIndicator(idleStale)).toEqual({ label: "idle", tone: "muted", urgent: false });

    // External + stale -> external (not stalled)
    const extStale = lane({ agent_sessions: [agent({ status: "running", external: true, stale: true })] });
    expect(laneIndicator(extStale)).toEqual({ label: "external", tone: "muted", urgent: false });
  });
});

describe("the repomind home", () => {
  const home = repo(9, "repomind");
  const project = repo(1, "repomon");

  it("keeps the home out of the repo groups, the lane counts, and the chip counts", async () => {
    const lanes = [
      lane({ id: 10, repo: project, agent_sessions: [agent({ status: "running" })] }),
      lane({
        id: 90,
        repo: home,
        role: "controller",
        agent_sessions: [agent({ status: "running" }), agent({ status: "waiting" })],
      }),
    ];
    const { fleet, teardown } = await startedStore([project, home], lanes);
    await fleet.refresh();

    expect(fleet.visibleRepos().map((r) => r.id)).toEqual([1]);
    expect(fleet.visibleLanes().map((l) => l.id)).toEqual([10]);
    expect(fleet.fleetLanes().map((l) => l.id)).toEqual([10]);
    // The controller's running agent and its waiting one are counted by the pinned row instead.
    expect(fleet.counts()).toEqual({ urgent: 0, running: 1, idle: 0 });
    // Auto-selection lands in the project, never in the home.
    expect(fleet.selectedLaneId()).toBe(10);
    // The daemon still hands the lane over, so the pinned row and Multitasking can use it.
    expect(fleet.controllerLanes().map((l) => l.id)).toEqual([90]);
    teardown();
  });

  it("holds an explicit selection of the home across a refresh", async () => {
    const lanes = [
      lane({ id: 10, repo: project }),
      lane({ id: 90, repo: home, role: "controller" }),
    ];
    const { fleet, teardown } = await startedStore([project, home], lanes);
    await fleet.refresh();

    fleet.setSelectedLaneId(90);
    await fleet.refresh();
    expect(fleet.selectedLaneId()).toBe(90);
    teardown();
  });

  it("makes the pinned row the first stop of arrow navigation", async () => {
    const lanes = [
      lane({ id: 10, repo: project }),
      lane({ id: 11, repo: project }),
      lane({ id: 90, repo: home, role: "controller" }),
    ];
    const { fleet, teardown } = await startedStore([project, home], lanes);
    await fleet.refresh();

    fleet.setSelectedLaneId(90);
    fleet.moveSelection(1);
    expect(fleet.selectedLaneId()).toBe(fleet.visibleLanes()[0].id);
    fleet.moveSelection(-1);
    expect(fleet.selectedLaneId()).toBe(90);
    teardown();
  });

  it("still shows a repo that owns both a controller lane and ordinary ones", () => {
    const mixed = [
      lane({ id: 90, repo: home, role: "controller" }),
      lane({ id: 91, repo: home }),
    ];
    expect(isControllerRepo(home.id, mixed)).toBe(false);
    expect(isControllerRepo(home.id, [mixed[0]])).toBe(true);
    // A repo with no lanes is empty, not the home.
    expect(isControllerRepo(project.id, mixed)).toBe(false);
  });

  it("summarizes the controllers with the fleet's own state vocabulary", () => {
    const idle = controllerSummary([lane({ id: 10, repo: project, agent_sessions: [agent()] })]);
    expect(idle).toEqual({ lane: null, agents: 0, state: null, urgent: 0 });

    const live = controllerSummary([
      lane({ id: 10, repo: project, agent_sessions: [agent({ status: "running" })] }),
      lane({
        id: 90,
        repo: home,
        role: "controller",
        agent_sessions: [agent({ status: "running" }), agent({ status: "waiting" })],
      }),
    ]);
    expect(live.lane?.id).toBe(90);
    expect(live.agents).toBe(2);
    // "needs you" outranks "running", and only the controllers are counted.
    expect(live.state).toBe("needs-you");
    expect(live.urgent).toBe(1);
  });
});


describe("manual usage refresh", () => {
  it("waits for the RPC before reloading quota age and today's cost", async () => {
    let complete!: () => void;
    const pending = new Promise<void>((resolve) => { complete = resolve; });
    let fresh = false;
    let loads = 0;
    const source: FleetSource = {
      load: async () => { loads += 1; return {
        repos: [], lanes: [], terminals: [], sortMode: null, tabSortMode: null, sortReposByActivity: null,
        usage: [{ key: "default", label: "main", age_secs: fresh ? 0 : 120, report: { windows: [] } }],
        costToday: fresh ? 12 : 10,
      }; },
      refreshUsage: async () => { await pending; fresh = true; return { refreshed: true, reason: "ok", detail: null, snapshot: [] }; },
      subscribe: async () => () => {},
    };
    const { fleet, teardown } = createRoot((dispose) => {
      const fleet = createFleetStore(source);
      fleet.start();
      return { fleet, teardown: () => { fleet.stop(); dispose(); } };
    });
    try {
      await fleet.refresh();
      const before = loads;
      const refresh = fleet.refreshUsage();
      await Promise.resolve();
      expect(loads).toBe(before);
      expect(fleet.costToday()).toBe(10);
      complete();
      expect((await refresh)?.reason).toBe("ok");
      expect(fleet.focusedUsage()?.age_secs).toBe(0);
      expect(fleet.costToday()).toBe(12);
    } finally { teardown(); }
  });
});


describe("ticketed manual refresh events", () => {
  function fixture() {
    let event!: Parameters<FleetSource["subscribe"]>[0];
    let acknowledge!: (result: UsageRefreshResult) => void;
    const ack = new Promise<UsageRefreshResult>((resolve) => { acknowledge = resolve; });
    const source: FleetSource = {
      load: async () => ({ repos: [], lanes: [], terminals: [], usage: [], sortMode: null, tabSortMode: null, sortReposByActivity: null }),
      refreshUsage: () => ack,
      subscribe: async (next) => { event = next; return () => {}; },
    };
    const view = createRoot((dispose) => {
      const fleet = createFleetStore(source); fleet.start();
      return { fleet, stop: () => { fleet.stop(); dispose(); } };
    });
    return { ...view, acknowledge, emit: (request_id: number, reason = "ok", detail: string | null = null) => event({ jsonrpc: "2.0", method: "event.usage.refreshed", params: { request_id, reason, detail, snapshot: [] } }) };
  }

  it.each(["probe_disabled", "no_active_kind", "timeout", "error"])("settles on a %s completion instead of waiting for the ceiling", async (reason) => {
    const f = fixture();
    try {
      await f.fleet.refresh();
      const result = f.fleet.refreshUsage();
      f.acknowledge({ refreshed: false, reason: "pending", request_id: 42, detail: null, snapshot: [] });
      await Promise.resolve(); await Promise.resolve();
      f.emit(42, reason, "No agent running to probe");
      const settled = await result;
      expect(settled?.reason).toBe(reason);
      expect(settled?.refreshed).toBe(false);
      expect(settled?.detail).toBe("No agent running to probe");
    } finally { f.stop(); }
  });

  it.each([false, true])("waits for the matching event, including before the ack (%s)", async (early) => {
    const f = fixture();
    try {
      await f.fleet.refresh();
      let settled = false;
      const result = f.fleet.refreshUsage().then((r) => { settled = true; return r; });
      f.emit(41);
      if (early) f.emit(42);
      f.acknowledge({ refreshed: false, reason: "pending", request_id: 42, detail: null, snapshot: [] });
      await Promise.resolve(); await Promise.resolve();
      if (!early) { expect(settled).toBe(false); f.emit(42); }
      expect((await result)?.reason).toBe("ok");
    } finally { f.stop(); }
  });

  it("stops waiting at the 20-second client ceiling if no completion arrives", async () => {
    vi.useFakeTimers();
    const f = fixture();
    try {
      await f.fleet.refresh();
      let settled = false;
      const result = f.fleet.refreshUsage().then((r) => { settled = true; return r; });
      f.acknowledge({ refreshed: false, reason: "pending", request_id: 42, detail: null, snapshot: [] });
      await vi.advanceTimersByTimeAsync(19_999);
      expect(settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      const waiting = await result;
      expect(waiting?.reason).toBe("timeout");
      expect(waiting?.detail).toBe("Still probing, this can take a moment");
    } finally { f.stop(); vi.useRealTimers(); }
  });
});

describe("serialized fleet refresh", () => {
  function harness() {
    const pending: Array<{ resolve: (snapshot: Awaited<ReturnType<FleetSource["load"]>>) => void; reject: (error: Error) => void }> = [];
    const source: FleetSource = {
      load: vi.fn(() => new Promise((resolve, reject) => pending.push({ resolve, reject }))),
      refreshUsage: async () => {},
      subscribe: async () => () => {},
    };
    const view = createRoot((dispose) => ({ fleet: createFleetStore(source), dispose }));
    const snapshot = (id: number) => ({ repos: [repo(id, String(id))], lanes: [], usage: [], terminals: [], sortReposByActivity: null, sortMode: null, tabSortMode: null });
    return { ...view, pending, source, snapshot };
  }

  it("applies slow loads while heartbeats coalesce into one follow-up", async () => {
    vi.useFakeTimers();
    const h = harness();
    try {
      h.fleet.start();
      await vi.advanceTimersByTimeAsync(3600);
      expect(h.source.load).toHaveBeenCalledTimes(1);
      const followup = h.fleet.refresh();
      h.pending[0].resolve(h.snapshot(1));
      await vi.advanceTimersByTimeAsync(0);
      expect(h.fleet.synced()).toBe(true);
      expect(h.fleet.repos()[0].id).toBe(1);
      expect(h.source.load).toHaveBeenCalledTimes(2);
      h.pending[1].resolve(h.snapshot(2));
      await followup;
      expect(h.fleet.repos()[0].id).toBe(2);
      expect(h.fleet.loading()).toBe(false);
      expect(h.source.load).toHaveBeenCalledTimes(2);
    } finally { h.fleet.stop(); h.dispose(); vi.useRealTimers(); }
  });

  it("discards a stopped load and starts only one fresh load after restart", async () => {
    vi.useFakeTimers();
    const h = harness();
    try {
      h.fleet.start();
      void h.fleet.refresh();
      h.fleet.stop();
      h.fleet.start();
      const fresh = h.fleet.refresh();
      expect(h.source.load).toHaveBeenCalledTimes(1);
      h.pending[0].resolve(h.snapshot(1));
      await vi.advanceTimersByTimeAsync(0);
      expect(h.fleet.synced()).toBe(false);
      expect(h.fleet.repos()).toHaveLength(0);
      expect(h.source.load).toHaveBeenCalledTimes(2);
      h.pending[1].resolve(h.snapshot(2));
      await fresh;
      expect(h.fleet.repos()[0].id).toBe(2);
    } finally { h.fleet.stop(); h.dispose(); vi.useRealTimers(); }
  });

  it("runs the coalesced load after an error and clears the error on recovery", async () => {
    vi.useFakeTimers();
    const h = harness();
    try {
      h.fleet.start();
      const recovered = h.fleet.refresh();
      h.pending[0].reject(new Error("offline"));
      await vi.advanceTimersByTimeAsync(0);
      expect(h.fleet.error()).toBe("offline");
      expect(h.source.load).toHaveBeenCalledTimes(2);
      h.pending[1].resolve(h.snapshot(2));
      await recovered;
      expect(h.fleet.error()).toBeNull();
      expect(h.fleet.synced()).toBe(true);
    } finally { h.fleet.stop(); h.dispose(); vi.useRealTimers(); }
  });
});
