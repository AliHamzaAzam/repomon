import { Show, createMemo, createSignal } from "solid-js";

import type { AgentSession } from "../bindings";
import { stateIndicator, type ControllerSummary, type FleetStore } from "../stores/fleet";
import type { ActionsStore } from "../stores/actions";
import type { EditorStore } from "../stores/editor";
import type { RepomindStore } from "../stores/repomind";
import type { WorkspaceStore } from "../stores/workspace";
import { IconClose, IconLayers, IconPlay, IconPlus, IconStop } from "./icons";
import RepomindControllers from "./RepomindControllers";
import RepomindDuties from "./RepomindDuties";
import RepomindMemory from "./RepomindMemory";
import RepomindPlans from "./RepomindPlans";
import RepomindPlaybooks from "./RepomindPlaybooks";

export { sinceLabel } from "./RepomindSection";

/// The Repomind panel: the control room for the fleet's memory and duties.
///
/// It is deliberately not a second chat. The controller's conversation happens in its pane in the
/// terminal bay, which is a real terminal with scrollback, dialogs and colour; a composer and a
/// transcript in the rail could only ever be a worse copy of it. What the pane cannot show is the
/// state that conversation is about, and that is what this panel is: the goals in flight, the
/// playbooks waiting on a human, the duties that run without one, and whether the memory feeding
/// all of it is current. Every row that names a controller ends in a way back to its pane.
export interface RepomindPanelProps {
  fullscreen?: boolean;
  onToggleFullscreen?: () => void;
  /// Supplies the controller lane. Everything here reads the home through `file.*` on it, because
  /// the home is a normal lane. Optional so the fullscreen host can mount the panel before the
  /// fleet has synced.
  fleet?: FleetStore;
  repomind?: RepomindStore;
  /// Drives the lifecycle controls in the header and the spawn modal on the controller lane.
  actions?: ActionsStore;
  editor?: EditorStore;
  /// Focusing a controller's pane means selecting its window, exactly as the sidebar does.
  workspace?: WorkspaceStore;
  /// Puts the center in Editor mode, so a clicked file actually becomes visible.
  onEnsureEditorOpen?: () => void;
}

/// An empty summary, for a mount that has no fleet store behind it yet.
const NO_CONTROLLERS: ControllerSummary = { lane: null, agents: 0, state: null, urgent: 0 };

