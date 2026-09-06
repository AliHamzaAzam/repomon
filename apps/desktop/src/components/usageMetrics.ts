/** Computes usage-chart data independently of rendering. */
import type { RatesStatus, UsageBucket, UsageGroupBy, UsageSeries, UsageTimeline } from "../bindings";
import { formatRelativeTime } from "./relativeTime";

/** Limits distinct chart colors, folding excess series into Other instead of reusing an identity
 * color. */
export const MAX_SERIES = 6;

/** The CSS variable carrying series `index`'s colour. */
export function seriesVar(index: number): string {
  return `var(--chart-${Math.min(index, MAX_SERIES - 1) + 1})`;
}

/** Labels a breakdown row, including a defensive fallback for unattributed model keys. */
export function groupRowLabel(row: { key: string; label: string }, groupBy: UsageGroupBy): string {
  if (groupBy === "model" && row.key.trim() === "") return "unknown model";
  return row.label;
}

/** Which measure a chart stacks. */
export type UsageMetric = "tokens" | "cost";

export interface StackSegment {
  key: string;
  label: string;
  value: number;
  color: string;
}

export interface StackedBar {
  at: string;
  total: number;
  segments: StackSegment[];
}

/** Match timestamps by instant because equivalent ISO strings can differ in fractional-second
 * formatting. */
function pointsByInstant(series: UsageSeries): Map<number, { tokens: number; cost: number }> {
  const index = new Map<number, { tokens: number; cost: number }>();
  for (const point of series.points) {
    index.set(Date.parse(point.at), { tokens: point.total_tokens, cost: point.cost_usd });
  }
  return index;
}

/** One stacked bar per bucket, segments in series order, zero segments dropped. */
export function toStackedBars(timeline: UsageTimeline, metric: UsageMetric): StackedBar[] {
  const indexed = timeline.series.map(pointsByInstant);
  return timeline.buckets.map((at) => {
    const instant = Date.parse(at);
    const segments: StackSegment[] = [];
    timeline.series.forEach((s, index) => {
      const point = indexed[index].get(instant);
      const value = point ? (metric === "cost" ? point.cost : point.tokens) : 0;
      if (value <= 0) return;
      segments.push({ key: s.key, label: s.label, value, color: seriesVar(index) });
    });
    return { at, total: segments.reduce((a, s) => a + s.value, 0), segments };
  });
}

/** Folds excess cost-ordered series into Other without reusing palette identities. */
export function foldTailSeries(timeline: UsageTimeline): UsageTimeline {
  if (timeline.series.length <= MAX_SERIES) return timeline;
  const kept = timeline.series.slice(0, MAX_SERIES - 1);
  const tail = timeline.series.slice(MAX_SERIES - 1);
  const tailIndex = tail.map(pointsByInstant);
  const points = timeline.buckets
    .map((at) => {
      const instant = Date.parse(at);
      const found = tailIndex.map((index) => index.get(instant));
      return {
        at,
        total_tokens: found.reduce((a, p) => a + (p?.tokens ?? 0), 0),
        cost_usd: found.reduce((a, p) => a + (p?.cost ?? 0), 0),
      };
    })
    .filter((p) => p.total_tokens > 0 || p.cost_usd > 0);
  const sum = (pick: (s: UsageSeries) => number) => tail.reduce((a, s) => a + pick(s), 0);
  const other: UsageSeries = {
    key: "__other",
    label: `Other (${tail.length})`,
    points,
    totals: {
      input_tokens: sum((s) => s.totals.input_tokens),
      output_tokens: sum((s) => s.totals.output_tokens),
      cache_read_tokens: sum((s) => s.totals.cache_read_tokens),
      cache_write_tokens: sum((s) => s.totals.cache_write_tokens),
      thinking_tokens: sum((s) => s.totals.thinking_tokens),
      total_tokens: sum((s) => s.totals.total_tokens),
      estimated_tokens: sum((s) => s.totals.estimated_tokens),
      subagent_tokens: sum((s) => s.totals.subagent_tokens),
      cost_usd: sum((s) => s.totals.cost_usd),
      events: sum((s) => s.totals.events),
    },
  };
  return { ...timeline, series: [...kept, other] };
}

/** Round an axis maximum up to 1, 2 or 5 times a power of ten, so gridlines land on round numbers. */
export function niceMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const scaled = value / magnitude;
  const step = scaled <= 1 ? 1 : scaled <= 2 ? 2 : scaled <= 5 ? 5 : 10;
  return step * magnitude;
}

