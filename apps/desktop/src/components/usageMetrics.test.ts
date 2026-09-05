import { describe, expect, it } from "vitest";

import type { UsageTimeline } from "../bindings";
import {
  MAX_SERIES,
  bucketAxis,
  formatDuration,
  formatTokens,
  formatUsd,
  foldTailSeries,
  isWeekend,
  narrowerBucket,
  niceMax,
  seriesVar,
  toStackedBars,
  withContinuousAxis,
} from "./usageMetrics";

function series(key: string, points: [string, number, number][]) {
  return {
    key,
    label: key,
    points: points.map(([at, total_tokens, cost_usd]) => ({ at, total_tokens, cost_usd })),
    totals: {
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      thinking_tokens: 0,
      total_tokens: points.reduce((a, p) => a + p[1], 0),
      estimated_tokens: 0,
      cost_usd: points.reduce((a, p) => a + p[2], 0),
      events: points.length,
    },
  };
}

function timeline(...s: ReturnType<typeof series>[]): UsageTimeline {
  const buckets = [...new Set(s.flatMap((x) => x.points.map((p) => p.at)))].sort();
  return { bucket: "hour", group_by: "kind", series: s, buckets };
}

describe("usage chart reducers", () => {
  it("builds one stacked bar per bucket with a segment per series", () => {
    const bars = toStackedBars(
      timeline(
        series("claude-code", [["2026-09-01T10:00:00Z", 100, 1], ["2026-09-01T11:00:00Z", 50, 0.5]]),
        series("codex", [["2026-09-01T10:00:00Z", 40, 0.2]]),
      ),
      "tokens",
    );
    expect(bars.map((b) => b.at)).toEqual(["2026-09-01T10:00:00Z", "2026-09-01T11:00:00Z"]);
    expect(bars[0].total).toBe(140);
    expect(bars[0].segments.map((s) => s.key)).toEqual(["claude-code", "codex"]);
    expect(bars[1].segments.map((s) => s.key)).toEqual(["claude-code"]);
  });

  it("drops a zero segment so a bar never carries an invisible slice", () => {
    const bars = toStackedBars(
      timeline(
        series("claude-code", [["2026-09-01T10:00:00Z", 100, 1]]),
        series("codex", [["2026-09-01T10:00:00Z", 0, 0]]),
      ),
      "tokens",
    );
    expect(bars[0].segments).toHaveLength(1);
  });

  it("stacks cost when asked for cost", () => {
    const bars = toStackedBars(
      timeline(series("claude-code", [["2026-09-01T10:00:00Z", 100, 2.5]])),
      "cost",
    );
    expect(bars[0].total).toBeCloseTo(2.5);
  });

  it("gives every series its own palette slot in a fixed order", () => {
    expect(seriesVar(0)).toBe("var(--chart-1)");
    expect(seriesVar(MAX_SERIES - 1)).toBe(`var(--chart-${MAX_SERIES})`);
  });

  it("folds a seventh series into one Other row rather than reusing a colour", () => {
    const many = timeline(
      ...Array.from({ length: 9 }, (_, i) =>
        series(`s${i}`, [["2026-09-01T10:00:00Z", 100 - i, 1 - i / 100]]),
      ),
    );
    const folded = foldTailSeries(many);
    expect(folded.series).toHaveLength(MAX_SERIES);
    expect(folded.series[MAX_SERIES - 1].key).toBe("__other");
    expect(folded.series[MAX_SERIES - 1].label).toBe("Other (4)");
    expect(folded.series[MAX_SERIES - 1].points[0].total_tokens).toBe(95 + 94 + 93 + 92);
  });

  it("leaves a palette-sized timeline untouched", () => {
    const six = timeline(
      ...Array.from({ length: 6 }, (_, i) => series(`s${i}`, [["2026-09-01T10:00:00Z", 10, 1]])),
    );
    expect(foldTailSeries(six).series.map((s) => s.key)).toEqual(six.series.map((s) => s.key));
  });

  it("rounds an axis maximum up to a readable step", () => {
    expect(niceMax(0)).toBe(1);
    expect(niceMax(87)).toBe(100);
    expect(niceMax(1200)).toBe(2000);
    expect(niceMax(4.2)).toBe(5);
  });

  it("abbreviates token counts and keeps small costs legible", () => {
    expect(formatTokens(945)).toBe("945");
    expect(formatTokens(12_400)).toBe("12.4k");
    expect(formatTokens(4_200_000)).toBe("4.2M");
    expect(formatUsd(12.487)).toBe("$12.49");
    expect(formatUsd(0.0031)).toBe("$0.0031");
    expect(formatUsd(0)).toBe("$0");
  });

  it("carries a token count up to the next unit rather than printing a trailing zero", () => {
    expect(formatTokens(1_000)).toBe("1k");
    expect(formatTokens(1_000_000)).toBe("1M");
    expect(formatTokens(12_580_000_000)).toBe("12.6B");
  });

  it("prints whole dollars above a thousand and cents below a hundred", () => {
    expect(formatUsd(12_580.4)).toBe("$12,580");
    expect(formatUsd(523.45)).toBe("$523.5");
    expect(formatUsd(12.5)).toBe("$12.50");
  });

  it("drops an empty unit from a duration so three hours reads as three hours", () => {
    expect(formatDuration(3 * 3_600_000)).toBe("3h");
    expect(formatDuration(3 * 3_600_000 + 20 * 60_000)).toBe("3h 20m");
    expect(formatDuration(45 * 60_000)).toBe("45m");
    expect(formatDuration(0)).toBe("");
  });

  it("enumerates every bucket in a window, including the empty ones", () => {
    const axis = bucketAxis("2026-09-05T10:07:00Z", "2026-09-05T13:00:00Z", "hour");
    expect(axis).toEqual([
      "2026-09-05T10:00:00.000Z",
      "2026-09-05T11:00:00.000Z",
      "2026-09-05T12:00:00.000Z",
      "2026-09-05T13:00:00.000Z",
    ]);
  });

  it("gives a sparse timeline a continuous axis so three busy hours do not fill the plot", () => {
    // The daemon writes "…:00Z"; a generated axis writes "…:00.000Z". Same bucket, so the axis
    // must match on the instant rather than on the text.
    const sparse = timeline(
      series("claude-code", [
        ["2026-09-05T10:00:00Z", 10, 1],
        ["2026-09-05T13:00:00Z", 20, 2],
      ]),
    );
    const filled = withContinuousAxis(sparse, "2026-09-05T10:00:00Z", "2026-09-05T13:00:00Z");
    expect(filled.buckets).toHaveLength(4);
    const bars = toStackedBars(filled, "tokens");
    expect(bars.map((bar) => bar.total)).toEqual([10, 0, 0, 20]);
    expect(bars[1].segments).toHaveLength(0);
  });

  it("knows which day buckets are weekends and where a click can narrow to", () => {
    expect(isWeekend("2026-09-05T00:00:00Z")).toBe(true);
    expect(isWeekend("2026-09-07T00:00:00Z")).toBe(false);
    expect(narrowerBucket("day")).toBe("hour");
    expect(narrowerBucket("hour")).toBe("quarter");
    expect(narrowerBucket("quarter")).toBeNull();
  });
});
