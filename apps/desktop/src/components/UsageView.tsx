import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import type { UsageBucket, UsageFinding, UsageGroupBy, UsageSessionRow } from "../bindings";
import type { FleetStore } from "../stores/fleet";
import { sessionDurationMs, type SessionSort, type UsageStore } from "../stores/usage";
import {
  IconCheck,
  IconChevronDown,
  IconChevronRight,
  IconCopy,
  IconMeter,
  IconRefresh,
  IconSearch,
} from "./icons";
import UsageChart from "./UsageChart";
import UsageRangePicker from "./UsageRangePicker";
import {
  AGENT_ID_MAX_CHARS,
  formatDuration,
  formatRatesFootnote,
  formatTokens,
  formatUsd,
  groupRowLabel,
  laneCell,
  retryTone,
  seriesVar,
  sessionColumnPlan,
  sessionColumnVisibility,
  subagentShare,
  truncateMiddle,
  windowLine,
  withContinuousAxis,
  type SessionColumnVisibility,
  type UsageMetric,
} from "./usageMetrics";

interface UsageViewProps {
  store: UsageStore;
  fleet: FleetStore;
  /** Focus the lane a session ran in. Absent when nothing can be focused. */
  onOpenLane?: (laneId: number) => void;
  /** Open the settings tab where a model price is set. Absent when settings cannot be opened. */
  onOpenSettings?: () => void;
}

