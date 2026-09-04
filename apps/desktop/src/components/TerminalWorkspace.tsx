import { For, Show, createEffect, createMemo, createSignal, lazy, onCleanup, onMount } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import type { ActionsStore } from "../stores/actions";
import type { EditorStore } from "../stores/editor";
import type { FleetStore } from "../stores/fleet";
import type { WorkspaceLayout, WorkspaceStore } from "../stores/workspace";
import { notifyLayoutChanged } from "../stores/uiSettings";
import Select from "./controls/Select";
import PanePicker from "./PanePicker";
import { agentLabel } from "./agentLabel";
import { agentSessionOrderKey, agentSessionTargetId } from "./agentIdentity";
import { agentSessionTitle } from "./LaneAgentRosterPopover";
import { createPointerReorder } from "./pointerReorder";
import {
  paneAccent,
  stableVisibleTargets,
  warmTargetWindows,
  type PaneTarget,
} from "./terminalTargets";
import {
  AgentIcon,
  IconBot,
  IconChevronLeft,
  IconChevronRight,
  IconClose,
  IconFocus,
  IconGrid,
  IconPlus,
  IconSplit,
  IconTerminal,
} from "./icons";

const TerminalPane = lazy(() => import("./TerminalPane"));
// Used only until a live xterm reports its authoritative minimum height.
const MULTITASK_FALLBACK_ROW_HEIGHT_PX = 224;

/// The shared Multitasking row minimum: the largest floor any currently visible pane has
/// reported, or the fallback while nothing has reported yet. Pulled out as a pure function (used
/// by the `multitaskRowMinimum` memo below) so it can be exercised directly with a plain heights
/// map instead of a full render tree. A pane's floor is a fixed value now (see
/// `terminalMetrics.ts`), so lowering it is just this recomputing over fresh input, and a pane
/// that stops being visible is excluded simply by not appearing in `visibleWindows`.
export function multitaskRowMinimumFromHeights(
  fallback: number,
  heights: Record<string, number>,
  visibleWindows: readonly string[],
): number {
  return Math.max(
    fallback,
    ...visibleWindows.flatMap((window) => {
      const height = heights[window];
      return height ? [height] : [];
    }),
  );
}

/// Drop recorded minimums for windows no longer in view. `multitaskRowMinimumFromHeights` already
/// ignores entries outside `visibleWindows`, so this doesn't change the current row minimum, but
/// without it a pane that closes and is later replaced by a new window reusing bookkeeping would
/// grow `paneMinimumHeights` forever, and a pane that leaves and re-enters the view briefly holds
/// a stale minimum from before its layout even changed. Keeps the same object when nothing needs
/// dropping so it doesn't trigger an extra reactive update.
export function pruneInvisiblePaneMinimumHeights(
  heights: Record<string, number>,
  visibleWindows: ReadonlySet<string>,
): Record<string, number> {
  let changed = false;
  const next: Record<string, number> = {};
  for (const [window, value] of Object.entries(heights)) {
    if (visibleWindows.has(window)) {
      next[window] = value;
    } else {
      changed = true;
    }
  }
  return changed ? next : heights;
}

interface TerminalWorkspaceProps {
  fleet: FleetStore;
  actions: ActionsStore;
  workspace: WorkspaceStore;
  editor?: EditorStore;
  onEnsureEditorOpen?: () => void;
}