export default function RepomindPanel(props: RepomindPanelProps) {
  const [busy, setBusy] = createSignal<"start" | "stop" | null>(null);

  const controller = () => props.fleet?.controller() ?? NO_CONTROLLERS;
  const home = () => props.repomind?.status() ?? null;
  const laneId = () => controller().lane?.id ?? home()?.lane_id ?? null;
  const running = () => controller().agents > 0;
  const indicator = () => stateIndicator(controller().agents ? controller().state : null);
  const sessions = () => controller().lane?.agent_sessions ?? [];
  const atCap = () => {
    const max = home()?.max_controllers;
    return max !== undefined && controller().agents >= max;
  };

  /// One number the sections watch instead of polling: it moves when the home's own counts move,
  /// which is when a file under it was written by somebody other than this panel.
  const revision = createMemo(() => {
    const counts = home()?.counts;
    if (!counts) return 0;
    return counts.active_plans + counts.standing + counts.playbooks + counts.drafts;
  });

  /// Open a home-relative file in the editor. The home is an ordinary worktree, so this selects
  /// its lane first and then opens the path exactly as any lane's file is opened.
  function openInEditor(path: string) {
    const lane = laneId();
    if (lane === null) return;
    props.fleet?.setSelectedLaneId(lane);
    props.onEnsureEditorOpen?.();
    void props.editor?.openFile(path);
  }

  /// Bring one controller's pane to the front of the terminal bay: select its lane, then its
  /// window. The same two steps the fleet sidebar takes when an agent tab is clicked.
  function focusPane(session: AgentSession) {
    const lane = controller().lane;
    if (lane) props.fleet?.setSelectedLaneId(lane.id);
    if (session.tmux_window) props.workspace?.setActiveWindow(session.tmux_window);
  }

  async function lifecycle(action: "start" | "stop") {
    setBusy(action);
    try {
      if (action === "start") await props.actions?.startRepomind();
      else await props.actions?.stopRepomind();
      await props.repomind?.refresh();
    } finally {
      setBusy(null);
    }
  }

  function spawnController() {
    const lane = controller().lane;
    if (lane) props.actions?.spawn(lane);
  }

  return (
    <div class="flex h-full flex-col bg-surface">
      <div class="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-line bg-surface/95 px-3.5">
        <div class="flex min-w-0 items-center gap-2">
          <span class={`lane-pulse ${running() ? `is-${indicator().tone}` : ""}`} />
          <span class="shrink-0 text-xs font-semibold text-foreground">Repomind</span>
          <span class={`lane-status is-${indicator().tone}`}>{indicator().label}</span>
          <Show when={controller().agents > 0}>
            <span
              class="inline-flex shrink-0 items-center gap-0.5 font-mono text-[10px] leading-none text-muted"
              title={`${controller().agents} controller${controller().agents === 1 ? "" : "s"} in the home lane`}
            >
              <IconLayers size={9} class="text-muted/70" />
              {controller().agents}
            </span>
          </Show>
        </div>
        <div class="flex shrink-0 items-center gap-1.5">
          <Show when={props.onToggleFullscreen}>
            <button
              type="button"
              class="focus-ring flex h-6 items-center rounded border border-line bg-raised/50 px-2 text-[10px] font-medium text-muted hover:text-foreground"
              onClick={props.onToggleFullscreen}
            >
              {props.fullscreen ? "Collapse" : "Expand"}
            </button>
          </Show>
          <Show when={running() && controller().lane}>
            <button
              type="button"
              class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
              onClick={spawnController}
              disabled={atCap()}
              title={
                atCap()
                  ? `The home already runs its maximum of ${home()?.max_controllers} controllers`
                  : "Spawn another controller in the home lane"
              }
              aria-label="Spawn controller"
            >
              <IconPlus size={12} />
            </button>
          </Show>
          <button
            type="button"
            class={`focus-ring flex h-6 items-center gap-1 rounded-md border px-2 text-[11px] font-medium transition-colors ${
              running()
                ? "border-fault/30 bg-fault/10 text-fault hover:bg-fault/20"
                : "border-signal/40 bg-signal/10 text-signal hover:bg-signal/20"
            }`}
            onClick={() => void lifecycle(running() ? "stop" : "start")}
            disabled={Boolean(busy())}
          >
            {running() ? <IconStop size={11} /> : <IconPlay size={11} />}
            <span>
              {busy() === "start"
                ? "Starting"
                : busy() === "stop"
                  ? "Stopping"
                  : running()
                    ? "Stop"
                    : "Start"}
            </span>
          </button>
        </div>
      </div>

      {/* Where the home is, in the fleet's terms and on disk. The row above says how it is doing;
          this one says what it is, and it is the only place either is stated. */}
      <div class="flex h-7 shrink-0 items-center gap-1.5 border-b border-line bg-surface/60 px-3.5">
        <span class="shrink-0 font-mono text-[10px] text-muted/70">
          {controller().lane?.worktree.name ?? "no lane"}
        </span>
        <span
          class="truncate-tail min-w-0 flex-1 font-mono text-[10px] text-muted/70"
          title={home()?.home ?? undefined}
        >
          {home()?.home ?? "home not created yet"}
        </span>
      </div>

      <Show when={props.repomind?.error()}>
        {(message) => (
          <div
            role="alert"
            class="m-3 mb-0 flex items-start justify-between gap-2 rounded-xl border border-fault/30 bg-fault/8 p-2.5 text-xs text-fault"
          >
            <span>{message()}</span>
            <button
              type="button"
              class="focus-ring text-muted hover:text-foreground"
              aria-label="Dismiss repomind error"
              onClick={() => props.repomind?.dismissError()}
            >
              <IconClose size={12} />
            </button>
          </div>
        )}
      </Show>

      <div class="min-h-0 flex-1 overflow-y-auto">
        <Show
          when={laneId()}
          fallback={
            <p class="p-4 text-xs leading-relaxed text-muted">
              The repomind home has no lane yet. The daemon creates it the first time it starts
              with a reachable home directory.
            </p>
          }
        >
          {(lane) => (
            <>
              <RepomindPlans
                laneId={lane()}
                revision={revision()}
                onOpen={openInEditor}
                onChanged={() => void props.repomind?.refresh()}
              />
              <RepomindPlaybooks
                revision={revision()}
                onOpen={openInEditor}
                onChanged={() => void props.repomind?.refresh()}
              />
              <RepomindDuties
                revision={revision()}
                onAdd={() => props.actions?.openSettingsTab("automation", "schedules")}
              />
              <RepomindMemory laneId={lane()} repomind={props.repomind} onOpen={openInEditor} />
              <RepomindControllers sessions={sessions()} onFocus={focusPane} />
            </>
          )}
        </Show>
      </div>
    </div>
  );
}
