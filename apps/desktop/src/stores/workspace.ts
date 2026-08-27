import { createMemo, createRenderEffect, createSignal } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import type { TerminalRenderer } from "../ipc/term";
import { agentLabel } from "../components/agentLabel";
import {
  dedupe,
  stabilizeTargets,
  type PaneTarget,
} from "../components/terminalTargets";
import { agentSessionTargetId } from "../components/agentIdentity";
import type { FleetStore } from "./fleet";

export type WorkspaceLayout = "auto" | "focused" | "split" | "grid";

export interface PaneSpan {
  columns: number;
  rows: number;
}

const LANE_PANES_KEY = "repomon.workspace.lane-panes.v1";
const MULTITASK_PANES_KEY = "repomon.workspace.multitask-panes.v1";
const MULTITASK_COLUMNS_KEY = "repomon.workspace.multitask-columns";
const MULTITASK_SPANS_KEY = "repomon.workspace.multitask-spans.v1";

function readRecord<T>(key: string): Record<string, T> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "{}");
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? parsed as Record<string, T>
      : {};
  } catch {
    return {};
  }
}

function persist(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {}
}

function liveSelection(
  available: PaneTarget[],
  saved: string[] | undefined,
  fallbackLimit?: number,
): PaneTarget[] {
  if (!Array.isArray(saved)) return fallbackLimit ? available.slice(0, fallbackLimit) : available;
  const byWindow = new Map(available.map((target) => [target.window, target]));
  const selected = saved.flatMap((window) => {
    const target = byWindow.get(window);
    return target ? [target] : [];
  });
  if (selected.length) return selected;
  return fallbackLimit ? available.slice(0, fallbackLimit) : available;
}

function readLayout(): WorkspaceLayout {
  const value = localStorage.getItem("repomon.workspace.layout");
  return value === "auto" || value === "split" || value === "grid" || value === "focused" ? value : "auto";
}

function readRenderer(): TerminalRenderer {
  const value = localStorage.getItem("repomon.terminal.renderer");
  // Default to "auto" (WebGL with automatic DOM fallback on context loss/failure): the DOM
  // renderer re-lays-out on every frame and is by far the slowest way to run a busy terminal.
  return value === "webgl" || value === "dom" ? value : "auto";
}

