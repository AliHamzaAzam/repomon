/**
 * Pure reducers behind the usage chart. Kept out of the component so the shapes a chart depends on
 * (bucket order, stacking, the palette assignment, axis rounding) are testable without a DOM.
 */
import type { UsageBucket, UsageGroupBy, UsageSeries, UsageTimeline } from "../bindings";

/**
 * How many series the categorical palette holds. Colours are assigned in this fixed order and are
 * never cycled: a seventh series folds into "Other" instead of borrowing the first series' colour,
 * which would make two different things look like the same thing.
 */
export const MAX_SERIES = 6;

/** The CSS variable carrying series `index`'s colour. */
export function seriesVar(index: number): string {
  return `var(--chart-${Math.min(index, MAX_SERIES - 1) + 1})`;
}

/**
 * What a breakdown row reads as. A blank key only ever turns up grouped by model, from a reader
 * that could not attribute a turn to one; every other grouping either always has a key or already
 * reads as "unattributed" once the daemon labels it. This is a defensive fallback should one slip
 * through regardless of which reader emitted it.
 */
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

/**
 * A series' points by the instant they start, so a bucket can be looked up by time rather than by
 * the exact text of its timestamp. The daemon writes `2026-09-05T10:00:00Z` and a generated axis
 * writes `2026-09-05T10:00:00.000Z`; those are the same bucket, and matching on the string would
 * quietly draw every bar as empty.
 */
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

/**
 * Keep the largest {@link MAX_SERIES} minus one series and sum the rest into a single "Other" row,
 * so the palette is never exhausted. Series arrive ordered by cost, so the tail is the cheap end.
 */
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

/**
 * Token counts as k, M or B with one decimal and no trailing ".0", so "12580.0M" reads "12.6B".
 * Mirrors `repomon_core::usage_ledger::tokens`, which formats the same numbers for the CLI.
 */
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

/**
 * Dollars: whole dollars above a thousand, cents below a hundred, and enough places below a cent
 * that a fraction of one still reads as a number. Mirrors `repomon_core::usage_ledger::money`.
 */
export function formatUsd(n: number): string {
  if (n === 0) return "$0";
  const sign = n < 0 ? "-" : "";
  const size = Math.abs(n);
  if (size >= 1000) return `${sign}$${Math.round(size).toLocaleString("en-US")}`;
  if (size >= 100) return `${sign}$${size.toFixed(1)}`;
  if (size >= 0.01) return `${sign}$${size.toFixed(2)}`;
  return `${sign}$${size.toFixed(4)}`;
}

/** How long something took, with an empty unit dropped so "3h 0m" reads "3h". */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "under a minute";
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

/**
 * The start of the bucket `ms` falls in. The daemon floors buckets in UTC, and every bucket length
 * divides a UTC day evenly, so flooring the epoch lands on exactly the same instants.
 */
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

/**
 * Replace a timeline's axis with every bucket in `[from, to]`, so a window with three busy hours
 * in it still draws as a continuous day rather than three bars stretched across the plot.
 */
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

/**
 * A lane repomon named reads as that name, unchanged. A session outside every lane has nothing but
 * a raw working-directory path to show, and a full absolute path (`/Users/.../some-long-repo-name`)
 * both crowds out every other column and tells the operator nothing a shorter form would not: the
 * last two segments are shown instead, tagged "external" so a bare path never reads as if repomon
 * had named it, with the full path kept in the tooltip.
 */
export function laneCell(row: { lane_label: string | null; cwd: string | null }): LaneCell {
  if (row.lane_label) {
    return { label: row.lane_label, title: row.cwd ?? row.lane_label, external: false };
  }
  if (row.cwd) {
    return { label: pathTail(row.cwd), title: row.cwd, external: true };
  }
  return { label: "unknown", title: "unknown", external: true };
}

/** Which of the sessions table's narrower columns fit at a given table width. */
export interface SessionColumnVisibility {
  tools: boolean;
  retries: boolean;
}

/**
 * Below this width Tools is the first column to go: it is the least-consulted of the counters, and
 * the one most redundant with Turns. Below the narrower threshold Retries goes too, leaving Task,
 * Agent, Lane and the three cost-relevant columns (Time, Tokens, Cost): what is being asked of the
 * fleet and what it costs, which is what the view exists to answer.
 */
const HIDE_TOOLS_BELOW_PX = 720;
const HIDE_RETRIES_BELOW_PX = 580;

export function sessionColumnVisibility(tableWidth: number): SessionColumnVisibility {
  return {
    tools: tableWidth >= HIDE_TOOLS_BELOW_PX,
    retries: tableWidth >= HIDE_RETRIES_BELOW_PX,
  };
}

/**
 * A session's start and duration on one line, for the sessions table's expanded row: "Sep 5,
 * 10:00 AM · 30m" rather than a locale timestamp followed by a separately-worded "for 30m", which
 * ran long enough to make the Window detail the widest cell in the row.
 */
export function windowLine(startedAt: string | null, duration: string): string {
  if (!startedAt) return "unknown";
  const start = new Date(startedAt);
  if (Number.isNaN(start.getTime())) return "unknown";
  const day = start.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const time = start.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  return duration ? `${day}, ${time} · ${duration}` : `${day}, ${time}`;
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
