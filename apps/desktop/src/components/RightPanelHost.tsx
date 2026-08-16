import { For, Show, createMemo, createSignal, type JSX } from "solid-js";

import RepomindPanel from "./RepomindPanel";
import { IconSparkles, type IconProps } from "./icons";
import { readRightPanelActiveTab, saveRightPanelActiveTab } from "../stores/uiSettings";

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
}

function buildDefaultPanels(onToggleFullscreen: () => void): RightPanelTabDef[] {
  return [
    {
      id: "repomind",
      label: "Repomind",
      icon: IconSparkles,
      component: () => <RepomindPanel onToggleFullscreen={onToggleFullscreen} />,
    },

    // Registered by C1: git status/diff for the active lane. Uncomment once GitPanel exists —
    // no other change in this file is needed to bring the tab strip to life.
    // { id: "git", label: "Git", icon: IconGitBranch, component: () => <GitPanel /> },

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
  const panels = createMemo(() => props.panels ?? buildDefaultPanels(props.onToggleFullscreen));

  const [activeId, setActiveId] = createSignal((() => {
    const stored = readRightPanelActiveTab();
    const list = panels();
    if (stored && list.some((panel) => panel.id === stored)) return stored;
    return list[0]?.id ?? "";
  })());

  // Only one tab exists today (Repomind), so the strip stays hidden and this whole host renders
  // with zero visual change from the pre-F2 panel — the registry is real, but nothing about the
  // chrome changes until a second panel is registered. Once Git/Editor land, the strip reuses
  // RepomindPanel's own segmented-control vocabulary (rounded-lg border, pill buttons) so it
  // reads as one more row of the same chrome rather than a bolted-on control.
  const showStrip = createMemo(() => panels().length > 1);

  function selectTab(id: string) {
    setActiveId(id);
    saveRightPanelActiveTab(id);
  }

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
