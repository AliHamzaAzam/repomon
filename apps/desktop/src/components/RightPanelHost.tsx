import { For, createEffect, createSignal, type JSX } from "solid-js";

import FileEditorPanel from "./FileEditorPanel";
import GitExplorerPanel from "./GitExplorerPanel";
import MailPanel from "./MailPanel";
import RepomindPanel from "./RepomindPanel";
import SupervisionPanel from "./SupervisionPanel";
import { IconGitBranch, IconLayers, IconMail, IconShield, IconSparkles, type IconProps } from "./icons";
import { readRightPanelActiveTab, saveRightPanelActiveTab } from "../stores/uiSettings";
import type { ActionsStore } from "../stores/actions";
import type { EditorStore } from "../stores/editor";
import type { FleetStore } from "../stores/fleet";
import type { MessageStore } from "../stores/messages";
import type { RepomindStore } from "../stores/repomind";
import type { WorkspaceStore } from "../stores/workspace";

/** A panel registration supplies its tab label, icon, and component factory. */
export interface RightPanelTabDef {
  id: string;
  label: string;
  icon: (props: IconProps) => JSX.Element;
  component: () => JSX.Element;
}

export interface RightPanelHostProps {
  /** Opens Repomind in full screen. */
  onToggleFullscreen: () => void;
  /** Optional registry for tests that exercise routing without daemon-backed panels. */
  panels?: RightPanelTabDef[];
  /** Shared fleet state for the default panels; optional with an injected registry. */
  fleet?: FleetStore;
  /** Threaded to SupervisionPanel for deep-linking into settings tabs. */
  actions?: ActionsStore;
  /** Shared durable-mail store used by the Repomail management panel. */
  messages?: MessageStore;
  /** Shared editor store used by the inline FileEditorPanel and GitExplorerPanel. */
  editor?: EditorStore;
  /** Shared workspace store. */
  workspace?: WorkspaceStore;
  /** The repomind home's status, shared with the pinned sidebar row. */
  repomind?: RepomindStore;
  /** Ensures center editor workspace is open when opening a file from GitExplorerPanel. */
  onEnsureEditorOpen?: () => void;
  /**
   * One-shot activation command some external shortcut can push to select a tab even after the
   * host has already mounted - e.g. App.tsx's `panel.git` binding switching away from an
   * already-open Repomind tab. Bump `token` on every dispatch (the id alone would not refire the
   * effect on repeated presses of the same shortcut); the id is also consulted on first mount so
   * "closed → open already on git" works without waiting on a post-mount effect.
   */
  requestTab?: { id: string; token: number } | null;
  /** Fires with the active tab id on mount and on every switch (click or `requestTab`), so a
   * caller can tell whether the panel is already showing the tab it's about to toggle. */
  onActiveTabChange?: (id: string) => void;
}

function buildDefaultPanels(
  onToggleFullscreen: () => void,
  fleet?: FleetStore,
  actions?: ActionsStore,
  messages?: MessageStore,
  editor?: EditorStore,
  workspace?: WorkspaceStore,
  onEnsureEditorOpen?: () => void,
  repomind?: RepomindStore,
): RightPanelTabDef[] {
  return [
    {
      id: "repomind",
      label: "Repomind",
      icon: IconSparkles,
      component: () => (
        <RepomindPanel
          onToggleFullscreen={onToggleFullscreen}
          fleet={fleet}
          repomind={repomind}
          actions={actions}
          editor={editor}
          workspace={workspace}
          onEnsureEditorOpen={onEnsureEditorOpen}
        />
      ),
    },

    // C1: git status/diff for the active lane.
    {
      id: "git",
      label: "Git",
      icon: IconGitBranch,
      component: () => (
        <GitExplorerPanel
          fleet={fleet}
          editor={editor}
          workspace={workspace}
          onEnsureEditorOpen={onEnsureEditorOpen}
        />
      ),
    },

    // D4: file tree + multi-tab editor for the active lane's worktree.
    { id: "editor", label: "Editor", icon: IconLayers, component: () => <FileEditorPanel fleet={fleet} editor={editor} onOpenFinder={() => editor?.openFinder()} /> },

    // Durable fleet mail across every lane, grouped by conversation thread.
    { id: "mail", label: "Repomail", icon: IconMail, component: () => <MailPanel fleet={fleet} messages={messages} actions={actions} /> },

    // E1: agent supervision policies and live audit log for the active lane.
    {
      id: "supervision",
      label: "Supervision",
      icon: IconShield,
      component: () => <SupervisionPanel fleet={fleet} actions={actions} />,
    },
  ];
}

// Bounds for the ResizableSplit handle App.tsx mounts alongside this host (F1 ships the
// primitive; F2 is the wiring). Exported so the width contract for the right rail lives in one
// place rather than being duplicated at the call site.
export const RIGHT_PANEL_MIN_WIDTH_PX = 256; // 16rem
export const RIGHT_PANEL_MAX_WIDTH_PX = 640; // 40rem
export const RIGHT_PANEL_DEFAULT_WIDTH_PX = 320; // 20rem — matches the pre-F2 fixed .repomind-panel width.

export default function RightPanelHost(props: RightPanelHostProps) {
  const panels = () =>
    props.panels ??
    buildDefaultPanels(
      props.onToggleFullscreen,
      props.fleet,
      props.actions,
      props.messages,
      props.editor,
      props.workspace,
      props.onEnsureEditorOpen,
      props.repomind,
    );

  const [activeId, setActiveId] = createSignal((() => {
    const requested = props.requestTab?.id;
    const list = panels();
    if (requested && list.some((panel) => panel.id === requested)) return requested;
    const stored = readRightPanelActiveTab();
    if (stored && list.some((panel) => panel.id === stored)) return stored;
    return list[0]?.id ?? "";
  })());

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