/** Formats token counts with compact suffixes consistently with the core CLI helper. */
export function formatTokens(n: number): string {
  const suffixes = ["", "k", "M", "B"];
  let value = Math.max(0, n);
  let unit = 0;
  // 999.95 rather than 1000: a value that would render as "1000.0k" belongs one unit up.
  while (value >= 999.95 && unit + 1 < suffixes.length) {
    value /= 1000;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(value)}`;
  return `${oneDecimal(value)}${suffixes[unit]}`;
}

/** One decimal place, with a trailing ".0" dropped so "3.0k" reads "3k". */
function oneDecimal(value: number): string {
  const text = value.toFixed(1);
  return text.endsWith(".0") ? text.slice(0, -2) : text;
}

/** Formats dollars consistently with the core ledger, showing nonzero sub-cent amounts as less than
 * one cent. */
export function formatUsd(n: number): string {
  if (n === 0) return "$0";
  const sign = n < 0 ? "-" : "";
  const size = Math.abs(n);
  if (size >= 1000) return `${sign}$${Math.round(size).toLocaleString("en-US")}`;
  if (size >= 100) return `${sign}$${size.toFixed(1)}`;
  if (size >= 0.01) return `${sign}$${size.toFixed(2)}`;
  return `${sign}<$0.01`;
}

/** How long something took, with an empty unit dropped so "3h 0m" reads "3h". */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "-";
  if (ms < 60_000) return "<1m";
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const restMinutes = minutes % 60;
  if (hours < 24) return restMinutes ? `${hours}h ${restMinutes}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  const restHours = hours % 24;
  return restHours ? `${days}d ${restHours}h` : `${days}d`;
}

/** How many milliseconds one bucket covers. */
export const BUCKET_MS: Record<UsageBucket, number> = {
  quarter: 15 * 60_000,
  hour: 60 * 60_000,
  day: 24 * 60 * 60_000,
};

/** Floors epoch time to the same UTC bucket boundaries as the daemon. */
export function floorToBucket(ms: number, bucket: UsageBucket): number {
  const step = BUCKET_MS[bucket];
  return Math.floor(ms / step) * step;
}

/** Every bucket start in `[from, to]`, oldest first, empty ones included. */
export function bucketAxis(from: string, to: string, bucket: UsageBucket): string[] {
  const step = BUCKET_MS[bucket];
  const start = floorToBucket(new Date(from).getTime(), bucket);
  const end = floorToBucket(new Date(to).getTime(), bucket);
  if (!Number.isFinite(start) || !Number.isFinite(end) || end < start) return [];
  // A window wider than this at the chosen bucket would draw more bars than there are pixels.
  const MAX_BUCKETS = 800;
  const count = Math.min(Math.floor((end - start) / step) + 1, MAX_BUCKETS);
  const axis: string[] = [];
  for (let index = 0; index < count; index += 1) {
    axis.push(new Date(start + index * step).toISOString());
  }
  return axis;
}

/** Fills the axis with every bucket so idle periods retain their place on the timeline. */
export function withContinuousAxis(
  timeline: UsageTimeline,
  from: string,
  to: string,
): UsageTimeline {
  const axis = bucketAxis(from, to, timeline.bucket);
  if (axis.length === 0) return timeline;
  return { ...timeline, buckets: axis };
}

/** Whether a bucket start falls on a Saturday or Sunday, which the day axis shades. */
export function isWeekend(at: string): boolean {
  const day = new Date(at).getUTCDay();
  return day === 0 || day === 6;
}

/** The finer bucket a click on `bucket` narrows to, or null when there is nowhere further down. */
export function narrowerBucket(bucket: UsageBucket): UsageBucket | null {
  if (bucket === "day") return "hour";
  if (bucket === "hour") return "quarter";
  return null;
}

