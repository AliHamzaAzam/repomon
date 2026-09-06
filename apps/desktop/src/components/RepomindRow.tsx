import { Show, onCleanup, onMount } from "solid-js";

import { stateIndicator, type ControllerSummary } from "../stores/fleet";
import { IconBrain, IconLayers, IconPlay, IconSparkles, IconStop } from "./icons";

export interface RepomindRowProps {
  controller: ControllerSummary;
  /// Goals in flight, from `repomind.status.counts.active_plans`. Null until the first status
  /// read lands, which is when the row simply says nothing about goals rather than claiming zero.
  activePlans: number | null;
  /// The home repo path, shown where a lane row shows its branch.
  home: string | null;
  selected: boolean;
  onSelect: () => void;
  onContextMenu: (x: number, y: number) => void;
}

/// Everything the row's context menu offers, in the order the menu lists it. Kept next to the row
/// so the menu and the row can never drift apart on what "start" means.
export type RepomindMenuAction = "start" | "stop" | "panel" | "home";

/// The row's right-click menu: the controller's lifecycle, the panel that details it, and the home
/// itself opened in the editor. Mirrors the panel header's controls, so whichever surface the
/// operator reaches for first offers the same four things.
export function RepomindRowMenu(props: {
  running: boolean;
  x: number;
  y: number;
  onAction: (action: RepomindMenuAction) => void;
  onClose: () => void;
}) {
  let menuRef: HTMLDivElement | undefined;
  let previouslyFocused: HTMLElement | null = null;

  function onKey(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      props.onClose();
      previouslyFocused?.focus();
      return;
    }
    const items = [...(menuRef?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])];
    const index = items.findIndex((item) => item === document.activeElement);
    let next: number;
    if (event.key === "ArrowDown") next = (index + 1) % items.length;
    else if (event.key === "ArrowUp") next = (index - 1 + items.length) % items.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = items.length - 1;
    else return;
    event.preventDefault();
    event.stopPropagation();
    items[next]?.focus();
  }
  onMount(() => {
    previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    menuRef?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    window.addEventListener("keydown", onKey, true);
  });
  onCleanup(() => window.removeEventListener("keydown", onKey, true));

  const left = () => Math.max(8, Math.min(props.x, window.innerWidth - 224 - 8));
  const top = () => Math.max(8, Math.min(props.y, window.innerHeight - 180));
  const item =
    "focus-ring flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium transition-colors hover:bg-raised";

  const choose = (action: RepomindMenuAction) => {
    props.onAction(action);
    props.onClose();
  };

  return (
    <>
      <div class="fixed inset-0 z-40" onClick={() => props.onClose()} />
      <div
        ref={menuRef}
        class="fixed z-50 w-56 rounded-xl border border-line bg-surface p-1.5 shadow-[0_12px_40px_var(--shadow)]"
        style={{ left: `${left()}px`, top: `${top()}px` }}
        role="menu"
        aria-label="Repomind"
      >
        <Show
          when={props.running}
          fallback={
            <button type="button" class={`${item} text-foreground`} role="menuitem" onClick={() => choose("start")}>
              <IconPlay size={13} />
              <span>Start Repomind</span>
            </button>
          }
        >
          <button type="button" class={`${item} text-fault`} role="menuitem" onClick={() => choose("stop")}>
            <IconStop size={13} />
            <span>Stop Repomind</span>
          </button>
        </Show>
        <button type="button" class={`${item} text-foreground`} role="menuitem" onClick={() => choose("panel")}>
          <IconSparkles size={13} />
          <span>Open panel</span>
        </button>
        <button type="button" class={`${item} text-foreground`} role="menuitem" onClick={() => choose("home")}>
          <IconLayers size={13} />
          <span>Open home in editor</span>
        </button>
      </div>
    </>
  );
}

/// Tone classes for the one-word state vocabulary, written out per tone rather than interpolated:
/// Tailwind only emits classes it can read as whole strings in the source.
const DOT_TONE = {
  signal: "bg-signal",
  attention: "bg-attention",
  fault: "bg-fault",
  muted: "bg-muted/50",
} as const;

