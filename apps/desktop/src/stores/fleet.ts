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
}

export interface FleetSource {
  load(): Promise<FleetSnapshot>;
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
    };
  },
  subscribe: subscribeDaemon,
};

export type LaneTone = "attention" | "fault" | "signal" | "muted";

export interface LaneIndicator {
  label: string;
  tone: LaneTone;
  urgent: boolean;
}

function gateSuffix(lane: Lane): string {
  const blocked = lane.agent_sessions.find((agent) => !agent.inferred && agent.gate && !agent.gate.allowed)?.gate;
  return blocked ? ` · gate ${blocked.net_new_findings}` : "";
}

export function laneIndicator(lane: Lane): LaneIndicator {
  const agents = lane.agent_sessions;
  const gate = gateSuffix(lane);
  if (agents.some((agent) => agent.pending_dialog)) {
    return { label: `decision${gate}`, tone: "attention", urgent: true };
  }
  if (agents.some((agent) => !agent.external && !agent.inferred && agent.status === "running" && agent.stale)) {
    return { label: "stalled", tone: "fault", urgent: true };
  }
  if (agents.some((agent) => agent.status === "rate-limited")) {
    return { label: "limited", tone: "fault", urgent: true };
  }
  if (agents.some((agent) => !agent.external && !agent.inferred && agent.status === "waiting")) {
    return { label: `needs you${gate}`, tone: "attention", urgent: true };
  }
  if (agents.some((agent) => agent.external)) {
    return { label: "external", tone: "muted", urgent: false };
  }
  const runningAgents = agents.filter((agent) => !agent.inferred && agent.status === "running");
  const runningCount = runningAgents.length;
  if (runningCount > 0) {
    const onlySubagents = runningAgents.every((agent) => Boolean(agent.subagent_running));
    const label = onlySubagents && runningCount === 1 ? "subagent running" : runningCount > 1 ? `${runningCount} running` : "running";
    return { label: `${label}${gate}`, tone: "signal", urgent: false };
  }
  if (agents.some((agent) => agent.inferred)) {
    return { label: "active · inferred", tone: "signal", urgent: false };
  }
  return { label: agents.length ? "idle" : "", tone: "muted", urgent: false };
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
  const [loading, setLoading] = createSignal(false);
  const [synced, setSynced] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  let active = false;
  let interval: ReturnType<typeof setInterval> | undefined;
  let unsubscribe: (() => void) | undefined;
  let refreshQueued = false;

  // Mirrors the daemon's `sort_repos_by_activity` setting, refreshed with every poll so a change
  // made in the TUI lands here too.
  const [sortByActivity, setSortByActivity] = createSignal(false);
  // The daemon keeps returning hidden repos (flagged) so we can offer a way back; everything that
  // renders the fleet works from `visibleRepos` / `visibleLanes` / `unhiddenLanes` instead.
  const visibleRepos = createMemo(() =>
    sortReposByActivity(repos().filter((repo) => !repo.hidden), lanes(), sortByActivity()),
  );
  const hiddenRepos = createMemo(() => repos().filter((repo) => repo.hidden));
  // Everything a hidden repo owns goes with it, including its share of the urgent/running counts:
  // a badge you cannot click through to is just noise.
  const unhiddenLanes = createMemo(() => lanes().filter((lane) => !lane.repo.hidden));

  const visibleLanes = createMemo(() =>
    unhiddenLanes()
      .filter((lane) => matchesLane(lane, query()))
      .filter((lane) => !urgentOnly() || laneIndicator(lane).urgent)
      .filter((lane) => !runningOnly() || lane.agent_sessions.some((agent) => agent.status === "running"))
      .sort(byPriority),
  );

  const selectedLane = createMemo(() =>
    lanes().find((lane) => lane.id === selectedLaneId()) ?? null,
  );

  // The usage pill follows the focused agent's account rather than always the first probe.
  const focusedUsage = createMemo(() => pickFocusedUsage(usage(), selectedLane(), focusedWindow()));

  const counts = createMemo(() => ({
    urgent: unhiddenLanes().filter((lane) => laneIndicator(lane).urgent).length,
    running: unhiddenLanes().filter((lane) => lane.agent_sessions.some((agent) => agent.status === "running")).length,
  }));

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
      if (snapshot.sortReposByActivity !== null) setSortByActivity(snapshot.sortReposByActivity);
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
    loading,
    synced,
    error,
    dismissError: () => setError(null),
    visibleLanes,
    counts,
    refresh,
    start,
    stop,
    moveSelection,
  };
}

export type FleetStore = ReturnType<typeof createFleetStore>;
