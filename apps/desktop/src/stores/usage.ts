/**
 * The Usage view's data. One store owns the window (range, grouping, bucket), fetches the four
 * reads together, and guards them with a monotonic token so a slow answer can never overwrite a
 * newer one.
 *
 * A window is either a named range the daemon resolves ("today", "7 days", "30 days") or an
 * explicit `[since, until]` pair. Narrowing into a bar produces the latter, and the windows it
 * came from are kept as a trail so the operator can step back out.
 *
 * The source is injectable so the store's behaviour is testable without a daemon.
 */
import { createEffect, createMemo, createSignal, onCleanup } from "solid-js";

import type {
  RatesStatus,
  UsageBucket,
  UsageFinding,
  UsageGroupBy,
  UsageRange,
  UsageSessionRow,
  UsageStatus,
  UsageSummary,
  UsageTimeline,
} from "../bindings";
import { daemonCall, subscribeDaemon, type DaemonEvent } from "../ipc/rpc";
import { BUCKET_MS, narrowerBucket } from "../components/usageMetrics";

/** What an export produced. */
export interface UsageExport {
  path: string;
  events: number;
  bytes: number;
}

/** What one on-demand scan did. */
export interface UsageIngestReport {
  listed: number;
  scanned: number;
  events: number;
  failed: number;
  /** Session headlines rewritten because their stored digest predated the current extractor. */
  redigested: number;
}

/** The window a read covers: a named range, or `custom` with both bounds spelled out. */
export interface UsageWindowParams {
  range: UsageRange;
  since?: string;
  until?: string;
}

/** One window on the trail, with the name the breadcrumb shows for it. */
export interface UsageWindowStep extends UsageWindowParams {
  label: string;
  bucket: UsageBucket;
}

export interface UsageSource {
  summary(p: UsageWindowParams & { group_by: UsageGroupBy }): Promise<UsageSummary>;
  timeline(
    p: UsageWindowParams & { group_by: UsageGroupBy; bucket: UsageBucket },
  ): Promise<UsageTimeline>;
  sessions(p: UsageWindowParams & { limit: number }): Promise<UsageSessionRow[]>;
  findings(p: UsageWindowParams): Promise<UsageFinding[]>;
  status(): Promise<UsageStatus>;
  exportRows(p: UsageWindowParams & { format: "csv" | "json" }): Promise<UsageExport>;
  ingestNow(): Promise<UsageIngestReport>;
  /** Where prices come from right now: source counts, LiteLLM freshness, last fetch error. */
  rates(): Promise<RatesStatus>;
  /** Force an immediate LiteLLM fetch, bypassing the daily cadence. */
  refreshRates(): Promise<RatesStatus>;
  subscribe?(onEvent: (event: DaemonEvent) => void): Promise<() => void>;
}

export const daemonUsageSource: UsageSource = {
  summary: (p) => daemonCall("usage.summary", p),
  timeline: (p) => daemonCall("usage.timeline", p),
  sessions: (p) => daemonCall("usage.sessions", p),
  findings: (p) => daemonCall("usage.findings", p),
  status: () => daemonCall("usage.status"),
  exportRows: (p) => daemonCall("usage.export", p),
  ingestNow: () => daemonCall("usage.ingest_now"),
  rates: () => daemonCall("usage.rates"),
  refreshRates: () => daemonCall("usage.refresh_rates"),
  subscribe: subscribeDaemon,
};

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** How many session rows the table asks for. */
const SESSION_LIMIT = 200;

/** Which column the sessions table is ordered by. */
export type SessionSort = "recent" | "cost" | "tokens" | "time" | "retries";

/** The names the range buttons carry, so the header and the breadcrumb agree on them. */
const RANGE_LABELS: Record<UsageRange, string> = {
  today: "Today",
  week: "7 days",
  month: "30 days",
  custom: "Custom",
};

/**
 * Midnight `back` days before the day `at` falls in, in the browser's own zone.
 *
 * Day arithmetic goes through the calendar rather than through milliseconds, so a day that a
 * daylight-saving change made 23 or 25 hours long still counts as one day.
 */
function localMidnight(at: Date, back = 0): Date {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate() - back);
}

/**
 * The `[from, to]` a window covers. A day-aligned range means local calendar days: "today" is the
 * day the operator is having, which is the day the agent CLIs report their own totals over.
 *
 * This is also what the store sends the daemon for a named range, so the two never disagree about
 * where a day starts.
 */
export function resolveWindow(step: UsageWindowParams, now = new Date()): { from: Date; to: Date } {
  if (step.since && step.until) {
    const since = new Date(step.since);
    const until = new Date(step.until);
    return since <= until ? { from: since, to: until } : { from: until, to: since };
  }
  const back = step.range === "week" ? 6 : step.range === "month" ? 29 : 0;
  return { from: localMidnight(now, back), to: now };
}

/** The bucket a window of this length reads best at. */
function bucketFor(range: UsageRange, from: Date, to: Date): UsageBucket {
  if (range === "today") return "hour";
  if (range !== "custom") return "day";
  const span = to.getTime() - from.getTime();
  if (span <= 6 * BUCKET_MS.hour) return "quarter";
  if (span <= 3 * BUCKET_MS.day) return "hour";
  return "day";
}