/// The Repomind toolbar button's state dot, the counterpart to Repomail's unread badge: a signal
/// dot while a controller runs, the attention (or fault) color when one wants the operator, and
/// nothing at all when the home is off. It says only "look here"; the word for what is happening
/// lives on the pinned sidebar row and in the panel header.
export function RepomindStateDot(props: { controller: ControllerSummary }) {
  const indicator = () => stateIndicator(props.controller.agents ? props.controller.state : null);
  return (
    <Show when={props.controller.agents > 0}>
      <span
        class={`size-1.5 shrink-0 rounded-full ${DOT_TONE[indicator().tone]}`}
        title={`Repomind: ${indicator().label}`}
        aria-label={`Repomind: ${indicator().label}`}
        role="img"
      />
    </Show>
  );
}

/// The pinned Repomind row: one lane row's worth of the fleet's own grammar, standing in for the
/// repo group the home would otherwise get. Line 1 names it and states it in the same one-word
/// vocabulary every lane pill uses; line 2 says where the home is and what it holds.
///
/// Clicking it selects the controller lane, which is what puts its agents in the terminal bay -
/// exactly what clicking a lane row does, because this is a lane row for a lane the groups hide.
export default function RepomindRow(props: RepomindRowProps) {
  const indicator = () => stateIndicator(props.controller.agents ? props.controller.state : null);
  const running = () => props.controller.agents > 0;
  const agentLabel = () =>
    `${props.controller.agents} controller${props.controller.agents === 1 ? "" : "s"} in the repomind home`;
  const goalLabel = () => {
    const plans = props.activePlans;
    if (plans === null) return undefined;
    return `${plans} goal${plans === 1 ? "" : "s"} in flight, from plans/active`;
  };
  const rowTitle = () =>
    running()
      ? `Repomind: ${indicator().label}. ${agentLabel()}.`
      : "Repomind is off. Right-click to start a controller.";

  return (
    <button
      type="button"
      class={`group/repomind-row fleet-row is-stacked focus-ring ${props.selected ? "is-selected" : ""}`}
      onClick={() => props.onSelect()}
      onContextMenu={(event) => {
        event.preventDefault();
        event.currentTarget.focus();
        props.onContextMenu(event.clientX, event.clientY);
      }}
      onKeyDown={(event) => {
        if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
        event.preventDefault();
        event.stopPropagation();
        const bounds = event.currentTarget.getBoundingClientRect();
        props.onContextMenu(bounds.left, bounds.bottom);
      }}
      aria-haspopup="menu"
      aria-current={props.selected ? "true" : undefined}
      title={rowTitle()}
    >
      {/* Line 1: the mark, the name, the needs-you pip, and the state pill - the same order and
          the same widths a lane row uses, so the two read as one column. */}
      <div class="flex min-w-0 items-center gap-1.5">
        <span
          class={`flex size-3 shrink-0 items-center justify-center ${running() ? "text-signal" : "text-muted/60"}`}
        >
          <IconBrain size={12} />
        </span>
        <span
          class={`min-w-0 flex-1 truncate text-left text-xs ${
            props.selected ? "font-semibold text-foreground" : "font-medium text-foreground/90"
          }`}
        >
          Repomind
        </span>
        <Show when={props.controller.urgent}>
          <span
            class="inline-flex shrink-0 items-center gap-1 font-mono text-[10px] font-semibold leading-none text-attention"
            title={`${props.controller.urgent} controller${props.controller.urgent === 1 ? "" : "s"} need you`}
          >
            <span class="size-1.5 rounded-full bg-attention" />
            {props.controller.urgent}
          </span>
        </Show>
        <span class={`lane-status is-${indicator().tone}`}>{indicator().label}</span>
      </div>

      {/* Line 2: where the home lives, then what it holds. */}
      <div class="flex min-w-0 items-center gap-1.5">
        <span class="size-3 shrink-0" aria-hidden="true" />
        <span class="truncate-tail min-w-0 flex-1 font-mono text-[10px] text-muted/70" title={props.home ?? undefined}>
          {props.home ?? "home not created yet"}
        </span>
        <Show when={props.controller.agents > 0}>
          <span
            class="inline-flex shrink-0 items-center gap-0.5 font-mono text-[10px] leading-none text-muted"
            aria-label={agentLabel()}
            title={agentLabel()}
          >
            <IconLayers size={9} class="shrink-0 text-muted/70" />
            {props.controller.agents}
          </span>
        </Show>
        <Show when={props.activePlans !== null}>
          <span class="shrink-0 font-mono text-[10px] leading-none text-muted" title={goalLabel()}>
            {props.activePlans} {props.activePlans === 1 ? "goal" : "goals"}
          </span>
        </Show>
      </div>
    </button>
  );
}
