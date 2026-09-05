import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import { IconChevronDown, IconChevronLeft, IconChevronRight } from "./icons";

export interface UsageRangePickerProps {
  /// What the button reads when the popover is shut.
  label: string;
  /// Whether the current window came from this picker rather than a named range.
  active: boolean;
  /// Both ends of the chosen window, at the day boundaries the calendar works in.
  onApply: (from: Date, to: Date, label?: string) => void;
}

/** Local midnight on the day `at` falls in. */
function startOfDay(at: Date): Date {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate());
}

/** The last instant of the day `at` falls in, so a window that ends today includes today. */
function endOfDay(at: Date): Date {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate(), 23, 59, 59, 999);
}

function addDays(at: Date, days: number): Date {
  const next = new Date(at);
  next.setDate(next.getDate() + days);
  return next;
}

function addMonths(at: Date, months: number): Date {
  const next = new Date(at);
  next.setDate(1);
  next.setMonth(next.getMonth() + months);
  return next;
}

function sameDay(a: Date, b: Date): boolean {
  return a.toDateString() === b.toDateString();
}

function sameMonth(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth();
}

/** Today's midnight is never a live signal here: nothing in this popover needs to move as the
 * clock ticks past midnight while it happens to be open, and reading it once keeps every future
 * check comparing against the same instant. */
function today(): Date {
  return startOfDay(new Date());
}

/** The window can never reach into the future: `to` is clamped at today and `from` at whatever
 * `to` came out to, so a stray future date never becomes a read the daemon has no data for. */
function clampToToday(from: Date, to: Date): [Date, Date] {
  const limit = today();
  const clampedTo = to > limit ? limit : to;
  const clampedFrom = from > clampedTo ? clampedTo : from;
  return [clampedFrom, clampedTo];
}

/** The six-week grid a month is drawn on, starting on Monday. */
function monthGrid(month: Date): Date[] {
  const first = new Date(month.getFullYear(), month.getMonth(), 1);
  const lead = (first.getDay() + 6) % 7;
  const start = addDays(first, -lead);
  return Array.from({ length: 42 }, (_, index) => addDays(start, index));
}

const WEEKDAYS = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/**
 * A window picked by hand: two days off a calendar, or one of the two presets people actually
 * ask for. It is a popover rather than a page because picking a range is a detour from reading
 * the numbers, and the numbers stay on screen behind it.
 *
 * Keyboard is the primary path: the grid holds one tab stop, the arrows walk days and weeks,
 * PageUp and PageDown walk months, and Enter takes the day under the cursor as the next end.
 */
