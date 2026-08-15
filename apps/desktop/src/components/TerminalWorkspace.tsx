import { For, Show, createEffect, createMemo, createSignal, lazy } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import type { TerminalRenderer } from "../ipc/term";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import type { WorkspaceStore } from "../stores/workspace";
import Select from "./controls/Select";
import { agentLabel } from "./agentLabel";
import {
  warmTargetWindows,
  type PaneTarget,
} from "./terminalTargets";
import {
  AgentIcon,
  IconBot,
  IconClose,
  IconFocus,
  IconGrid,
  IconPlus,
  IconSplit,
  IconTerminal,
} from "./icons";

const TerminalPane = lazy(() => import("./TerminalPane"));

interface TerminalWorkspaceProps {
  fleet: FleetStore;
  actions: ActionsStore;
  workspace: WorkspaceStore;
}

export default function TerminalWorkspace(props: TerminalWorkspaceProps) {
  const [openingShell, setOpeningShell] = createSignal(false);
  const [adopting, setAdopting] = createSignal(false);
  const [closingShell, setClosingShell] = createSignal<string | null>(null);
  const [workspaceError, setWorkspaceError] = createSignal<string | null>(null);
  const [warmWindows, setWarmWindows] = createSignal<string[]>([]);

  const layout = () => props.workspace.layout();
  const renderer = () => props.workspace.renderer();
  const activeWindow = () => props.workspace.activeWindow();
  const setActiveWindow = props.workspace.setActiveWindow;
  const targets = () => props.workspace.targets();
  const laneTargets = () => props.workspace.laneTargets();

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

  createEffect(() => {
    const available = laneTargets();
    if (!available.some((target) => target.window === activeWindow())) {
      setActiveWindow(available[0]?.window ?? null);
    }
  });

  const effectiveLayout = createMemo(() => {
    const l = layout();
    if (l !== "auto") return l;
    const count = laneTargets().length;
    if (count <= 1) return "focused";
    if (count === 2) return "split";
    return "grid";
  });

  const visibleTargets = createMemo(() => {
    const all = targets();
    const active = all.find((target) => target.window === activeWindow()) ?? laneTargets()[0];
    if (!active) return [];
    const eff = effectiveLayout();
    if (eff === "focused") return [active];
    if (eff === "split") {
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
    const available = targets();
    const visible = visibleTargets();
    setWarmWindows((previous) => warmTargetWindows(previous, visible, available));
  });

  const mountedTargets = createMemo(() => {
    const byWindow = new Map(targets().map((target) => [target.window, target]));
    return warmWindows().flatMap((window) => {
      const target = byWindow.get(window);
      return target ? [target] : [];
    });
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

  const chooseLayout = props.workspace.chooseLayout;
  const chooseRenderer = props.workspace.chooseRenderer;

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
    setClosingShell(target.window);
    setWorkspaceError(null);
    try {
      await daemonCall("terminal.close", { id: target.window });
      if (activeWindow() === target.window) setActiveWindow(null);
      await props.fleet.refresh();
    } catch (error) {
      setWorkspaceError(error instanceof Error ? error.message : String(error));
    } finally {
      setClosingShell(null);
    }
  }

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

  return (
    <div class="relative grid h-full min-h-0 grid-rows-[2.5rem_minmax(0,1fr)] bg-background">
      <div class="flex h-10 shrink-0 min-w-0 items-center justify-between border-b border-line bg-surface/95 px-3.5 backdrop-blur">
        <div class="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto" role="group" aria-label="Lane terminals and actions">
          <For each={laneTargets()}>
            {(target) => (
              <div
                class={`group/tab flex h-7 shrink-0 items-center rounded-lg border text-xs font-medium transition-all ${
                  activeWindow() === target.window
                    ? "border-line bg-background text-foreground shadow-sm ring-1 ring-black/5 dark:ring-white/5"
                    : "border-transparent bg-transparent text-muted hover:bg-raised/60 hover:text-foreground"
                }`}
              >
                <button
                  type="button"
                  aria-pressed={activeWindow() === target.window}
                  class="focus-ring flex min-w-[7.5rem] max-w-[13rem] items-center gap-1.5 px-2.5 py-1 text-left"
                  onClick={() => setActiveWindow(target.window)}
                >
                  <span class={`shrink-0 ${target.shell ? "text-attention" : "text-signal"}`}>
                    <AgentIcon agent={target.agent} shell={target.shell} size={13} />
                  </span>
                  <span class="truncate flex-1 min-w-0">{labelOf(target)}</span>
                </button>
                <Show when={target.shell}>
                  <button
                    type="button"
                    class="focus-ring mr-1 flex size-5 items-center justify-center rounded text-muted opacity-60 transition-opacity hover:bg-fault/10 hover:text-fault hover:opacity-100 disabled:opacity-30"
                    aria-label={`Close ${target.label}`}
                    disabled={closingShell() === target.window}
                    onClick={() => void closeShell(target)}
                  >
                    <IconClose size={11} />
                  </button>
                </Show>
              </div>
            )}
          </For>
          <div class="ml-2 flex items-center gap-2 border-l border-line/60 pl-2">
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

        <div class="ml-3 flex shrink-0 items-center gap-2">
          <div class="flex items-center rounded-lg border border-line bg-raised/50 p-0.5" role="group" aria-label="Layout view mode">
            <button
              type="button"
              class={`focus-ring flex size-6 items-center justify-center rounded-md transition-colors ${effectiveLayout() === "focused" ? "bg-surface text-foreground shadow-sm" : "text-muted hover:text-foreground"}`}
              onClick={() => chooseLayout("focused")}
              title="Focused layout"
              aria-label="Focused layout"
            >
              <IconFocus size={13} />
            </button>
            <button
              type="button"
              class={`focus-ring flex size-6 items-center justify-center rounded-md transition-colors ${effectiveLayout() === "split" ? "bg-surface text-foreground shadow-sm" : "text-muted hover:text-foreground"}`}
              onClick={() => chooseLayout("split")}
              title="Split layout"
              aria-label="Split layout"
            >
              <IconSplit size={13} />
            </button>
            <button
              type="button"
              class={`focus-ring flex size-6 items-center justify-center rounded-md transition-colors ${effectiveLayout() === "grid" ? "bg-surface text-foreground shadow-sm" : "text-muted hover:text-foreground"}`}
              onClick={() => chooseLayout("grid")}
              title="Grid layout"
              aria-label="Grid layout"
            >
              <IconGrid size={13} />
            </button>
          </div>

          <Select
            size="sm"
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
        <div class={`terminal-layout is-${effectiveLayout()} count-${visibleTargets().length}`}>
          <For each={mountedTargets()}>
            {(target) => {
              const visibleIndex = createMemo(() => visibleTargets().findIndex((item) => item.window === target.window));
              const visible = createMemo(() => visibleIndex() >= 0);
              const sessionId = createMemo(() => (
                targets().find((item) => item.window === target.window)?.sessionId ?? null
              ));
              return (
                <div
                  class={`min-h-0 min-w-0 border-line ${visible() ? "" : "warm-terminal-hidden"}`}
                  style={{ order: visible() ? visibleIndex() : undefined }}
                  aria-hidden={visible() ? undefined : "true"}
                  inert={!visible()}
                  onPointerDown={() => {
                    if (!visible()) return;
                    setActiveWindow(target.window);
                    props.fleet.setSelectedLaneId(target.laneId);
                  }}
                >
                  <TerminalPane
                    laneId={target.laneId}
                    window={target.window}
                    label={labelOf(target)}
                    renderer={renderer()}
                    focused={activeWindow() === target.window}
                    visible={visible()}
                    shell={target.shell}
                    sessionId={sessionId()}
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
