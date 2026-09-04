import { createMemo, createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";

import type { AccountUsage, AgentSession, Lane, Repo } from "../bindings";
import { daemonCall, subscribeDaemon, type DaemonEvent } from "../ipc/rpc";

export interface FleetSnapshot {
  repos: Repo[];
  lanes: Lane[];
  usage: AccountUsage[];
  terminals: Array<{ lane_id: number; id: string }>;
  /// Sidebar-affecting settings, read from the daemon rather than cached locally so a change made
  /// in the TUI shows up here without a restart. Null when the call failed; the store keeps the
  /// last good value in that case.
  sortReposByActivity: boolean | null;
  /// Resolved repo sort mode ("default" | "activity" | "manual"). Null when config.get failed.
  sortMode: string | null;
  /// Resolved per-lane agent tab sort mode ("activity" | "manual"). Null when config.get failed.
  tabSortMode: string | null;
}

export interface FleetSource {
  load(): Promise<FleetSnapshot>;
  refreshUsage(): Promise<void>;
  subscribe(onEvent: (event: DaemonEvent) => void): Promise<() => void>;
}

export const daemonFleetSource: FleetSource = {
  async load() {
    const [repos, lanes, usage, terminals, config] = await Promise.all([
      daemonCall("repo.list"),
      daemonCall("lane.list"),
      daemonCall("usage.get").catch(() => []),
      daemonCall("terminal.list_all").catch(() => []),
      daemonCall("config.get").catch(() => null),
    ]);
    return {
      repos,
      lanes,
      usage,
      terminals,
      sortReposByActivity: config ? Boolean(config.sort_repos_by_activity) : null,
      sortMode: config && typeof config.sort_mode === "string" ? config.sort_mode : null,
      tabSortMode:
        config && typeof config.tab_sort_mode === "string" ? config.tab_sort_mode : null,
    };
  },
  refreshUsage: async () => {
    await daemonCall("usage.refresh");
  },
  subscribe: subscribeDaemon,
};

export type LaneTone = "attention" | "fault" | "signal" | "muted";

function isRepoSortMode(value: string): value is RepoSortMode {
  return value === "default" || value === "activity" || value === "manual";
}

export interface LaneIndicator {
  label: string;
  tone: LaneTone;
  urgent: boolean;
}

function gateSuffix(lane: Lane): string {
  const blocked = lane.agent_sessions.find((agent) => !agent.inferred && agent.gate && !agent.gate.allowed)?.gate;
  return blocked ? ` · gate ${blocked.net_new_findings}` : "";
}

/// One agent's state, the single vocabulary the sidebar speaks. Every pill, chip, count and
/// filter is derived from this function and nothing else, so a lane pill reading "2 running" and
/// a "Running" chip reading 1 can no longer describe the same fleet.
///
/// This is a pure projection of daemon fields. The frontend does not re-read pane text or run its
/// own timers: when a status looks wrong, the daemon's `status_reason` says why, and the fix
/// belongs there.
export type AgentState =
  | "decision"
  | "stalled"
  | "limited"
  | "needs-you"
  | "external"
  | "running"
  | "inferred"
  | "idle";

/// Most urgent first. `agentState` walks this order, and `laneIndicator` shows the most urgent
/// state among a lane's agents.
const STATE_PRIORITY: readonly AgentState[] = [
  "decision",
  "stalled",
  "limited",
  "needs-you",
  "external",
  "running",
  "inferred",
  "idle",
];

export function agentState(agent: AgentSession): AgentState {
  if (agent.pending_dialog) return "decision";
  const managed = !agent.external && !agent.inferred;
  if (managed && agent.status === "running" && agent.stale) return "stalled";
  if (agent.status === "rate-limited") return "limited";
  if (managed && agent.status === "waiting") return "needs-you";
  if (agent.external) return "external";
  if (!agent.inferred && agent.status === "running") return "running";
  if (agent.inferred) return "inferred";
  return "idle";
}

/// States that put a lane in the "Needs attention" filter, and so in its count.
const URGENT_STATES: ReadonlySet<AgentState> = new Set<AgentState>([
  "decision",
  "stalled",
  "limited",
  "needs-you",
]);

export function isUrgentState(state: AgentState): boolean {
  return URGENT_STATES.has(state);
}

/// The most urgent state among a lane's agents, or null for a lane with no agents.
export function laneState(lane: Lane): AgentState | null {
  const states = new Set(lane.agent_sessions.map(agentState));
  return STATE_PRIORITY.find((state) => states.has(state)) ?? null;
}

/// How many of a lane's agents share the lane's headline state, so the pill can say "2 running"
/// truthfully rather than counting the whole roster.
export function laneStateCount(lane: Lane, state: AgentState): number {
  return lane.agent_sessions.filter((agent) => agentState(agent) === state).length;
}

const STATE_TONE: Record<AgentState, LaneTone> = {
  decision: "attention",
  stalled: "fault",
  limited: "fault",
  "needs-you": "attention",
  external: "muted",
  running: "signal",
  inferred: "signal",
  idle: "muted",
};

export function laneIndicator(lane: Lane): LaneIndicator {
  const state = laneState(lane);
  if (state === null) return { label: "", tone: "muted", urgent: false };
  const gate = gateSuffix(lane);
  const urgent = isUrgentState(state);
  const tone = STATE_TONE[state];
  if (state === "running") {
    const count = laneStateCount(lane, "running");
    const running = lane.agent_sessions.filter((agent) => agentState(agent) === "running");
    const onlySubagents = running.every((agent) => Boolean(agent.subagent_running));
    const label =
      onlySubagents && count === 1 ? "subagent running" : count > 1 ? `${count} running` : "running";
    return { label: `${label}${gate}`, tone, urgent };
  }
  const label =
    state === "decision"
      ? `decision${gate}`
      : state === "needs-you"
        ? `needs you${gate}`
        : state === "inferred"
          ? "active · inferred"
          : state;
  return { label, tone, urgent };
}

/// Why an agent's state reads the way it does, straight from the daemon. Never invented here:
/// with no reason on the payload the tooltip simply says less.
export function agentStateReason(agent: AgentSession): string | null {
  return agent.status_reason ?? null;
}

/// The lane pill's tooltip: the headline state plus the daemon's reasons for the agents in it.
export function laneIndicatorTitle(lane: Lane): string | undefined {
  const state = laneState(lane);
  if (state === null) return undefined;
  const reasons = lane.agent_sessions
    .filter((agent) => agentState(agent) === state)
    .map((agent) => agentStateReason(agent))
    .filter((reason): reason is string => Boolean(reason));
  if (state === "external" && !reasons.length) {
    return "External session running outside repomon. Select lane to adopt into tmux management.";
  }
  if (!reasons.length) return undefined;
  return reasons.join("\n");
}

export interface FleetCounts {
  urgent: number;
  running: number;
  idle: number;
}

/// Agent counts, not lane counts. The lane pill counts agents, so the chips must too, or a lane
/// showing "2 running" sits under a chip reading "Running 1".
export function fleetCounts(lanes: Lane[]): FleetCounts {
  const counts: FleetCounts = { urgent: 0, running: 0, idle: 0 };
  for (const lane of lanes) {
    for (const agent of lane.agent_sessions) {
      const state = agentState(agent);
      if (isUrgentState(state)) counts.urgent += 1;
      else if (state === "running") counts.running += 1;
      else if (state === "idle") counts.idle += 1;
    }
  }
  return counts;
}

/// The usage-probe key for the account a session runs under, matching how the daemon keys its
/// reports. Codex has one account and is probed under `"codex"`; Claude is keyed by config dir
/// (`config_dir: null` = the default `~/.claude`, else the dir path). Branching on the agent
/// matters: a codex session also has `config_dir: null`, so keying on it alone resolved codex
/// lanes to `"default"` and showed them Claude's numbers.
export function accountKeyOf(session: AgentSession): string {
  if (session.agent === "codex") return "codex";
  if (session.agent === "antigravity" || session.agent === "agy") return "antigravity";
  return session.config_dir ?? "default";
}

/// The usage report for the focused agent's account, matched by account key, so the pill follows
/// whichever account you are actually looking at instead of always showing the first probed one.
///
/// `focusedWindow` is the tmux window of the pane in view: a lane can run several agents on
/// different accounts at once, and the visible tab is the one the numbers should describe. With no
/// pane focused (or its session gone), fall back to the lane's first non-inferred session.
/// Returns `null` when the resolved account has not been probed, rather than another account's
/// numbers; falls back to the first report only when there is no agent to attribute to.
export function pickFocusedUsage(
  reports: AccountUsage[],
  lane: Lane | null,
  focusedWindow: string | null = null,
): AccountUsage | null {
  if (!reports.length) return null;
  const agent = (focusedWindow
    ? lane?.agent_sessions.find((session) => session.tmux_window === focusedWindow)
    : undefined)
    ?? lane?.agent_sessions.find((session) => !session.inferred)
    ?? lane?.agent_sessions[0];
  if (!agent) return reports[0];
  const key = accountKeyOf(agent);
  return reports.find((report) => report.key === key) ?? null;
}

/// The sidebar repo sort modes, mirroring the daemon's `SortMode`.
export type RepoSortMode = "default" | "activity" | "manual";

/// Order repo groups for the sidebar according to `mode`.
///
/// - `"activity"`: most recent lane activity first (see `sortReposByActivity`).
/// - `"manual"`: the daemon's order is taken as-is — `repo.list` already sorts by the persisted
///   manual positions, so re-sorting here would fight the user's drag-and-drop.
/// - `"default"`: the daemon's order untouched.
export function orderRepos(repos: Repo[], lanes: Lane[], mode: string): Repo[] {
  if (mode === "activity") return sortReposByActivity(repos, lanes, true);
  return repos;
}

/// Order repo groups by their most recent lane activity, newest first, when the setting is on.
///
/// Only the groups move. Ordering *lanes* by activity is what the TUI removed on purpose: it made
/// rows bubble around on every agent output. A repo's activity changes far less often, so the
/// groups stay put while you work in one.
///
/// Repos with no lanes have no activity to sort by and sink to the bottom. Ties keep the incoming
/// (daemon) order, so the result is stable across polls.
export function sortReposByActivity(repos: Repo[], lanes: Lane[], enabled: boolean): Repo[] {
  if (!enabled) return repos;
  const newest = new Map<number, number>();
  for (const lane of lanes) {
    const at = Date.parse(lane.last_activity_at);
    if (Number.isNaN(at)) continue;
    const seen = newest.get(lane.repo.id);
    if (seen === undefined || at > seen) newest.set(lane.repo.id, at);
  }
  return repos
    .map((repo, index) => ({ repo, index, at: newest.get(repo.id) ?? -Infinity }))
    // Compared for equality first: two lane-less repos are both -Infinity, and subtracting those
    // yields NaN, which sorts unpredictably.
    .sort((a, b) => (a.at === b.at ? a.index - b.index : b.at - a.at))
    .map((entry) => entry.repo);
}

export function matchesLane(lane: Lane, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  const haystack = [
    lane.repo.name,
    lane.worktree.name,
    lane.worktree.branch ?? "",
    lane.worktree.path,
    ...lane.agent_sessions.flatMap((agent) => [
      agent.agent,
      agent.custom_label ?? "",
      agent.title ?? "",
      agent.last_message ?? "",
    ]),
  ]
    .join(" ")
    .toLowerCase();

  let cursor = 0;
  for (const char of haystack) {
    if (char === needle[cursor]) cursor += 1;
    if (cursor === needle.length) return true;
  }
  return false;
}

/// Overlay sessions all arrive with `id: 0` (they have no store row), but `reconcile` keys
/// nested arrays by `id` too — duplicate keys collapse a lane's sessions to one, hiding every
/// agent tab after the first. Re-key each session by its stable identity (transcript id, else
/// its window) hashed to a number, so reconcile can tell them apart across polls.
export function withSessionKeys(lanes: Lane[]): Lane[] {
  return lanes.map((lane) => ({
    ...lane,
    agent_sessions: lane.agent_sessions.map((agent, index) => {
      if (agent.id !== 0) return agent;
      const seed = agent.session_id ?? agent.tmux_window ?? `idx-${index}`;
      let hash = 0;
      for (let i = 0; i < seed.length; i += 1) hash = (hash * 31 + seed.charCodeAt(i)) | 0;
      return { ...agent, id: hash || index + 1 };
    }),
  }));
}

function byPriority(a: Lane, b: Lane): number {
  if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
  const urgent = Number(laneIndicator(b).urgent) - Number(laneIndicator(a).urgent);
  if (urgent) return urgent;
  return Date.parse(b.last_activity_at) - Date.parse(a.last_activity_at);
}

export function createFleetStore(source: FleetSource = daemonFleetSource) {
  // repos/lanes are Solid stores updated with keyed `reconcile`, so a poll only touches the fields
  // that actually changed and leaves every unchanged row's identity intact. That keeps the sidebar
  // DOM stable across the 2s heartbeat — otherwise every row would be rebuilt each poll, which
  // resets CSS :hover and makes hover states flicker.
  const [repoStore, setRepoStore] = createStore<Repo[]>([]);
  const [laneStore, setLaneStore] = createStore<Lane[]>([]);
  const repos = () => repoStore;
  const lanes = () => laneStore;
  const [usage, setUsage] = createSignal<AccountUsage[]>([]);
  const [terminals, setTerminals] = createSignal<Array<{ lane_id: number; id: string }>>([]);
  const [selectedLaneId, setSelectedLaneId] = createSignal<number | null>(null);
  // The tmux window of the pane in view. Owned by the workspace store (which holds the layout and
  // tab state) and mirrored here, because the usage memo lives on this side of the wiring.
  const [focusedWindow, setFocusedWindow] = createSignal<string | null>(null);
  const [query, setQuery] = createSignal("");
  const [urgentOnly, setUrgentOnly] = createSignal(false);
  const [runningOnly, setRunningOnly] = createSignal(false);
  const [idleOnly, setIdleOnly] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  const [synced, setSynced] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  let active = false;
  let interval: ReturnType<typeof setInterval> | undefined;
  let unsubscribe: (() => void) | undefined;
  let refreshQueued = false;

  // Mirrors the daemon's repo sort mode, refreshed with every poll so a change made in the TUI
  // lands here too. Falls back to the legacy boolean for daemons that predate `sort_mode`.
  const [sortMode, setSortMode] = createSignal<RepoSortMode>("default");
  // Mirrors the daemon's per-lane agent tab sort mode ("activity" | "manual").
  const [tabSortMode, setTabSortMode] = createSignal<"activity" | "manual">("activity");
  // The daemon keeps returning hidden repos (flagged) so we can offer a way back; everything that
  // renders the fleet works from `visibleRepos` / `visibleLanes` / `unhiddenLanes` instead.
  const visibleRepos = createMemo(() =>
    orderRepos(repos().filter((repo) => !repo.hidden), lanes(), sortMode()),
  );
  const hiddenRepos = createMemo(() => repos().filter((repo) => repo.hidden));
  // Everything a hidden repo owns goes with it, including its share of the urgent/running counts:
  // a badge you cannot click through to is just noise.
  const unhiddenLanes = createMemo(() => lanes().filter((lane) => !lane.repo.hidden));

  // Every filter reads the same `agentState` the rows and chips do, so a chip can never select a
  // set the rows disagree with.
  const visibleLanes = createMemo(() =>
    unhiddenLanes()
      .filter((lane) => matchesLane(lane, query()))
      .filter((lane) => !urgentOnly() || laneIndicator(lane).urgent)
      .filter(
        (lane) =>
          !runningOnly() || lane.agent_sessions.some((agent) => agentState(agent) === "running"),
      )
      .filter(
        (lane) => !idleOnly() || lane.agent_sessions.some((agent) => agentState(agent) === "idle"),
      )
      .sort(byPriority),
  );

  const selectedLane = createMemo(() =>
    lanes().find((lane) => lane.id === selectedLaneId()) ?? null,
  );

  // The usage pill follows the focused agent's account rather than always the first probe.
  const focusedUsage = createMemo(() => pickFocusedUsage(usage(), selectedLane(), focusedWindow()));

  const counts = createMemo(() => fleetCounts(unhiddenLanes()));

  async function refresh() {
    if (!active) return;
    setLoading(true);
    try {
      const snapshot = await source.load();
      if (!active) return;
      setRepoStore(reconcile(snapshot.repos, { key: "id" }));
      setLaneStore(reconcile(withSessionKeys(snapshot.lanes), { key: "id" }));
      setUsage(snapshot.usage);
      setTerminals(snapshot.terminals);
      if (snapshot.sortMode !== null && isRepoSortMode(snapshot.sortMode)) {
        setSortMode(snapshot.sortMode);
      } else if (snapshot.sortReposByActivity !== null) {
        setSortMode(snapshot.sortReposByActivity ? "activity" : "default");
      }
      if (snapshot.tabSortMode === "manual" || snapshot.tabSortMode === "activity") {
        setTabSortMode(snapshot.tabSortMode);
      }
      setSynced(true);
      setError(null);
      const current = selectedLaneId();
      // Never auto-select into a repo the user hid, and drop the selection if the repo it lives
      // in was just hidden.
      const selectable = snapshot.lanes.filter((lane) => !lane.repo.hidden);
      if (current === null || !selectable.some((lane) => lane.id === current)) {
        setSelectedLaneId([...selectable].sort(byPriority)[0]?.id ?? null);
      }
    } catch (cause) {
      if (active) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (active) setLoading(false);
    }
  }

  async function refreshUsage() {
    if (!active) return;
    await source.refreshUsage();
    await refresh();
  }

  function queueRefresh() {
    if (refreshQueued) return;
    refreshQueued = true;
    setTimeout(() => {
      refreshQueued = false;
      void refresh();
    }, 60);
  }

  function start() {
    if (active) return;
    active = true;
    void refresh();
    // Heartbeat poll at 1.2s cadence to ensure fast UI updates without excessive overhead.
    interval = setInterval(() => void refresh(), 1200);
    void source
      .subscribe(queueRefresh)
      .then((stop) => {
        if (active) unsubscribe = stop;
        else stop();
      })
      .catch(() => undefined);
  }

  function stop() {
    active = false;
    if (interval) clearInterval(interval);
    interval = undefined;
    unsubscribe?.();
    unsubscribe = undefined;
  }

  function moveSelection(delta: number, urgent = false) {
    const candidates = visibleLanes().filter((lane) => !urgent || laneIndicator(lane).urgent);
    if (!candidates.length) return;
    const index = candidates.findIndex((lane) => lane.id === selectedLaneId());
    const next = index < 0 ? 0 : (index + delta + candidates.length) % candidates.length;
    setSelectedLaneId(candidates[next].id);
  }

  return {
    repos,
    visibleRepos,
    hiddenRepos,
    lanes,
    unhiddenLanes,
    usage,
    focusedUsage,
    terminals,
    selectedLane,
    selectedLaneId,
    setSelectedLaneId,
    focusedWindow,
    setFocusedWindow,
    query,
    setQuery,
    urgentOnly,
    setUrgentOnly,
    runningOnly,
    setRunningOnly,
    idleOnly,
    setIdleOnly,
    loading,
    synced,
    error,
    sortMode,
    tabSortMode,
    dismissError: () => setError(null),
    visibleLanes,
    counts,
    refresh,
    refreshUsage,
    start,
    stop,
    moveSelection,
  };
}

export type FleetStore = ReturnType<typeof createFleetStore>;
