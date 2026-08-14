import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";

import type { Lane, Repo } from "../bindings";
import type { ActionsStore } from "../stores/actions";
import { laneIndicator, type FleetStore } from "../stores/fleet";
import type { MessageStore } from "../stores/messages";
import type { NotificationStore } from "../stores/notifications";
import {
  formatTime,
  groupApprovalRules,
  journalQueryParams,
  playbookState,
  replacementDialog,
  scheduleAddParams,
} from "./automation";
import {
  AgentIcon,
  IconBot,
  IconCheck,
  IconClose,
  IconCommand,
  IconLayers,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconSettings,
  IconSparkles,
  IconTerminal,
} from "./icons";

// Re-export pure helpers for backwards compatibility and test suites
export {
  formatTime,
  groupApprovalRules,
  journalQueryParams,
  playbookState,
  replacementDialog,
  scheduleAddParams,
};

export interface ControlCenterProps {
  fleet: FleetStore;
  notifications: NotificationStore;
  messages: MessageStore;
  actions: ActionsStore;
  onToggleExtensions?: () => void;
  onToggleRepomind?: () => void;
}

interface PaletteItem {
  id: string;
  category: "Actions" | "Lanes & Sessions" | "Repositories";
  title: string;
  subtitle?: string;
  badge?: string;
  badgeType?: "signal" | "attention" | "fault" | "muted";
  icon: "command" | "bot" | "plus" | "sparkles" | "settings" | "refresh" | "terminal" | "layers" | "lane" | "repo";
  shortcut?: string;
  run: () => void | Promise<void>;
}