/** How a custom window is named in the header and the breadcrumb. */
export function windowLabel(from: Date, to: Date): string {
  const day = (d: Date) => d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const time = (d: Date) => d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  if (from.toDateString() !== to.toDateString()) return `${day(from)} to ${day(to)}`;
  if (to.getTime() - from.getTime() >= BUCKET_MS.day - 1) return day(from);
  return `${day(from)}, ${time(from)} to ${time(to)}`;
}

export function createUsageStore(source: UsageSource = daemonUsageSource) {
  const [step, setStep] = createSignal<UsageWindowStep>({
    range: "week",
    label: RANGE_LABELS.week,
    bucket: "day",
  });
  // Where narrowing came from, oldest first. Empty whenever a range is picked outright.
  const [trail, setTrail] = createSignal<UsageWindowStep[]>([]);
  const [groupBy, setGroupBySignal] = createSignal<UsageGroupBy>("kind");
  const [query, setQuery] = createSignal("");
  const [sort, setSortSignal] = createSignal<SessionSort>("recent");
  const [summary, setSummary] = createSignal<UsageSummary | null>(null);
  const [timeline, setTimeline] = createSignal<UsageTimeline | null>(null);
  const [sessions, setSessions] = createSignal<UsageSessionRow[]>([]);
  const [findings, setFindings] = createSignal<UsageFinding[]>([]);
  const [status, setStatus] = createSignal<UsageStatus | null>(null);
  const [rates, setRates] = createSignal<RatesStatus | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [scanning, setScanning] = createSignal(false);
  const [ratesRefreshing, setRatesRefreshing] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [lastExport, setLastExport] = createSignal<UsageExport | null>(null);

  // A named range is sent as the two instants it resolves to here, not as its name alone: the
  // daemon would resolve the name in its own zone, and only the client knows the one the operator
  // is reading in. The name still travels, as the label the window is known by.
  const params = (): UsageWindowParams => {
    const current = step();
    if (current.range === "custom") {
      return { range: "custom", since: current.since, until: current.until };
    }
    const { from, to } = resolveWindow({ range: current.range });
    return { range: current.range, since: from.toISOString(), until: to.toISOString() };
  };

  // Every load carries a token; only the newest one may write to the signals. Without this a slow
  // "30 days" answer arriving after a fast "today" would repaint the view with the wrong window.
  let loadToken = 0;

  async function refresh() {
    const token = ++loadToken;
    setLoading(true);
    setError(null);
    try {
      const window = params();
      const [nextSummary, nextTimeline, nextSessions, nextFindings, nextStatus, nextRates] =
        await Promise.all([
          source.summary({ ...window, group_by: groupBy() }),
          source.timeline({ ...window, group_by: groupBy(), bucket: step().bucket }),
          source.sessions({ ...window, limit: SESSION_LIMIT }),
          source.findings(window).catch(() => [] as UsageFinding[]),
          source.status().catch(() => null),
          source.rates().catch(() => null),
        ]);
      if (token !== loadToken) return;
      setSummary(nextSummary);
      setTimeline(nextTimeline);
      setSessions(nextSessions);
      setFindings(nextFindings);
      setStatus(nextStatus);
      setRates(nextRates);
    } catch (cause) {
      if (token !== loadToken) return;
      setError(message(cause));
    } finally {
      if (token === loadToken) setLoading(false);
    }
  }

  onCleanup(() => { loadToken += 1; });

  // Poll only while a recount is pending. A newer window load or a disposed store owns the
  // result, so a slow status response cannot repaint that newer state.
  createEffect(() => {
    if ((status()?.stale_sources ?? 0) <= 0) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      const token = loadToken;
      try {
        const next = await source.status();
        if (!active || token !== loadToken) return;
        setStatus(next);
        if (next.stale_sources === 0) await refresh();
      } catch {
        // A transient status failure leaves the last progress visible until the next poll.
      } finally {
        if (active) timer = setTimeout(() => void poll(), 3000);
      }
    };
    timer = setTimeout(() => void poll(), 3000);
    onCleanup(() => { active = false; clearTimeout(timer); });
  });

  const reload = () => {
    void refresh();
  };

  const resolved = createMemo(() => resolveWindow(step()));

  function goTo(next: UsageWindowStep, keepTrail: UsageWindowStep[]) {
    setTrail(keepTrail);
    setStep(next);
    reload();
  }

  return {
    /** The window being read right now. */
    step,
    /** The windows narrowing came from, oldest first. */
    trail,
    range: () => step().range,
    bucket: () => step().bucket,
    /** The dates the current window covers, resolved the way the daemon resolves them. */
    resolved,
    groupBy,
    query,
    sort,
    summary,
    timeline,
    sessions,
    findings,
    status,
    rates,
    loading,
    scanning,
    ratesRefreshing,
    error,
    lastExport,
    setQuery,
    setSort(next: SessionSort) {
      setSortSignal(next);
    },
    /** Pick a named range. A named range is a fresh start, so the trail is dropped. */
    setRange(next: UsageRange) {
      if (next === step().range && trail().length === 0) return;
      const now = new Date();
      const { from, to } = resolveWindow({ range: next }, now);
      goTo({ range: next, label: RANGE_LABELS[next], bucket: bucketFor(next, from, to) }, []);
    },
    /**
     * Pick an explicit window from the date picker or a preset. Clamped so the window can never
     * reach into the future: whatever the caller asks for, the end lands at latest on "now" and
     * the start at latest on that clamped end, so a stray future date never becomes a read the
     * daemon has no data for and the header never promises tomorrow's numbers.
     */
    setCustomRange(since: Date, until: Date, label?: string) {
      const [start, end] = since <= until ? [since, until] : [until, since];
      const now = new Date();
      const clampedEnd = end > now ? now : end;
      const clampedStart = start > clampedEnd ? clampedEnd : start;
      goTo(
        {
          range: "custom",
          since: clampedStart.toISOString(),
          until: clampedEnd.toISOString(),
          label: label ?? windowLabel(clampedStart, clampedEnd),
          bucket: bucketFor("custom", clampedStart, clampedEnd),
        },
        [],
      );
    },
    /**
     * Narrow into one bar: a day opens as hours, an hour as quarter hours. The window it came
     * from goes on the trail, so the breadcrumb can put it back.
     */
    narrowTo(bucketStart: string) {
      const current = step();
      const finer = narrowerBucket(current.bucket);
      if (!finer) return;
      const from = new Date(bucketStart);
      const to = new Date(from.getTime() + BUCKET_MS[current.bucket]);
      goTo(
        {
          range: "custom",
          since: from.toISOString(),
          until: to.toISOString(),
          label: windowLabel(from, to),
          bucket: finer,
        },
        [...trail(), current],
      );
    },
    /** Step back out to a window on the trail, dropping everything narrower than it. */
    backTo(index: number) {
      const path = trail();
      const target = path[index];
      if (!target) return;
      goTo(target, path.slice(0, index));
    },
    setGroupBy(next: UsageGroupBy) {
      if (next === groupBy()) return;
      setGroupBySignal(next);
      reload();
    },
    setBucket(next: UsageBucket) {
      if (next === step().bucket) return;
      setStep({ ...step(), bucket: next });
      reload();
    },
    refresh,
    /** Sessions matching the search text and the chosen order, sorted locally so typing is free. */
    visibleSessions: createMemo(() => {
      const needle = query().trim().toLowerCase();
      const matched = !needle
        ? sessions()
        : sessions().filter((s) =>
            [s.headline ?? "", s.session_id, s.agent_kind, s.model, s.lane_label ?? "", s.cwd ?? ""]
              .join(" ")
              .toLowerCase()
              .includes(needle),
          );
      return [...matched].sort(compareSessions(sort()));
    }),
    async exportRows(format: "csv" | "json") {
      setError(null);
      try {
        setLastExport(await source.exportRows({ ...params(), format }));
      } catch (cause) {
        setError(message(cause));
      }
    },
    /** Read the transcripts now, and reload only if that found something. */
    async ingestNow() {
      setScanning(true);
      setError(null);
      try {
        const report = await source.ingestNow();
        // A redigest-only pass adds no events but can still rewrite headlines the table shows.
        if (report.events > 0 || report.redigested > 0) await refresh();
        else setStatus(await source.status().catch(() => status()));
        return report;
      } catch (cause) {
        setError(message(cause));
        return null;
      } finally {
        setScanning(false);
      }
    },
    /**
     * Force an immediate LiteLLM fetch, bypassing the daily cadence, then reload the window so
     * the summary and sessions tables re-price against whatever just changed.
     */
    async refreshRates() {
      setRatesRefreshing(true);
      try {
        setRates(await source.refreshRates());
        await refresh();
      } catch (cause) {
        setError(message(cause));
      } finally {
        setRatesRefreshing(false);
      }
    },
    subscribe: source.subscribe,
  };
}

/** How long a session ran, in milliseconds, or zero when it has no bounds. */
export function sessionDurationMs(row: UsageSessionRow): number {
  if (!row.started_at || !row.ended_at) return 0;
  const ms = new Date(row.ended_at).getTime() - new Date(row.started_at).getTime();
  return Number.isFinite(ms) && ms > 0 ? ms : 0;
}

/** Order the sessions table. Every column but "recent" is descending: the biggest is the story. */
function compareSessions(by: SessionSort): (a: UsageSessionRow, b: UsageSessionRow) => number {
  switch (by) {
    case "cost":
      return (a, b) => b.totals.cost_usd - a.totals.cost_usd;
    case "tokens":
      return (a, b) => b.totals.total_tokens - a.totals.total_tokens;
    case "time":
      return (a, b) => sessionDurationMs(b) - sessionDurationMs(a);
    case "retries":
      return (a, b) => b.retries - a.retries;
    default:
      return (a, b) => (b.ended_at ?? "").localeCompare(a.ended_at ?? "");
  }
}

export type UsageStore = ReturnType<typeof createUsageStore>;
