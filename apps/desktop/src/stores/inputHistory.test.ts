import { createRoot } from "solid-js";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { daemonCall } from "../ipc/rpc";
import { createInputHistory, resetInputHistoryCacheForTests } from "./inputHistory";

vi.mock("../ipc/rpc", () => ({ daemonCall: vi.fn() }));

beforeEach(() => { resetInputHistoryCacheForTests(); vi.mocked(daemonCall).mockReset(); });

describe("createInputHistory", () => {
  it("fetches once per (lane_id, window) and serves the second mount from cache", async () => {
    vi.mocked(daemonCall).mockResolvedValue({
      entries: [{ text: "first", at: null }, { text: "second", at: null }],
      source: "session",
    });
    const first = createRoot((dispose) => ({ store: createInputHistory(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(first.store.history().entries.map((e) => e.text)).toEqual(["first", "second"]);
    first.dispose();
    const second = createRoot((dispose) => ({ store: createInputHistory(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    expect(second.store.history().entries.map((e) => e.text)).toEqual(["first", "second"]);
    expect(daemonCall).toHaveBeenCalledTimes(1);
    second.dispose();
  });

  it("reports source \"none\" as the real answer it is, not folded into an empty session/project", async () => {
    vi.mocked(daemonCall).mockResolvedValue({ entries: [], source: "none" });
    const { store, dispose } = createRoot((dispose) => ({ store: createInputHistory(() => ({ lane_id: 2, window: "lane-2" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(store.history()).toEqual({ entries: [], source: "none" });
    expect(store.error()).toBeNull();
    dispose();
  });

  it("surfaces a rejected fetch distinctly from a real empty/none history, never as a silent guess", async () => {
    vi.mocked(daemonCall).mockRejectedValue(new Error("daemon unreachable"));
    const { store, dispose } = createRoot((dispose) => ({ store: createInputHistory(() => ({ lane_id: 3, window: "lane-3" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(store.history()).toEqual({ entries: [], source: "none" });
    expect(store.error()).toMatch(/daemon unreachable/);
    dispose();
  });

  it("does not cache a failed fetch, so a later mount of the same target gets a fresh attempt", async () => {
    vi.mocked(daemonCall).mockRejectedValueOnce(new Error("daemon unreachable"));
    const first = createRoot((dispose) => ({ store: createInputHistory(() => ({ lane_id: 4, window: "lane-4" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(first.store.error()).not.toBeNull();
    first.dispose();
    vi.mocked(daemonCall).mockResolvedValueOnce({ entries: [{ text: "recovered", at: null }], source: "project" });
    const second = createRoot((dispose) => ({ store: createInputHistory(() => ({ lane_id: 4, window: "lane-4" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(second.store.error()).toBeNull();
    expect(second.store.history().entries.map((e) => e.text)).toEqual(["recovered"]);
    second.dispose();
  });

  it("refetches for a different (lane_id, window) instead of reusing another pane's cache entry", async () => {
    vi.mocked(daemonCall).mockImplementation(async (_method, params) => ({
      entries: [{ text: (params as { window?: string }).window ?? "", at: null }],
      source: "session",
    }));
    const a = createRoot((dispose) => ({ store: createInputHistory(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(a.store.history().entries[0].text).toBe("lane-1");
    a.dispose();
    const b = createRoot((dispose) => ({ store: createInputHistory(() => ({ lane_id: 1, window: "lane-2" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(b.store.history().entries[0].text).toBe("lane-2");
    expect(daemonCall).toHaveBeenCalledTimes(2);
    b.dispose();
  });
});
