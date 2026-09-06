import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import type { UsageTimeline } from "../bindings";
import {
  bucketLabel,
  bucketSpanLabel,
  foldTailSeries,
  formatTokens,
  formatUsd,
  isWeekend,
  narrowerBucket,
  niceMax,
  seriesVar,
  toStackedBars,
  type UsageMetric,
} from "./usageMetrics";

interface UsageChartProps {
  timeline: UsageTimeline | null;
  metric: UsageMetric;
  onChangeMetric: (metric: UsageMetric) => void;
  /// Narrow the window to one bucket. Absent when there is nothing narrower to show.
  onNarrow?: (bucketStart: string) => void;
}

/** Plot geometry, in real pixels. The chart is measured rather than stretched. */
const PLOT = { height: 216, top: 10, bottom: 26, left: 56, right: 10 };
/** The widest a bar is drawn, however few buckets share the plot. */
const MAX_BAR_WIDTH = 28;
/** The gap kept between one bar and the next, so a dense axis still reads as separate bars. */
const MIN_BAR_GAP = 3;
/** The gap between stacked segments, so two touching series never read as one block. */
const SEGMENT_GAP = 2;
/** The rounded cap on the top of each bar. */
const BAR_RADIUS = 4;
/** The width the chart assumes before it has been measured. */
const FALLBACK_WIDTH = 960;
/** Past this many buckets the bars stop being individual tab stops. */
const MAX_FOCUSABLE_BARS = 60;
/** Keep the readout inside the measured chart even when a series has a long name. */
const TOOLTIP_WIDTH = 256;

/**
 * Tokens or cost over time, stacked by group.
 *
 * Bars rather than an area: a bucket is a discrete period of work, and the question the chart
 * answers is "how much, when", which is magnitude, not a continuous trend. Every bucket in the
 * window is drawn, empty ones included, so the axis is a timeline rather than a list of the hours
 * that happened to be busy. Identity never rests on colour alone: every series is in the legend,
 * in the hover readout, and in the breakdown table below with the same colour beside its name.
 */
