import { createMemo, createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";

import type { AccountUsage, UsageRefreshResult, UsageRefreshed, AgentSession, Lane, Repo } from "../bindings";
import type { UsageRefreshedReason } from "../bindings/UsageRefreshedReason";

/** Every completion reason the daemon can broadcast; a completion is never dropped on its reason. */
const REFRESHED_REASONS: readonly UsageRefreshedReason[] = [
  "ok",
  "probe_disabled",
  "no_active_kind",
  "timeout",
  "error",
];
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
  /// Today's equivalent API cost from the usage ledger, for the sidebar's rate-limits card.
  /// Optional: a source that does not read the ledger simply leaves the line off.
  costToday?: number | null;
}

export interface FleetSource {
  load(): Promise<FleetSnapshot>;
  refreshUsage(): Promise<UsageRefreshResult | void>;
  subscribe(onEvent: (event: DaemonEvent) => void): Promise<() => void>;
}

export const daemonFleetSource: FleetSource = {
  async load() {
    const [repos, lanes, usage, terminals, config, today] = await Promise.all([
      daemonCall("repo.list"),
      daemonCall("lane.list"),
      daemonCall("usage.get").catch(() => []),
      daemonCall("terminal.list_all").catch(() => []),
      daemonCall("config.get").catch(() => null),
      daemonCall("usage.summary", { range: "today", group_by: "kind" }).catch(() => null),
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
      costToday: today ? today.totals.cost_usd : null,
    };
  },
  refreshUsage: async () => {
    return daemonCall("usage.refresh");
  },
  subscribe: subscribeDaemon,
};

export type LaneTone = "attention" | "fault" | "signal" | "muted";

/// Identify the controller lane for pinned sidebar presentation while retaining ordinary lane
/// behavior.
export function isControllerLane(lane: Pick<Lane, "role">): boolean {
  return lane.role === "controller";
}

/// Whether a repo is the repomind home: every lane it owns is a controller lane. A repo with no
/// lanes at all is an ordinary empty repo, not the home.
export function isControllerRepo(repoId: number, lanes: Lane[]): boolean {
  const owned = lanes.filter((lane) => lane.repo.id === repoId);
  return owned.length > 0 && owned.every(isControllerLane);
}

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

/// Shared agent-state vocabulary derived only from daemon fields, without client-side pane parsing
/// or timers.
export type AgentState =
  | "decision"
  | "stalled"
  | "limited"
  | "needs-you"
  | "external"
  | "running"
  | "inferred"
  | "idle"
  | "exited";

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
  "exited",
];

/// Attention words (the daemon's `Attention::as_str`) that mean "this turn is over", as opposed
/// to "answer me". A session carrying one of these is waiting for its next instruction, not for
/// the operator.
const ENDED_TURN_ATTENTION: ReadonlySet<string> = new Set(["end_of_turn", "done_candidate"]);

/// Require an explicit end-of-turn classification so missing attention metadata cannot hide a
/// possible question.
function endedItsTurn(agent: AgentSession): boolean {
  if (agent.pending_dialog || agent.pending_prompt) return false;
  return ENDED_TURN_ATTENTION.has(agent.attention_kind ?? "");
}

/// Project daemon status into a shared display state, treating an ended controller turn as idle and
/// an ended worker turn as needing attention.
export function agentState(agent: AgentSession, controller = false): AgentState {
  if (agent.pending_dialog) return "decision";
  const managed = !agent.external && !agent.inferred;
  if (managed && agent.status === "running" && agent.stale) return "stalled";
  if (agent.status === "rate-limited") return "limited";
  if (managed && agent.status === "waiting" && !(controller && endedItsTurn(agent)))
    return "needs-you";
  if (agent.external) return "external";
  if (!agent.inferred && agent.status === "running") return "running";
  if (agent.inferred) return "inferred";
  if (agent.status === "ended") return "exited";
  return "idle";
}

/// [`agentState`] for an agent known to be a controller. The one entry point the pinned row, the
/// toolbar dot and the panel share, so the three can never word the same controller differently.
export function controllerAgentState(agent: AgentSession): AgentState {
  return agentState(agent, true);
}

/// States that put a lane in the "Needs you" filter, and so in its count.
const URGENT_STATES: ReadonlySet<AgentState> = new Set<AgentState>([
  "decision",
  "stalled",
  "limited",
  "needs-you",
]);

export function isUrgentState(state: AgentState): boolean {
  return URGENT_STATES.has(state);
}

/// One agent's state read in its own lane's terms: the controller reading inside the repomind
/// home, the ordinary one everywhere else.
export function agentStateIn(lane: Pick<Lane, "role">, agent: AgentSession): AgentState {
  return agentState(agent, isControllerLane(lane));
}