const RANGES: { id: "today" | "week" | "month"; label: string }[] = [
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

/**
 * Sortable numeric headings, in display order. The shared column plan owns their widths;
 * Retries drops out at narrow widths according to `sessionColumnVisibility`.
 */
const SESSION_COLUMNS: { id: SessionSort; label: string }[] = [
  { id: "retries", label: "Retries" },
  { id: "time", label: "Time" },
  { id: "tokens", label: "Tokens" },
  { id: "cost", label: "Cost" },
];

function percent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

/** The dates a window covers, spelled out so a named range is never ambiguous. */
function windowDates(from: Date, to: Date): string {
  const day = (d: Date) => d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const time = (d: Date) => d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  if (from.toDateString() === to.toDateString()) return `${day(from)}, ${time(from)} to ${time(to)}`;
  return `${day(from)} to ${day(to)}`;
}

/** A segmented control: one row of options, the current one carrying the accent. */
function Segmented<T extends string>(props: {
  label: string;
  options: { id: T; label: string }[];
  value: T | null;
  onSelect: (id: T) => void;
  children?: import("solid-js").JSX.Element;
}) {
  return (
    <div class="flex items-center gap-1.5">
      <span class="section-label">{props.label}</span>
      <div
        class="flex items-center rounded-lg border border-line bg-surface p-0.5"
        role="group"
        aria-label={props.label}
      >
        <For each={props.options}>
          {(option) => {
            const active = () => props.value === option.id;
            return (
              <button
                type="button"
                class={`focus-ring rounded-md px-2 py-0.5 text-xs transition-colors ${
                  active()
                    ? "bg-signal/12 font-semibold text-signal"
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
        {props.children}
      </div>
    </div>
  );
}

/** A card: one heading, one body, the same frame every time. */
function Card(props: { title: string; aside?: import("solid-js").JSX.Element; children: import("solid-js").JSX.Element }) {
  return (
    <section class="min-w-80 flex-1 rounded-xl border border-line bg-surface p-3">
      <div class="mb-2 flex items-baseline justify-between gap-2">
        <h2 class="section-label">{props.title}</h2>
        {props.aside}
      </div>
      {props.children}
    </section>
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
  const [metric, setMetric] = createSignal<UsageMetric>("cost");
  const [expanded, setExpanded] = createSignal<string | null>(null);
  const [sessionsEl, setSessionsEl] = createSignal<HTMLDivElement>();
  // Generous until the first measurement lands, so every column shows rather than flashing narrow.
  const [sessionsWidth, setSessionsWidth] = createSignal(2000);
  const columns = createMemo<SessionColumnVisibility>(() => sessionColumnVisibility(sessionsWidth()));
  const columnPlan = createMemo(() => sessionColumnPlan(columns()));
  const tableMinWidth = createMemo(() => columnPlan().reduce((sum, col) => sum + col.width, 0));
  const detailGrid = createMemo(() =>
    columnPlan()
      .map((col) => col.id === "task" ? `minmax(${col.width}px, 1fr)` : `${col.width}px`)
      .join(" "),
  );
  // Task, Agent, Lane, Turns, Time, Tokens, Cost are always shown; Tools, Sub and Retries add to
  // that.
  const sessionColSpan = createMemo(
    () =>
      7 +
      (columns().tools ? 1 : 0) +
      (columns().subagents ? 1 : 0) +
      (columns().retries ? 1 : 0),
  );

  /** Which lanes the fleet can still focus, so only a live lane becomes a link. */
  const liveLanes = createMemo(() => new Set(props.fleet.lanes().map((lane) => lane.id)));

  /**
   * The window on screen. The daemon answers with the window it actually read, so that is what
   * the header prints and what the chart's axis spans; the store's own resolution is only the
   * stand-in until the first answer lands.
   */
  const windowRange = createMemo(() => {
    const answered = summary();
    if (!answered) return store.resolved();
    return { from: new Date(answered.from), to: new Date(answered.to) };
  });

  /** The timeline drawn with every bucket in the window, empty ones included. */
  const continuous = createMemo(() => {
    const timeline = store.timeline();
    if (!timeline) return null;
    const { from, to } = windowRange();
    return withContinuousAxis(timeline, from.toISOString(), to.toISOString());
  });

  /** Point the sessions table at one session, which is what a finding is a shortcut to. */
  function openSession(row: { session_id: string }) {
    store.setQuery(row.session_id);
    setExpanded(row.session_id);
    // Guarded: the sessions table is only in the DOM once there is something to show.
    sessionsEl()?.scrollIntoView?.({ block: "start", behavior: "smooth" });
  }

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

  // Sub, Tools and Retries give way first as the sessions table narrows, so Task keeps room to
  // read.
  // The section only enters the DOM once there is data, so this watches the element signal rather
  // than observing once on mount.
  createEffect(() => {
    const el = sessionsEl();
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width;
      if (width !== undefined) setSessionsWidth(width);
    });
    observer.observe(el);
    onCleanup(() => observer.disconnect());
  });

  return (
    <div class="flex h-full min-h-0 flex-col bg-background">
      <div class="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-line px-4 py-2.5">
        <Segmented
          label="Range"
          options={RANGES}
          value={store.range() === "custom" ? null : (store.range() as "today" | "week" | "month")}
          onSelect={store.setRange}
        >
          <UsageRangePicker
            label={store.step().label}
            active={store.range() === "custom"}
            onApply={(from, to, label) => store.setCustomRange(from, to, label)}
          />
        </Segmented>
        <Segmented
          label="Split by"
          options={GROUPS}
          value={store.groupBy()}
          onSelect={store.setGroupBy}
        />
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

      {/* The window in words: which dates are on screen, and the way back out of a narrowed one. */}
      <div class="flex flex-wrap items-center gap-1.5 border-b border-line px-4 py-1.5 text-xs text-muted">
        <For each={store.trail()}>
          {(previous, index) => (
            <>
              <button
                type="button"
                class="focus-ring rounded px-1 text-signal transition-colors hover:underline"
                onClick={() => store.backTo(index())}
              >
                {previous.label}
              </button>
              <IconChevronRight size={10} class="text-muted/70" aria-hidden="true" />
            </>
          )}
        </For>
        <span class="font-medium text-foreground">{store.step().label}</span>
        <span aria-hidden="true">·</span>
        <span class="tabular-nums">{windowDates(windowRange().from, windowRange().to)}</span>
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
          {/* The bill first, at a size nothing else competes with; what it is made of beside it;
              how much work it took last, because that is context rather than the answer. */}
          <div class="flex flex-wrap items-end gap-x-10 gap-y-4 px-4 py-4">
            <div class="flex flex-col gap-0.5">
              <span class="text-4xl font-semibold leading-none tabular-nums text-foreground">
                {formatUsd(totals()?.cost_usd ?? 0)}
              </span>
              <span class="section-label">Equivalent API cost</span>
            </div>
            <Figure value={formatTokens(totals()?.total_tokens ?? 0)} label="Tokens" />
            <Figure value={percent(summary()?.cache_hit_rate ?? 0)} label="Cache hit rate" />
            <div class="flex flex-col gap-0.5 text-xs text-muted">
              <span class="tabular-nums">{totals()?.events ?? 0} turns</span>
              <span class="tabular-nums">{percent(summary()?.estimated_share ?? 0)} estimated</span>
            </div>
          </div>

          <div class="border-t border-line px-4 py-4">
            <UsageChart
              timeline={continuous()}
              metric={metric()}
              onChangeMetric={setMetric}
              onNarrow={(at) => store.narrowTo(at)}
            />
          </div>

          <div class="flex flex-wrap items-start gap-4 border-t border-line px-4 py-4">
            <Card title="Where it went">
              <div class="overflow-x-auto">
                <table class="usage-table w-full min-w-100 table-fixed text-xs" aria-label="Where it went">
                  <colgroup>
                    <col />
                    <col class="w-16" /><col class="w-16" /><col class="w-16" /><col class="w-24" />
                  </colgroup>
                  <thead>
                    <tr class="border-b border-line text-muted">
                      <th class="px-2 py-2 text-left font-medium">Group</th>
                      <th class="px-2 py-2 text-right font-medium">In</th>
                      <th class="px-2 py-2 text-right font-medium">Out</th>
                      <th class="px-2 py-2 text-right font-medium">Cached</th>
                      <th class="px-2 py-2 text-right font-medium">Cost</th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={summary()?.groups ?? []}>
                      {(row, index) => (
                        <tr class="border-b border-line/30 hover:bg-raised/60">
                          <td class="truncate px-2 py-2 text-foreground" title={groupRowLabel(row, store.groupBy())}>
                            <span
                              class="mr-1.5 inline-block h-2 w-2 rounded-xs align-middle"
                              style={{ "background-color": seriesVar(index()) }}
                              aria-hidden="true"
                            />
                            {groupRowLabel(row, store.groupBy())}
                          </td>
                          <td class="px-2 py-2 text-right tabular-nums text-muted">
                            {formatTokens(row.totals.input_tokens)}
                          </td>
                          <td class="px-2 py-2 text-right tabular-nums text-muted">
                            {formatTokens(row.totals.output_tokens)}
                          </td>
                          <td class="px-2 py-2 text-right tabular-nums text-muted">
                            {formatTokens(row.totals.cache_read_tokens)}
                          </td>
                          <td class="px-2 py-2 text-right font-semibold tabular-nums text-foreground">
                            {formatUsd(row.totals.cost_usd)}
                          </td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
              <Show when={(summary()?.unpriced_models.length ?? 0) > 0}>
                <p class="mt-2 text-xs text-attention">
                  No published rate for {summary()?.unpriced_models.join(", ")}. Tokens are counted;
                  cost shows as $0 until you set a price in{" "}
                  <Show when={props.onOpenSettings} fallback={<span>Settings</span>}>
                    <button
                      type="button"
                      class="focus-ring rounded-xs underline"
                      onClick={() => props.onOpenSettings?.()}
                    >
                      Settings
                    </button>
                  </Show>
                  .
                </p>
              </Show>
            </Card>

            <Card title="What to look at">
              <Show
                when={store.findings().length > 0}
                fallback={<p class="text-xs text-muted">Nothing stands out in this window.</p>}
              >
                <ul class="flex flex-col gap-2">
                  <For each={store.findings().slice(0, 6)}>
                    {(finding) => <FindingRow finding={finding} onOpenSession={openSession} />}
                  </For>
                </ul>
              </Show>
            </Card>
          </div>

          <div class="border-t border-line px-4 py-4" ref={setSessionsEl}>
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
              <div class="focus-ring max-h-128 overflow-auto" tabindex="0" role="region" aria-label="Sessions table">
                <table class="usage-table w-full table-fixed text-xs" aria-label="Sessions"
                  style={{ "min-width": `${tableMinWidth()}px`, "--session-grid": detailGrid() }}>
                  <colgroup>
                    <For each={columnPlan()}>{(col) => <col style={col.id === "task" ? {} : { width: `${col.width}px` }} />}</For>
                  </colgroup>
                  <thead class="sticky top-0 z-10 bg-surface">
                    <tr class="border-b border-line text-muted">
                      <th
                        class="px-2 py-2 text-left font-medium"
                        aria-sort={store.sort() === "recent" ? "descending" : "none"}
                      >
                        <SortButton
                          column={{ id: "recent", label: "Task" }}
                          active={store.sort() === "recent"}
                          onSelect={store.setSort}
                        />
                      </th>
                      <th class="px-2 py-2 text-left font-medium">Agent</th>
                      <th class="px-2 py-2 text-left font-medium">Lane</th>
                      <th class="px-2 py-2 text-right font-medium">Turns</th>
                      <Show when={columns().tools}>
                        <th class="px-2 py-2 text-right font-medium">Tools</th>
                      </Show>
                      <Show when={columns().subagents}>
                        <th
                          class="px-2 py-2 text-right font-medium"
                          title="Share of the session's tokens its subagents spent"
                        >
                          Sub
                        </th>
                      </Show>
                      <For each={SESSION_COLUMNS}>
                        {(column) => (
                          <Show when={column.id !== "retries" || columns().retries}>
                            <th
                              class="px-2 py-2 text-right font-medium"
                              aria-sort={store.sort() === column.id ? "descending" : "none"}
                            >
                              <SortButton
                                column={column}
                                active={store.sort() === column.id}
                                onSelect={store.setSort}
                              />
                            </th>
                          </Show>
                        )}
                      </For>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={store.visibleSessions()}>
                      {(row) => (
                        <SessionRow
                          row={row}
                          expanded={expanded() === row.session_id}
                          laneIsLive={row.lane_id !== null && liveLanes().has(row.lane_id)}
                          columns={columns()}
                          colSpan={sessionColSpan()}
                          onToggle={() =>
                            setExpanded((open) => (open === row.session_id ? null : row.session_id))
                          }
                          onOpenLane={props.onOpenLane}
                        />
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </Show>
            <div class="mt-3 flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
              <p
                class="text-[11px]"
                classList={{
                  "text-attention": !!store.rates()?.last_error,
                  "text-muted": !store.rates()?.last_error,
                }}
              >
                {formatRatesFootnote(store.rates())} On a subscription plan this is what the plan
                returned, not what it billed.
              </p>
              <button
                type="button"
                class="focus-ring flex h-6 shrink-0 items-center gap-1 rounded-lg border border-line bg-surface px-2 text-[11px] text-muted transition-colors hover:text-foreground disabled:opacity-50"
                onClick={() => void store.refreshRates()}
                disabled={store.ratesRefreshing()}
              >
                <IconRefresh size={11} />
                <span>{store.ratesRefreshing() ? "Refreshing" : "Refresh rates"}</span>
              </button>
            </div>
          </div>
        </Show>
      </div>
    </div>
  );
}

/** One headline number and what it counts. */
function Figure(props: { value: string; label: string }) {
  return (
    <div class="flex flex-col gap-0.5">
      <span class="text-2xl font-medium leading-none tabular-nums text-foreground">
        {props.value}
      </span>
      <span class="section-label">{props.label}</span>
    </div>
  );
}

/** A sortable heading with a stable indicator slot, emphasized for the active sort. */
function SortButton(props: {
  column: { id: SessionSort; label: string };
  active: boolean;
  onSelect: (id: SessionSort) => void;
}) {
  return (
    <button
      type="button"
      class={`focus-ring inline-flex items-center gap-1 rounded-xs whitespace-nowrap transition-colors ${
        props.active ? "font-semibold text-foreground" : "text-muted hover:text-foreground"
      }`}
      title={
        props.column.id === "recent"
          ? "Newest activity first"
          : `Largest ${props.column.label.toLowerCase()} first`
      }
      onClick={() => props.onSelect(props.column.id)}
    >
      {props.column.label}
      <span class={props.active ? "text-foreground" : "text-muted/60"} aria-hidden="true">
        <IconChevronDown size={10} />
      </span>
    </button>
  );
}

/** One finding, and the session it is about when it is about exactly one. */
function FindingRow(props: {
  finding: UsageFinding;
  onOpenSession: (row: { session_id: string }) => void;
}) {
  const sessionId = () => props.finding.session_id;
  return (
    <li class="border-b border-line/60 pb-2 last:border-0">
      <div class="flex items-baseline gap-1.5">
        <Show
          when={sessionId()}
          fallback={<p class="text-xs text-foreground">{props.finding.headline}</p>}
        >
          {(id) => (
            <button
              type="button"
              class="focus-ring rounded-xs text-left text-xs text-foreground hover:underline"
              title="Show this session in the table below"
              onClick={() => props.onOpenSession({ session_id: id() })}
            >
              {props.finding.headline}
            </button>
          )}
        </Show>
        <Show when={props.finding.count > 1}>
          <span class="shrink-0 rounded-full bg-raised px-1.5 font-mono text-[10px] text-muted">
            {props.finding.count}
          </span>
        </Show>
      </div>
      <p class="mt-0.5 text-xs text-muted">{props.finding.detail}</p>
    </li>
  );
}

/** One session, and everything about it that does not fit a row, one click away. */
function SessionRow(props: {
  row: UsageSessionRow;
  expanded: boolean;
  laneIsLive: boolean;
  columns: SessionColumnVisibility;
  colSpan: number;
  onToggle: () => void;
  onOpenLane?: (laneId: number) => void;
}) {
  const row = () => props.row;
  const task = () => {
    const headline = row().headline?.trim();
    return headline ? headline : "Untitled session";
  };
  const cell = () => laneCell(row());
  const share = () => subagentShare(row().totals);
  const lane = () => row().lane_label ?? row().cwd ?? "";
  const duration = () => formatDuration(
    row().started_at && row().ended_at ? sessionDurationMs(row()) : NaN,
  );

  return (
    <>
      <tr
        class="cursor-pointer border-b border-line/30 hover:bg-raised/60"
        onClick={() => props.onToggle()}
      >
        <td class="min-w-0 px-2 py-2 text-foreground" title={row().headline_raw ?? task()}>
          <button
            type="button"
            class="focus-ring flex w-full min-w-0 items-center gap-1 rounded-xs text-left"
            aria-expanded={props.expanded}
            onClick={(event) => {
              event.stopPropagation();
              props.onToggle();
            }}
          >
            <span class="shrink-0 text-muted" aria-hidden="true">
              <Show when={props.expanded} fallback={<IconChevronRight size={10} />}>
                <IconChevronDown size={10} />
              </Show>
            </span>
            <span class={`min-w-0 flex-1 truncate ${row().headline ? "" : "text-muted"}`}>
              {task()}
            </span>
          </button>
        </td>
        <td class="px-2 py-2 text-muted" title={row().model || row().agent_kind}>
          <span class="block truncate">{truncateMiddle(row().model || row().agent_kind, AGENT_ID_MAX_CHARS)}</span>
        </td>
        <td class="px-2 py-2">
          <Show
            when={props.laneIsLive && row().lane_id !== null}
            fallback={
              <span class="flex min-w-0 items-center gap-1" title={cell().title}>
                <span class="truncate-start min-w-0 flex-1 text-muted"><bdi dir="ltr">{cell().label}</bdi></span>
                <Show when={cell().external}>
                  <span class="shrink-0 text-[10px] text-muted">external</span>
                </Show>
              </span>
            }
          >
            <button
              type="button"
              class="focus-ring truncate-start block w-full min-w-0 rounded-xs text-signal hover:underline"
              title={row().cwd ?? undefined}
              onClick={(event) => {
                event.stopPropagation();
                props.onOpenLane?.(row().lane_id as number);
              }}
            >
              <bdi dir="ltr">{lane()}</bdi>
            </button>
          </Show>
        </td>
        <td class="px-2 py-2 text-right tabular-nums text-muted">{row().turns}</td>
        <Show when={props.columns.tools}>
          <td class="px-2 py-2 text-right tabular-nums text-muted">{row().tool_calls}</td>
        </Show>
        <Show when={props.columns.subagents}>
          <td
            class="px-2 py-2 text-right tabular-nums text-muted"
            title={
              share() === null
                ? "No subagent turns"
                : `${formatTokens(row().totals.subagent_tokens)} spent by subagents`
            }
          >
            {share() === null ? "-" : `${share()}%`}
          </td>
        </Show>
        <Show when={props.columns.retries}>
          <td
            class={`px-2 py-2 text-right tabular-nums ${retryTone(row().retries) === "notice" ? "font-medium text-attention" : "text-muted"}`}
          >
            {row().retries}
          </td>
        </Show>
        <td class="px-2 py-2 text-right tabular-nums text-muted">{duration()}</td>
        <td class="px-2 py-2 text-right tabular-nums text-muted">
          {formatTokens(row().totals.total_tokens)}
        </td>
        <td class="px-2 py-2 text-right font-semibold tabular-nums text-foreground">
          {formatUsd(row().totals.cost_usd)}
        </td>
      </tr>
      <Show when={props.expanded}>
        <tr class="border-b border-line/30 bg-raised/40">
          <td colspan={props.colSpan} class="usage-detail-cell">
            <div class="usage-session-details">
              <div class="min-w-0 px-2 py-3">
                <p class="mb-2 break-words text-xs text-foreground" title={row().headline_raw ?? undefined}>{task()}</p>
                <dl><SessionIdDetail sessionId={row().session_id} /></dl>
              </div>
              <dl class="min-w-0 space-y-2 px-2 py-3">
                <Detail term="Model" value={row().model || "unknown"} />
                <Detail term="Agent" value={row().agent_kind} />
              </dl>
              <dl class="min-w-0 space-y-2 px-2 py-3">
                <Detail term="Lane" value={lane() || "outside a lane"} />
                <Detail term="Window" value={windowLine(row().started_at, duration())} />
              </dl>
              <dl class="col-start-4 col-end-[-1] grid grid-cols-3 gap-y-2 py-3">
                <Detail numeric term="Input" value={formatTokens(row().totals.input_tokens)} />
                <Detail numeric term="Output" value={formatTokens(row().totals.output_tokens)} />
                <Detail numeric term="Cache read" value={formatTokens(row().totals.cache_read_tokens)} />
                <Detail numeric term="Cache write" value={formatTokens(row().totals.cache_write_tokens)} />
                <Detail numeric term="Thinking" value={formatTokens(row().totals.thinking_tokens)} />
                <Detail numeric term="Subagents" value={share() === null ? "none" : `${formatTokens(row().totals.subagent_tokens)} · ${share()}%`} />
                <Detail numeric term="Turns" value={String(row().turns)} />
                <Detail numeric term="Tools" value={String(row().tool_calls)} />
                <Detail numeric term="Retries" value={String(row().retries)} />
                <Show when={row().estimated}><Detail term="Counts" value="estimated from content length" /></Show>
              </dl>
            </div>
          </td>
        </tr>
      </Show>
    </>
  );
}

function Detail(props: { term: string; value: string; numeric?: boolean }) {
  return (
    <div class={props.numeric ? "min-w-0 px-2 text-right tabular-nums" : "min-w-0"}>
      <dt class="section-label">{props.term}</dt>
      <dd class="break-words text-xs text-foreground" title={props.value}>{props.value}</dd>
    </div>
  );
}

/** The session id: muted so it reads as an identifier rather than a fact, with a copy button next
 * to it since it is the one detail an operator is likely to paste elsewhere (a bug report, a log
 * search) rather than just read. */
function SessionIdDetail(props: { sessionId: string }) {
  const [copied, setCopied] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));

  async function copy(event: MouseEvent) {
    event.stopPropagation();
    try {
      await navigator.clipboard.writeText(props.sessionId);
      setCopied(true);
      clearTimeout(timer);
      timer = setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard access can be denied; the id is still visible to select and copy by hand.
    }
  }

  return (
    <div class="min-w-0">
      <dt class="section-label">Session</dt>
      <dd class="flex items-center gap-1">
        <span class="truncate font-mono text-[11px] text-muted" title={props.sessionId}>
          {props.sessionId}
        </span>
        <button
          type="button"
          class="focus-ring shrink-0 rounded-xs p-0.5 text-muted transition-colors hover:text-foreground"
          title="Copy session ID"
          aria-label="Copy session ID"
          onClick={copy}
        >
          <Show when={copied()} fallback={<IconCopy size={10} />}>
            <IconCheck size={10} class="text-signal" />
          </Show>
        </button>
      </dd>
    </div>
  );
}