/// Owns workspace-level state that spans the terminal bay: layout mode, renderer choice, the
/// active tab, and the derived pane target lists. Created once in App.tsx and handed down so the
/// keyboard shortcut handlers (layout and tab cycling) have something to call.
export function createWorkspaceStore(fleet: FleetStore) {
  const [layout, setLayout] = createSignal<WorkspaceLayout>(readLayout());
  const [renderer, setRenderer] = createSignal<TerminalRenderer>(readRenderer());
  const [activeWindow, setActiveWindowSignal] = createSignal<string | null>(null);
  const [multitasking, setMultitasking] = createSignal(false);
  const [lanePaneSelections, setLanePaneSelections] = createSignal<Record<string, string[]>>(
    readRecord<string[]>(LANE_PANES_KEY),
  );
  const [multitaskSelection, setMultitaskSelection] = createSignal<string[] | null>((() => {
    try {
      const parsed: unknown = JSON.parse(localStorage.getItem(MULTITASK_PANES_KEY) ?? "null");
      return Array.isArray(parsed) && parsed.every((item) => typeof item === "string") ? parsed : null;
    } catch {
      return null;
    }
  })());
  const [multitaskColumns, setMultitaskColumnsSignal] = createSignal((() => {
    const value = Number(localStorage.getItem(MULTITASK_COLUMNS_KEY));
    return value >= 1 && value <= 3 ? value : 2;
  })());
  const [multitaskSpans, setMultitaskSpans] = createSignal<Record<string, PaneSpan>>(
    readRecord<PaneSpan>(MULTITASK_SPANS_KEY),
  );
  const lastWindowByLane = new Map<number, string>();

  // The fleet store attributes account usage to the agent in view, so it needs to know which pane
  // that is. This store owns the tab state, so mirror it across rather than duplicating the state.
  // A render effect, not `createEffect`: this is a plain signal copy with no DOM to wait for, and
  // running it in the same update cycle keeps the usage pill from lagging a tab switch by a tick.
  createRenderEffect(() => fleet.setFocusedWindow(activeWindow()));

  // Fleet polls every second and hands us a brand-new lanes array each time. Reconcile the
  // rebuilt targets against this cache so each window keeps a stable object reference, and the
  // reference-keyed <For> in the component keeps its TerminalPane (and its byte watch) mounted
  // instead of tearing it down every poll.
  const targetCache = new Map<string, PaneTarget>();
  // `equals` keeps the previous array when the window set is unchanged (stabilizeTargets
  // reuses object refs), so the 2s fleet poll stops cascading through laneTargets /
  // visibleTargets / the viewport.set effect when nothing actually changed.
  const sameTargets = (a: PaneTarget[], b: PaneTarget[]) =>
    a.length === b.length && a.every((target, index) => target === b[index]);
  const lanes = () => fleet.lanes();
  const terminals = () => fleet.terminals();
  const targets = createMemo(() => stabilizeTargets(targetCache, dedupe(lanes().flatMap((lane) => [
    ...lane.agent_sessions.flatMap((agent): PaneTarget[] => agent.tmux_window ? [{
      laneId: lane.id,
      repoId: lane.repo.id,
      repoName: lane.repo.label || lane.repo.name,
      laneName: lane.worktree.name,
      branch: lane.worktree.branch,
      window: agent.tmux_window,
      label: agentLabel(agent),
      shell: false,
      sessionId: agent.session_id,
      targetId: agentSessionTargetId(agent),
      agent: agent.agent,
    }] : []),
    ...terminals()
      .filter((terminal) => terminal.lane_id === lane.id)
      .map((terminal): PaneTarget => ({
        laneId: lane.id,
        repoId: lane.repo.id,
        repoName: lane.repo.label || lane.repo.name,
        laneName: lane.worktree.name,
        branch: lane.worktree.branch,
        window: terminal.id,
        label: `shell ${terminal.id.split("-").slice(-1)[0]}`,
        shell: true,
        sessionId: null,
        targetId: null,
        agent: null,
      })),
  ]))), undefined, { equals: sameTargets });

  const laneTargets = createMemo(() => targets().filter((target) => target.laneId === fleet.selectedLaneId()));
  const selectedLaneTargets = createMemo(() => {
    const laneId = fleet.selectedLaneId();
    if (laneId === null) return [];
    return liveSelection(laneTargets(), lanePaneSelections()[String(laneId)]);
  });
  // Until the user saves a picker order, retain the first live order we saw and only append/remove
  // windows. `fleet.lanes()` may be activity-sorted and is rebuilt on every poll; slicing it
  // directly made every pane jump when another agent emitted output. Once configured, the saved
  // selection remains the authority and `liveSelection` filters out stopped agents.
  const multitaskTargets = createMemo<PaneTarget[]>((previous) => {
    const available = targets();
    const saved = multitaskSelection();
    if (saved !== null) return liveSelection(available, saved, 6);

    const byWindow = new Map(available.map((target) => [target.window, target]));
    const retained = previous.flatMap((target) => {
      const live = byWindow.get(target.window);
      return live ? [live] : [];
    });
    const retainedWindows = new Set(retained.map((target) => target.window));
    const appended = available.filter((target) => !retainedWindows.has(target.window));
    return [...retained, ...appended].slice(0, 6);
  }, []);

  function setActiveWindow(window: string | null) {
    setActiveWindowSignal(window);
    const target = targets().find((item) => item.window === window);
    if (target && !target.shell) lastWindowByLane.set(target.laneId, target.window);
  }

  let previousLaneId: number | null | undefined;
  createRenderEffect(() => {
    const laneId = fleet.selectedLaneId();
    const available = laneTargets();
    const active = activeWindow();
    if (laneId !== previousLaneId) {
      const remembered = laneId === null ? null : lastWindowByLane.get(laneId);
      const desired = available.find((target) => target.window === active)?.window
        ?? available.find((target) => target.window === remembered)?.window
        ?? available[0]?.window
        ?? null;
      setActiveWindowSignal(desired);
      const desiredTarget = available.find((target) => target.window === desired);
      if (laneId !== null && desiredTarget && !desiredTarget.shell) {
        lastWindowByLane.set(laneId, desiredTarget.window);
      }
      previousLaneId = laneId;
      return;
    }
    const activeTarget = available.find((target) => target.window === active);
    if (laneId !== null && activeTarget && !activeTarget.shell) {
      lastWindowByLane.set(laneId, activeTarget.window);
    } else if (laneId !== null && available.length && (!active || !targets().some((target) => target.window === active))) {
      setActiveWindow(available[0].window);
    }
  });

  function setLanePaneSelection(laneId: number, windows: string[]) {
    const next = { ...lanePaneSelections(), [String(laneId)]: [...new Set(windows)] };
    setLanePaneSelections(next);
    persist(LANE_PANES_KEY, next);
  }

  function setMultitaskPaneSelection(windows: string[]) {
    const next = [...new Set(windows)];
    setMultitaskSelection(next);
    persist(MULTITASK_PANES_KEY, next);
  }

  function setMultitaskColumns(columns: number) {
    const next = Math.max(1, Math.min(3, Math.round(columns)));
    setMultitaskColumnsSignal(next);
    try { localStorage.setItem(MULTITASK_COLUMNS_KEY, String(next)); } catch {}
  }

  function setMultitaskSpan(window: string, span: PaneSpan) {
    const next = {
      ...multitaskSpans(),
      [window]: {
        columns: Math.max(1, Math.min(3, Math.round(span.columns))),
        rows: Math.max(1, Math.min(2, Math.round(span.rows))),
      },
    };
    setMultitaskSpans(next);
    persist(MULTITASK_SPANS_KEY, next);
  }

  function toggleMultitasking() {
    setMultitasking((value) => !value);
  }

  function chooseLayout(next: WorkspaceLayout) {
    setLayout(next);
    localStorage.setItem("repomon.workspace.layout", next);
  }

  function chooseRenderer(next: TerminalRenderer) {
    setRenderer(next);
    localStorage.setItem("repomon.terminal.renderer", next);
  }

  /// Move to the next or previous tab, wrapping at both ends. `targets` is the lane's tab strip
  /// in render order, passed in by the component so the store does not duplicate that memo.
  function cycleTab(delta: number, targets: PaneTarget[]) {
    if (targets.length === 0) return;
    const index = targets.findIndex((target) => target.window === activeWindow());
    // Nothing active yet: step in from the start rather than jumping to the end.
    const next = index < 0 ? 0 : (index + delta + targets.length) % targets.length;
    setActiveWindow(targets[next].window);
  }

  /// Opens a shell terminal for the selected lane. Never rejects: a caller that does not pass
  /// `onError` (the keyboard shortcut path, unlike the toolbar) would otherwise turn a daemon
  /// failure into an unhandled rejection with no feedback for the user.
  async function openShell(onError?: (message: string) => void) {
    const laneId = fleet.selectedLaneId();
    if (laneId === null) return;
    try {
      const terminal = await daemonCall("terminal.open", { lane_id: laneId });
      await fleet.refresh();
      setActiveWindow(terminal.id);
    } catch (cause) {
      onError?.(cause instanceof Error ? cause.message : String(cause));
    }
  }

  const [closingWindows, setClosingWindows] = createSignal<Set<string>>(new Set());

  function markClosing(window: string) {
    setClosingWindows((prev) => {
      const next = new Set(prev);
      next.add(window);
      return next;
    });
    if (activeWindow() === window) {
      const remaining = laneTargets().filter((t) => t.window !== window && !closingWindows().has(t.window));
      setActiveWindow(remaining[0]?.window ?? null);
    }
  }

  function unmarkClosing(window: string) {
    setClosingWindows((prev) => {
      if (!prev.has(window)) return prev;
      const next = new Set(prev);
      next.delete(window);
      return next;
    });
  }

  function isClosing(window: string): boolean {
    return closingWindows().has(window);
  }

  createRenderEffect(() => {
    const activeIds = new Set(targets().map((t) => t.window));
    setClosingWindows((prev) => {
      let changed = false;
      const next = new Set<string>();
      for (const win of prev) {
        if (activeIds.has(win)) {
          next.add(win);
        } else {
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  });

  return {
    layout,
    chooseLayout,
    renderer,
    chooseRenderer,
    activeWindow,
    setActiveWindow,
    multitasking,
    setMultitasking,
    toggleMultitasking,
    targets,
    laneTargets,
    selectedLaneTargets,
    lanePaneSelections,
    setLanePaneSelection,
    multitaskTargets,
    multitaskSelection,
    setMultitaskPaneSelection,
    multitaskColumns,
    setMultitaskColumns,
    multitaskSpans,
    setMultitaskSpan,
    cycleTab,
    openShell,
    closingWindows,
    markClosing,
    unmarkClosing,
    isClosing,
  };
}

export type WorkspaceStore = ReturnType<typeof createWorkspaceStore>;
