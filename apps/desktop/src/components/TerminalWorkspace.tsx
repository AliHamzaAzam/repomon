import { For, Show, createEffect, createMemo, createSignal, lazy } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import type { TerminalRenderer } from "../ipc/term";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import { dedupe, stabilizeTargets, type PaneTarget } from "./terminalTargets";

const TerminalPane = lazy(() => import("./TerminalPane"));

export type WorkspaceLayout = "focused" | "split" | "grid";

interface TerminalWorkspaceProps {
  fleet: FleetStore;
  actions: ActionsStore;
}

function readLayout(): WorkspaceLayout {
  const value = localStorage.getItem("repomon.workspace.layout");
  return value === "split" || value === "grid" ? value : "focused";
}

function readRenderer(): TerminalRenderer {
  const value = localStorage.getItem("repomon.terminal.renderer");
  return value === "webgl" || value === "dom" ? value : "auto";
}

export default function TerminalWorkspace(props: TerminalWorkspaceProps) {
  const [layout, setLayout] = createSignal<WorkspaceLayout>(readLayout());
  const [renderer, setRenderer] = createSignal<TerminalRenderer>(readRenderer());
  const [activeWindow, setActiveWindow] = createSignal<string | null>(null);
  const [openingShell, setOpeningShell] = createSignal(false);

  // Fleet polls every second and hands us a brand-new lanes array each time. Reconcile the
  // rebuilt targets against this cache so each window keeps a stable object reference, and the
  // reference-keyed <For> below keeps its TerminalPane (and its byte watch) mounted instead of
  // tearing it down every poll.
  const targetCache = new Map<string, PaneTarget>();
  const targets = createMemo(() => stabilizeTargets(targetCache, dedupe(props.fleet.lanes().flatMap((lane) => [
    ...lane.agent_sessions.flatMap((agent, index): PaneTarget[] => agent.tmux_window ? [{
      laneId: lane.id,
      window: agent.tmux_window,
      label: agent.custom_label ?? agent.title ?? `${agent.agent} ${index + 1}`,
      shell: false,
    }] : []),
    ...props.fleet.terminals()
      .filter((terminal) => terminal.lane_id === lane.id)
      .map((terminal): PaneTarget => ({
        laneId: lane.id,
        window: terminal.id,
        label: `shell ${terminal.id.split("-").slice(-1)[0]}`,
        shell: true,
      })),
  ]))));

  const laneTargets = createMemo(() => targets().filter((target) => target.laneId === props.fleet.selectedLaneId()));

  createEffect(() => {
    const available = laneTargets();
    if (!available.some((target) => target.window === activeWindow())) {
      setActiveWindow(available[0]?.window ?? null);
    }
  });

  const visibleTargets = createMemo(() => {
    const all = targets();
    const active = all.find((target) => target.window === activeWindow()) ?? laneTargets()[0];
    if (!active) return [];
    if (layout() === "focused") return [active];
    if (layout() === "split") {
      const peer = laneTargets().find((target) => target.window !== active.window)
        ?? all.find((target) => target.window !== active.window);
      return peer ? [active, peer] : [active];
    }
    return [
      ...laneTargets(),
      ...all.filter((target) => target.laneId !== props.fleet.selectedLaneId()),
    ].slice(0, 6);
  });

  createEffect(() => {
    const visible = visibleTargets();
    void daemonCall("viewport.set", {
      lane_ids: [...new Set(visible.map((target) => target.laneId))],
      focus_lane: props.fleet.selectedLaneId() ?? undefined,
      focus_window: activeWindow() ?? undefined,
      windows: visible.filter((target) => target.shell).map((target) => target.window),
    }).catch(() => undefined);
  });

  function chooseLayout(next: WorkspaceLayout) {
    setLayout(next);
    localStorage.setItem("repomon.workspace.layout", next);
  }

  function chooseRenderer(next: TerminalRenderer) {
    setRenderer(next);
    localStorage.setItem("repomon.terminal.renderer", next);
  }

  async function openShell() {
    const laneId = props.fleet.selectedLaneId();
    if (laneId === null) return;
    setOpeningShell(true);
    try {
      const terminal = await daemonCall("terminal.open", { lane_id: laneId });
      await props.fleet.refresh();
      setActiveWindow(terminal.id);
    } finally {
      setOpeningShell(false);
    }
  }

  async function closeShell(target: PaneTarget) {
    await daemonCall("terminal.close", { id: target.window });
    if (activeWindow() === target.window) setActiveWindow(null);
    await props.fleet.refresh();
  }

  return (
    <div class="grid h-full min-h-0 grid-rows-[2.5rem_minmax(0,1fr)]">
      <div class="flex min-w-0 items-center justify-between border-b border-line bg-surface/90 px-2 backdrop-blur">
        <div class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          <For each={laneTargets()}>
            {(target) => (
              <button
                type="button"
                class={`focus-ring group flex h-7 shrink-0 items-center gap-1.5 rounded border px-2 font-mono text-[0.58rem] ${activeWindow() === target.window ? "border-signal/40 bg-signal/10 text-foreground" : "border-line bg-raised text-muted"}`}
                onClick={() => setActiveWindow(target.window)}
              >
                <span class={target.shell ? "text-attention" : "text-signal"}>{target.shell ? ">_" : "●"}</span>
                <span class="max-w-32 truncate">{target.label}</span>
                <Show when={target.shell}>
                  <span
                    role="button"
                    tabIndex={0}
                    class="ml-1 text-muted hover:text-fault"
                    aria-label={`Close ${target.label}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      void closeShell(target);
                    }}
                  >×</span>
                </Show>
              </button>
            )}
          </For>
          <button
            type="button"
            class="focus-ring h-7 shrink-0 rounded border border-dashed border-signal/40 px-2 font-mono text-[0.58rem] text-signal hover:bg-signal/10 disabled:opacity-40"
            onClick={() => {
              const lane = props.fleet.selectedLane();
              if (lane) props.actions.spawn(lane);
            }}
            disabled={!props.fleet.selectedLane()}
            title="Spawn an agent in this lane"
          >
            + agent
          </button>
          <button
            type="button"
            class="focus-ring h-7 shrink-0 rounded border border-dashed border-line px-2 font-mono text-[0.58rem] text-muted hover:text-foreground"
            onClick={() => void openShell()}
            disabled={props.fleet.selectedLaneId() === null || openingShell()}
          >
            {openingShell() ? "opening…" : "+ shell"}
          </button>
        </div>

        <div class="ml-2 flex shrink-0 items-center gap-1">
          <For each={["focused", "split", "grid"] as WorkspaceLayout[]}>
            {(item) => (
              <button
                type="button"
                class={`focus-ring rounded px-1.5 py-1 font-mono text-[0.52rem] uppercase ${layout() === item ? "bg-signal/12 text-signal" : "text-muted"}`}
                onClick={() => chooseLayout(item)}
              >{item}</button>
            )}
          </For>
          <select
            aria-label="Terminal renderer"
            class="focus-ring ml-1 h-6 rounded border border-line bg-raised px-1 font-mono text-[0.5rem] uppercase text-muted"
            value={renderer()}
            onChange={(event) => chooseRenderer(event.currentTarget.value as TerminalRenderer)}
          >
            <option value="auto">auto</option>
            <option value="webgl">webgl</option>
            <option value="dom">dom</option>
          </select>
        </div>
      </div>

      <Show
        when={visibleTargets().length}
        fallback={
          <div class="relative flex items-center justify-center px-8 text-center">
            <section class="max-w-md">
              <div class="mx-auto mb-5 grid size-14 place-items-center rounded-xl border border-line bg-surface shadow-[0_14px_40px_var(--shadow)]">
                <div class="terminal-glyph" aria-hidden="true"><span>&gt;</span><i /></div>
              </div>
              <p class="section-label mb-2">Terminal bay</p>
              <h2 class="text-xl font-semibold tracking-[-0.025em]">
                {props.fleet.selectedLane()?.worktree.branch ?? "Ready for the first lane"}
              </h2>
              <p class="mx-auto mt-2 max-w-sm text-sm leading-relaxed text-muted">
                {props.fleet.selectedLane() ? "Spawn an agent or open a shell to work in this lane." : "Add a repository to begin monitoring work."}
              </p>
              <Show when={props.fleet.selectedLane()}>
                {(lane) => (
                  <div class="mt-4 flex items-center justify-center gap-2">
                    <button
                      type="button"
                      class="focus-ring rounded-md border border-signal/40 bg-signal/10 px-3 py-1.5 font-mono text-[0.58rem] uppercase tracking-[0.1em] text-signal"
                      onClick={() => props.actions.spawn(lane())}
                    >Spawn agent</button>
                    <button
                      type="button"
                      class="focus-ring rounded-md border border-line px-3 py-1.5 font-mono text-[0.58rem] uppercase tracking-[0.1em] text-muted hover:text-foreground"
                      onClick={() => void openShell()}
                    >Open shell</button>
                  </div>
                )}
              </Show>
            </section>
          </div>
        }
      >
        <div class={`terminal-layout is-${layout()} count-${visibleTargets().length}`}>
          <For each={visibleTargets()}>
            {(target) => (
              <div class="min-h-0 min-w-0 border-line" onPointerDown={() => {
                setActiveWindow(target.window);
                props.fleet.setSelectedLaneId(target.laneId);
              }}>
                <TerminalPane
                  laneId={target.laneId}
                  window={target.window}
                  label={target.label}
                  renderer={renderer()}
                  focused={activeWindow() === target.window}
                  shell={target.shell}
                />
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
