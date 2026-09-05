import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { UsageSessionRow, UsageSummary, UsageTimeline } from "../bindings";
import type { FleetStore } from "../stores/fleet";
import { createUsageStore, type UsageSource } from "../stores/usage";
import UsageView from "./UsageView";

vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn().mockResolvedValue(null),
  subscribeDaemon: vi.fn().mockResolvedValue(() => undefined),
}));

afterEach(() => {
  cleanup();
});

const totals = {
  input_tokens: 120_000,
  output_tokens: 40_000,
  cache_read_tokens: 900_000,
  cache_write_tokens: 10_000,
  thinking_tokens: 5_000,
  total_tokens: 1_070_000,
  estimated_tokens: 0,
  cost_usd: 4.75,
  events: 88,
};

const summary: UsageSummary = {
  from: "2026-09-01T00:00:00Z",
  to: "2026-09-05T12:00:00Z",
  group_by: "kind",
  totals,
  groups: [
    { key: "claude-code", label: "claude-code", totals },
    { key: "codex", label: "codex", totals: { ...totals, cost_usd: 0.4 } },
  ],
  cache_hit_rate: 0.88,
  estimated_share: 0.02,
  unpriced_models: ["local-model"],
};

const timeline: UsageTimeline = {
  bucket: "hour",
  group_by: "kind",
  series: [
    {
      key: "claude-code",
      label: "claude-code",
      points: [
        { at: "2026-09-05T10:00:00Z", total_tokens: 700_000, cost_usd: 3.5 },
        { at: "2026-09-05T11:00:00Z", total_tokens: 370_000, cost_usd: 1.25 },
      ],
      totals,
    },
  ],
  buckets: ["2026-09-05T10:00:00Z", "2026-09-05T11:00:00Z"],
};

const session: UsageSessionRow = {
  session_id: "sess-1",
  agent_kind: "claude-code",
  model: "claude-sonnet-5",
  headline: "Wire up the ledger",
  repo_id: 1,
  lane_id: 3,
  cwd: "/repos/demo",
  started_at: "2026-09-05T10:00:00Z",
  ended_at: "2026-09-05T10:30:00Z",
  turns: 12,
  tool_calls: 40,
  retries: 2,
  totals,
  estimated: false,
  external: false,
};

const emptySummary: UsageSummary = {
  ...summary,
  totals: { ...totals, events: 0, total_tokens: 0, cost_usd: 0 },
  groups: [],
  unpriced_models: [],
};

function source(overrides: Partial<UsageSource> = {}): UsageSource {
  return {
    summary: vi.fn().mockResolvedValue(summary),
    timeline: vi.fn().mockResolvedValue(timeline),
    sessions: vi.fn().mockResolvedValue([session]),
    findings: vi.fn().mockResolvedValue([
      {
        kind: "cost_driver",
        subject: "claude-opus-5",
        headline: "claude-opus-5 is 74 percent of the bill",
        detail: "88 turns, 1070000 tokens, $4.75.",
        cost_usd: 4.75,
      },
    ]),
    status: vi.fn().mockResolvedValue({
      sources: 4,
      last_scan_at: "2026-09-05T12:00:00Z",
      errors: [],
      events: 88,
      first_event_at: null,
      last_event_at: null,
      ingesting: false,
    }),
    exportRows: vi.fn().mockResolvedValue({ path: "/data/usage.csv", events: 88, bytes: 4096 }),
    ingestNow: vi.fn().mockResolvedValue({ listed: 4, scanned: 0, events: 0, failed: 0 }),
    subscribe: vi.fn().mockResolvedValue(() => undefined),
    ...overrides,
  };
}

function fleet(lanes: { id: number; repo: string; worktree: string }[] = []): FleetStore {
  return {
    lanes: () => lanes.map((l) => ({ id: l.id, repo: { name: l.repo }, worktree: { name: l.worktree } })),
  } as unknown as FleetStore;
}

async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** Create the store inside the render root, so its memos are disposed with the component. */
function mount(src: UsageSource, f: FleetStore = fleet(), onOpenLane?: (id: number) => void) {
  return render(() => (
    <UsageView store={createUsageStore(src)} fleet={f} onOpenLane={onOpenLane} />
  ));
}

describe("UsageView", () => {
  it("shows the headline figures, the breakdown and the sessions once data arrives", async () => {
    mount(source(), fleet([{ id: 3, repo: "demo", worktree: "main" }]));
    await flush();
    expect(screen.getAllByText("$4.75").length).toBeGreaterThan(0);
    expect(screen.getByText("Equivalent API cost")).toBeTruthy();
    expect(screen.getByText("88%")).toBeTruthy();
    expect(screen.getAllByText("claude-code").length).toBeGreaterThan(0);
    expect(screen.getByText("Wire up the ledger")).toBeTruthy();
    expect(screen.getByText("demo/main")).toBeTruthy();
  });

  it("names the models it could not price rather than pretending they were free", async () => {
    mount(source(), fleet());
    await flush();
    expect(screen.getByText(/No published rate for local-model/)).toBeTruthy();
  });

  it("shows the findings the optimize panel was given", async () => {
    mount(source(), fleet());
    await flush();
    expect(screen.getByText("claude-opus-5 is 74 percent of the bill")).toBeTruthy();
  });

  it("invites a scan when the window holds nothing", async () => {
    mount(source({ summary: vi.fn().mockResolvedValue(emptySummary) }), fleet());
    await flush();
    expect(screen.getByText("No agent turns recorded in this window.")).toBeTruthy();
    expect(screen.getAllByText("Scan now").length).toBeGreaterThan(0);
    expect(screen.queryByText("Wire up the ledger")).toBeNull();
  });

  it("filters the sessions table from the search box", async () => {
    mount(source({
        sessions: vi
          .fn()
          .mockResolvedValue([session, { ...session, session_id: "sess-2", headline: "Fix the flake" }]),
      }), fleet());
    await flush();
    const search = screen.getByPlaceholderText("Filter by task, model or path") as HTMLInputElement;
    fireEvent.input(search, { target: { value: "flake" } });
    await flush();
    expect(screen.queryByText("Wire up the ledger")).toBeNull();
    expect(screen.getByText("Fix the flake")).toBeTruthy();
  });

  it("switches the grouping through the segmented control", async () => {
    const s = source();
    mount(s, fleet());
    await flush();
    fireEvent.click(screen.getByText("Model"));
    await flush();
    expect(s.summary).toHaveBeenLastCalledWith({ range: "week", group_by: "model" });
  });

  it("reports where an export landed", async () => {
    mount(source(), fleet());
    await flush();
    fireEvent.click(screen.getByText("Export CSV"));
    await flush();
    expect(screen.getByText("/data/usage.csv")).toBeTruthy();
  });

  it("surfaces a load failure instead of an empty screen", async () => {
    mount(source({ summary: vi.fn().mockRejectedValue(new Error("daemon is down")) }), fleet());
    await flush();
    expect(screen.getByText("daemon is down")).toBeTruthy();
  });

  it("opens the lane a session ran in", async () => {
    const onOpenLane = vi.fn();
    mount(source(), fleet([{ id: 3, repo: "demo", worktree: "main" }]), onOpenLane);
    await flush();
    fireEvent.click(screen.getByText("demo/main"));
    expect(onOpenLane).toHaveBeenCalledWith(3);
  });
});