/** A bucket start, formatted for the width its axis has. */
export function bucketLabel(at: string, bucket: UsageBucket): string {
  const d = new Date(at);
  if (Number.isNaN(d.getTime())) return at;
  if (bucket === "day") {
    return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  }
  return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/**
 * The Agent column's character budget keeps "claude-fable-5-1" intact.
 */
export const AGENT_ID_MAX_CHARS = 18;

/** Elides the middle beyond the character budget while preserving an identifier’s family and
 * version. */
export function truncateMiddle(text: string, max: number): string {
  if (text.length <= max) return text;
  if (max <= 1) return text.slice(0, max);
  const keep = max - 1;
  const head = Math.ceil(keep / 2);
  const tail = Math.floor(keep / 2);
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`;
}

/** Above this many retries a session's retry count is worth flagging; below it, it is routine. */
const RETRY_NOTICE_THRESHOLD = 5;

/** Selects an attention tone for repeated retries. */
export function retryTone(retries: number): "quiet" | "notice" {
  return retries >= RETRY_NOTICE_THRESHOLD ? "notice" : "quiet";
}

/** The last two "/"-separated segments of a path, or the whole path when it has fewer than that. */
export function pathTail(path: string): string {
  const parts = path.split("/").filter((part) => part.length > 0);
  return parts.length <= 2 ? parts.join("/") : parts.slice(-2).join("/");
}

/** What the sessions table's Lane cell shows for one row. */
export interface LaneCell {
  label: string;
  /** The full path, or the label itself when there is no path behind it. */
  title: string;
  /** True for a session repomon did not run: no lane, sometimes not even a known repo. */
  external: boolean;
}

/** Uses the named lane or a shortened external path with its full tooltip, leaving sessions without
 * paths unattributed. */
export function laneCell(row: { lane_label: string | null; cwd: string | null }): LaneCell {
  if (row.lane_label) {
    return { label: row.lane_label, title: row.cwd ?? row.lane_label, external: false };
  }
  if (row.cwd) {
    return { label: pathTail(row.cwd), title: row.cwd, external: true };
  }
  return { label: "unknown", title: "unknown", external: false };
}

/** Which of the sessions table's narrower columns fit at a given table width. */
export interface SessionColumnVisibility {
  subagents: boolean;
  tools: boolean;
  retries: boolean;
}

/** Hide secondary counters first as space narrows; expanded rows retain their details. */
// Pixel budgets include both 8px gutters. Task gets the remaining width, never less than 180px.
export const SESSION_WIDTHS = {
  task: 180,
  agent: 160,
  lane: 176,
  turns: 64,
  tools: 64,
  subagents: 56,
  retries: 80,
  time: 80,
  tokens: 80,
  cost: 104,
} as const;
const ESSENTIAL_SESSION_WIDTH = SESSION_WIDTHS.task + SESSION_WIDTHS.agent + SESSION_WIDTHS.lane
  + SESSION_WIDTHS.turns + SESSION_WIDTHS.time + SESSION_WIDTHS.tokens + SESSION_WIDTHS.cost;
const HIDE_RETRIES_BELOW_PX = ESSENTIAL_SESSION_WIDTH + SESSION_WIDTHS.retries;
const HIDE_TOOLS_BELOW_PX = HIDE_RETRIES_BELOW_PX + SESSION_WIDTHS.tools;
const HIDE_SUBAGENTS_BELOW_PX = HIDE_TOOLS_BELOW_PX + SESSION_WIDTHS.subagents;

/** One plan for the table's colgroup and the detail panel's grid. */
export function sessionColumnPlan(visible: SessionColumnVisibility) {
  return Object.entries(SESSION_WIDTHS)
    .filter(([id]) => !(id in visible) || visible[id as keyof SessionColumnVisibility])
    .map(([id, width]) => ({ id, width }));
}

export function sessionColumnVisibility(tableWidth: number): SessionColumnVisibility {
  return {
    subagents: tableWidth >= HIDE_SUBAGENTS_BELOW_PX,
    tools: tableWidth >= HIDE_TOOLS_BELOW_PX,
    retries: tableWidth >= HIDE_RETRIES_BELOW_PX,
  };
}

/** Returns the rounded subagent token share or null without delegation, showing any positive share
 * as at least one percent. */
export function subagentShare(totals: {
  subagent_tokens: number;
  total_tokens: number;
}): number | null {
  if (totals.subagent_tokens <= 0 || totals.total_tokens <= 0) return null;
  return Math.max(1, Math.round((totals.subagent_tokens / totals.total_tokens) * 100));
}

/** Formats session start and duration together for compact expanded-row display. */
export function windowLine(startedAt: string | null, duration: string): string {
  if (!startedAt) return "unknown";
  const start = new Date(startedAt);
  if (Number.isNaN(start.getTime())) return "unknown";
  const day = start.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const time = start.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  return duration ? `${day}, ${time} · ${duration}` : `${day}, ${time}`;
}

/** Formats rate provenance consistently with the core CLI helper while leaving the RPC
 * language-neutral. */
export function formatRatesFootnote(status: RatesStatus | null, now: number = Date.now()): string {
  if (!status) return "Rates: published API rates.";
  if (!status.enabled) {
    return `Rates: built-in only (${totalModels(status)} models). LiteLLM refresh is off ([usage] refresh_prices).`;
  }
  if (status.last_error) {
    return `Rates: LiteLLM fetch failed (${status.last_error}); using ${totalModels(status)} cached/built-in model(s).`;
  }
  let line = status.fetched_at
    ? `Rates: LiteLLM, updated ${formatRelativeTime(status.fetched_at, now)} (${status.source_counts.litellm} models)`
    : "Rates: LiteLLM not fetched yet";
  if (status.source_counts.overrides > 0) line += `, ${status.source_counts.overrides} from overrides`;
  if (status.source_counts.builtin > 0) line += `, ${status.source_counts.builtin} built-in`;
  return line;
}

function totalModels(status: RatesStatus): number {
  return status.source_counts.builtin + status.source_counts.litellm + status.source_counts.overrides;
}

/** A bucket start and end, spelled out for a tooltip heading. */
export function bucketSpanLabel(at: string, bucket: UsageBucket): string {
  const start = new Date(at);
  if (Number.isNaN(start.getTime())) return at;
  if (bucket === "day") {
    return start.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  }
  const end = new Date(start.getTime() + BUCKET_MS[bucket]);
  const time = (d: Date) => d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  return `${start.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${time(start)} to ${time(end)}`;
}
