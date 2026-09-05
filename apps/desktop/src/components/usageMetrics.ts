/**
 * Pure reducers behind the usage chart. Kept out of the component so the shapes a chart depends on
 * (bucket order, stacking, the palette assignment, axis rounding) are testable without a DOM.
 */
import type { UsageBucket, UsageSeries, UsageTimeline } from "../bindings";

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

function pointValue(series: UsageSeries, at: string, metric: UsageMetric): number {
  const point = series.points.find((p) => p.at === at);
  if (!point) return 0;
  return metric === "cost" ? point.cost_usd : point.total_tokens;
}

/** One stacked bar per bucket, segments in series order, zero segments dropped. */
export function toStackedBars(timeline: UsageTimeline, metric: UsageMetric): StackedBar[] {
  return timeline.buckets.map((at) => {
    const segments: StackSegment[] = [];
    timeline.series.forEach((s, index) => {
      const value = pointValue(s, at, metric);
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
  const points = timeline.buckets
    .map((at) => ({
      at,
      total_tokens: tail.reduce((a, s) => a + pointValue(s, at, "tokens"), 0),
      cost_usd: tail.reduce((a, s) => a + pointValue(s, at, "cost"), 0),
    }))
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

/** Token counts, abbreviated so an axis label stays short. */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${Math.round(n)}`;
}

/** Dollars, with enough places that a cent-scale figure is still readable. */
export function formatUsd(n: number): string {
  if (n === 0) return "$0";
  if (n >= 1) return `$${n.toFixed(2)}`;
  return `$${n.toFixed(4)}`;
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
