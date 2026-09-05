import { For, Show, createEffect, createMemo, createSignal, onCleanup, type JSX } from "solid-js";

import type { ModelRateRow, RatesStatus } from "../bindings";
import { daemonCall, type ConfigView } from "../ipc/rpc";
import { readSidebarShowTodayCost, saveSidebarShowTodayCost, onSidebarShowTodayCostChanged } from "../stores/uiSettings";
import Switch from "./controls/Switch";
import {
  IconArrowDown,
  IconArrowUp,
  IconCheck,
  IconClose,
  IconPlus,
  IconRefresh,
  IconSearch,
} from "./icons";
import { formatRatesFootnote, formatTokens, formatUsd } from "./usageMetrics";

export interface UsageSettingsViewProps {
  settings: ConfigView;
  /** The same merge-and-persist helper every other Settings tab uses for `config.set`. */
  patch: (next: Partial<ConfigView>) => void;
  /** A model id to pre-fill the filter with, from the Usage view's unpriced-model warning. */
  initialFilter?: string;
}

type SortKey = "model" | "input" | "output" | "cache_read" | "cache_write" | "source" | "last_seen" | "tokens_30d";
type SortDir = "asc" | "desc";

interface EditValues {
  input: string;
  output: string;
  cacheRead: string;
  cacheWrite: string;
}

const EMPTY_EDIT: EditValues = { input: "", output: "", cacheRead: "", cacheWrite: "" };

const SOURCE_LABEL: Record<ModelRateRow["source"], string> = {
  builtin: "Built-in",
  litellm: "LiteLLM",
  override: "Override",
  unpriced: "Unpriced",
};

const SOURCE_CLASS: Record<ModelRateRow["source"], string> = {
  builtin: "border-line bg-raised/50 text-muted",
  litellm: "border-signal/30 bg-signal/10 text-signal",
  override: "border-signal/30 bg-signal/10 text-signal",
  unpriced: "border-attention/30 bg-attention/10 text-attention",
};

