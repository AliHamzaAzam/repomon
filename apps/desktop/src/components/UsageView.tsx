import { For, Show, createMemo, onCleanup, onMount } from "solid-js";

import type { UsageBucket, UsageGroupBy, UsageRange, UsageSessionRow } from "../bindings";
import type { FleetStore } from "../stores/fleet";
import type { UsageStore } from "../stores/usage";
import { IconMeter, IconRefresh, IconSearch } from "./icons";
import UsageChart from "./UsageChart";
import { formatTokens, formatUsd, seriesVar } from "./usageMetrics";

interface UsageViewProps {
  store: UsageStore;
  fleet: FleetStore;
  /** Focus the lane a session ran in. Absent when nothing can be focused. */
  onOpenLane?: (laneId: number) => void;
}

const RANGES: { id: UsageRange; label: string }[] = [
  { id: "today", label: "Today" },
  { id: "week", label: "7 days" },
  { id: "month", label: "30 days" },
];

const GROUPS: { id: UsageGroupBy; label: string }[] = [
  { id: "kind", label: "Agent" },
  { id: "model", label: "Model" },
  { id: "repo", label: "Repo" },
  { id: "lane", label: "Lane" },
  { id: "account", label: "Account" },
];

const BUCKETS: { id: UsageBucket; label: string }[] = [
  { id: "quarter", label: "15m" },
  { id: "hour", label: "Hour" },
  { id: "day", label: "Day" },
];

function percent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

