import { createRoot } from "solid-js";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { daemonCall } from "../ipc/rpc";
import { createCommandCatalog, resetCommandCatalogCacheForTests } from "./commandCatalog";

vi.mock("../ipc/rpc", () => ({ daemonCall: vi.fn() }));

beforeEach(() => { resetCommandCatalogCacheForTests(); vi.mocked(daemonCall).mockReset(); });

describe("createCommandCatalog", () => {
  it("fetches once per (lane_id, window) and serves the second mount from cache", async () => {
    vi.mocked(daemonCall).mockResolvedValue({
      commands: [{ name: "compact", description: "", source: "builtin", one_shot: true }],
      models: [],
      model_command: null,
    });
    const first = createRoot((dispose) => ({ store: createCommandCatalog(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(first.store.catalog().commands.map((c) => c.name)).toEqual(["compact"]);
    first.dispose();
    const second = createRoot((dispose) => ({ store: createCommandCatalog(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    expect(second.store.catalog().commands.map((c) => c.name)).toEqual(["compact"]);
    expect(daemonCall).toHaveBeenCalledTimes(1);
    second.dispose();
  });

  it("treats a rejected fetch the same as an empty catalog, never a guess", async () => {
    vi.mocked(daemonCall).mockRejectedValue(new Error("not implemented"));
    const { store, dispose } = createRoot((dispose) => ({ store: createCommandCatalog(() => ({ lane_id: 2, window: "lane-2" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(store.catalog()).toEqual({ commands: [], models: [], model_command: null });
    dispose();
  });

  it("refetches for a different (lane_id, window) instead of reusing another pane's cache entry", async () => {
    vi.mocked(daemonCall).mockImplementation(async (_method, params) => ({
      commands: [], models: [], model_command: (params as { window?: string }).window ?? null,
    }));
    const a = createRoot((dispose) => ({ store: createCommandCatalog(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(a.store.catalog().model_command).toBe("lane-1");
    a.dispose();
    const b = createRoot((dispose) => ({ store: createCommandCatalog(() => ({ lane_id: 1, window: "lane-2" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(b.store.catalog().model_command).toBe("lane-2");
    expect(daemonCall).toHaveBeenCalledTimes(2);
    b.dispose();
  });
});
