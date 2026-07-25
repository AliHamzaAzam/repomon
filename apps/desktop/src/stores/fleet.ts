import { createMemo, createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";

import type { AccountUsage, Lane, Repo } from "../bindings";
import { daemonCall, subscribeDaemon, type DaemonEvent } from "../ipc/rpc";

export interface FleetSnapshot {
  repos: Repo[];
  lanes: Lane[];
  usage: AccountUsage[];
  terminals: Array<{ lane_id: number; id: string }>;
}

export interface FleetSource {
  load(): Promise<FleetSnapshot>;
  subscribe(onEvent: (event: DaemonEvent) => void): Promise<() => void>;
}

export const daemonFleetSource: FleetSource = {
  async load() {
    const [repos, lanes, usage, terminals] = await Promise.all([
      daemonCall("repo.list"),
      daemonCall("lane.list"),
      daemonCall("usage.get").catch(() => []),
      daemonCall("terminal.list_all").catch(() => []),
    ]);
    return { repos, lanes, usage, terminals };
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
  if (agents.some((agent) => agent.stale)) {
    return { label: "stalled", tone: "fault", urgent: true };
  }
  if (agents.some((agent) => agent.status === "rate-limited")) {
    return { label: "limited", tone: "fault", urgent: true };
  }
  if (agents.some((agent) => !agent.inferred && agent.status === "waiting")) {
    return { label: `needs you${gate}`, tone: "attention", urgent: true };
  }
  if (agents.some((agent) => agent.external)) {
    return { label: "external", tone: "muted", urgent: false };
  }
  if (agents.some((agent) => !agent.inferred && agent.status === "running")) {
    return { label: `${agents.length > 1 ? `${agents.length} running` : "running"}${gate}`, tone: "signal", urgent: false };
  }
  if (agents.some((agent) => agent.inferred)) {
    return { label: "active · inferred", tone: "signal", urgent: false };
  }
  return { label: agents.length ? "idle" : "open", tone: "muted", urgent: false };
}

/// The usage report for the focused lane's Claude account, matched by account key. Each agent
/// carries the config dir it runs under (`config_dir: null` = the default `~/.claude`), and each
/// usage report carries the same key (`account_key`: `"default"` for the default account, else the
/// dir path). So the pill follows whichever account the focused lane actually uses instead of always
/// showing the first probed account (which is how a `claude-work` probe leaked onto default-account
/// lanes). Returns `null` when the focused account has not been probed, rather than another
/// account's numbers; falls back to the first report only when there is no agent to attribute to.
export function pickFocusedUsage(reports: AccountUsage[], lane: Lane | null): AccountUsage | null {
  if (!reports.length) return null;
  const agent = lane?.agent_sessions.find((session) => !session.inferred) ?? lane?.agent_sessions[0];
  if (!agent) return reports[0];
  const key = agent.config_dir ?? "default";
  return reports.find((report) => report.key === key) ?? null;
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
  const [query, setQuery] = createSignal("");
  const [urgentOnly, setUrgentOnly] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  let active = false;
  let interval: ReturnType<typeof setInterval> | undefined;
  let unsubscribe: (() => void) | undefined;
  let refreshQueued = false;

  const visibleLanes = createMemo(() =>
    lanes()
      .filter((lane) => matchesLane(lane, query()))
      .filter((lane) => !urgentOnly() || laneIndicator(lane).urgent)
      .sort(byPriority),
  );

  const selectedLane = createMemo(() =>
    lanes().find((lane) => lane.id === selectedLaneId()) ?? null,
  );

  // The usage pill follows the focused lane's Claude account rather than always the first probe.
  const focusedUsage = createMemo(() => pickFocusedUsage(usage(), selectedLane()));

  const counts = createMemo(() => ({
    urgent: lanes().filter((lane) => laneIndicator(lane).urgent).length,
    running: lanes().filter((lane) => lane.agent_sessions.some((agent) => agent.status === "running")).length,
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
      setError(null);
      const current = selectedLaneId();
      if (current === null || !snapshot.lanes.some((lane) => lane.id === current)) {
        setSelectedLaneId(snapshot.lanes.sort(byPriority)[0]?.id ?? null);
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
    // Heartbeat poll. Kept at 2s (not 1s) because pushed event.* notifications already trigger a
    // coalesced refresh between beats; this is the fallback/reconciler. Each poll is a full
    // lane.list, which drives the daemon's expensive per-lane overlay, so a second client (the TUI)
    // polling in parallel doubles that cost — halving our cadence keeps the daemon load down.
    interval = setInterval(() => void refresh(), 2000);
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
    lanes,
    usage,
    focusedUsage,
    terminals,
    selectedLane,
    selectedLaneId,
    setSelectedLaneId,
    query,
    setQuery,
    urgentOnly,
    setUrgentOnly,
    loading,
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