function lastSeenLabel(iso: string | null): string {
  if (!iso) return "Never";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "Never";
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

/**
 * Settings > Usage: the `[usage]` ledger toggles, where rates come from right now, and a table of
 * every model the ledger has seen (or has an override for) with an inline rate editor. Reads
 * `usage.rates` for the provenance line the Usage view's own footnote uses, and `usage.models` for
 * the table, so the two surfaces never disagree about where a number came from.
 */
export default function UsageSettingsView(props: UsageSettingsViewProps) {
  const [showTodayCost, setShowTodayCost] = createSignal(readSidebarShowTodayCost());
  onCleanup(onSidebarShowTodayCostChanged(setShowTodayCost));
  const [rates, setRates] = createSignal<RatesStatus | null>(null);
  const [rows, setRows] = createSignal<ModelRateRow[]>([]);
  const [loading, setLoading] = createSignal(true);
  const [refreshing, setRefreshing] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const [filter, setFilter] = createSignal(props.initialFilter ?? "");
  const [sortKey, setSortKey] = createSignal<SortKey>("tokens_30d");
  const [sortDir, setSortDir] = createSignal<SortDir>("desc");

  const [editing, setEditing] = createSignal<string | null>(null);
  const [editValues, setEditValues] = createSignal<EditValues>(EMPTY_EDIT);
  const [rowError, setRowError] = createSignal<string | null>(null);
  const [busyModel, setBusyModel] = createSignal<string | null>(null);
  const [addModel, setAddModel] = createSignal("");

  let section!: HTMLElement;
  let draftModel: string | null = null;
  let requestToken = 0;
  let mutationToken = 0;
  onCleanup(() => { requestToken++; mutationToken++; });

  async function load(refresh = false) {
    const token = ++requestToken;
    setError(null);
    setRefreshing(refresh);
    try {
      const status = await daemonCall(refresh ? "usage.refresh_rates" : "usage.rates");
      if (token !== requestToken) return;
      const models = await daemonCall("usage.models");
      if (token !== requestToken) return;
      setRates(status);
      const draft = rows().find((row) => row.model === draftModel);
      setRows(draft && !models.some((row) => row.model === draft.model) ? [...models, draft] : models);
    } catch (cause) {
      if (token === requestToken) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (token === requestToken) { setLoading(false); setRefreshing(false); }
    }
  }

  const refreshPrices = createMemo(() => props.settings.usage_refresh_prices);
  createEffect(() => {
    refreshPrices();
    void load();
  });
  createEffect(() => setFilter(props.initialFilter ?? ""));

  function toggleSort(key: SortKey) {
    if (sortKey() === key) {
      setSortDir((dir) => (dir === "asc" ? "desc" : "asc"));
      return;
    }
    setSortKey(key);
    setSortDir(key === "model" || key === "source" ? "asc" : "desc");
  }

  const filtered = createMemo(() => {
    const query = filter().trim().toLowerCase();
    if (!query) return rows();
    return rows().filter((row) => row.model.toLowerCase().includes(query));
  });

  /** Unpriced rows first by default, then whatever the operator actually sorted by. */
  const sorted = createMemo(() => {
    const key = sortKey();
    const dir = sortDir() === "asc" ? 1 : -1;
    const value = (row: ModelRateRow): number | string => {
      switch (key) {
        case "model":
          return row.model;
        case "input":
          return row.input_per_mtok;
        case "output":
          return row.output_per_mtok;
        case "cache_read":
          return row.cache_read_per_mtok;
        case "cache_write":
          return row.cache_write_per_mtok;
        case "source":
          return row.source;
        case "last_seen":
          return row.last_seen ? Date.parse(row.last_seen) : 0;
        case "tokens_30d":
          return row.tokens_30d;
      }
    };
    const list = [...filtered()].sort((a, b) => {
      const av = value(a);
      const bv = value(b);
      if (typeof av === "string" || typeof bv === "string") {
        return String(av).localeCompare(String(bv)) * dir;
      }
      return (av - bv) * dir;
    });
    if (key === "tokens_30d" && sortDir() === "desc") {
      // The default sort: bring every unpriced row to the top regardless of its 30-day volume,
      // since an unpriced model is the one thing worth acting on first.
      const unpriced = list.filter((row) => row.source === "unpriced");
      const rest = list.filter((row) => row.source !== "unpriced");
      return [...unpriced, ...rest];
    }
    return list;
  });

  function startEdit(row: ModelRateRow) {
    cancelEdit(false);
    setRowError(null);
    setEditing(row.model);
    setEditValues({ ...EMPTY_EDIT });
    queueMicrotask(() => section.querySelector<HTMLInputElement>("#usage-rate-input")?.focus());
  }

  function restoreFocus(model: string | null) {
    queueMicrotask(() => {
      if (!section.isConnected) return;
      const edit = [...section.querySelectorAll<HTMLButtonElement>("[data-edit-model]")]
        .find((button) => button.dataset.editModel === model);
      (edit ?? section.querySelector<HTMLInputElement>("[data-add-model]"))?.focus();
    });
  }

  function cancelEdit(focus = true) {
    const model = editing();
    setRows((prev) => prev.filter((row) => row.model !== draftModel));
    draftModel = null;
    setEditing(null);
    setRowError(null);
    if (focus) restoreFocus(model);
  }

  async function save(model: string) {
    if (busyModel()) return;
    const values = editValues();
    const fields: Array<[string, string]> = [
      ["input_per_mtok", values.input],
      ["output_per_mtok", values.output],
      ["cache_read_per_mtok", values.cacheRead],
      ["cache_write_per_mtok", values.cacheWrite],
    ];
    const patch: Record<string, number> = {};
    for (const [key, raw] of fields) {
      const trimmed = raw.trim();
      if (trimmed === "") continue;
      const num = Number(trimmed);
      if (!Number.isFinite(num) || num < 0) {
        setRowError(`${key.replace(/_per_mtok$/, "").replace("_", " ")} must be a non-negative number`);
        return;
      }
      patch[key] = num;
    }
    if (Object.keys(patch).length === 0) {
      setRowError("Enter at least one rate. Use Reset to remove an existing override.");
      return;
    }
    setRowError(null);
    setBusyModel(model);
    const token = ++mutationToken;
    requestToken++;
    try {
      await daemonCall("config.set", {
        usage_price_override_upsert: { model, ...patch },
      });
      if (token !== mutationToken) return;
      draftModel = null;
      setEditing(null);
      await load();
      if (token === mutationToken) restoreFocus(model);
    } catch (cause) {
      if (token === mutationToken) setRowError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (token === mutationToken) setBusyModel(null);
    }
  }

  async function resetOverride(model: string) {
    if (busyModel()) return;
    const token = ++mutationToken;
    requestToken++;
    setBusyModel(model);
    setError(null);
    try {
      await daemonCall("config.set", { usage_price_override_reset: model });
      if (token !== mutationToken) return;
      await load();
    } catch (cause) {
      if (token === mutationToken) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (token === mutationToken) setBusyModel(null);
    }
  }

  function addNewModel() {
    const model = addModel().trim();
    if (!model || busyModel()) return;
    cancelEdit(false);
    setAddModel("");
    if (!rows().some((row) => row.model === model)) {
      draftModel = model;
      setRows((prev) => [
        ...prev,
        {
          model,
          input_per_mtok: 0,
          output_per_mtok: 0,
          cache_read_per_mtok: 0,
          cache_write_per_mtok: 0,
          source: "unpriced",
          override: null,
          last_seen: null,
          tokens_30d: 0,
        },
      ]);
    }
    setFilter(model);
    setEditing(model);
    setEditValues({ ...EMPTY_EDIT });
    queueMicrotask(() => section.querySelector<HTMLInputElement>("#usage-rate-input")?.focus());
  }

  function SortHeader(headerProps: { label: string; sortKeyValue: SortKey; align?: "right" }) {
    const active = () => sortKey() === headerProps.sortKeyValue;
    return (
      <th aria-sort={active() ? (sortDir() === "asc" ? "ascending" : "descending") : "none"} class={`py-1 font-normal ${headerProps.align === "right" ? "text-right" : "text-left"}`}>
        <button
          type="button"
          class="focus-ring inline-flex items-center gap-1 rounded-xs text-muted hover:text-foreground"
          classList={{ "text-foreground font-medium": active() }}
          onClick={() => toggleSort(headerProps.sortKeyValue)}
          aria-pressed={active()}
        >
          {headerProps.label}
          <Show when={active()}>
            {sortDir() === "asc" ? <IconArrowUp size={10} /> : <IconArrowDown size={10} />}
          </Show>
        </button>
      </th>
    );
  }

  function RateCell(cellProps: { label: string; value: number }) {
    return (
      <td class="py-1 text-right tabular-nums text-foreground" title={`${cellProps.label}: ${formatUsd(cellProps.value)} / Mtok`}>
        {formatUsd(cellProps.value)}
      </td>
    );
  }

  function RateInput(inputProps: {
    label: string;
    id?: string;
    value: string;
    placeholder: string;
    onInput: (value: string) => void;
  }): JSX.Element {
    return (
      <input
        class="focus-ring h-7 w-20 rounded-md border border-line bg-background px-1.5 text-right text-xs tabular-nums text-foreground outline-none placeholder:text-muted"
        id={inputProps.id}
        disabled={!!busyModel()}
        aria-describedby="usage-rate-help"
        aria-label={inputProps.label}
        inputmode="decimal"
        placeholder={inputProps.placeholder}
        value={inputProps.value}
        onInput={(event) => inputProps.onInput(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") void save(editing() ?? "");
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            cancelEdit();
          }
        }}
      />
    );
  }

  return (
    <section ref={section} class="space-y-4">
      <div>
        <h3 class="text-sm font-semibold text-foreground">Usage ledger</h3>
        <p class="mt-0.5 text-xs text-muted">
          Token usage tracking, where prices come from, and per-model corrections.
        </p>
      </div>

      <div class="space-y-2.5 rounded-xl border border-line bg-surface p-3.5">
        <span class="section-label">Ledger</span>
        <Switch
          label="Track token usage"
          checked={props.settings.usage_enabled}
          onChange={(value) => props.patch({ usage_enabled: value })}
        />
        <p class="text-[11px] text-muted">
          Reads agent transcripts already on disk into the local ledger. Nothing is sent anywhere.
        </p>
        <Switch
          label="Show today's cost in the sidebar"
          checked={showTodayCost()}
          onChange={saveSidebarShowTodayCost}
        />
        <Switch
          label="Refresh prices from LiteLLM daily"
          checked={props.settings.usage_refresh_prices}
          onChange={(value) => props.patch({ usage_refresh_prices: value })}
        />
        <p class="text-[11px] text-muted">
          Fetches a public daily price list. Turn off to keep pricing fully offline on the built-in
          table; overrides below always win either way.
        </p>
      </div>

      <div class="flex items-center justify-between gap-3 rounded-xl border border-line bg-surface p-3.5">
        <p class="text-xs text-muted">{formatRatesFootnote(rates())}</p>
        <button
          type="button"
          class="focus-ring flex shrink-0 items-center gap-1.5 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-line/40 disabled:opacity-60"
          disabled={refreshing() || !!busyModel() || !!editing()}
          onClick={() => void load(true)}
        >
          <IconRefresh size={13} class={refreshing() ? "animate-spin text-signal" : "text-muted"} />
          <span>{refreshing() ? "Refreshing…" : "Refresh"}</span>
        </button>
      </div>

      <div class="space-y-3 rounded-xl border border-line bg-surface p-3.5">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h3 class="text-sm font-semibold text-foreground">Model rates</h3>
          <div class="flex h-7 items-center gap-1.5 rounded-lg border border-line bg-background px-2">
            <IconSearch size={12} class="text-muted" />
            <input
              class="focus-ring h-full w-40 rounded-xs bg-transparent text-xs text-foreground outline-none placeholder:text-muted/60"
              placeholder="Filter models"
              value={filter()}
              onInput={(event) => setFilter(event.currentTarget.value)}
              aria-label="Filter models"
            />
          </div>
        </div>

        <p id="usage-rate-help" class="text-xs text-muted">USD per million tokens. Blank fields keep the resolved rate. Reset removes the model override.</p>

        <Show when={error()}>
          <p role="alert" class="text-xs text-fault">{error()}</p>
        </Show>

        <Show
          when={!loading()}
          fallback={<p class="py-4 text-center text-xs text-muted">Loading model rates…</p>}
        >
          <Show
            when={sorted().length > 0}
            fallback={
              <p class="py-4 text-center text-xs text-muted">
                {filter()
                  ? `No model matches "${filter()}".`
                  : "No models yet. Rates come from the LiteLLM daily snapshot, the built-in table, and any override you add below."}
              </p>
            }
          >
            <div class="focus-ring overflow-x-auto rounded-xs" role="region" aria-label="Model rates" tabindex="0">
              <table class="w-full min-w-[58rem] border-separate border-spacing-x-2 text-xs">
                <thead>
                  <tr class="border-b border-line text-muted">
                    <SortHeader label="Model" sortKeyValue="model" />
                    <SortHeader label="Input" sortKeyValue="input" align="right" />
                    <SortHeader label="Output" sortKeyValue="output" align="right" />
                    <SortHeader label="Cache read" sortKeyValue="cache_read" align="right" />
                    <SortHeader label="Cache write" sortKeyValue="cache_write" align="right" />
                    <SortHeader label="Source" sortKeyValue="source" />
                    <SortHeader label="Last seen" sortKeyValue="last_seen" />
                    <SortHeader label="30d tokens" sortKeyValue="tokens_30d" align="right" />
                    <th class="py-1 text-right font-normal">
                      <span class="sr-only">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <For each={sorted()}>
                    {(row) => (
                      <Show
                        when={editing() === row.model}
                        fallback={
                          <tr class="border-b border-line/60 odd:bg-raised/30">
                            <td class="max-w-48 truncate py-1.5 text-foreground" title={row.model}>
                              {row.model}
                            </td>
                            <RateCell label="Input" value={row.input_per_mtok} />
                            <RateCell label="Output" value={row.output_per_mtok} />
                            <RateCell label="Cache read" value={row.cache_read_per_mtok} />
                            <RateCell label="Cache write" value={row.cache_write_per_mtok} />
                            <td class="py-1.5">
                              <span
                                class={`inline-flex items-center rounded-full border px-1.5 py-0.5 text-[10px] font-medium ${SOURCE_CLASS[row.source]}`}
                              >
                                {SOURCE_LABEL[row.source]}
                              </span>
                            </td>
                            <td class="py-1.5 whitespace-nowrap text-muted">{lastSeenLabel(row.last_seen)}</td>
                            <td class="py-1.5 text-right tabular-nums text-muted">
                              {formatTokens(row.tokens_30d)}
                            </td>
                            <td class="py-1.5 text-right">
                              <div class="flex items-center justify-end gap-1">
                                <button
                                  type="button"
                                  class="focus-ring rounded-md border border-line bg-surface px-2 py-0.5 text-[11px] font-medium text-foreground hover:bg-line/40"
                                  data-edit-model={row.model}
                                  aria-label={`Edit rates for ${row.model}`}
                                  disabled={!!busyModel()}
                                  onClick={() => startEdit(row)}
                                >
                                  Edit
                                </button>
                                <Show when={row.override}>
                                  <button
                                    type="button"
                                    class="focus-ring rounded-md border border-line bg-surface p-1 text-muted hover:text-fault disabled:opacity-50"
                                    title="Remove override"
                                    aria-label={`Reset rates for ${row.model}`}
                                    disabled={!!busyModel()}
                                    onClick={() => void resetOverride(row.model)}
                                  >
                                    Reset
                                  </button>
                                </Show>
                              </div>
                            </td>
                          </tr>
                        }
                      >
                        <tr class="border-b border-line/60 bg-raised/40">
                          <td class="max-w-48 truncate py-1.5 text-foreground" title={row.model}>
                            {row.model}
                          </td>
                          <td class="py-1 text-right">
                            <RateInput
                              id="usage-rate-input"
                              label={`${row.model} input rate`}
                              placeholder={formatUsd(row.input_per_mtok)}
                              value={editValues().input}
                              onInput={(value) => setEditValues((prev) => ({ ...prev, input: value }))}
                            />
                          </td>
                          <td class="py-1 text-right">
                            <RateInput
                              label={`${row.model} output rate`}
                              placeholder={formatUsd(row.output_per_mtok)}
                              value={editValues().output}
                              onInput={(value) => setEditValues((prev) => ({ ...prev, output: value }))}
                            />
                          </td>
                          <td class="py-1 text-right">
                            <RateInput
                              label={`${row.model} cache read rate`}
                              placeholder={formatUsd(row.cache_read_per_mtok)}
                              value={editValues().cacheRead}
                              onInput={(value) => setEditValues((prev) => ({ ...prev, cacheRead: value }))}
                            />
                          </td>
                          <td class="py-1 text-right">
                            <RateInput
                              label={`${row.model} cache write rate`}
                              placeholder={formatUsd(row.cache_write_per_mtok)}
                              value={editValues().cacheWrite}
                              onInput={(value) => setEditValues((prev) => ({ ...prev, cacheWrite: value }))}
                            />
                          </td>
                          <td class="py-1.5" colSpan={3}>
                            <span class="text-[11px] text-muted">
                              Blank keeps the rate shown in the field.
                            </span>
                          </td>
                          <td class="py-1.5 text-right">
                            <div class="flex items-center justify-end gap-1">
                              <button
                                type="button"
                                class="focus-ring rounded-md border border-signal/40 bg-signal/10 p-1 text-signal disabled:opacity-50"
                                aria-label={`Save rates for ${row.model}`}
                                disabled={!!busyModel()}
                                onClick={() => void save(row.model)}
                              >
                                <IconCheck size={12} />
                              </button>
                              <button
                                type="button"
                                class="focus-ring rounded-md border border-line bg-surface p-1 text-muted hover:text-foreground"
                                aria-label="Cancel edit"
                                disabled={!!busyModel()}
                                onClick={() => cancelEdit()}
                              >
                                <IconClose size={12} />
                              </button>
                            </div>
                          </td>
                        </tr>
                      </Show>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
            <Show when={rowError()}>
              <p role="alert" class="text-xs text-fault">{rowError()}</p>
            </Show>
          </Show>
        </Show>

        <div class="flex items-center gap-1.5 border-t border-line pt-3">
          <input
            class="focus-ring h-7 flex-1 rounded-lg border border-line bg-background px-2 text-xs text-foreground outline-none placeholder:text-muted/60"
            placeholder="Add a model id or family prefix for a future run"
            value={addModel()}
            onInput={(event) => setAddModel(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") addNewModel();
            }}
            data-add-model
            aria-label="Add a model id or family prefix"
          />
          <button
            type="button"
            class="focus-ring flex h-7 items-center gap-1 rounded-lg border border-line bg-surface px-2 text-xs font-medium text-foreground hover:bg-line/40"
            disabled={!!busyModel() || loading() || refreshing() || !addModel().trim()}
            onClick={addNewModel}
          >
            <IconPlus size={12} />
            Add model
          </button>
        </div>
      </div>
    </section>
  );
}