/// The most urgent state among a lane's agents, or null for a lane with no agents.
export function laneState(lane: Lane): AgentState | null {
  const states = new Set(lane.agent_sessions.map((agent) => agentStateIn(lane, agent)));
  return STATE_PRIORITY.find((state) => states.has(state)) ?? null;
}

/// How many of a lane's agents share the lane's headline state, so the pill can say "2 running"
/// truthfully rather than counting the whole roster.
export function laneStateCount(lane: Lane, state: AgentState): number {
  return lane.agent_sessions.filter((agent) => agentStateIn(lane, agent) === state).length;
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
  exited: "muted",
};

/// Return a short lane-state label, leaving counts and explanations to its tooltip.
export function laneIndicator(lane: Lane): LaneIndicator {
  const state = laneState(lane);
  if (state === null) return { label: "", tone: "muted", urgent: false };
  const gate = gateSuffix(lane);
  const urgent = isUrgentState(state);
  const tone = STATE_TONE[state];
  const label =
    state === "decision"
      ? `decision${gate}`
      : state === "needs-you"
        ? `needs you${gate}`
        : state === "running"
          ? `running${gate}`
          : state; // stalled, limited, idle, external, inferred, exited render as their own name
  return { label, tone, urgent };
}

/// The pill for a bare state, with no lane to read a gate count off. `null` means nothing is
/// running at all, which is what the pinned Repomind row shows as "off".
export function stateIndicator(state: AgentState | null): LaneIndicator {
  if (state === null) return { label: "off", tone: "muted", urgent: false };
  return {
    label: state === "needs-you" ? "needs you" : state,
    tone: STATE_TONE[state],
    urgent: isUrgentState(state),
  };
}

/// Why an agent's state reads the way it does, straight from the daemon. Never invented here:
/// with no reason on the payload the tooltip simply says less.
export function agentStateReason(agent: AgentSession): string | null {
  return agent.status_reason ?? null;
}

/// Explain counts, subagent-only activity, and unattributed work in the tooltip rather than
/// widening the row label.
function laneIndicatorDetail(lane: Lane, state: AgentState): string | null {
  if (state === "running") {
    const count = laneStateCount(lane, "running");
    const running = lane.agent_sessions.filter(
      (agent) => agentStateIn(lane, agent) === "running",
    );
    const onlySubagents = running.every((agent) => Boolean(agent.subagent_running));
    if (onlySubagents && count === 1) return "subagent running";
    if (count > 1) return `${count} running`;
    return null;
  }
  if (state === "inferred") {
    return "the worktree is changing but the agent behind it could not be identified";
  }
  return null;
}

/// Builds the lane tooltip from its headline and daemon-provided agent reasons.
export function laneIndicatorTitle(lane: Lane): string | undefined {
  const state = laneState(lane);
  if (state === null) return undefined;
  const detail = laneIndicatorDetail(lane, state);
  const reasons = lane.agent_sessions
    .filter((agent) => agentStateIn(lane, agent) === state)
    .map((agent) => agentStateReason(agent))
    .filter((reason): reason is string => Boolean(reason));
  const lines = detail ? [detail, ...reasons] : reasons;
  if (state === "external" && !lines.length) {
    return "External session running outside repomon. Select lane to adopt into tmux management.";
  }
  if (!lines.length) return undefined;
  return lines.join("\n");
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
      const state = agentStateIn(lane, agent);
      if (isUrgentState(state)) counts.urgent += 1;
      else if (state === "running") counts.running += 1;
      else if (state === "idle") counts.idle += 1;
    }
  }
  return counts;
}

/// Summarize the controller lane using the shared agent-state vocabulary, with null state when no
/// controller is live.
export interface ControllerSummary {
  lane: Lane | null;
  agents: number;
  state: AgentState | null;
  urgent: number;
}

export function controllerSummary(lanes: Lane[]): ControllerSummary {
  const controllers = lanes.filter(isControllerLane);
  const sessions = controllers.flatMap((lane) => lane.agent_sessions);
  const states = new Set(sessions.map(controllerAgentState));
  return {
    lane: controllers[0] ?? null,
    agents: sessions.length,
    state: STATE_PRIORITY.find((state) => states.has(state)) ?? null,
    urgent: sessions.filter((session) => isUrgentState(controllerAgentState(session))).length,
  };
}

/// Resolve the provider-specific usage account key so a null Codex config directory cannot select
/// Claude's default account.
export function accountKeyOf(session: AgentSession): string {
  if (session.agent === "codex") return "codex";
  if (session.agent === "antigravity" || session.agent === "agy") return "antigravity";
  return session.config_dir ?? "default";
}

