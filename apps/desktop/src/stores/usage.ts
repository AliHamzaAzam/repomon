/**
 * The Usage view's data. One store owns the window (range, grouping, bucket), fetches the four
 * reads together, and guards them with a monotonic token so a slow answer can never overwrite a
 * newer one.
 *
 * The source is injectable so the store's behaviour is testable without a daemon.
 */
import { createMemo, createSignal } from "solid-js";

import type {
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
}

export interface UsageSource {
  summary(p: { range: UsageRange; group_by: UsageGroupBy }): Promise<UsageSummary>;
  timeline(p: {
    range: UsageRange;
    group_by: UsageGroupBy;
    bucket: UsageBucket;
  }): Promise<UsageTimeline>;
  sessions(p: { range: UsageRange; limit: number }): Promise<UsageSessionRow[]>;
  findings(p: { range: UsageRange }): Promise<UsageFinding[]>;
  status(): Promise<UsageStatus>;
  exportRows(p: { range: UsageRange; format: "csv" | "json" }): Promise<UsageExport>;
  ingestNow(): Promise<UsageIngestReport>;
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
  subscribe: subscribeDaemon,
};

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** How many session rows the table asks for. */
const SESSION_LIMIT = 200;

export function createUsageStore(source: UsageSource = daemonUsageSource) {
  const [range, setRangeSignal] = createSignal<UsageRange>("week");
  const [groupBy, setGroupBySignal] = createSignal<UsageGroupBy>("kind");
  const [bucket, setBucketSignal] = createSignal<UsageBucket>("hour");
  const [query, setQuery] = createSignal("");
  const [summary, setSummary] = createSignal<UsageSummary | null>(null);
  const [timeline, setTimeline] = createSignal<UsageTimeline | null>(null);
  const [sessions, setSessions] = createSignal<UsageSessionRow[]>([]);
  const [findings, setFindings] = createSignal<UsageFinding[]>([]);
  const [status, setStatus] = createSignal<UsageStatus | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [scanning, setScanning] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [lastExport, setLastExport] = createSignal<UsageExport | null>(null);

  // Every load carries a token; only the newest one may write to the signals. Without this a slow
  // "30 days" answer arriving after a fast "today" would repaint the view with the wrong window.
  let loadToken = 0;

  async function refresh() {
    const token = ++loadToken;
    setLoading(true);
    setError(null);
    try {
      const [nextSummary, nextTimeline, nextSessions, nextFindings, nextStatus] = await Promise.all([
        source.summary({ range: range(), group_by: groupBy() }),
        source.timeline({ range: range(), group_by: groupBy(), bucket: bucket() }),
        source.sessions({ range: range(), limit: SESSION_LIMIT }),
        source.findings({ range: range() }).catch(() => [] as UsageFinding[]),
        source.status().catch(() => null),
      ]);
      if (token !== loadToken) return;
      setSummary(nextSummary);
      setTimeline(nextTimeline);
      setSessions(nextSessions);
      setFindings(nextFindings);
      setStatus(nextStatus);
    } catch (cause) {
      if (token !== loadToken) return;
      setError(message(cause));
    } finally {
      if (token === loadToken) setLoading(false);
    }
  }

  const reload = () => {
    void refresh();
  };

  return {
    range,
    groupBy,
    bucket,
    query,
    summary,
    timeline,
    sessions,
    findings,
    status,
    loading,
    scanning,
    error,
    lastExport,
    setQuery,
    setRange(next: UsageRange) {
      if (next === range()) return;
      setRangeSignal(next);
      reload();
    },
    setGroupBy(next: UsageGroupBy) {
      if (next === groupBy()) return;
      setGroupBySignal(next);
      reload();
    },
    setBucket(next: UsageBucket) {
      if (next === bucket()) return;
      setBucketSignal(next);
      reload();
    },
    refresh,
    /** Sessions matching the search text, filtered locally so typing never hits the daemon. */
    visibleSessions: createMemo(() => {
      const needle = query().trim().toLowerCase();
      if (!needle) return sessions();
      return sessions().filter((s) =>
        [s.headline ?? "", s.session_id, s.agent_kind, s.model, s.cwd ?? ""]
          .join(" ")
          .toLowerCase()
          .includes(needle),
      );
    }),
    async exportRows(format: "csv" | "json") {
      setError(null);
      try {
        setLastExport(await source.exportRows({ range: range(), format }));
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
        if (report.events > 0) await refresh();
        else setStatus(await source.status().catch(() => status()));
        return report;
      } catch (cause) {
        setError(message(cause));
        return null;
      } finally {
        setScanning(false);
      }
    },
    subscribe: source.subscribe,
  };
}

export type UsageStore = ReturnType<typeof createUsageStore>;
