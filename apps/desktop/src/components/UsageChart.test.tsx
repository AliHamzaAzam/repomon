import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { UsageTimeline } from "../bindings";
import UsageChart from "./UsageChart";

const longLabel = "local-runtime-with-a-deliberately-long-identifying-name-for-layout-review";
const timeline: UsageTimeline = {
  bucket: "day",
  group_by: "kind",
  buckets: ["2026-09-05T00:00:00Z", "2026-09-06T00:00:00Z"],
  series: [{
    key: "local-runtime",
    label: longLabel,
    points: [{ at: "2026-09-06T00:00:00Z", total_tokens: 5000, cost_usd: 0 }],
    totals: {
      input_tokens: 4000, output_tokens: 1000, cache_read_tokens: 0, cache_write_tokens: 0,
      thinking_tokens: 0, total_tokens: 5000, estimated_tokens: 0, subagent_tokens: 0,
      cost_usd: 0, events: 1,
    },
  }],
};

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("UsageChart tooltip", () => {
  it.each([360, 180])("keeps a long-label readout inside a %ipx chart", (chartWidth) => {
    vi.stubGlobal("ResizeObserver", class {
      constructor(private callback: ResizeObserverCallback) {}
      observe() {
        this.callback([{ contentRect: { width: chartWidth } } as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      disconnect() {}
    });
    const { container } = render(() => (
      <UsageChart timeline={timeline} metric="tokens" onChangeMetric={() => undefined} />
    ));
    const buckets = container.querySelectorAll('rect[fill="transparent"]');
    fireEvent.mouseMove(buckets[buckets.length - 1]!);
    const tooltip = screen.getByRole("tooltip");
    const left = Number.parseFloat(tooltip.style.left);
    const width = Number.parseFloat(tooltip.style.width);
    expect(left).toBeGreaterThanOrEqual(0);
    expect(width).toBeGreaterThan(0);
    expect(left + width).toBeLessThanOrEqual(chartWidth);
    expect(tooltip).toHaveTextContent(longLabel);
    expect(tooltip).toHaveTextContent("5k");
  });
});