/// Select usage for the focused session's account, returning null for an unprobed account and
/// falling back to the first report only without an attributable agent.
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

/// Sort by activity when requested, otherwise preserve the daemon's persisted repository order.
export function orderRepos(repos: Repo[], lanes: Lane[], mode: string): Repo[] {
  if (mode === "activity") return sortReposByActivity(repos, lanes, true);
  return repos;
}

/// Sort repository groups by recent lane activity with stable ties and empty groups last, leaving
/// lane order unchanged.
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

/// Assign stable session keys because duplicate wire ids would collapse nested sessions during
/// keyed reconciliation.
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
  // Keyed reconciliation preserves unchanged row identity and hover state across heartbeat updates.
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
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;

  // Refresh repository ordering on each poll to reflect changes from other clients, using the
  // boolean fallback when sort_mode is absent.
  const [sortMode, setSortMode] = createSignal<RepoSortMode>("default");
  // Mirrors the daemon's per-lane agent tab sort mode ("activity" | "manual").
  const [tabSortMode, setTabSortMode] = createSignal<"activity" | "manual">("activity");
  // The daemon keeps returning hidden repos (flagged) so we can offer a way back; everything that
  // renders the fleet works from `visibleRepos` / `visibleLanes` / `unhiddenLanes` instead.
  const visibleRepos = createMemo(() =>
    orderRepos(
      // The repomind home is pinned above the groups instead of being one of them.
      repos().filter((repo) => !repo.hidden && !isControllerRepo(repo.id, lanes())),
      lanes(),
      sortMode(),
    ),
  );
  const hiddenRepos = createMemo(() => repos().filter((repo) => repo.hidden));
  // Everything a hidden repo owns goes with it, including its share of the urgent/running counts:
  // a badge you cannot click through to is just noise.
  const unhiddenLanes = createMemo(() => lanes().filter((lane) => !lane.repo.hidden));
  // The lanes the repo groups actually render, and so the set every header count and filter chip
  // is drawn from. Controller lanes are excluded on purpose: the pinned Repomind row carries
  // their agents, and counting them twice would make a chip disagree with the rows under it.
  const fleetLanes = createMemo(() => unhiddenLanes().filter((lane) => !isControllerLane(lane)));
  const controllerLanes = createMemo(() => lanes().filter(isControllerLane));
  const controller = createMemo(() => controllerSummary(lanes()));

  // Every filter reads the same `agentState` the rows and chips do, so a chip can never select a
  // set the rows disagree with.
  const visibleLanes = createMemo(() =>
    fleetLanes()
      .filter((lane) => matchesLane(lane, query()))
      .filter((lane) => !urgentOnly() || laneIndicator(lane).urgent)
      .filter(
        (lane) =>
          !runningOnly() || lane.agent_sessions.some((agent) => agentState(agent) === "running"),
      )
      .sort(byPriority),
  );

  const selectedLane = createMemo(() =>
    lanes().find((lane) => lane.id === selectedLaneId()) ?? null,
  );

  // Today's ledger cost, shown as one line on the rate-limits card. Null until a load supplies it.
  const [costToday, setCostToday] = createSignal<number | null>(null);

  // The usage pill follows the focused agent's account rather than always the first probe.
  const focusedUsage = createMemo(() => pickFocusedUsage(usage(), selectedLane(), focusedWindow()));

  const counts = createMemo(() => fleetCounts(fleetLanes()));

  let loadToken = 0;
  let loadPromise: Promise<void> | undefined;
  let followupPromise: Promise<void> | undefined;

  function refresh(): Promise<void> {
    if (!active) return Promise.resolve();
    if (loadPromise) {
      if (!followupPromise) {
        const token = loadToken;
        const next = loadPromise.then(() => {
          if (followupPromise === next) followupPromise = undefined;
          if (active && token === loadToken) return refresh();
        });
        followupPromise = next;
      }
      return followupPromise;
    }
    const pending = load(loadToken).finally(() => {
      if (loadPromise === pending) loadPromise = undefined;
    });
    loadPromise = pending;
    return pending;
  }

  async function load(token: number) {
    setLoading(true);
    try {
      const snapshot = await source.load();
      if (!active || token !== loadToken) return;
      setRepoStore(reconcile(snapshot.repos, { key: "id" }));
      setLaneStore(reconcile(withSessionKeys(snapshot.lanes), { key: "id" }));
      setUsage(snapshot.usage);
      setCostToday(snapshot.costToday ?? null);
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
        // The repomind home is selectable (the pinned row selects it) but never the default:
        // landing there on first sync would leave every repo group unhighlighted.
        const ordinary = selectable.filter((lane) => !isControllerLane(lane));
        setSelectedLaneId([...ordinary].sort(byPriority)[0]?.id ?? null);
      }
    } catch (cause) {
      if (active && token === loadToken) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (active && token === loadToken) setLoading(false);
    }
  }

  let subscriptionReady: Promise<void> = Promise.resolve();
  type UsageWait = {
    requestId?: number;
    early: Map<number, UsageRefreshed>;
    complete: (result: UsageRefreshResult | void) => void;
    fail: (cause: unknown) => void;
    promise: Promise<UsageRefreshResult | void>;
    timer?: ReturnType<typeof setTimeout>;
  };
  let usageWait: UsageWait | undefined;

  function settleUsage(wait: UsageWait, result: UsageRefreshResult | void) {
    if (usageWait !== wait) return;
    clearTimeout(wait.timer);
    usageWait = undefined;
    wait.complete(result);
  }

  function usageEvent(event: DaemonEvent) {
    if (event.method === "event.usage.refreshed") {
      const result = event.params as UsageRefreshed | undefined;
      if (result && Number.isFinite(result.request_id) && Array.isArray(result.snapshot)
        && REFRESHED_REASONS.includes(result.reason)) {
        const wait = usageWait;
        if (wait) {
          if (wait.requestId === undefined) wait.early.set(result.request_id, result);
          else if (wait.requestId === result.request_id) {
            settleUsage(wait, { ...result, refreshed: result.reason === "ok" });
          }
        }
      }
    }
    queueRefresh();
  }

  async function refreshUsage() {
    if (!active) return;
    if (usageWait) return usageWait.promise;
    let complete!: UsageWait["complete"];
    let fail!: UsageWait["fail"];
    const promise = new Promise<UsageRefreshResult | void>((resolve, reject) => { complete = resolve; fail = reject; });
    const wait: UsageWait = { early: new Map(), complete, fail, promise };
    usageWait = wait;
    wait.timer = setTimeout(() => settleUsage(wait, {
      refreshed: false, request_id: wait.requestId, reason: "timeout", detail: "Still probing, this can take a moment", snapshot: usage(),
    }), 20_000);
    // Subscribe before requesting the probe. A fast completion may precede the RPC response;
    // buffer it until the response identifies the ticket, then ignore every other round.
    void (async () => {
      await subscriptionReady;
      if (usageWait !== wait) return;
      const result = await source.refreshUsage();
      if (usageWait !== wait) return;
      if (result?.reason === "pending") {
        wait.requestId = result.request_id;
        const early = result.request_id === undefined ? undefined : wait.early.get(result.request_id);
        wait.early.clear();
        if (early) settleUsage(wait, { ...early, refreshed: early.reason === "ok" });
      } else settleUsage(wait, result);
    })().catch((cause) => {
      if (usageWait !== wait) return;
      clearTimeout(wait.timer);
      usageWait = undefined;
      wait.fail(cause);
    });
    const result = await promise;
    await refresh();
    return result;
  }

  function queueRefresh() {
    if (refreshTimer !== undefined) return;
    refreshTimer = setTimeout(() => {
      refreshTimer = undefined;
      void refresh();
    }, 60);
  }

  function start() {
    if (active) return;
    active = true;
    const token = loadToken;
    void refresh();
    // Heartbeat poll at 1.2s cadence to ensure fast UI updates without excessive overhead.
    interval = setInterval(() => void refresh(), 1200);
    subscriptionReady = source
      .subscribe(usageEvent)
      .then((stop) => {
        if (active && token === loadToken) unsubscribe = stop;
        else stop();
      })
      .catch(() => undefined);
  }

  function stop() {
    active = false;
    loadToken += 1;
    followupPromise = undefined;
    if (refreshTimer !== undefined) clearTimeout(refreshTimer);
    refreshTimer = undefined;
    setLoading(false);
    if (usageWait) settleUsage(usageWait, undefined);
    if (interval) clearInterval(interval);
    interval = undefined;
    unsubscribe?.();
    unsubscribe = undefined;
  }

  function moveSelection(delta: number, urgent = false) {
    // The pinned Repomind row is the sidebar's first stop, so arrow navigation starts there and
    // then walks the repo groups in their rendered order.
    const candidates = [...controllerLanes(), ...visibleLanes()].filter(
      (lane) => !urgent || laneIndicator(lane).urgent,
    );
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
    fleetLanes,
    controllerLanes,
    controller,
    usage,
    focusedUsage,
    costToday,
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
