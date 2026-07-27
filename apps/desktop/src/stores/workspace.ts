import { createMemo, createRenderEffect, createSignal } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import type { TerminalRenderer } from "../ipc/term";
import { agentLabel } from "../components/agentLabel";
import {
  dedupe,
  stabilizeTargets,
  type PaneTarget,
} from "../components/terminalTargets";
import type { FleetStore } from "./fleet";

export type WorkspaceLayout = "focused" | "split" | "grid";

function readLayout(): WorkspaceLayout {
  const value = localStorage.getItem("repomon.workspace.layout");
  return value === "split" || value === "grid" ? value : "focused";
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
  const [activeWindow, setActiveWindow] = createSignal<string | null>(null);

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
      window: agent.tmux_window,
      label: agentLabel(agent),
      shell: false,
      sessionId: agent.session_id,
    }] : []),
    ...terminals()
      .filter((terminal) => terminal.lane_id === lane.id)
      .map((terminal): PaneTarget => ({
        laneId: lane.id,
        window: terminal.id,
        label: `shell ${terminal.id.split("-").slice(-1)[0]}`,
        shell: true,
        sessionId: null,
      })),
  ]))), undefined, { equals: sameTargets });

  const laneTargets = createMemo(() => targets().filter((target) => target.laneId === fleet.selectedLaneId()));

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

  return {
    layout,
    chooseLayout,
    renderer,
    chooseRenderer,
    activeWindow,
    setActiveWindow,
    targets,
    laneTargets,
    cycleTab,
    openShell,
  };
}

export type WorkspaceStore = ReturnType<typeof createWorkspaceStore>;