export default function UsageRangePicker(props: UsageRangePickerProps) {
  const [open, setOpen] = createSignal(false);
  const [month, setMonth] = createSignal(startOfDay(new Date()));
  const [cursor, setCursor] = createSignal(startOfDay(new Date()));
  const [from, setFrom] = createSignal<Date | null>(null);
  const [to, setTo] = createSignal<Date | null>(null);
  let rootRef: HTMLDivElement | undefined;
  let gridRef: HTMLDivElement | undefined;

  function close() {
    setOpen(false);
  }

  function toggle() {
    const next = !open();
    setOpen(next);
    if (next) {
      // The last 7 days ending today, so a picker opened and applied without touching a day
      // still lands on a sensible window rather than an empty one.
      setFrom(addDays(today(), -6));
      setTo(today());
      queueMicrotask(() => gridRef?.focus());
    }
  }

  // One document listener rather than one per open, so nothing leaks when the view unmounts.
  function onPointerDown(event: PointerEvent) {
    if (!open()) return;
    if (rootRef && event.target instanceof Node && rootRef.contains(event.target)) return;
    close();
  }
  onMount(() => document.addEventListener("pointerdown", onPointerDown, true));
  onCleanup(() => document.removeEventListener("pointerdown", onPointerDown, true));

  function pick(day: Date) {
    if (day > today()) return; // A future day is shown muted and takes no click.
    const start = from();
    if (!start || to()) {
      setFrom(day);
      setTo(null);
      return;
    }
    setTo(day);
  }

  function apply() {
    const start = from();
    if (!start) return;
    const end = to() ?? start;
    const [a, b] = start <= end ? [start, end] : [end, start];
    const [clampedA, clampedB] = clampToToday(a, b);
    props.onApply(startOfDay(clampedA), endOfDay(clampedB));
    close();
  }

  function applyMonth(offset: number) {
    const anchor = addMonths(startOfDay(new Date()), offset);
    const first = new Date(anchor.getFullYear(), anchor.getMonth(), 1);
    const last = new Date(anchor.getFullYear(), anchor.getMonth() + 1, 0);
    const now = new Date();
    const end = offset === 0 && last > now ? now : endOfDay(last);
    props.onApply(first, end, offset === 0 ? "This month" : "Last month");
    close();
  }

  function onGridKeyDown(event: KeyboardEvent) {
    const moves: Record<string, number> = {
      ArrowLeft: -1,
      ArrowRight: 1,
      ArrowUp: -7,
      ArrowDown: 7,
    };
    const step = moves[event.key];
    if (step !== undefined) {
      event.preventDefault();
      const next = addDays(cursor(), step);
      setCursor(next);
      setMonth(startOfDay(new Date(next.getFullYear(), next.getMonth(), 1)));
      return;
    }
    if (event.key === "PageUp" || event.key === "PageDown") {
      event.preventDefault();
      const next = addMonths(cursor(), event.key === "PageUp" ? -1 : 1);
      setCursor(next);
      setMonth(next);
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      pick(cursor());
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  }

  const days = createMemo(() => monthGrid(month()));
  const inRange = (day: Date) => {
    const start = from();
    const end = to();
    if (!start || !end) return false;
    const [a, b] = start <= end ? [start, end] : [end, start];
    return day >= startOfDay(a) && day <= endOfDay(b);
  };
  const isEdge = (day: Date) => {
    const start = from();
    const end = to();
    return (start !== null && sameDay(day, start)) || (end !== null && sameDay(day, end));
  };

  return (
    <div class="relative" ref={rootRef}>
      <button
        type="button"
        class={`focus-ring flex h-7 items-center gap-1 rounded-md px-2 text-xs transition-colors ${
          props.active ? "bg-signal/12 font-semibold text-signal" : "text-muted hover:text-foreground"
        }`}
        aria-haspopup="dialog"
        aria-expanded={open()}
        onClick={toggle}
      >
        <span>{props.active ? props.label : "Custom"}</span>
        <IconChevronDown size={10} />
      </button>

      <Show when={open()}>
        <div
          class="absolute left-0 top-8 z-50 w-64 rounded-xl border border-line bg-surface p-3 shadow-2xl"
          role="dialog"
          aria-label="Pick a date range"
        >
          <div class="mb-2 flex gap-1.5">
            <button
              type="button"
              class="focus-ring flex-1 rounded-md border border-line px-2 py-1 text-xs text-muted transition-colors hover:bg-raised hover:text-foreground"
              onClick={() => applyMonth(0)}
            >
              This month
            </button>
            <button
              type="button"
              class="focus-ring flex-1 rounded-md border border-line px-2 py-1 text-xs text-muted transition-colors hover:bg-raised hover:text-foreground"
              onClick={() => applyMonth(-1)}
            >
              Last month
            </button>
          </div>

          <div class="mb-1.5 flex items-center justify-between">
            <button
              type="button"
              class="focus-ring rounded p-1 text-muted transition-colors hover:text-foreground"
              aria-label="Previous month"
              onClick={() => setMonth(addMonths(month(), -1))}
            >
              <IconChevronLeft size={12} />
            </button>
            <span class="text-xs font-semibold text-foreground">
              {month().toLocaleDateString(undefined, { month: "long", year: "numeric" })}
            </span>
            <button
              type="button"
              class="focus-ring rounded p-1 text-muted transition-colors hover:text-foreground disabled:opacity-30 disabled:hover:text-muted"
              aria-label="Next month"
              disabled={sameMonth(month(), today())}
              onClick={() => setMonth(addMonths(month(), 1))}
            >
              <IconChevronRight size={12} />
            </button>
          </div>

          <div class="grid grid-cols-7 gap-px" aria-hidden="true">
            <For each={WEEKDAYS}>
              {(name) => (
                <span class="py-0.5 text-center text-[10px] font-medium text-muted">{name}</span>
              )}
            </For>
          </div>

          <div
            ref={gridRef}
            class="focus-ring grid grid-cols-7 gap-px rounded"
            role="grid"
            aria-label="Days"
            tabindex="0"
            onKeyDown={onGridKeyDown}
          >
            <For each={days()}>
              {(day) => {
                const outside = () => day.getMonth() !== month().getMonth();
                const selected = () => isEdge(day);
                const covered = () => inRange(day) && !selected();
                const focused = () => sameDay(day, cursor());
                // A day after today is not a read the daemon can answer, so it is shown muted
                // and takes neither hover nor a click rather than quietly picking a future date.
                const future = () => day > today();
                return (
                  <button
                    type="button"
                    role="gridcell"
                    tabindex="-1"
                    aria-selected={selected()}
                    aria-disabled={future()}
                    aria-label={day.toDateString()}
                    disabled={future()}
                    class={`h-7 rounded text-center text-[11px] tabular-nums transition-colors ${
                      future()
                        ? "cursor-default text-muted/30"
                        : selected()
                          ? "bg-signal font-semibold text-background"
                          : covered()
                            ? "bg-signal/15 text-foreground"
                            : outside()
                              ? "text-muted/50 hover:bg-raised"
                              : "text-foreground hover:bg-raised"
                    } ${focused() && !selected() ? "ring-1 ring-signal/60" : ""}`}
                    onClick={() => {
                      if (future()) return;
                      setCursor(day);
                      pick(day);
                      gridRef?.focus();
                    }}
                  >
                    {day.getDate()}
                  </button>
                );
              }}
            </For>
          </div>

          <div class="mt-2 flex items-center justify-between gap-2">
            <span class="min-w-0 flex-1 truncate text-[11px] text-muted">
              {from()
                ? to()
                  ? `${from()?.toLocaleDateString()} to ${to()?.toLocaleDateString()}`
                  : "Pick the last day"
                : "Pick the first day"}
            </span>
            <button
              type="button"
              class="focus-ring rounded-md border border-signal/50 bg-signal/10 px-2 py-0.5 text-xs font-semibold text-signal transition-colors hover:bg-signal/20 disabled:opacity-40"
              disabled={!from()}
              onClick={apply}
            >
              Apply
            </button>
          </div>
        </div>
      </Show>
    </div>
  );
}
