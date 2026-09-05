import { createRoot } from "solid-js";
import { describe, expect, it, vi } from "vitest";

import type { UsageSessionRow, UsageSummary, UsageTimeline } from "../bindings";
import { createUsageStore, type UsageSource } from "./usage";

const totals = {
  input_tokens: 100,
  output_tokens: 50,
  cache_read_tokens: 300,
  cache_write_tokens: 0,
  thinking_tokens: 0,
  total_tokens: 450,
  estimated_tokens: 0,
  cost_usd: 1.25,
  events: 3,
};

const summary: UsageSummary = {
  from: "2026-09-05T00:00:00Z",
  to: "2026-09-05T12:00:00Z",
  group_by: "kind",
  totals,
  groups: [{ key: "claude-code", label: "claude-code", totals }],
  cache_hit_rate: 0.75,
  estimated_share: 0,
  unpriced_models: [],
};

const timeline: UsageTimeline = {
  bucket: "hour",
  group_by: "kind",
  series: [
    {
      key: "claude-code",
      label: "claude-code",
      points: [{ at: "2026-09-05T10:00:00Z", total_tokens: 450, cost_usd: 1.25 }],
      totals,
    },
  ],
  buckets: ["2026-09-05T10:00:00Z"],
};

const session: UsageSessionRow = {
  session_id: "sess-1",
  agent_kind: "claude-code",
  model: "claude-sonnet-5",
  headline: "Wire up the ledger",
  headline_raw: "<local-command-caveat>ran /status</local-command-caveat>\nWire up the ledger",
  lane_label: "demo/main",
  repo_id: 1,
  lane_id: 3,
  cwd: "/repos/demo",
  started_at: "2026-09-05T10:00:00Z",
  ended_at: "2026-09-05T10:30:00Z",
  turns: 12,
  tool_calls: 40,
  retries: 0,
  totals,
  estimated: false,
  external: false,
};

function source(overrides: Partial<UsageSource> = {}): UsageSource {
  return {
    summary: vi.fn().mockResolvedValue(summary),
    timeline: vi.fn().mockResolvedValue(timeline),
    sessions: vi.fn().mockResolvedValue([session]),
    findings: vi.fn().mockResolvedValue([]),
    status: vi.fn().mockResolvedValue({
      sources: 4,
      last_scan_at: "2026-09-05T12:00:00Z",
      errors: [],
      events: 3,
      first_event_at: null,
      last_event_at: null,
      ingesting: false,
    }),
    exportRows: vi.fn().mockResolvedValue({ path: "/data/usage.csv", events: 3, bytes: 120 }),
    ingestNow: vi
      .fn()
      .mockResolvedValue({ listed: 4, scanned: 1, events: 3, failed: 0, redigested: 0 }),
    ...overrides,
  };
}