export default function ControlCenter(props: ControlCenterProps) {
  const [query, setQuery] = createSignal("");
  const [selectedIndex, setSelectedIndex] = createSignal(0);
  let inputRef!: HTMLInputElement;
  let triggerRef!: HTMLButtonElement;
  let listRef!: HTMLDivElement;
  let previouslyFocused: HTMLElement | null = null;

  const isOpen = () => props.actions.controlOpen();

  function openPalette() {
    previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : triggerRef;
    props.actions.openControl();
    setQuery("");
    setSelectedIndex(0);
  }

  function closePalette(restoreFocus = true) {
    props.actions.closeControl();
    setQuery("");
    if (restoreFocus) {
      queueMicrotask(() => (previouslyFocused?.isConnected ? previouslyFocused : triggerRef)?.focus());
    }
  }

  createEffect(() => {
    if (isOpen()) {
      setQuery("");
      setSelectedIndex(0);
      queueMicrotask(() => {
        inputRef?.focus();
        inputRef?.select();
      });
    }
  });

  const allItems = createMemo<PaletteItem[]>(() => {
    const items: PaletteItem[] = [];

    // --- ACTIONS ---
    items.push({
      id: "action-spawn-agent",
      category: "Actions",
      title: "Spawn New Agent Session",
      subtitle: "Launch a new assistant session in a lane",
      icon: "plus",
      shortcut: "⌘N",
      run: () => {
        const lane = props.fleet.selectedLane();
        if (lane) props.actions.spawn(lane);
        else props.actions.newLane();
      },
    });

    items.push({
      id: "action-add-repo",
      category: "Actions",
      title: "Add Repository",
      subtitle: "Track a new git repository in repomon",
      icon: "layers",
      shortcut: "⌘O",
      run: () => {
        void props.actions.addRepo();
      },
    });

    items.push({
      id: "action-toggle-repomind",
      category: "Actions",
      title: "Toggle Repomind Panel",
      subtitle: "Fleet intelligence and autonomous agent assistance",
      icon: "sparkles",
      shortcut: "⌘3",
      run: () => {
        if (props.onToggleRepomind) props.onToggleRepomind();
      },
    });

    items.push({
      id: "action-open-automation",
      category: "Actions",
      title: "Open Automation & Standing Rules",
      subtitle: "Manage Playbooks, recurring schedules, and auto-approvals",
      icon: "command",
      run: () => {
        props.actions.openSettingsTab("automation");
      },
    });

    items.push({
      id: "action-open-settings",
      category: "Actions",
      title: "Open Settings",
      subtitle: "Configure agent defaults, notifications, and appearance",
      icon: "settings",
      shortcut: "⌘,",
      run: () => {
        props.actions.openSettings("general");
      },
    });

    items.push({
      id: "action-open-appearance",
      category: "Actions",
      title: "Appearance & Themes",
      subtitle: "Switch color themes, OLED dark mode, and custom accents",
      icon: "sparkles",
      run: () => {
        props.actions.openSettingsTab("appearance");
      },
    });

    items.push({
      id: "action-open-keyboard",
      category: "Actions",
      title: "Keyboard Shortcuts",
      subtitle: "View all fleet navigation and terminal keybindings",
      icon: "terminal",
      shortcut: "⌘/",
      run: () => {
        props.actions.openSettingsTab("keyboard");
      },
    });

    items.push({
      id: "action-refresh-fleet",
      category: "Actions",
      title: "Refresh Fleet State",
      subtitle: "Sync git status, agent turns, and daemon monitors",
      icon: "refresh",
      shortcut: "⌘R",
      run: () => {
        void props.fleet.refresh();
      },
    });

    // --- LANES & SESSIONS ---
    const lanes = props.fleet.lanes();
    for (const lane of lanes) {
      const ind = laneIndicator(lane);
      const activeSessionsCount = lane.agent_sessions.length;
      const repoName = lane.repo?.name ?? "";
      const subtitle = `${repoName ? `${repoName} · ` : ""}${lane.worktree.path} ${
        activeSessionsCount > 0 ? `(${activeSessionsCount} agent${activeSessionsCount > 1 ? "s" : ""})` : ""
      }`;

      let badgeType: PaletteItem["badgeType"] = "muted";
      let badge: string | undefined = undefined;
      if (ind?.attention) {
        badge = ind.label;
        badgeType = "attention";
      } else if (ind?.running) {
        badge = "RUNNING";
        badgeType = "signal";
      }

      items.push({
        id: `lane-${lane.id}`,
        category: "Lanes & Sessions",
        title: lane.worktree.branch ?? lane.worktree.name,
        subtitle,
        badge,
        badgeType,
        icon: "lane",
        run: () => {
          props.fleet.setSelectedLaneId(lane.id);
          const firstWindow = lane.agent_sessions[0]?.tmux_window;
          if (firstWindow) props.fleet.setFocusedWindow(firstWindow);
        },
      });

      for (const sess of lane.agent_sessions) {
        if (sess.external) {
          items.push({
            id: `adopt-${lane.id}-${sess.session_id || "ext"}`,
            category: "Actions",
            title: `Adopt External ${sess.agent} Session`,
            subtitle: `Resume into repomon management on ${lane.worktree.branch ?? lane.worktree.name}`,
            badge: "EXTERNAL",
            badgeType: "attention",
            icon: "bot",
            run: () => {
              props.fleet.setSelectedLaneId(lane.id);
              void props.actions.adoptAgent(lane, sess);
            },
          });
        }
      }
    }

    // --- REPOSITORIES ---
    const repos = props.fleet.repos();
    for (const repo of repos) {
      items.push({
        id: `repo-${repo.id}`,
        category: "Repositories",
        title: repo.name,
        subtitle: repo.path,
        icon: "repo",
        run: () => {
          const firstLane = props.fleet.lanes().find((l) => l.repo?.id === repo.id);
          if (firstLane) {
            props.fleet.setSelectedLaneId(firstLane.id);
          }
        },
      });
    }

    return items;
  });

  const filteredItems = createMemo(() => {
    const q = query().trim().toLowerCase();
    const all = allItems();
    if (!q) return all;
    return all.filter((item) => {
      return (
        item.title.toLowerCase().includes(q) ||
        (item.subtitle && item.subtitle.toLowerCase().includes(q)) ||
        item.category.toLowerCase().includes(q) ||
        (item.shortcut && item.shortcut.toLowerCase().includes(q))
      );
    });
  });

  createEffect(() => {
    // Reset selection when filtered results change
    filteredItems();
    setSelectedIndex(0);
  });

  createEffect(() => {
    // Keep selected item visible during arrow navigation
    const idx = selectedIndex();
    if (idx >= 0 && listRef) {
      const activeEl = listRef.querySelector<HTMLElement>(`[data-item-index="${idx}"]`);
      activeEl?.scrollIntoView({ block: "nearest" });
    }
  });

  function executeItem(item: PaletteItem) {
    closePalette(false);
    item.run();
  }

  function onKeyDown(e: KeyboardEvent) {
    if (!isOpen()) return;

    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closePalette();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      const list = filteredItems();
      if (list.length > 0) {
        setSelectedIndex((idx) => (idx + 1) % list.length);
      }
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      const list = filteredItems();
      if (list.length > 0) {
        setSelectedIndex((idx) => (idx - 1 + list.length) % list.length);
      }
    } else if (e.key === "Enter") {
      e.preventDefault();
      const list = filteredItems();
      const selected = list[selectedIndex()];
      if (selected) {
        executeItem(selected);
      }
    }
  }

  onMount(() => {
    window.addEventListener("keydown", onKeyDown, true);
  });

  onCleanup(() => {
    window.removeEventListener("keydown", onKeyDown, true);
  });

  // Group filtered items by category for visual headers
  const groupedSections = createMemo(() => {
    const items = filteredItems();
    const categories: Array<{ category: PaletteItem["category"]; items: Array<{ item: PaletteItem; globalIndex: number }> }> = [];
    const categoryMap = new Map<PaletteItem["category"], Array<{ item: PaletteItem; globalIndex: number }>>();

    items.forEach((item, index) => {
      let bucket = categoryMap.get(item.category);
      if (!bucket) {
        bucket = [];
        categoryMap.set(item.category, bucket);
        categories.push({ category: item.category, items: bucket });
      }
      bucket.push({ item, globalIndex: index });
    });

    return categories;
  });

  function renderIcon(type: PaletteItem["icon"]) {
    switch (type) {
      case "command": return <IconCommand size={14} />;
      case "bot": return <IconBot size={14} />;
      case "plus": return <IconPlus size={14} />;
      case "sparkles": return <IconSparkles size={14} />;
      case "settings": return <IconSettings size={14} />;
      case "refresh": return <IconRefresh size={14} />;
      case "terminal": return <IconTerminal size={14} />;
      case "lane": return <IconBot size={14} />;
      case "repo": return <IconLayers size={14} />;
      case "layers": default: return <IconLayers size={14} />;
    }
  }

  return (
    <>
      {/* Toolbar Trigger Button */}
      <button
        ref={triggerRef}
        type="button"
        class="focus-ring flex h-7 items-center gap-1.5 rounded-lg border border-line bg-raised/70 px-2.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
        onClick={() => (isOpen() ? closePalette() : openPalette())}
        aria-label="Command Palette"
        title="Command Palette (⌘K)"
      >
        <IconCommand size={13} />
        <span>Control</span>
        <kbd class="ml-0.5 rounded border border-line bg-surface px-1 py-0.2 text-[9px] font-mono text-muted/80">⌘K</kbd>
      </button>

      {/* Command Palette Modal */}
      <Show when={isOpen()}>
        <Portal>
          <div
            class="fixed inset-0 z-[70] flex items-start justify-center pt-[14vh] bg-background/80 p-4 backdrop-blur-md"
            onClick={(e) => {
              if (e.target === e.currentTarget) closePalette();
            }}
            role="presentation"
          >
            <div
              class="focus-ring flex w-full max-w-xl flex-col rounded-2xl border border-line bg-surface shadow-2xl overflow-hidden transition-all animate-in fade-in zoom-in-95 duration-150"
              role="dialog"
              aria-modal="true"
              aria-label="Command Palette"
            >
              {/* Search Bar Header */}
              <div class="flex items-center gap-3 border-b border-line bg-surface px-4 py-3.5">
                <IconSearch size={16} class="text-muted shrink-0" />
                <input
                  ref={inputRef}
                  type="text"
                  class="w-full bg-transparent text-sm text-foreground outline-none placeholder:text-muted/60"
                  placeholder="Type a command or search repos, lanes…"
                  value={query()}
                  onInput={(e) => setQuery(e.currentTarget.value)}
                  aria-label="Search commands, repositories, and lanes"
                />
                <Show when={query()}>
                  <button
                    type="button"
                    class="rounded p-1 text-muted hover:text-foreground"
                    onClick={() => {
                      setQuery("");
                      inputRef?.focus();
                    }}
                    aria-label="Clear query"
                  >
                    <IconClose size={14} />
                  </button>
                </Show>
                <kbd class="hidden sm:inline-block rounded border border-line bg-raised px-1.5 py-0.5 text-[10px] font-mono text-muted">
                  ESC
                </kbd>
              </div>

              {/* Results List */}
              <div
                ref={listRef}
                class="max-h-[55vh] overflow-y-auto p-2 space-y-3"
                role="listbox"
                aria-label="Commands and targets"
              >
                <For each={groupedSections()}>
                  {(section) => (
                    <div class="space-y-1">
                      <p class="px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wider text-muted/70">
                        {section.category}
                      </p>
                      <For each={section.items}>
                        {({ item, globalIndex }) => {
                          const isSelected = () => selectedIndex() === globalIndex;
                          return (
                            <div
                              data-item-index={globalIndex}
                              role="option"
                              aria-selected={isSelected()}
                              class={`focus-ring flex cursor-pointer items-center justify-between rounded-xl px-3 py-2.5 transition-colors ${
                                isSelected()
                                  ? "bg-signal/15 text-foreground font-medium"
                                  : "text-foreground/90 hover:bg-raised/60"
                              }`}
                              onClick={() => executeItem(item)}
                              onMouseEnter={() => setSelectedIndex(globalIndex)}
                            >
                              <div class="flex min-w-0 items-center gap-3">
                                <div
                                  class={`flex size-6 shrink-0 items-center justify-center rounded-lg border transition-colors ${
                                    isSelected()
                                      ? "border-signal/40 bg-signal/20 text-signal"
                                      : "border-line bg-raised text-muted"
                                  }`}
                                >
                                  {renderIcon(item.icon)}
                                </div>
                                <div class="min-w-0">
                                  <div class="flex items-center gap-2">
                                    <span class="truncate text-xs font-medium">{item.title}</span>
                                    <Show when={item.badge}>
                                      <span
                                        class={`rounded-full px-1.5 py-0.2 font-mono text-[9px] font-semibold uppercase ${
                                          item.badgeType === "attention"
                                            ? "bg-attention/15 text-attention"
                                            : item.badgeType === "signal"
                                            ? "bg-signal/15 text-signal"
                                            : "bg-raised text-muted"
                                        }`}
                                      >
                                        {item.badge}
                                      </span>
                                    </Show>
                                  </div>
                                  <Show when={item.subtitle}>
                                    <span class="block truncate text-[11px] text-muted">
                                      {item.subtitle}
                                    </span>
                                  </Show>
                                </div>
                              </div>

                              <Show when={item.shortcut}>
                                {(kbd) => (
                                  <kbd
                                    class={`ml-2 shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] ${
                                      isSelected()
                                        ? "border-signal/30 bg-signal/10 text-signal"
                                        : "border-line bg-raised text-muted"
                                    }`}
                                  >
                                    {kbd()}
                                  </kbd>
                                )}
                              </Show>
                            </div>
                          );
                        }}
                      </For>
                    </div>
                  )}
                </For>

                <Show when={!filteredItems().length}>
                  <div class="py-12 text-center">
                    <p class="text-xs font-medium text-foreground">No matching commands or lanes</p>
                    <p class="mt-1 text-[11px] text-muted">Try searching for a branch name, repo, or action.</p>
                  </div>
                </Show>
              </div>

              {/* Footer Navigation Bar */}
              <div class="flex items-center justify-between border-t border-line bg-raised/30 px-4 py-2 text-[11px] text-muted">
                <div class="flex items-center gap-3">
                  <span class="flex items-center gap-1">
                    <kbd class="rounded border border-line bg-surface px-1 text-[10px] font-mono">↑</kbd>
                    <kbd class="rounded border border-line bg-surface px-1 text-[10px] font-mono">↓</kbd>
                    <span>Navigate</span>
                  </span>
                  <span class="flex items-center gap-1">
                    <kbd class="rounded border border-line bg-surface px-1 text-[10px] font-mono">↵</kbd>
                    <span>Select</span>
                  </span>
                </div>
                <div class="flex items-center gap-1">
                  <span>Repomon Fleet Command</span>
                </div>
              </div>
            </div>
          </div>
        </Portal>
      </Show>
    </>
  );
}