function duration(row: UsageSessionRow): string {
  if (!row.started_at || !row.ended_at) return "";
  const ms = new Date(row.ended_at).getTime() - new Date(row.started_at).getTime();
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const minutes = Math.round(ms / 60000);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** A segmented control: one row of options, the current one carrying the accent. */
function Segmented<T extends string>(props: {
  label: string;
  options: { id: T; label: string }[];
  value: T;
  onSelect: (id: T) => void;
}) {
  return (
    <div class="flex items-center gap-1.5">
      <span class="section-label">{props.label}</span>
      <div class="flex items-center rounded-lg border border-line bg-surface p-0.5" role="group" aria-label={props.label}>
        <For each={props.options}>
          {(option) => {
            const active = () => props.value === option.id;
            return (
              <button
                type="button"
                class={`focus-ring rounded-md px-2 py-0.5 text-xs transition-colors ${
                  active()
                    ? "bg-signal/12 text-signal font-semibold"
                    : "text-muted hover:text-foreground"
                }`}
                aria-pressed={active()}
                onClick={() => props.onSelect(option.id)}
              >
                {option.label}
              </button>
            );
          }}
        </For>
      </div>
    </div>
  );
}

/**
 * The Usage view: what the fleet spent, where it went, and what is worth changing.
 *
 * Numbers are what the same tokens would cost on the provider's API. On a subscription plan that
 * is the value the plan returned, not an invoice, and the view says so rather than implying a bill.
 */
export default function UsageView(props: UsageViewProps) {
  const store = props.store;
  const summary = () => store.summary();
  const totals = () => summary()?.totals;
  const empty = () => !store.loading() && (totals()?.events ?? 0) === 0;

  const laneName = createMemo(() => {
    const names = new Map<number, string>();
    for (const lane of props.fleet.lanes()) {
      names.set(lane.id, `${lane.repo.name}/${lane.worktree.name}`);
    }
    return names;
  });

  onMount(() => {
    void store.refresh();
    let stop: (() => void) | undefined;
    let active = true;
    void store.subscribe?.((event) => {
      if (event.method === "event.usage.changed") void store.refresh();
    }).then((off) => {
      if (active) stop = off;
      else off();
    });
    onCleanup(() => {
      active = false;
      stop?.();
    });
  });

  return (
    <div class="flex h-full min-h-0 flex-col bg-background">
      <div class="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-line px-4 py-2.5">
        <Segmented label="Range" options={RANGES} value={store.range()} onSelect={store.setRange} />
        <Segmented label="Split by" options={GROUPS} value={store.groupBy()} onSelect={store.setGroupBy} />
        <Segmented label="Bucket" options={BUCKETS} value={store.bucket()} onSelect={store.setBucket} />
        <div class="ml-auto flex items-center gap-1.5">
          <button
            type="button"
            class="focus-ring flex h-7 items-center gap-1.5 rounded-lg border border-line bg-surface px-2 text-xs text-muted transition-colors hover:text-foreground disabled:opacity-50"
            onClick={() => void store.ingestNow()}
            disabled={store.scanning()}
          >
            <IconRefresh size={12} />
            <span>{store.scanning() ? "Reading transcripts" : "Scan now"}</span>
          </button>
          <button
            type="button"
            class="focus-ring flex h-7 items-center rounded-lg border border-line bg-surface px-2 text-xs text-muted transition-colors hover:text-foreground"
            onClick={() => void store.exportRows("csv")}
          >
            Export CSV
          </button>
          <button
            type="button"
            class="focus-ring flex h-7 items-center rounded-lg border border-line bg-surface px-2 text-xs text-muted transition-colors hover:text-foreground"
            onClick={() => void store.exportRows("json")}
          >
            JSON
          </button>
        </div>
      </div>

      <Show when={store.error()}>
        <p class="border-b border-line px-4 py-2 text-xs text-fault">{store.error()}</p>
      </Show>
      <Show when={store.lastExport()}>
        <p class="border-b border-line px-4 py-2 text-xs text-muted">
          Wrote {store.lastExport()?.events} row(s) to{" "}
          <span class="font-mono text-foreground">{store.lastExport()?.path}</span>
        </p>
      </Show>

      <div class="min-h-0 flex-1 overflow-y-auto">
        <Show
          when={!empty()}
          fallback={
            <div class="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
              <IconMeter size={22} class="text-muted" />
              <p class="text-sm text-foreground">No agent turns recorded in this window.</p>
              <p class="max-w-md text-xs text-muted">
                The ledger reads the transcripts your agents already write to disk. It scans on a
                timer; read them now if you just finished a session.
              </p>
              <button
                type="button"
                class="focus-ring flex h-7 items-center gap-1.5 rounded-lg border border-signal/50 bg-signal/10 px-2.5 text-xs font-semibold text-signal"
                onClick={() => void store.ingestNow()}
                disabled={store.scanning()}
              >
                <IconRefresh size={12} />
                <span>{store.scanning() ? "Reading transcripts" : "Scan now"}</span>
              </button>
              <Show when={store.status()}>
                <p class="text-xs text-muted">
                  {store.status()?.sources ?? 0} source(s) tracked, {store.status()?.events ?? 0}{" "}
                  event(s) in the ledger.
                </p>
              </Show>
            </div>
          }
        >
          <div class="flex flex-wrap items-end gap-x-10 gap-y-4 px-4 py-4">
            <Figure value={formatUsd(totals()?.cost_usd ?? 0)} label="Equivalent API cost" wide />
            <Figure value={formatTokens(totals()?.total_tokens ?? 0)} label="Tokens" />
            <Figure value={percent(summary()?.cache_hit_rate ?? 0)} label="Cache hit rate" />
            <Figure value={percent(summary()?.estimated_share ?? 0)} label="Estimated" />
            <Figure value={`${totals()?.events ?? 0}`} label="Turns" />
            <p class="ml-auto max-w-xs text-right text-xs text-muted">
              Priced at published API rates. On a subscription plan this is what the plan returned,
              not what it billed.
            </p>
          </div>

          <div class="border-t border-line px-4 py-4">
            <UsageChart timeline={store.timeline()} metric="cost" />
          </div>

          <div class="flex flex-wrap items-start gap-6 border-t border-line px-4 py-4">
            <div class="min-w-80 flex-1">
              <h2 class="section-label mb-2">Where it went</h2>
              <table class="w-full text-xs">
                <thead>
                  <tr class="border-b border-line text-muted">
                    <th class="py-1 text-left font-normal">Group</th>
                    <th class="py-1 text-right font-normal">In</th>
                    <th class="py-1 text-right font-normal">Out</th>
                    <th class="py-1 text-right font-normal">Cached</th>
                    <th class="py-1 text-right font-normal">Cost</th>
                  </tr>
                </thead>
                <tbody>
                  <For each={summary()?.groups ?? []}>
                    {(row, index) => (
                      <tr class="border-b border-line/60">
                        <td class="max-w-56 truncate py-1 text-foreground">
                          <span class="mr-1.5 inline-block h-2 w-2 rounded-xs align-middle"
                            style={{ "background-color": seriesVar(index()) }}
                            aria-hidden="true"
                          />
                          {row.label}
                        </td>
                        <td class="py-1 text-right tabular-nums text-muted">
                          {formatTokens(row.totals.input_tokens)}
                        </td>
                        <td class="py-1 text-right tabular-nums text-muted">
                          {formatTokens(row.totals.output_tokens)}
                        </td>
                        <td class="py-1 text-right tabular-nums text-muted">
                          {formatTokens(row.totals.cache_read_tokens)}
                        </td>
                        <td class="py-1 text-right tabular-nums font-semibold text-foreground">
                          {formatUsd(row.totals.cost_usd)}
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
              <Show when={(summary()?.unpriced_models.length ?? 0) > 0}>
                <p class="mt-2 text-xs text-attention">
                  No published rate for {summary()?.unpriced_models.join(", ")}. Their tokens count;
                  their cost reads as zero until you set a price in the config.
                </p>
              </Show>
            </div>

            <div class="min-w-72 flex-1">
              <h2 class="section-label mb-2">What to look at</h2>
              <Show
                when={store.findings().length > 0}
                fallback={<p class="text-xs text-muted">Nothing stands out in this window.</p>}
              >
                <ul class="flex flex-col gap-2">
                  <For each={store.findings().slice(0, 6)}>
                    {(finding) => (
                      <li class="border-b border-line/60 pb-2 last:border-0">
                        <p class="text-xs text-foreground">{finding.headline}</p>
                        <p class="mt-0.5 text-xs text-muted">{finding.detail}</p>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </div>
          </div>

          <div class="border-t border-line px-4 py-4">
            <div class="mb-2 flex items-center gap-3">
              <h2 class="section-label">Sessions</h2>
              <label class="flex h-7 flex-1 items-center gap-1.5 rounded-lg border border-line bg-surface px-2">
                <IconSearch size={12} class="text-muted" />
                <input
                  type="search"
                  class="min-w-0 flex-1 bg-transparent text-xs text-foreground outline-none placeholder:text-muted"
                  placeholder="Filter by task, model or path"
                  value={store.query()}
                  onInput={(event) => store.setQuery(event.currentTarget.value)}
                />
              </label>
            </div>
            <Show
              when={store.visibleSessions().length > 0}
              fallback={<p class="text-xs text-muted">No session matches that filter.</p>}
            >
              <table class="w-full text-xs">
                <thead>
                  <tr class="border-b border-line text-muted">
                    <th class="py-1 text-left font-normal">Task</th>
                    <th class="py-1 text-left font-normal">Agent</th>
                    <th class="py-1 text-left font-normal">Lane</th>
                    <th class="py-1 text-right font-normal">Turns</th>
                    <th class="py-1 text-right font-normal">Tools</th>
                    <th class="py-1 text-right font-normal">Retries</th>
                    <th class="py-1 text-right font-normal">Time</th>
                    <th class="py-1 text-right font-normal">Tokens</th>
                    <th class="py-1 text-right font-normal">Cost</th>
                  </tr>
                </thead>
                <tbody>
                  <For each={store.visibleSessions()}>
                    {(row) => (
                      <tr class="border-b border-line/60">
                        <td class="max-w-96 truncate py-1 text-foreground">
                          {row.headline ?? row.session_id}
                          <Show when={row.external}>
                            <span class="ml-1.5 text-muted">outside a lane</span>
                          </Show>
                        </td>
                        <td class="py-1 text-muted">{row.model || row.agent_kind}</td>
                        <td class="py-1">
                          <Show
                            when={row.lane_id !== null && laneName().has(row.lane_id)}
                            fallback={<span class="text-muted">{row.cwd ?? ""}</span>}
                          >
                            <button
                              type="button"
                              class="focus-ring rounded-xs text-signal hover:underline"
                              onClick={() => props.onOpenLane?.(row.lane_id as number)}
                            >
                              {laneName().get(row.lane_id as number)}
                            </button>
                          </Show>
                        </td>
                        <td class="py-1 text-right tabular-nums text-muted">{row.turns}</td>
                        <td class="py-1 text-right tabular-nums text-muted">{row.tool_calls}</td>
                        <td
                          class={`py-1 text-right tabular-nums ${
                            row.retries > 0 ? "text-attention" : "text-muted"
                          }`}
                        >
                          {row.retries}
                        </td>
                        <td class="py-1 text-right tabular-nums text-muted">{duration(row)}</td>
                        <td class="py-1 text-right tabular-nums text-muted">
                          {formatTokens(row.totals.total_tokens)}
                        </td>
                        <td class="py-1 text-right tabular-nums font-semibold text-foreground">
                          {formatUsd(row.totals.cost_usd)}
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}

/** One headline number and what it counts. */
function Figure(props: { value: string; label: string; wide?: boolean }) {
  return (
    <div class="flex flex-col gap-0.5">
      <span
        class={`tabular-nums leading-none text-foreground ${
          props.wide ? "text-3xl font-semibold" : "text-2xl font-medium"
        }`}
      >
        {props.value}
      </span>
      <span class="section-label">{props.label}</span>
    </div>
  );
}