export default function TerminalWorkspace(props: TerminalWorkspaceProps) {
  let tabStripRef: HTMLDivElement | undefined;
  const [canScrollLeft, setCanScrollLeft] = createSignal(false);
  const [canScrollRight, setCanScrollRight] = createSignal(false);
  const [openingShell, setOpeningShell] = createSignal(false);
  const [adopting, setAdopting] = createSignal(false);
  const [workspaceError, setWorkspaceError] = createSignal<string | null>(null);
  const [warmWindows, setWarmWindows] = createSignal<string[]>([]);
  const [paneMinimumHeights, setPaneMinimumHeights] = createSignal<Record<string, number>>({});

  const updateScrollIndicators = () => {
    if (!tabStripRef) return;
    const { scrollLeft, scrollWidth, clientWidth } = tabStripRef;
    const maxScroll = scrollWidth - clientWidth;
    setCanScrollLeft(scrollLeft > 2);
    setCanScrollRight(maxScroll > 2 && scrollLeft < maxScroll - 2);
  };

  const scrollByDelta = (delta: number) => {
    if (!tabStripRef) return;
    tabStripRef.scrollBy({ left: delta, behavior: "smooth" });
  };

  const onTabStripWheel = (event: WheelEvent) => {
    if (!tabStripRef) return;
    if (Math.abs(event.deltaY) > Math.abs(event.deltaX) && tabStripRef.scrollWidth > tabStripRef.clientWidth) {
      event.preventDefault();
      tabStripRef.scrollLeft += event.deltaY;
      updateScrollIndicators();
    }
  };

  const layout = () => props.workspace.layout();
  const renderer = () => props.workspace.renderer();
  const activeWindow = () => props.workspace.activeWindow();
  const setActiveWindow = props.workspace.setActiveWindow;
  const targets = () => props.workspace.targets();
  const laneTargets = () => props.workspace.laneTargets();
  const selectedLaneTargets = () => props.workspace.selectedLaneTargets();
  const multitasking = () => props.workspace.multitasking();

  // Manual tab mode (settings): agent pills drag-to-reorder, right-click renames. Shells are
  // plain terminals and stay out of both. The optimistic order holds the strip steady until the
  // daemon returns the persisted arrangement or another surface changes it.
  const tabsReorderable = () => props.fleet.tabSortMode() === "manual";
  const [localTabOrder, setLocalTabOrder] = createSignal<string[] | null>(null);
  /// The lane's targets in display order: the persisted manual arrangement while it's set,
  /// otherwise the wire order. Only agent pills carry transcript ids; shells keep their place.
  const stripTargets = createMemo(() => {
    const all = laneTargets();
    const order = localTabOrder();
    if (!order || order.length === 0) return all;
    const position = new Map(order.map((sid, index) => [sid, index]));
    const rank = (target: PaneTarget) =>
      target.targetId !== null
        ? (position.get(target.targetId) ?? Number.MAX_SAFE_INTEGER)
        : Number.MAX_SAFE_INTEGER;
    return [...all].sort((a, b) => rank(a) - rank(b));
  });

  // Chrome-style pointer dragging for the strip's agent pills (see pointerReorder.ts). Shells
  // and placeholder pills (no transcript id) never participate.
  const tabDrag = createPointerReorder<string>({
    axis: "x",
    ids: () =>
      stripTargets()
        .map((target) => target.targetId)
        .filter((id): id is string => id !== null),
    reorder: setLocalTabOrder,
    commit: (order) => {
      const laneId = props.fleet.selectedLaneId();
      if (laneId !== null) void props.actions.setAgentTabOrder(laneId, order);
    },
    enabled: () => tabsReorderable(),
  });
  // Drop the optimistic order whenever the authoritative lane/order changes. This also catches a
  // reorder committed by the sidebar roster, keeping the two surfaces synchronized without a lane
  // switch. The lane id is part of the comparison so a same-shaped order in another lane still
  // tears down a drag against the old lane.
  let lastBackendLaneId: number | null | undefined;
  let lastBackendOrder: string | undefined;
  createEffect(() => {
    const laneId = props.fleet.selectedLaneId();
    const backendOrder = agentSessionOrderKey(props.fleet.selectedLane()?.agent_sessions ?? []);
    if (laneId !== lastBackendLaneId || backendOrder !== lastBackendOrder) {
      lastBackendLaneId = laneId;
      lastBackendOrder = backendOrder;
      tabDrag.abort();
      setLocalTabOrder(null);
    }
  });
  onCleanup(() => tabDrag.abort());

  const sessionForTarget = (target: PaneTarget) =>
    props.fleet
      .selectedLane()
      ?.agent_sessions.find((session) => session.tmux_window === target.window) ?? null;

  const onTabContextMenu = (target: PaneTarget, event: MouseEvent) => {
    if (target.shell) return;
    const session = sessionForTarget(target);
    if (!session) return;
    const targetId = agentSessionTargetId(session);
    if (!targetId) return;
    event.preventDefault();
    props.actions.rename({
      targetId,
      sessionId: session.session_id,
      current: agentSessionTitle(session),
    });
  };

  const labelByWindow = createMemo(() => {
    const map = new Map<string, string>();
    for (const lane of props.fleet.lanes()) {
      lane.agent_sessions.forEach((agent) => {
        if (agent.tmux_window) {
          map.set(agent.tmux_window, agentLabel(agent));
        }
      });
    }
    return map;
  });
  const labelOf = (target: PaneTarget) => labelByWindow().get(target.window) ?? target.label;

  const effectiveLayout = createMemo(() => {
    if (multitasking()) return "grid" as const;
    const l = layout();
    if (l !== "auto") return l;
    const count = selectedLaneTargets().length;
    if (count <= 1) return "focused";
    if (count === 2) return "split";
    return "grid";
  });

  const visibleTargets = createMemo(() => {
    if (multitasking()) return props.workspace.multitaskTargets();
    const all = selectedLaneTargets();
    const active = laneTargets().find((target) => target.window === activeWindow()) ?? laneTargets()[0];
    if (!active) return [];
    const eff = effectiveLayout();
    if (eff === "focused") return [active];
    // Split/grid use a selection-independent order: clicking an agent moves focus, not the grid.
    return stableVisibleTargets(all, active.window, eff);
  });

  const multitaskRowMinimum = createMemo(() => multitaskRowMinimumFromHeights(
    MULTITASK_FALLBACK_ROW_HEIGHT_PX,
    paneMinimumHeights(),
    visibleTargets().map((target) => target.window),
  ));

  const recordPaneMinimumHeight = (window: string, pixels: number) => {
    setPaneMinimumHeights((current) => {
      if (current[window] === pixels) return current;
      return { ...current, [window]: pixels };
    });
  };

  createEffect(() => {
    const visible = new Set(visibleTargets().map((target) => target.window));
    setPaneMinimumHeights((current) => pruneInvisiblePaneMinimumHeights(current, visible));
  });

  createEffect(() => {
    const available = targets();
    const visible = visibleTargets();
    setWarmWindows((previous) => warmTargetWindows(
      previous,
      visible,
      available,
      multitasking() ? Math.max(6, visible.length) : 6,
    ));
  });

  createEffect(() => {
    // Notify terminal panes when layout mode or visible targets change
    effectiveLayout();
    activeWindow();
    visibleTargets();
    notifyLayoutChanged();
  });

  const mountedTargets = createMemo(() => {
    const byWindow = new Map(targets().map((target) => [target.window, target]));
    const warm = warmWindows().flatMap((window) => {
      const target = byWindow.get(window);
      return target ? [target] : [];
    });
    // `warmWindows` is bookkeeping maintained by a createEffect, which can lag one tick behind
    // the synchronous `visibleTargets()` memo it reads (e.g. right after a layout toggle or a
    // picker selection change). A window counted in "N panes" and given a CSS grid placement
    // must never end up with no mounted <TerminalPane> behind it — that's an empty, borderless
    // grid cell the operator sees as a blank pane. Union in anything currently visible that the
    // warm cache hasn't caught up to yet, so the render is always a strict superset of what's
    // selected.
    const mountedWindows = new Set(warm.map((target) => target.window));
    const missing = visibleTargets().filter((target) => !mountedWindows.has(target.window));
    return missing.length ? [...warm, ...missing] : warm;
  });

  const syncViewport = () => {
    const visible = visibleTargets();
    return daemonCall("viewport.set", {
      lane_ids: [...new Set(visible.map((target) => target.laneId))],
      focus_lane: multitasking()
        ? visible.find((target) => target.window === activeWindow())?.laneId
        : props.fleet.selectedLaneId() ?? undefined,
      focus_window: activeWindow() ?? undefined,
      fit_windows: visible.filter((target) => !target.shell).map((target) => target.window),
      windows: visible.filter((target) => target.shell).map((target) => target.window),
    }).then(() => undefined).catch(() => undefined);
  };

  createEffect(() => {
    // After the daemon installs this viewport's fit claims, retry local pane geometry. A first
    // ResizeObserver callback can race the RPC during a layout switch and receive the old shared
    // grid; this post-ack pass guarantees it is corrected against the now-authoritative claims.
    void syncViewport().finally(() => notifyLayoutChanged());
  });

  onMount(() => {
    // Keep multi-pane fit ownership fresh just like the TUI's viewport heartbeat. Without this,
    // an unchanged desktop layout loses its 15s claim and a later resize can again be denied by a
    // peer that continues heartbeating.
    const timer = window.setInterval(() => void syncViewport(), 5_000);
    onCleanup(() => window.clearInterval(timer));
  });

  const chooseLayout = props.workspace.chooseLayout;

  async function openShell() {
    if (props.fleet.selectedLaneId() === null) return;
    setOpeningShell(true);
    setWorkspaceError(null);
    try {
      await props.workspace.openShell(setWorkspaceError);
    } finally {
      setOpeningShell(false);
    }
  }

  async function closeShell(target: PaneTarget) {
    props.workspace.markClosing(target.window);
    setWorkspaceError(null);
    try {
      await daemonCall("terminal.close", { id: target.window });
      await props.fleet.refresh();
    } catch (error) {
      props.workspace.unmarkClosing(target.window);
      setWorkspaceError(error instanceof Error ? error.message : String(error));
    }
  }

  const isTargetClosing = (target: PaneTarget) => props.workspace.isClosing(target.window);

  async function handleAdopt(lane: import("../bindings").Lane, session: import("../bindings").AgentSession) {
    setAdopting(true);
    setWorkspaceError(null);
    try {
      await props.actions.adoptAgent(lane, session);
    } catch (error) {
      setWorkspaceError(error instanceof Error ? error.message : String(error));
    } finally {
      setAdopting(false);
    }
  }

  createEffect(() => {
    laneTargets();
    activeWindow();
    requestAnimationFrame(() => {
      updateScrollIndicators();
      if (!tabStripRef) return;
      const activeEl = tabStripRef.querySelector<HTMLElement>('[aria-pressed="true"]');
      activeEl?.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "nearest" });
    });
  });

  onMount(() => {
    if (!tabStripRef) return;
    tabStripRef.addEventListener("wheel", onTabStripWheel, { passive: false });
    let resizeObserver: ResizeObserver | undefined;
    if (typeof ResizeObserver !== "undefined") {
      resizeObserver = new ResizeObserver(() => updateScrollIndicators());
      resizeObserver.observe(tabStripRef);
    }
    onCleanup(() => {
      tabStripRef?.removeEventListener("wheel", onTabStripWheel);
      resizeObserver?.disconnect();
    });
  });

  return (
    <div class="relative grid h-full min-h-0 grid-rows-[2.5rem_minmax(0,1fr)] bg-background">
      <div class="flex h-10 shrink-0 min-w-0 items-center justify-between border-b border-line bg-surface/95 px-3.5 backdrop-blur">
        <Show when={multitasking()}>
          <div class="flex min-w-0 flex-1 items-center gap-3">
            <div class="flex min-w-0 items-center gap-2">
              <span class="section-label text-foreground">Multitasking</span>
              <span class="truncate font-mono text-[9px] uppercase tracking-wider text-muted">
                {new Set(visibleTargets().map((target) => target.laneId)).size} lanes · {visibleTargets().length} panes
              </span>
            </div>
            <div class="ml-auto flex shrink-0 items-center gap-2">
              <PanePicker
                multitasking
                available={targets()}
                selected={props.workspace.multitaskTargets()}
                spans={props.workspace.multitaskSpans()}
                onChange={props.workspace.setMultitaskPaneSelection}
                onSpanChange={props.workspace.setMultitaskSpan}
              />
            </div>
          </div>
        </Show>
        {/* Scrollable Tab Strip Container with Edge Masks and Overflow Controls */}
        <div class={`relative min-w-0 flex-1 items-center ${multitasking() ? "hidden" : "flex"}`}>
          <Show when={canScrollLeft()}>
            <div class="pointer-events-none absolute left-0 top-0 bottom-0 z-10 flex w-16 items-center bg-gradient-to-r from-surface from-40% via-surface/70 to-transparent pl-0.5">
              <button
                type="button"
                class="pointer-events-auto focus-ring flex size-5.5 items-center justify-center rounded-md border border-line/70 bg-surface text-muted shadow-sm hover:border-line hover:bg-raised hover:text-foreground transition-colors"
                onClick={() => scrollByDelta(-140)}
                aria-label="Scroll tabs left"
                title="Scroll left"
              >
                <IconChevronLeft size={12} />
              </button>
            </div>
          </Show>

          <div
            ref={tabStripRef}
            class="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto no-scrollbar scroll-smooth py-0.5"
            onScroll={updateScrollIndicators}
            role="group"
            aria-label="Lane terminals and actions"
            data-reorder-container
          >
            <For each={stripTargets()}>
              {(target) => {
                const draggablePill = () =>
                  tabsReorderable() && !target.shell && target.targetId !== null && !isTargetClosing(target);
                return (
                <div
                  class={`group/tab relative flex h-7 shrink-0 items-center rounded-lg border text-xs font-medium transition-all duration-200 ${
                    isTargetClosing(target)
                      ? "pointer-events-none opacity-40 scale-95 border-line/40 bg-raised/30 text-muted/60"
                      : activeWindow() === target.window
                        ? "border-line bg-background text-foreground shadow-sm ring-1 ring-black/5 dark:ring-white/5"
                        : "border-transparent bg-transparent text-muted hover:bg-raised/60 hover:text-foreground"
                  } ${tabsReorderable() && !target.shell && target.targetId !== null ? "cursor-grab active:cursor-grabbing" : ""}`}
                  {...(draggablePill() ? tabDrag.itemHandlers(target.targetId!) : {})}
                  style={{ "touch-action": draggablePill() ? "none" : undefined }}
                  onContextMenu={(e) => onTabContextMenu(target, e)}
                >
                  <button
                    type="button"
                    aria-pressed={activeWindow() === target.window}
                    class="focus-ring flex min-w-[7.5rem] max-w-[13rem] items-center gap-1.5 px-2.5 py-1 text-left"
                    onClick={() => !isTargetClosing(target) && setActiveWindow(target.window)}
                    disabled={isTargetClosing(target)}
                  >
                    <span class={`shrink-0 ${target.shell ? "text-attention" : "text-signal"}`}>
                      <AgentIcon agent={target.agent} shell={target.shell} size={13} />
                    </span>
                    <span class="truncate flex-1 min-w-0">
                      {isTargetClosing(target) ? "Closing…" : labelOf(target)}
                    </span>
                  </button>
                  <Show when={target.shell}>
                    <button
                      type="button"
                      class="focus-ring mr-1 flex size-5 items-center justify-center rounded text-muted opacity-60 transition-opacity hover:bg-fault/10 hover:text-fault hover:opacity-100 disabled:opacity-30"
                      aria-label={`Close ${target.label}`}
                      disabled={isTargetClosing(target)}
                      onClick={() => void closeShell(target)}
                    >
                      <IconClose size={11} />
                    </button>
                  </Show>
                  <Show when={!target.shell}>
                    <button
                      type="button"
                      class="focus-ring mr-1 flex size-5 items-center justify-center rounded text-muted opacity-0 group-hover/tab:opacity-60 transition-opacity hover:bg-fault/10 hover:text-fault hover:!opacity-100 disabled:opacity-30"
                      aria-label={`Stop ${labelOf(target)}`}
                      disabled={isTargetClosing(target)}
                      title="Stop agent"
                      onClick={(e) => {
                        e.stopPropagation();
                        const lane = props.fleet.selectedLane();
                        if (lane) {
                          const sess = lane.agent_sessions.find((s) => s.tmux_window === target.window) ?? null;
                          props.actions.stopAgent(lane, sess, target.window);
                        }
                      }}
                    >
                      <IconClose size={11} />
                    </button>
                  </Show>
                </div>
                );
              }}
            </For>
            <div class="ml-2 flex shrink-0 items-center gap-2 border-l border-line/60 pl-2">
              <Show when={props.fleet.selectedLane()?.agent_sessions.find((s) => s.external)}>
                {(extSess) => (
                  <button
                    type="button"
                    class="focus-ring flex h-6 items-center gap-1 rounded-md border border-signal/40 bg-signal/15 px-2 text-[11px] font-medium text-signal transition-colors hover:bg-signal/25 disabled:opacity-40"
                    onClick={() => void handleAdopt(props.fleet.selectedLane()!, extSess())}
                    disabled={adopting()}
                    title={`Adopt external ${extSess().agent} session into repomon tmux management`}
                  >
                    <IconBot size={11} />
                    <span>{adopting() ? "Adopting…" : "Adopt External"}</span>
                  </button>
                )}
              </Show>
              <button
                type="button"
                class="focus-ring flex h-6 items-center gap-1 rounded-md border border-line bg-raised/50 px-2 text-[11px] font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                onClick={() => {
                  const lane = props.fleet.selectedLane();
                  if (lane) props.actions.spawn(lane);
                }}
                disabled={!props.fleet.selectedLane()}
                title="Spawn an agent in this lane"
              >
                <IconPlus size={11} />
                <span>Agent</span>
              </button>
              <button
                type="button"
                class="focus-ring flex h-6 items-center gap-1 rounded-md border border-line bg-raised/50 px-2 text-[11px] font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                onClick={() => void openShell()}
                disabled={props.fleet.selectedLaneId() === null || openingShell()}
              >
                <IconPlus size={11} />
                <span>{openingShell() ? "Opening…" : "Shell"}</span>
              </button>
            </div>
          </div>

          <Show when={canScrollRight()}>
            <div class="pointer-events-none absolute right-0 top-0 bottom-0 z-10 flex w-16 items-center justify-end bg-gradient-to-l from-surface from-40% via-surface/70 to-transparent pr-0.5">
              <button
                type="button"
                class="pointer-events-auto focus-ring flex size-5.5 items-center justify-center rounded-md border border-line/70 bg-surface text-muted shadow-sm hover:border-line hover:bg-raised hover:text-foreground transition-colors"
                onClick={() => scrollByDelta(140)}
                aria-label="Scroll tabs right"
                title="Scroll right"
              >
                <IconChevronRight size={12} />
              </button>
            </div>
          </Show>
        </div>

        <div class={`ml-3 shrink-0 items-center ${multitasking() ? "hidden" : "flex"}`}>
          <div class="flex items-center" role="group" aria-label="Layout view mode">
            <button
              type="button"
              class={`focus-ring flex size-6 items-center justify-center transition-colors ${
                effectiveLayout() === "focused"
                  ? "text-signal font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => chooseLayout("focused")}
              title="Focused layout"
              aria-label="Focused layout"
            >
              <IconFocus size={13} />
            </button>
            <span class="h-2.5 w-px bg-line/60 mx-0.5" aria-hidden="true" />
            <button
              type="button"
              class={`focus-ring flex size-6 items-center justify-center transition-colors ${
                effectiveLayout() === "split"
                  ? "text-signal font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => chooseLayout("split")}
              title="Split layout"
              aria-label="Split layout"
            >
              <IconSplit size={13} />
            </button>
            <span class="h-2.5 w-px bg-line/60 mx-0.5" aria-hidden="true" />
            <button
              type="button"
              class={`focus-ring flex size-6 items-center justify-center transition-colors ${
                effectiveLayout() === "grid"
                  ? "text-signal font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => chooseLayout("grid")}
              title="Grid layout"
              aria-label="Grid layout"
            >
              <IconGrid size={13} />
            </button>
          </div>

          <span class="h-3.5 w-px bg-line/60 mx-1.5" aria-hidden="true" />

          <PanePicker
            available={laneTargets()}
            selected={selectedLaneTargets()}
            onChange={(windows) => {
              const laneId = props.fleet.selectedLaneId();
              if (laneId !== null) props.workspace.setLanePaneSelection(laneId, windows);
            }}
          />

          <span class="h-3.5 w-px bg-line/60 mx-1.5" aria-hidden="true" />

          <Select
            size="sm"
            variant="frameless"
            align="right"
            ariaLabel="Layout mode"
            value={layout()}
            options={[
              { value: "auto", label: "auto" },
              { value: "focused", label: "focused" },
              { value: "split", label: "split" },
              { value: "grid", label: "grid" },
            ]}
            onChange={(val) => chooseLayout(val as WorkspaceLayout)}
          />
        </div>
      </div>

      <Show when={workspaceError()}>
        {(message) => (
          <div role="alert" class="absolute right-4 top-12 z-40 flex max-w-md items-start gap-3 rounded-xl border border-fault/30 bg-surface p-3 text-xs text-fault shadow-[0_12px_36px_var(--shadow)]">
            <span class="flex-1 font-medium">{message()}</span>
            <button
              type="button"
              class="focus-ring -mr-1 -mt-1 flex size-5 items-center justify-center rounded text-muted hover:text-foreground"
              aria-label="Dismiss terminal error"
              onClick={() => setWorkspaceError(null)}
            >
              <IconClose size={12} />
            </button>
          </div>
        )}
      </Show>

      <Show
        when={visibleTargets().length}
        fallback={
          <div class="relative flex items-center justify-center px-8 text-center">
            <section class="max-w-md rounded-2xl border border-line bg-surface/80 p-8 shadow-[0_20px_60px_var(--shadow)]">
              <div class="mx-auto mb-4 flex size-12 items-center justify-center rounded-xl border border-line bg-raised text-signal">
                <IconTerminal size={22} />
              </div>
              <p class="section-label mb-1">Terminal Bay</p>
              <h2 class="text-lg font-semibold tracking-tight text-foreground">
                {props.fleet.selectedLane()?.worktree.branch ?? "Select or create a lane"}
              </h2>
              <p class="mx-auto mt-2 max-w-xs text-xs leading-relaxed text-muted">
                {props.fleet.selectedLane() ? "Spawn an AI agent or open an interactive shell in this worktree." : "Choose a lane from the fleet sidebar or register a repository to begin."}
              </p>
              <Show when={props.fleet.selectedLane()}>
                {(lane) => {
                  const extSess = () => lane().agent_sessions.find((s) => s.external);
                  return (
                    <div class="mt-5 space-y-4">
                      <Show when={extSess()}>
                        {(ext) => (
                          <div class="rounded-xl border border-signal/30 bg-signal/5 p-3.5 text-left">
                            <div class="flex items-center gap-1.5 text-xs font-semibold text-signal">
                              <IconBot size={14} />
                              <span>External session detected</span>
                            </div>
                            <p class="mt-1 text-[11px] leading-relaxed text-muted">
                              An external <strong class="text-foreground">{ext().agent}</strong> session is running in this worktree ({ext().title || "running in another terminal"}). Adopt it to bring it under full tmux management with live streaming, rate limits, and fleet mail.
                            </p>
                            <div class="mt-2.5">
                              <button
                                type="button"
                                class="focus-ring inline-flex items-center gap-1.5 rounded-lg border border-signal bg-signal/15 px-3 py-1.5 text-xs font-medium text-signal transition-colors hover:bg-signal/25 disabled:opacity-40"
                                onClick={() => void handleAdopt(lane(), ext())}
                                disabled={adopting()}
                              >
                                <IconBot size={13} />
                                <span>{adopting() ? "Adopting session…" : `Adopt ${ext().agent} session`}</span>
                              </button>
                            </div>
                          </div>
                        )}
                      </Show>

                      <div class="flex items-center justify-center gap-2">
                        <button
                          type="button"
                          class="focus-ring inline-flex items-center gap-1.5 rounded-lg border border-signal/40 bg-signal/10 px-3.5 py-2 text-xs font-medium text-signal transition-colors hover:bg-signal/20"
                          onClick={() => props.actions.spawn(lane())}
                        >
                          <IconPlus size={13} />
                          <span>Spawn agent</span>
                        </button>
                        <button
                          type="button"
                          class="focus-ring inline-flex items-center gap-1.5 rounded-lg border border-line bg-surface px-3.5 py-2 text-xs font-medium text-foreground transition-colors hover:bg-raised"
                          onClick={() => void openShell()}
                        >
                          <IconTerminal size={13} />
                          <span>Open shell</span>
                        </button>
                      </div>
                    </div>
                  );
                }}
              </Show>
            </section>
          </div>
        }
      >
        <div
          class={`terminal-layout is-${effectiveLayout()} count-${visibleTargets().length} ${multitasking() ? "is-multitasking" : ""}`}
          style={multitasking()
            ? {
                "grid-template-columns": "repeat(3, minmax(0, 1fr))",
                "--multitask-row-min-height": `${multitaskRowMinimum()}px`,
              }
            : undefined}
        >
          <For each={mountedTargets()}>
            {(target) => {
              const visibleIndex = createMemo(() => visibleTargets().findIndex((item) => item.window === target.window));
              const visible = createMemo(() => visibleIndex() >= 0);
              const sessionId = createMemo(() => (
                targets().find((item) => item.window === target.window)?.sessionId ?? null
              ));
              const closing = createMemo(() => isTargetClosing(target));
              const paneSpan = createMemo(() => props.workspace.multitaskSpans()[target.window] ?? { columns: 1, rows: 1 });
              // Only worth calling out the active pane when there's more than one on screen to
              // tell apart — a lone focused pane is already unambiguous.
              const isActivePane = createMemo(() => (
                visible() && !closing() && effectiveLayout() !== "focused" && activeWindow() === target.window
              ));
              return (
                <div
                  class={`min-h-0 min-w-0 border-line transition-[opacity,transform,box-shadow] duration-200 ${multitasking() && visible() ? "multitask-pane" : ""} ${
                    visible() ? "" : "warm-terminal-hidden"
                  } ${closing() ? "pointer-events-none opacity-0 scale-[0.98]" : ""} ${
                    isActivePane() ? "is-active-pane ring-1 ring-inset ring-signal/60" : ""
                  }`}
                  style={{
                    order: visible() ? visibleIndex() : undefined,
                    "grid-column": multitasking() && visible()
                      ? `span ${Math.min(paneSpan().columns, 3)}`
                      : undefined,
                    "grid-row": multitasking() && visible() ? `span ${paneSpan().rows}` : undefined,
                    "--pane-accent": multitasking() ? paneAccent(target) : undefined,
                  }}
                  aria-hidden={visible() && !closing() ? undefined : "true"}
                  inert={!visible() || closing()}
                  onPointerDown={() => {
                    if (!visible() || closing()) return;
                    setActiveWindow(target.window);
                    props.fleet.setSelectedLaneId(target.laneId);
                  }}
                >
                  <TerminalPane
                    laneId={target.laneId}
                    window={target.window}
                    label={multitasking()
                      ? `${target.repoName ?? "repo"} / ${target.branch || target.laneName || `lane ${target.laneId}`} · ${labelOf(target)}`
                      : labelOf(target)}
                    renderer={renderer()}
                    focused={activeWindow() === target.window}
                    visible={visible()}
                    followTail={multitasking()}
                    onMinimumHeight={(pixels) => recordPaneMinimumHeight(target.window, pixels)}
                    shell={target.shell}
                    sessionId={sessionId()}
                    fleet={props.fleet}
                    editor={props.editor}
                    workspace={props.workspace}
                    onEnsureEditorOpen={props.onEnsureEditorOpen}
                  />
                </div>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
}
