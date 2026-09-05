import { createMemo, createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";

import type { AccountUsage, UsageRefreshResult, UsageRefreshed, AgentSession, Lane, Repo } from "../bindings";
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

/// The repomind home's lane, flagged by the daemon when it ensures the home repo. It stays a
/// normal lane in every other respect (file RPCs, supervision and Multitasking treat it like any
/// other), but the sidebar gives it the pinned Repomind row instead of a repo group, and its
/// agents are counted there rather than in the chips and repo headers.
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

/// Whether a `waiting` session is waiting only because its turn ended: nothing is open on screen
/// and the daemon classified it as an end of turn rather than a dialog or a question.
///
/// A payload with no `attention_kind` (an older daemon) says nothing, so it is not treated as an
/// ended turn: the conservative reading keeps the operator looking rather than not.
function endedItsTurn(agent: AgentSession): boolean {
  if (agent.pending_dialog || agent.pending_prompt) return false;
  return ENDED_TURN_ATTENTION.has(agent.attention_kind ?? "");
}

/// `controller` marks an agent in the repomind home lane. A controller is a standing coordinator
/// rather than a task a human handed out: its turn ending is the normal resting state, so it
/// reads IDLE, and NEEDS YOU is kept for a pending dialog or an explicit question. A worker in a
/// project lane keeps the old reading, where an ended turn means work is waiting to be picked up.
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

/// The row's status pill speaks one short word: `needs you`, `stalled`, `decision`, `limited`,
/// `running`, `inferred`, `idle`, `external`, `exited`. Anything more specific than that (how many
/// agents share the state, whether a "running" lane's only activity is a background subagent, why
/// an inferred lane could not be identified) lives in the tooltip instead, via
/// `laneIndicatorDetail`/`laneIndicatorTitle` - never packed into the label, where it used to
/// crowd the row into truncating the lane name and branch next to it.
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

/// The headline detail that used to live in the pill label itself (how many agents share the
/// state, and whether a "running" lane's only activity is a background subagent), surfaced only
/// in the tooltip now. `inferred` gets a standing explanation here because the daemon rarely has
/// a `status_reason` for it: nothing failed, it just could not attribute the worktree's changes to
/// a session.
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

/// The lane pill's tooltip: the headline detail the label used to carry, plus the daemon's
/// reasons for the agents in it.
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

/// What the pinned Repomind row says: which lane carries the controllers, how many are in it, the
/// most urgent state among them, and how many of those want the operator.
///
/// The state deliberately reuses `agentState`'s vocabulary, so the pinned row's pill and a lane
/// pill in the groups below can never describe the same agent with two different words. A lane
/// with no live controller has `state: null`, which the row renders as "off".
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
  async function refresh() {
    if (!active) return;
    const token = ++loadToken;
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
        && ["ok", "timeout", "error"].includes(result.reason)) {
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
      refreshed: false, request_id: wait.requestId, reason: "timeout", detail: "Probe timed out", snapshot: usage(),
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
    subscriptionReady = source
      .subscribe(usageEvent)
      .then((stop) => {
        if (active) unsubscribe = stop;
        else stop();
      })
      .catch(() => undefined);
  }

  function stop() {
    active = false;
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