async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("usage store", () => {
  it("loads a summary, a timeline and sessions for the default range", async () => {
    await createRoot(async (dispose) => {
      const s = source();
      const store = createUsageStore(s);
      await store.refresh();
      await flush();
      expect(store.summary()?.totals.cost_usd).toBe(1.25);
      expect(store.timeline()?.series).toHaveLength(1);
      expect(store.sessions()).toHaveLength(1);
      expect(s.summary).toHaveBeenCalledWith({ range: "week", group_by: "kind" });
      dispose();
    });
  });

  it("refetches when the range, grouping or bucket changes", async () => {
    await createRoot(async (dispose) => {
      const s = source();
      const store = createUsageStore(s);
      await store.refresh();
      store.setGroupBy("model");
      await flush();
      expect(s.summary).toHaveBeenLastCalledWith({ range: "week", group_by: "model" });
      store.setRange("today");
      await flush();
      expect(s.summary).toHaveBeenLastCalledWith({ range: "today", group_by: "model" });
      store.setBucket("day");
      await flush();
      expect(s.timeline).toHaveBeenLastCalledWith({
        range: "today",
        group_by: "model",
        bucket: "day",
      });
      dispose();
    });
  });

  it("keeps the newest result when a slow request finishes after a fast one", async () => {
    await createRoot(async (dispose) => {
      let resolveSlow: (v: UsageSummary) => void = () => {};
      const slow = new Promise<UsageSummary>((r) => {
        resolveSlow = r;
      });
      const stale: UsageSummary = { ...summary, totals: { ...totals, cost_usd: 99 } };
      const s = source({
        summary: vi
          .fn()
          .mockImplementationOnce(() => slow)
          .mockResolvedValue(summary),
      });
      const store = createUsageStore(s);
      const first = store.refresh();
      const second = store.refresh();
      await second;
      resolveSlow(stale);
      await first;
      await flush();
      expect(store.summary()?.totals.cost_usd).toBe(1.25);
      dispose();
    });
  });

  it("filters sessions by the search text without refetching", async () => {
    await createRoot(async (dispose) => {
      const s = source({
        sessions: vi.fn().mockResolvedValue([
          session,
          { ...session, session_id: "sess-2", headline: "Fix the flake" },
        ]),
      });
      const store = createUsageStore(s);
      await store.refresh();
      await flush();
      store.setQuery("flake");
      expect(store.visibleSessions()).toHaveLength(1);
      expect(store.visibleSessions()[0].session_id).toBe("sess-2");
      expect(s.sessions).toHaveBeenCalledTimes(1);
      dispose();
    });
  });

  it("surfaces a failure as an error and stops loading", async () => {
    await createRoot(async (dispose) => {
      const store = createUsageStore(
        source({ summary: vi.fn().mockRejectedValue(new Error("daemon is down")) }),
      );
      await store.refresh();
      await flush();
      expect(store.error()).toContain("daemon is down");
      expect(store.loading()).toBe(false);
      dispose();
    });
  });

  it("sends explicit bounds for a custom range and names the window", async () => {
    await createRoot(async (dispose) => {
      const s = source();
      const store = createUsageStore(s);
      await store.refresh();
      store.setCustomRange(
        new Date("2026-08-01T00:00:00Z"),
        new Date("2026-08-31T23:59:59Z"),
        "Last month",
      );
      await flush();
      expect(s.summary).toHaveBeenLastCalledWith({
        range: "custom",
        since: "2026-08-01T00:00:00.000Z",
        until: "2026-08-31T23:59:59.000Z",
        group_by: "kind",
      });
      expect(store.step().label).toBe("Last month");
      dispose();
    });
  });

  it("narrows into a bucket and steps back out through the trail", async () => {
    await createRoot(async (dispose) => {
      const s = source();
      const store = createUsageStore(s);
      store.setRange("month");
      await flush();
      expect(store.bucket()).toBe("day");

      store.narrowTo("2026-09-05T00:00:00.000Z");
      await flush();
      expect(store.range()).toBe("custom");
      expect(store.bucket()).toBe("hour");
      expect(store.trail()).toHaveLength(1);
      expect(s.timeline).toHaveBeenLastCalledWith({
        range: "custom",
        since: "2026-09-05T00:00:00.000Z",
        until: "2026-09-06T00:00:00.000Z",
        group_by: "kind",
        bucket: "hour",
      });

      store.narrowTo("2026-09-05T10:00:00.000Z");
      await flush();
      expect(store.bucket()).toBe("quarter");
      expect(store.trail()).toHaveLength(2);

      store.backTo(0);
      await flush();
      expect(store.range()).toBe("month");
      expect(store.trail()).toHaveLength(0);
      dispose();
    });
  });

  it("orders the sessions table by the column that was asked for", async () => {
    await createRoot(async (dispose) => {
      const s = source({
        sessions: vi.fn().mockResolvedValue([
          { ...session, session_id: "cheap", totals: { ...totals, cost_usd: 0.5 }, retries: 7 },
          { ...session, session_id: "dear", totals: { ...totals, cost_usd: 9 }, retries: 0 },
        ]),
      });
      const store = createUsageStore(s);
      await store.refresh();
      await flush();
      store.setSort("cost");
      expect(store.visibleSessions()[0].session_id).toBe("dear");
      store.setSort("retries");
      expect(store.visibleSessions()[0].session_id).toBe("cheap");
      dispose();
    });
  });

  it("reports where an export landed", async () => {
    await createRoot(async (dispose) => {
      const store = createUsageStore(source());
      await store.exportRows("csv");
      await flush();
      expect(store.lastExport()?.path).toBe("/data/usage.csv");
      dispose();
    });
  });

  it("reloads after an on-demand scan finds new events", async () => {
    await createRoot(async (dispose) => {
      const s = source();
      const store = createUsageStore(s);
      await store.refresh();
      await store.ingestNow();
      await flush();
      expect(s.summary).toHaveBeenCalledTimes(2);
      dispose();
    });
  });

  it("does not reload when a scan found nothing new", async () => {
    await createRoot(async (dispose) => {
      const s = source({
        ingestNow: vi
          .fn()
          .mockResolvedValue({ listed: 4, scanned: 0, events: 0, failed: 0, redigested: 0 }),
      });
      const store = createUsageStore(s);
      await store.refresh();
      await store.ingestNow();
      await flush();
      expect(s.summary).toHaveBeenCalledTimes(1);
      dispose();
    });
  });

  it("reloads after a scan redigests headlines even with no new events", async () => {
    await createRoot(async (dispose) => {
      const s = source({
        ingestNow: vi
          .fn()
          .mockResolvedValue({ listed: 4, scanned: 1, events: 0, failed: 0, redigested: 2 }),
      });
      const store = createUsageStore(s);
      await store.refresh();
      await store.ingestNow();
      await flush();
      expect(s.summary).toHaveBeenCalledTimes(2);
      dispose();
    });
  });
});
