import { For, Show, createMemo, createSignal } from "solid-js";

import type { UsageTimeline } from "../bindings";
import {
  bucketLabel,
  foldTailSeries,
  formatTokens,
  formatUsd,
  niceMax,
  seriesVar,
  toStackedBars,
  type UsageMetric,
} from "./usageMetrics";

interface UsageChartProps {
  timeline: UsageTimeline | null;
  metric: UsageMetric;
}

/** Plot geometry, in SVG user units. The chart scales to its container through a viewBox. */
const PLOT = { width: 1000, height: 210, top: 8, bottom: 26, left: 52, right: 8 };
/** The gap between stacked segments, so two touching series never read as one block. */
const SEGMENT_GAP = 2;
/** The rounded cap on the top of each bar. */
const BAR_RADIUS = 4;

/**
 * Tokens or cost over time, stacked by group.
 *
 * Bars rather than an area: a bucket is a discrete period of work, and the question the chart
 * answers is "how much, when", which is magnitude, not a continuous trend. Identity never rests on
 * colour alone: every series is in the legend, in the hover readout, and in the breakdown table
 * below with the same colour beside its name.
 */
export default function UsageChart(props: UsageChartProps) {
  const [hover, setHover] = createSignal<{ index: number; x: number; y: number } | null>(null);

  const folded = createMemo(() => (props.timeline ? foldTailSeries(props.timeline) : null));
  const bars = createMemo(() => {
    const t = folded();
    return t ? toStackedBars(t, props.metric) : [];
  });
  const max = createMemo(() => niceMax(Math.max(...bars().map((b) => b.total), 0)));
  const format = (v: number) => (props.metric === "cost" ? formatUsd(v) : formatTokens(v));

  const plotWidth = () => PLOT.width - PLOT.left - PLOT.right;
  const plotHeight = () => PLOT.height - PLOT.top - PLOT.bottom;
  const slot = () => plotWidth() / Math.max(bars().length, 1);
  const barWidth = () => Math.max(2, Math.min(28, slot() - 4));
  const scale = (v: number) => (v / max()) * plotHeight();
  const slotX = (index: number) => PLOT.left + slot() * index;
  const barX = (index: number) => slotX(index) + (slot() - barWidth()) / 2;

  /** Gridlines at zero, a half and the maximum: enough to read a value, quiet enough to ignore. */
  const gridlines = () => [0, max() / 2, max()];

  /** Show at most eight x labels, evenly sampled, so they never collide. */
  const labelStride = () => Math.max(1, Math.ceil(bars().length / 8));

  const hoveredBar = () => {
    const h = hover();
    return h ? bars()[h.index] : undefined;
  };

  const summaryLabel = () => {
    const t = folded();
    if (!t) return "Usage over time";
    return `Usage over time by ${t.group_by}, ${bars().length} buckets, peak ${format(
      Math.max(...bars().map((b) => b.total), 0),
    )}`;
  };

  return (
    <div class="relative">
      <svg
        class="block w-full"
        style={{ height: "210px" }}
        viewBox={`0 0 ${PLOT.width} ${PLOT.height}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={summaryLabel()}
      >
        <For each={gridlines()}>
          {(value) => (
            <g>
              <line
                x1={PLOT.left}
                x2={PLOT.width - PLOT.right}
                y1={PLOT.top + plotHeight() - scale(value)}
                y2={PLOT.top + plotHeight() - scale(value)}
                stroke="var(--line)"
                stroke-width="1"
                vector-effect="non-scaling-stroke"
              />
              <text
                x={PLOT.left - 8}
                y={PLOT.top + plotHeight() - scale(value) + 4}
                text-anchor="end"
                fill="var(--muted)"
                style={{ "font-size": "11px", "font-variant-numeric": "tabular-nums" }}
              >
                {format(value)}
              </text>
            </g>
          )}
        </For>

        <For each={bars()}>
          {(bar, index) => {
            let offset = 0;
            return (
              <g>
                <For each={bar.segments}>
                  {(segment, segmentIndex) => {
                    const top = offset + segment.value;
                    const y = PLOT.top + plotHeight() - scale(top);
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
                        rx={isTop() ? BAR_RADIUS : 0}
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
          x2={PLOT.width - PLOT.right}
          y1={PLOT.top + plotHeight()}
          y2={PLOT.top + plotHeight()}
          stroke="var(--line)"
          stroke-width="1"
          vector-effect="non-scaling-stroke"
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
                {bucketLabel(bar.at, folded()?.bucket ?? "hour")}
              </text>
            </Show>
          )}
        </For>

        {/* One hit band per bucket, wider than the bar it covers, so hovering is forgiving. */}
        <For each={bars()}>
          {(_, index) => (
            <rect
              x={slotX(index())}
              y={PLOT.top}
              width={slot()}
              height={plotHeight()}
              fill="transparent"
              onMouseMove={(event) =>
                setHover({ index: index(), x: event.clientX, y: event.clientY })
              }
              onMouseLeave={() => setHover(null)}
            />
          )}
        </For>
      </svg>

      <Show when={hoveredBar()}>
        {(bar) => (
          <div
            class="pointer-events-none fixed z-50 min-w-44 rounded-lg border border-line bg-surface p-2 text-xs shadow-lg"
            style={{
              left: `${Math.min((hover()?.x ?? 0) + 12, window.innerWidth - 200)}px`,
              top: `${Math.max((hover()?.y ?? 0) - 12, 8)}px`,
            }}
          >
            <div class="mb-1 text-muted">
              {bucketLabel(bar().at, folded()?.bucket ?? "hour")}
            </div>
            <For each={[...bar().segments].reverse()}>
              {(segment) => (
                <div class="flex items-center gap-2 py-0.5">
                  <span
                    class="h-2 w-2 shrink-0 rounded-xs"
                    style={{ "background-color": segment.color }}
                    aria-hidden="true"
                  />
                  <span class="min-w-0 flex-1 truncate text-foreground">{segment.label}</span>
                  <span class="tabular-nums text-muted">{format(segment.value)}</span>
                </div>
              )}
            </For>
            <div class="mt-1 flex items-center gap-2 border-t border-line pt-1">
              <span class="min-w-0 flex-1 text-muted">Total</span>
              <span class="tabular-nums font-semibold text-foreground">{format(bar().total)}</span>
            </div>
          </div>
        )}
      </Show>

      <Show when={(folded()?.series.length ?? 0) > 1}>
        <div class="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1">
          <For each={folded()?.series ?? []}>
            {(series, index) => (
              <span class="flex items-center gap-1.5 text-xs text-muted">
                <span
                  class="h-2 w-2 rounded-xs"
                  style={{ "background-color": seriesVar(index()) }}
                  aria-hidden="true"
                />
                {series.label}
              </span>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
