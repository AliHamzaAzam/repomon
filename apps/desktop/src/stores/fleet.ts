import { createMemo, createSignal } from "solid-js";

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

export function laneIndicator(lane: Lane): LaneIndicator {
  const agents = lane.agent_sessions;
  if (agents.some((agent) => agent.pending_dialog)) {
    return { label: "decision", tone: "attention", urgent: true };
  }
  if (agents.some((agent) => agent.stale)) {
    return { label: "stalled", tone: "fault", urgent: true };
  }
  if (agents.some((agent) => agent.status === "waiting")) {
    return { label: "needs you", tone: "attention", urgent: true };
  }
  if (agents.some((agent) => agent.status === "rate-limited")) {
    return { label: "limited", tone: "fault", urgent: true };
  }
  if (agents.some((agent) => agent.external)) {
    return { label: "external", tone: "muted", urgent: false };
  }
  if (agents.some((agent) => agent.status === "running")) {
    return { label: agents.length > 1 ? `${agents.length} running` : "running", tone: "signal", urgent: false };
  }
  return { label: agents.length ? "idle" : "open", tone: "muted", urgent: false };
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

function byPriority(a: Lane, b: Lane): number {
  if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
  const urgent = Number(laneIndicator(b).urgent) - Number(laneIndicator(a).urgent);
  if (urgent) return urgent;
  return Date.parse(b.last_activity_at) - Date.parse(a.last_activity_at);
}

export function createFleetStore(source: FleetSource = daemonFleetSource) {
  const [repos, setRepos] = createSignal<Repo[]>([]);
  const [lanes, setLanes] = createSignal<Lane[]>([]);
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
      setRepos(snapshot.repos);
      setLanes(snapshot.lanes);
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
