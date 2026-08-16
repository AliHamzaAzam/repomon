import { For, Show, createEffect, createMemo, createSignal, type JSX } from "solid-js";

import GitExplorerPanel from "./GitExplorerPanel";
import RepomindPanel from "./RepomindPanel";
import { IconGitBranch, IconSparkles, type IconProps } from "./icons";
import { readRightPanelActiveTab, saveRightPanelActiveTab } from "../stores/uiSettings";
import type { FleetStore } from "../stores/fleet";

/**
 * F2: the right rail generalizes from "the Repomind panel" into a tabbed host any number of
 * panels can register into. `RightPanelTabDef` is that registration contract — id, label, icon,
 * and a zero-arg `component` factory (closures over whatever the panel needs, e.g. Repomind's
 * fullscreen toggle) — so a future panel (Git status/diff for C1, an inline file editor for D4)
 * is added by appending one entry to `buildDefaultPanels`, never by touching this file's layout
 * or tab-switching logic.
 */
export interface RightPanelTabDef {
  id: string;
  label: string;
  icon: (props: IconProps) => JSX.Element;
  component: () => JSX.Element;
}

export interface RightPanelHostProps {
  /** Forwarded to Repomind's own "Expand" control — unchanged from pre-F2 behavior. */
  onToggleFullscreen: () => void;
  /**
   * Test-only escape hatch: supply a synthetic registry to exercise tab routing / strip
   * visibility / switching without depending on RepomindPanel's daemon calls. Production never
   * passes this — the host always builds its real registry from `buildDefaultPanels`.
   */
  panels?: RightPanelTabDef[];
  /**
   * C1: threaded down to GitExplorerPanel's registry entry the same way FleetSidebar and
   * TerminalWorkspace receive the fleet store — the panel only reads `selectedLane()` off it, but
   * that memo lives on the store. Optional purely so the pre-C1 test suite (which renders this
   * host with a synthetic `panels` list and never touches the real registry) keeps compiling.
   */
  fleet?: FleetStore;
  /**
   * One-shot activation command some external shortcut can push to select a tab even after the
   * host has already mounted — e.g. App.tsx's `panel.git` binding switching away from an
   * already-open Repomind tab. Bump `token` on every dispatch (the id alone would not refire the
   * effect on repeated presses of the same shortcut); the id is also consulted on first mount so
   * "closed → open already on git" works without waiting on a post-mount effect.
   */
  requestTab?: { id: string; token: number } | null;
  /** Fires with the active tab id on mount and on every switch (click or `requestTab`), so a
   * caller can tell whether the panel is already showing the tab it's about to toggle. */
  onActiveTabChange?: (id: string) => void;
}

function buildDefaultPanels(onToggleFullscreen: () => void, fleet?: FleetStore): RightPanelTabDef[] {
  return [
    {
      id: "repomind",
      label: "Repomind",
      icon: IconSparkles,
      component: () => <RepomindPanel onToggleFullscreen={onToggleFullscreen} />,
    },

    // C1: git status/diff for the active lane.
    { id: "git", label: "Git", icon: IconGitBranch, component: () => <GitExplorerPanel fleet={fleet} /> },

    // Registered by D4: inline file editor for the active lane. Same deal — uncomment when
    // EditorPanel ships.
    // { id: "editor", label: "Editor", icon: IconLayers, component: () => <EditorPanel /> },
  ];
}

// Bounds for the ResizableSplit handle App.tsx mounts alongside this host (F1 ships the
// primitive; F2 is the wiring). Exported so the width contract for the right rail lives in one
// place rather than being duplicated at the call site.
export const RIGHT_PANEL_MIN_WIDTH_PX = 256; // 16rem
export const RIGHT_PANEL_MAX_WIDTH_PX = 640; // 40rem
export const RIGHT_PANEL_DEFAULT_WIDTH_PX = 320; // 20rem — matches the pre-F2 fixed .repomind-panel width.

export default function RightPanelHost(props: RightPanelHostProps) {
  const panels = createMemo(() => props.panels ?? buildDefaultPanels(props.onToggleFullscreen, props.fleet));

  const [activeId, setActiveId] = createSignal((() => {
    const requested = props.requestTab?.id;
    const list = panels();
    if (requested && list.some((panel) => panel.id === requested)) return requested;
    const stored = readRightPanelActiveTab();
    if (stored && list.some((panel) => panel.id === stored)) return stored;
    return list[0]?.id ?? "";
  })());

  // C1 registers Git as a second tab, so the strip is live: rounded-lg border, pill buttons,
  // matching RepomindPanel's own segmented-control vocabulary so it reads as one more row of the
  // same chrome rather than a bolted-on control.
  const showStrip = createMemo(() => panels().length > 1);

  function selectTab(id: string) {
    setActiveId(id);
    saveRightPanelActiveTab(id);
  }

  // Reports the active tab on mount and every switch — App.tsx's `panel.git` shortcut needs this
  // to tell whether the panel is already showing "git" before deciding to switch vs. close.
  createEffect((prev?: string) => {
    const id = activeId();
    if (id !== prev) props.onActiveTabChange?.(id);
    return id;
  });

  // The imperative half of the `requestTab` command described on the prop: a shortcut pressed
  // while this host is already mounted (panel open, on some other tab) bumps the token, and this
  // effect switches tabs in response. The mount-time value is handled by `activeId`'s initializer
  // above so "closed → open already on git" doesn't wait a tick for this effect to run.
  let lastRequestToken: number | undefined;
  createEffect(() => {
    const req = props.requestTab;
    if (!req || req.token === lastRequestToken) return;
    lastRequestToken = req.token;
    if (panels().some((panel) => panel.id === req.id)) selectTab(req.id);
  });

  return (
    <div class="flex h-full min-h-0 flex-1 flex-col">
      <Show when={showStrip()}>
        <div
          class="flex h-10 shrink-0 items-center border-b border-line bg-surface/95 px-3.5"
          role="tablist"
          aria-label="Right panel"
        >
          <div class="flex items-center rounded-lg border border-line bg-raised/50 p-0.5">
            <For each={panels()}>
              {(panel) => (
                <button
                  type="button"
                  role="tab"
                  aria-selected={activeId() === panel.id}
                  class={`focus-ring flex items-center gap-1.5 rounded-md px-2.5 py-0.5 text-[11px] font-medium transition-colors ${
                    activeId() === panel.id
                      ? "bg-surface text-foreground shadow-xs font-semibold"
                      : "text-muted hover:text-foreground"
                  }`}
                  onClick={() => selectTab(panel.id)}
                >
                  <panel.icon size={12} />
                  <span>{panel.label}</span>
                </button>
              )}
            </For>
          </div>
        </div>
      </Show>

      <div class="relative min-h-0 flex-1">
        <For each={panels()}>
          {(panel) => {
            const isActive = () => activeId() === panel.id;
            return (
              <div
                class={isActive() ? "flex h-full min-h-0 flex-col" : "warm-panel-hidden"}
                aria-hidden={isActive() ? undefined : "true"}
                inert={!isActive()}
              >
                <panel.component />
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}