export default function UsageChart(props: UsageChartProps) {
  const [hover, setHover] = createSignal<number | null>(null);
  const [width, setWidth] = createSignal(FALLBACK_WIDTH);
  // Clicking a legend entry isolates that series; clicking it again puts the rest back.
  const [isolated, setIsolated] = createSignal<string | null>(null);
  let frameRef: HTMLDivElement | undefined;

  onMount(() => {
    if (!frameRef || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const measured = entries[0]?.contentRect.width ?? 0;
      if (measured > 0) setWidth(measured);
    });
    observer.observe(frameRef);
    onCleanup(() => observer.disconnect());
  });

  const folded = createMemo(() => (props.timeline ? foldTailSeries(props.timeline) : null));
  const bars = createMemo(() => {
    const t = folded();
    if (!t) return [];
    const only = isolated();
    if (only === null) return toStackedBars(t, props.metric);
    return toStackedBars(t, props.metric).map((bar) => {
      const segments = bar.segments.filter((segment) => segment.key === only);
      return {
        ...bar,
        segments,
        total: segments.reduce((sum, segment) => sum + segment.value, 0),
      };
    });
  });
  const peak = createMemo(() => Math.max(...bars().map((b) => b.total), 0));
  const max = createMemo(() => niceMax(peak()));
  const format = (v: number) => (props.metric === "cost" ? formatUsd(v) : formatTokens(v));

  const plotWidth = () => Math.max(80, width() - PLOT.left - PLOT.right);
  const plotHeight = () => PLOT.height - PLOT.top - PLOT.bottom;
  const slot = () => plotWidth() / Math.max(bars().length, 1);
  const barWidth = () => Math.max(2, Math.min(MAX_BAR_WIDTH, slot() - MIN_BAR_GAP));
  const scale = (v: number) => (v / max()) * plotHeight();
  const slotX = (index: number) => PLOT.left + slot() * index;
  const barX = (index: number) => slotX(index) + (slot() - barWidth()) / 2;
  const baseline = () => PLOT.top + plotHeight();

  /** Gridlines at quarters of the axis maximum: enough to read a value, quiet enough to ignore. */
  const gridlines = () => [0, 0.25, 0.5, 0.75, 1].map((fraction) => max() * fraction);

  /** Show at most eight x labels, evenly sampled, so they never collide. */
  const labelStride = () => Math.max(1, Math.ceil(bars().length / 8));

  const bucket = () => folded()?.bucket ?? "hour";
  const canNarrow = () => Boolean(props.onNarrow) && narrowerBucket(bucket()) !== null;
  const hoveredBar = () => {
    const index = hover();
    return index === null ? undefined : bars()[index];
  };

  const summaryLabel = () => {
    const t = folded();
    if (!t) return "Usage over time";
    return `Usage over time by ${t.group_by}, ${bars().length} buckets, peak ${format(peak())}`;
  };

  function narrow(at: string) {
    if (canNarrow()) props.onNarrow?.(at);
  }

  return (
    <div>
      <div class="mb-1.5 flex items-center justify-between gap-3">
        <h2 class="section-label">Over time</h2>
        <div
          class="flex items-center rounded-lg border border-line bg-surface p-0.5"
          role="group"
          aria-label="Chart measure"
        >
          <For
            each={
              [
                { id: "cost", label: "Cost" },
                { id: "tokens", label: "Tokens" },
              ] as { id: UsageMetric; label: string }[]
            }
          >
            {(option) => (
              <button
                type="button"
                class={`focus-ring rounded-md px-2 py-0.5 text-xs transition-colors ${
                  props.metric === option.id
                    ? "bg-signal/12 font-semibold text-signal"
                    : "text-muted hover:text-foreground"
                }`}
                aria-pressed={props.metric === option.id}
                onClick={() => props.onChangeMetric(option.id)}
              >
                {option.label}
              </button>
            )}
          </For>
        </div>
      </div>

      <div class="relative" ref={frameRef}>
        <svg
          class="block"
          width={width()}
          height={PLOT.height}
          viewBox={`0 0 ${width()} ${PLOT.height}`}
          role="img"
          aria-label={summaryLabel()}
          onMouseLeave={() => setHover(null)}
        >
          {/* Weekends behind everything: a quiet band, not a colour with a meaning of its own. */}
          <Show when={bucket() === "day"}>
            <For each={bars()}>
              {(bar, index) => (
                <Show when={isWeekend(bar.at)}>
                  <rect
                    x={slotX(index())}
                    y={PLOT.top}
                    width={slot()}
                    height={plotHeight()}
                    fill="var(--raised)"
                  />
                </Show>
              )}
            </For>
          </Show>

          <For each={gridlines()}>
            {(value) => (
              <g>
                <line
                  x1={PLOT.left}
                  x2={width() - PLOT.right}
                  y1={baseline() - scale(value)}
                  y2={baseline() - scale(value)}
                  stroke="var(--line)"
                  stroke-width="1"
                />
                <text
                  x={PLOT.left - 8}
                  y={baseline() - scale(value) + 4}
                  text-anchor="end"
                  fill="var(--muted)"
                  style={{ "font-size": "11px", "font-variant-numeric": "tabular-nums" }}
                >
                  {format(value)}
                </text>
              </g>
            )}
          </For>

          {/* The crosshair sits under the bars, so it never dims the thing being read. */}
          <Show when={hover() !== null}>
            <line
              x1={slotX(hover() ?? 0) + slot() / 2}
              x2={slotX(hover() ?? 0) + slot() / 2}
              y1={PLOT.top}
              y2={baseline()}
              stroke="var(--muted)"
              stroke-width="1"
              stroke-dasharray="3 3"
            />
          </Show>

          <For each={bars()}>
            {(bar, index) => {
              let offset = 0;
              return (
                <g>
                  <For each={bar.segments}>
                    {(segment, segmentIndex) => {
                      const top = offset + segment.value;
                      const y = baseline() - scale(top);
                      const rawHeight = scale(segment.value);
                      offset = top;
                      const isTop = () => segmentIndex() === bar.segments.length - 1;
                      const height = Math.max(1, rawHeight - (isTop() ? 0 : SEGMENT_GAP));
                      return (
                        <rect
                          x={barX(index())}
                          y={y}
                          width={barWidth()}
                          height={height}
                          rx={isTop() ? Math.min(BAR_RADIUS, barWidth() / 2) : 0}
                          fill={segment.color}
                        />
                      );
                    }}
                  </For>
                </g>
              );
            }}
          </For>

          <line
            x1={PLOT.left}
            x2={width() - PLOT.right}
            y1={baseline()}
            y2={baseline()}
            stroke="var(--line)"
            stroke-width="1"
          />

          <For each={bars()}>
            {(bar, index) => (
              <Show when={index() % labelStride() === 0}>
                <text
                  x={slotX(index()) + slot() / 2}
                  y={PLOT.height - 8}
                  text-anchor="middle"
                  fill="var(--muted)"
                  style={{ "font-size": "11px", "font-variant-numeric": "tabular-nums" }}
                >
                  {bucketLabel(bar.at, bucket())}
                </text>
              </Show>
            )}
          </For>

          {/* One hit band per bucket, wider than the bar it covers, so hovering is forgiving and
              an empty bucket answers for itself instead of reading as a gap in the chart. */}
          <For each={bars()}>
            {(bar, index) => (
              <rect
                x={slotX(index())}
                y={PLOT.top}
                width={slot()}
                height={plotHeight()}
                fill="transparent"
                class={canNarrow() ? "cursor-zoom-in" : undefined}
                role={canNarrow() ? "button" : undefined}
                tabindex={canNarrow() && bars().length <= MAX_FOCUSABLE_BARS ? 0 : undefined}
                aria-label={
                  canNarrow()
                    ? `Narrow to ${bucketSpanLabel(bar.at, bucket())}, ${format(bar.total)}`
                    : undefined
                }
                onMouseMove={() => setHover(index())}
                onFocus={() => setHover(index())}
                onBlur={() => setHover(null)}
                onClick={() => narrow(bar.at)}
                onKeyDown={(event) => {
                  if (event.key !== "Enter" && event.key !== " ") return;
                  event.preventDefault();
                  narrow(bar.at);
                }}
              />
            )}
          </For>
        </svg>

        <Show when={hoveredBar()}>
          {(bar) => (
            <div
              class="pointer-events-none absolute z-40 rounded-lg border border-line bg-surface p-2 text-xs shadow-lg"
              role="tooltip"
              style={{
                width: `${Math.min(TOOLTIP_WIDTH, width())}px`,
                left: `${Math.min(
                  slotX(hover() ?? 0) + slot() / 2 + 10,
                  Math.max(0, width() - TOOLTIP_WIDTH),
                )}px`,
                top: `${PLOT.top}px`,
              }}
            >
              <div class="mb-1 text-muted">{bucketSpanLabel(bar().at, bucket())}</div>
              <Show
                when={bar().segments.length > 0}
                fallback={<div class="text-foreground">No usage</div>}
              >
                <For each={[...bar().segments].reverse()}>
                  {(segment) => (
                    <div class="flex items-center gap-2 py-0.5">
                      <span
                        class="h-2 w-2 shrink-0 rounded-xs"
                        style={{ "background-color": segment.color }}
                        aria-hidden="true"
                      />
                      <span class="min-w-0 flex-1 break-words text-foreground">{segment.label}</span>
                      <span class="shrink-0 tabular-nums text-muted">{format(segment.value)}</span>
                    </div>
                  )}
                </For>
                <div class="mt-1 flex items-center gap-2 border-t border-line pt-1">
                  <span class="min-w-0 flex-1 text-muted">Total</span>
                  <span class="tabular-nums font-semibold text-foreground">
                    {format(bar().total)}
                  </span>
                </div>
              </Show>
            </div>
          )}
        </Show>
      </div>

      <Show when={(folded()?.series.length ?? 0) > 1}>
        <div class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1">
          <For each={folded()?.series ?? []}>
            {(series, index) => {
              const only = () => isolated() === series.key;
              const dimmed = () => isolated() !== null && !only();
              return (
                <button
                  type="button"
                  class={`focus-ring flex items-center gap-1.5 rounded px-1 py-0.5 text-xs text-muted transition-opacity ${
                    dimmed() ? "opacity-40" : ""
                  }`}
                  aria-pressed={only()}
                  title={only() ? "Show every series again" : `Show only ${series.label}`}
                  onClick={() => setIsolated(only() ? null : series.key)}
                >
                  <span
                    class="h-2 w-2 rounded-xs"
                    style={{ "background-color": seriesVar(index()) }}
                    aria-hidden="true"
                  />
                  {series.label}
                </button>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
}
