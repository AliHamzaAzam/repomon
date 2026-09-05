import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { RatesStatus, UsageSessionRow, UsageSummary, UsageTimeline } from "../bindings";
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
  subagent_tokens: 428_000,
  cost_usd: 4.75,
  events: 88,
};

const summary: UsageSummary = {
  from: "2026-09-01T00:00:00Z",
  to: "2026-09-05T12:00:00Z",
  group_by: "kind",
  totals,
  groups: [
    { key: "claude-code", label: "claude-code", totals, unpriced: false },
    { key: "codex", label: "codex", totals: { ...totals, cost_usd: 0.4 }, unpriced: false },
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
  headline_raw: "<local-command-caveat>ran /status</local-command-caveat>\nWire up the ledger",
  lane_label: "demo/main",
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

const rates: RatesStatus = {
  source_counts: { builtin: 4, litellm: 12, overrides: 2 },
  fetched_at: "2026-09-05T09:00:00Z",
  etag: '"snap-1"',
  next_refresh_at: "2026-09-06T09:00:00Z",
  last_error: null,
  enabled: true,
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
        detail: "88 turns, 1.1M tokens, $4.75.",
        cost_usd: 4.75,
        session_id: null,
        count: 1,
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
    ingestNow: vi.fn().mockResolvedValue({ listed: 4, scanned: 0, events: 0, failed: 0, redigested: 0 }),
    rates: vi.fn().mockResolvedValue(rates),
    refreshRates: vi.fn().mockResolvedValue(rates),
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

  it("renders a blank model key as 'unknown model' rather than a blank row", async () => {
    const withUnlabelledModel: UsageSummary = {
      ...summary,
      group_by: "model",
      groups: [{ key: "", label: "", totals, unpriced: false }],
    };
    mount(source({ summary: vi.fn().mockResolvedValue(withUnlabelledModel) }), fleet());
    await flush();
    fireEvent.click(screen.getByText("Model"));
    await flush();
    expect(screen.getByText("unknown model")).toBeTruthy();
  });

  it("shows where rates came from in the pricing footnote", async () => {
    mount(source(), fleet());
    await flush();
    expect(screen.getByText(/LiteLLM, updated/)).toBeTruthy();
    expect(screen.getByText(/12 models/)).toBeTruthy();
    expect(screen.getByText(/2 from overrides/)).toBeTruthy();
    expect(screen.getByText(/4 built-in/)).toBeTruthy();
  });

  it("flags a failed rate fetch in the footnote instead of showing a stale one", async () => {
    mount(
      source({
        rates: vi.fn().mockResolvedValue({ ...rates, last_error: "connection timed out" }),
      }),
      fleet(),
    );
    await flush();
    expect(screen.getByText(/fetch failed/)).toBeTruthy();
    expect(screen.getByText(/connection timed out/)).toBeTruthy();
  });

  it("refreshes rates on demand from the footnote button", async () => {
    const src = source();
    mount(src, fleet());
    await flush();
    const button = screen.getByText("Refresh rates");
    fireEvent.click(button);
    await flush();
    expect(src.refreshRates).toHaveBeenCalledTimes(1);
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

  it("names a session with no task text rather than showing its identifier", async () => {
    mount(
      source({
        sessions: vi
          .fn()
          .mockResolvedValue([{ ...session, headline: null, headline_raw: null }]),
      }),
      fleet(),
    );
    await flush();
    expect(screen.getByText("Untitled session")).toBeTruthy();
    expect(screen.queryByText("sess-1")).toBeNull();
  });

  it("shows the lane a session ran in even when the lane is gone", async () => {
    mount(
      source({
        sessions: vi
          .fn()
          .mockResolvedValue([{ ...session, lane_id: 404, lane_label: "demo (lane removed)" }]),
      }),
      fleet([{ id: 3, repo: "demo", worktree: "main" }]),
    );
    await flush();
    expect(screen.getByText("demo (lane removed)")).toBeTruthy();
  });

  it("shortens a raw cwd path and tags it external for a session outside every lane", async () => {
    const cwd = "/Users/azaleas/Documents/Codex/2026-08-30/frontend-design-plugin-frontend-design-claude";
    mount(
      source({
        sessions: vi
          .fn()
          .mockResolvedValue([{ ...session, lane_id: null, lane_label: null, cwd, external: true }]),
      }),
      fleet(),
    );
    await flush();
    expect(
      screen.getByText("2026-08-30/frontend-design-plugin-frontend-design-claude"),
    ).toBeTruthy();
    expect(screen.getByText("external")).toBeTruthy();
    expect(screen.queryByText(cwd)).toBeNull();
  });

  it("links a finding to the session row it is about", async () => {
    mount(
      source({
        findings: vi.fn().mockResolvedValue([
          {
            kind: "retries",
            subject: "sess-1",
            headline: "Wire up the ledger retried 2 of 12 turns",
            detail: "claude-sonnet-5, $4.75.",
            cost_usd: 4.75,
            session_id: "sess-1",
            count: 1,
          },
        ]),
      }),
      fleet(),
    );
    await flush();
    fireEvent.click(screen.getByText("Wire up the ledger retried 2 of 12 turns"));
    await flush();
    const search = screen.getByPlaceholderText("Filter by task, model or path") as HTMLInputElement;
    expect(search.value).toBe("sess-1");
  });

  it("counts the sessions a folded finding stands for", async () => {
    mount(
      source({
        findings: vi.fn().mockResolvedValue([
          {
            kind: "model_choice",
            subject: "claude-opus-5",
            headline: "3 sessions did light work on claude-opus-5",
            detail: "4.2k output tokens and $1.20 between them.",
            cost_usd: 1.2,
            session_id: null,
            count: 3,
          },
        ]),
      }),
      fleet(),
    );
    await flush();
    expect(screen.getByText("3 sessions did light work on claude-opus-5")).toBeTruthy();
    expect(screen.getByText("3")).toBeTruthy();
  });

  it("opens a session row to the breakdown behind it", async () => {
    mount(source(), fleet());
    await flush();
    expect(screen.queryByText("Cache read")).toBeNull();
    fireEvent.click(screen.getByText("Wire up the ledger"));
    await flush();
    expect(screen.getByText("Cache read")).toBeTruthy();
    expect(screen.getByText("Session")).toBeTruthy();
  });

  it("copies the session id from the expanded row", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    mount(source(), fleet());
    await flush();
    fireEvent.click(screen.getByText("Wire up the ledger"));
    await flush();
    fireEvent.click(screen.getByTitle("Copy session ID"));
    await flush();
    expect(writeText).toHaveBeenCalledWith("sess-1");
  });

  it("reorders the sessions table from a column heading", async () => {
    const s = source();
    mount(s, fleet());
    await flush();
    // "Cost" also names the chart's measure toggle and the breakdown table's column, so pick the
    // one that is actually a sortable heading.
    const heading = screen
      .getAllByText("Cost")
      .map((node) => node.closest("th"))
      .find((cell) => cell?.hasAttribute("aria-sort")) as HTMLTableCellElement;
    expect(heading.getAttribute("aria-sort")).toBe("none");
    fireEvent.click(heading.querySelector("button") as HTMLButtonElement);
    await flush();
    expect(heading.getAttribute("aria-sort")).toBe("descending");
  });

  it("asks the daemon for an explicit window when a month preset is picked", async () => {
    const s = source();
    mount(s, fleet());
    await flush();
    fireEvent.click(screen.getByText("Custom"));
    await flush();
    fireEvent.click(screen.getByText("This month"));
    await flush();
    const calls = (s.summary as unknown as { mock: { calls: unknown[][] } }).mock.calls;
    const last = calls[calls.length - 1]?.[0] as {
      range: string;
      since?: string;
      until?: string;
    };
    expect(last.range).toBe("custom");
    expect(last.since).toBeTruthy();
    expect(last.until).toBeTruthy();
  });

  it("shows the dates the window resolves to", async () => {
    mount(source(), fleet());
    await flush();
    expect(screen.getAllByText("7 days").length).toBeGreaterThan(0);
    expect(screen.getAllByText(/ to /).length).toBeGreaterThan(0);
  });

  it("opens the lane a session ran in", async () => {
    const onOpenLane = vi.fn();
    mount(source(), fleet([{ id: 3, repo: "demo", worktree: "main" }]), onOpenLane);
    await flush();
    fireEvent.click(screen.getByText("demo/main"));
    expect(onOpenLane).toHaveBeenCalledWith(3);
  });
});
