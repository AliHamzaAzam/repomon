import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ModelRateRow, RatesStatus } from "../bindings";
import { daemonCall, type ConfigView } from "../ipc/rpc";
import Modal from "./Modal";
import UsageSettingsView from "./UsageSettingsView";

vi.mock("../ipc/rpc", () => ({ daemonCall: vi.fn() }));
const rpc = vi.mocked(daemonCall);
const settings = { usage_enabled: true, usage_refresh_prices: true } as ConfigView;
const status: RatesStatus = {
  source_counts: { builtin: 1, litellm: 1, overrides: 0 }, enabled: true,
  fetched_at: null, next_refresh_at: null, last_error: null, etag: null,
};
function row(model: string, source: ModelRateRow["source"], tokens = 1): ModelRateRow {
  return { model, source, input_per_mtok: 2, output_per_mtok: 10,
    cache_read_per_mtok: 0.2, cache_write_per_mtok: 2.5, override: null,
    last_seen: "2026-09-05T10:00:00Z", tokens_30d: tokens };
}
let rows: ModelRateRow[];
beforeEach(() => {
  rows = [row("alpha", "litellm", 10), row("zed", "unpriced"), row("beta", "builtin", 30)];
  rpc.mockReset();
  rpc.mockImplementation(async (method, params) => {
    if (method === "usage.models") return rows;
    if (method === "usage.rates" || method === "usage.refresh_rates") return status;
    if (method === "config.set") {
      const p = params as { usage_price_override_reset?: string };
      if (p.usage_price_override_reset) rows = rows.map((r) => r.model === p.usage_price_override_reset ? { ...r, override: null, source: "litellm" } : r);
      return settings;
    }
    throw new Error(`unexpected ${method}`);
  });
});
afterEach(cleanup);
async function mount(filter?: string) {
  const result = render(() => <UsageSettingsView settings={settings} patch={vi.fn()} initialFilter={filter} />);
  await screen.findByRole("table");
  return result;
}
function models() {
  return screen.getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell")[0].textContent);
}
describe("UsageSettingsView", () => {
  it("puts unpriced rows first, sorts each column, and filters models", async () => {
    await mount();
    expect(screen.getByText("Unpriced")).toBeTruthy();
    expect(models()).toEqual(["zed", "beta", "alpha"]);
    fireEvent.click(screen.getByRole("button", { name: "Model" }));
    expect(models()).toEqual(["alpha", "beta", "zed"]);
    expect(screen.getByRole("columnheader", { name: "Model" }).getAttribute("aria-sort")).toBe("ascending");
    fireEvent.click(screen.getByRole("button", { name: "Model" }));
    expect(models()).toEqual(["zed", "beta", "alpha"]);
    fireEvent.input(screen.getByRole("textbox", { name: "Filter models" }), { target: { value: "ALP" } });
    expect(models()).toEqual(["alpha"]);
  });
  it("saves only typed fields even when the model already has an override", async () => {
    rows[0].override = { input_per_mtok: 7, output_per_mtok: null, cache_read_per_mtok: null, cache_write_per_mtok: null, effective_from: null };
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Edit rates for alpha" }));
    fireEvent.input(screen.getByLabelText("alpha output rate"), { target: { value: "9" } });
    fireEvent.click(screen.getByRole("button", { name: "Save rates for alpha" }));
    await waitFor(() => expect(rpc).toHaveBeenCalledWith("config.set", {
      usage_price_override_upsert: { model: "alpha", output_per_mtok: 9 },
    }));
  });
  it("resets the model override through config.set", async () => {
    rows[0].override = { input_per_mtok: 7, output_per_mtok: null, cache_read_per_mtok: null, cache_write_per_mtok: null, effective_from: null };
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Reset rates for alpha" }));
    await waitFor(() => expect(rpc).toHaveBeenCalledWith("config.set", { usage_price_override_reset: "alpha" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Reset rates for alpha" })).toBeNull());
  });
  it("validates rates and accepts an explicit zero", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Edit rates for alpha" }));
    const input = screen.getByLabelText("alpha input rate");
    for (const value of ["-1", "NaN", "Infinity", ""]) {
      fireEvent.input(input, { target: { value } });
      fireEvent.click(screen.getByRole("button", { name: "Save rates for alpha" }));
      expect(screen.getByRole("alert")).toBeTruthy();
    }
    expect(rpc.mock.calls.filter(([m]) => m === "config.set")).toHaveLength(0);
    fireEvent.input(input, { target: { value: "0" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(rpc).toHaveBeenCalledWith("config.set", { usage_price_override_upsert: { model: "alpha", input_per_mtok: 0 } }));
  });
  it("adds an unseen family prefix and focuses its inline editor", async () => {
    await mount();
    fireEvent.input(screen.getByLabelText("Add a model id or family prefix"), { target: { value: "future-model" } });
    fireEvent.click(screen.getByRole("button", { name: "Add model" }));
    const input = screen.getByLabelText("future-model input rate");
    await waitFor(() => expect(document.activeElement).toBe(input));
    fireEvent.input(input, { target: { value: "3" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(rpc).toHaveBeenCalledWith("config.set", { usage_price_override_upsert: { model: "future-model", input_per_mtok: 3 } }));
  });
  it("cancels an unsaved new model without leaving a fake ledger row", async () => {
    await mount();
    fireEvent.input(screen.getByLabelText("Add a model id or family prefix"), { target: { value: "future-model" } });
    fireEvent.click(screen.getByRole("button", { name: "Add model" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel edit" }));
    expect(screen.queryByText("future-model")).toBeNull();
    expect(rpc.mock.calls.filter(([m]) => m === "config.set")).toHaveLength(0);
  });
  it("prefills the warning filter and explains an empty ledger", async () => {
    await mount("zed");
    expect(models()).toEqual(["zed"]);
    cleanup();
    rows = [];
    render(() => <UsageSettingsView settings={settings} patch={vi.fn()} />);
    expect(await screen.findByText(/No models yet. Rates come from/)).toBeTruthy();
  });
  it("patches the existing usage toggles", async () => {
    const patch = vi.fn();
    render(() => <UsageSettingsView settings={settings} patch={patch} />);
    fireEvent.click(screen.getByRole("switch", { name: "Track token usage" }));
    fireEvent.click(screen.getByRole("switch", { name: "Refresh prices from LiteLLM daily" }));
    expect(patch.mock.calls).toEqual([[{ usage_enabled: false }], [{ usage_refresh_prices: false }]]);
  });
  it("preserves an unsaved model when the refresh preference reloads the table", async () => {
    const [config, setConfig] = createSignal(settings);
    render(() => <UsageSettingsView settings={config()} patch={vi.fn()} />);
    await screen.findByRole("table");
    fireEvent.input(screen.getByLabelText("Add a model id or family prefix"), { target: { value: "future-model" } });
    fireEvent.click(screen.getByRole("button", { name: "Add model" }));
    fireEvent.input(screen.getByLabelText("future-model input rate"), { target: { value: "3" } });
    setConfig({ ...settings, usage_refresh_prices: false });
    await waitFor(() => expect(rpc.mock.calls.filter(([m]) => m === "usage.models")).toHaveLength(2));
    await Promise.resolve();
    expect((screen.getByLabelText("future-model input rate") as HTMLInputElement).value).toBe("3");
    fireEvent.click(screen.getByRole("button", { name: "Cancel edit" }));
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Add a model id or family prefix")));
    expect(screen.getByRole("button", { name: "Refresh" })).not.toBeDisabled();
  });
  it("cancels inline editing before Escape closes the modal and restores focus", async () => {
    const close = vi.fn();
    render(() => <Modal title="Settings" onClose={close}><UsageSettingsView settings={settings} patch={vi.fn()} /></Modal>);
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: "Edit rates for alpha" }));
    const input = screen.getByLabelText("alpha input rate");
    await waitFor(() => expect(document.activeElement).toBe(input));
    fireEvent.keyDown(input, { key: "Escape" });
    expect(close).not.toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Edit rates for alpha" })));
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(close).toHaveBeenCalledOnce();
  });

  it("does not reload model rates for unrelated config patches", async () => {
    const [config, setConfig] = createSignal(settings);
    render(() => <UsageSettingsView settings={config()} patch={vi.fn()} />);
    await screen.findByRole("table");
    setConfig({ ...settings, usage_enabled: false });
    await Promise.resolve();
    expect(rpc.mock.calls.filter(([m]) => m === "usage.models")).toHaveLength(1);
    setConfig({ ...settings, usage_refresh_prices: false });
    await waitFor(() => expect(rpc.mock.calls.filter(([m]) => m === "usage.models")).toHaveLength(2));
  });

  it("ignores a stale rates read after the refresh preference changes", async () => {
    let resolveOld!: (value: ModelRateRow[]) => void;
    rpc.mockImplementation(async (method) => method === "usage.models" ? rows : status);
    rpc.mockImplementationOnce(async () => status);
    rpc.mockImplementationOnce(() => new Promise((resolve) => { resolveOld = resolve; }));
    const [config, setConfig] = createSignal(settings);
    render(() => <UsageSettingsView settings={config()} patch={vi.fn()} />);
    await waitFor(() => expect(resolveOld).toBeTruthy());
    setConfig({ ...settings, usage_refresh_prices: false });
    await screen.findByText("alpha");
    resolveOld([row("stale", "builtin")]);
    await Promise.resolve();
    expect(screen.queryByText("stale")).toBeNull();
    expect(screen.getByText("alpha")).toBeTruthy();
  });
});
