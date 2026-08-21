import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import type { AgentSession, Lane, Repo } from "../bindings";
import { laneIndicator, type FleetStore } from "../stores/fleet";
import type { ActionsStore } from "../stores/actions";
import type { WorkspaceStore } from "../stores/workspace";
import {
  readAutoCollapseEmptyLanes,
  onAutoCollapseChanged,
  notifyLayoutChanged,
} from "../stores/uiSettings";
import { primarySession } from "./agentLabel";
import { agentSessionTitle } from "./LaneAgentRosterPopover";
import { formatResetAt } from "./resetTime";
import Modal from "./Modal";
import { reorderAround } from "./ordering";
import RepoExtMenu from "./RepoExtMenu";
import {
  AgentIcon,
  IconArrowDown,
  IconArrowUp,
  IconChevronDown,
  IconChevronRight,
  IconClose,
  IconCpu,
  IconGitBranch,
  IconHide,
  IconLayers,
  IconPin,
  IconPlus,
  IconRefresh,
  IconSearch,
} from "./icons";
import { LaneAgentRosterPopover } from "./LaneAgentRosterPopover";

interface FleetSidebarProps {
  fleet: FleetStore;
  actions: ActionsStore;
  workspace?: WorkspaceStore;
  searchRef?: (element: HTMLInputElement) => void;
  onOpenExtensions?: (repoId: number) => void;
  onSelectAgent?: (lane: Lane, session: AgentSession) => void;
}

function dirtyCount(lane: Lane): number {
  const dirty = lane.state.dirty;
  return dirty.staged + dirty.unstaged + dirty.untracked;
}

function formatUsageWindow(label: string): string {
  const lower = label.toLowerCase();
  if (lower === "5h" || lower.includes("5h") || lower.includes("5-hour")) {
    if (lower.startsWith("claude") || lower.startsWith("gemini") || lower.startsWith("gpt")) {
      const prefix = lower.split(/[-_]/)[0];
      return `${prefix.charAt(0).toUpperCase() + prefix.slice(1)} 5h Quota`;
    }
    return "5-Hour Quota";
  }
  if (lower === "wk" || lower.includes("wk") || lower.includes("week") || lower === "7d") {
    if (lower.startsWith("claude") || lower.startsWith("gemini") || lower.startsWith("gpt")) {
      const prefix = lower.split(/[-_]/)[0];
      return `${prefix.charAt(0).toUpperCase() + prefix.slice(1)} Weekly Quota`;
    }
    return "Weekly Quota";
  }
  if (lower === "mo" || lower.includes("month") || lower === "30d") return "Monthly Quota";
  if (lower === "day" || lower === "24h" || lower === "1d") return "Daily Limit";
  if (lower.includes("model") || lower.includes("sonnet") || lower.includes("opus") || lower === "fable") {
    return "Model Quota";
  }
  return label;
}

function usageTone(pct: number): string {
  if (pct >= 95) return "text-fault font-semibold";
  if (pct >= 75) return "text-attention font-semibold";
  return "text-foreground";
}

function LaneRow(props: {
  lane: Lane;
  selected: boolean;
  select: () => void;
  collapsed?: boolean;
  toggleCollapse?: () => void;
  onSelectAgent?: (lane: Lane, session: AgentSession) => void;
  tabsReorderable?: boolean;
  onReorderTabs?: (laneId: number, orderedSessionIds: string[]) => void;
  onRenameAgent?: (session: AgentSession) => void;
}) {
  let rowRef: HTMLButtonElement | undefined;
  const [isHovered, setIsHovered] = createSignal(false);
  const [anchorRect, setAnchorRect] = createSignal<DOMRect | null>(null);
  let showTimer: number | undefined;
  let hideTimer: number | undefined;

  const clearTimers = () => {
    if (showTimer) {
      clearTimeout(showTimer);
      showTimer = undefined;
    }
    if (hideTimer) {
      clearTimeout(hideTimer);
      hideTimer = undefined;
    }
  };

  const indicator = () => laneIndicator(props.lane);
  const primary = () => primarySession(props.lane.agent_sessions);
  const title = () => primary()?.custom_label ?? props.lane.worktree.name;
  const branchName = () => props.lane.worktree.branch ?? "detached";
  const dirty = () => dirtyCount(props.lane);
  const sessionCount = () => props.lane.agent_sessions.length;

  const onRowMouseEnter = () => {
    clearTimers();
    if (sessionCount() === 0) return;
    if (isHovered()) return;
    showTimer = window.setTimeout(() => {
      if (rowRef) {
        setAnchorRect(rowRef.getBoundingClientRect());
        setIsHovered(true);
      }
    }, 250);
  };

  const onRowMouseLeave = () => {
    clearTimers();
    hideTimer = window.setTimeout(() => {
      setIsHovered(false);
      setAnchorRect(null);
    }, 200);
  };

  const onPopoverMouseEnter = () => {
    clearTimers();
  };

  const onPopoverMouseLeave = () => {
    clearTimers();
    hideTimer = window.setTimeout(() => {
      setIsHovered(false);
      setAnchorRect(null);
    }, 200);
  };

  const onClick = () => {
    clearTimers();
    setIsHovered(false);
    setAnchorRect(null);
    props.select();
  };

  const handleSelectAgent = (lane: Lane, session: AgentSession) => {
    clearTimers();
    setIsHovered(false);
    setAnchorRect(null);
    props.onSelectAgent?.(lane, session);
  };

  return (
    <Show
      when={props.collapsed}
      fallback={
        <>
          <button
            ref={rowRef}
            type="button"
            class={`group/lane-row fleet-row focus-ring ${props.selected ? "is-selected" : ""}`}
            onClick={onClick}
            onMouseEnter={onRowMouseEnter}
            onMouseLeave={onRowMouseLeave}
            aria-current={props.selected ? "true" : undefined}
          >
            {/* 1. Leading Icon Slot (Fixed Width with Corner Status Pulse or Left-Aligned Minimize Button when Empty) */}
            <div class="relative flex size-6 shrink-0 items-center justify-center rounded-md bg-raised/60">
              <Show
                when={sessionCount() === 0}
                fallback={
                  <>
                    <Show
                      when={primary()}
                      fallback={<AgentIcon shell size={13} class="text-muted/60" />}
                    >
                      {(agentSession) => (
                        <AgentIcon
                          agent={agentSession().agent}
                          size={13}
                          class={
                            indicator().tone === "signal"
                              ? "text-signal"
                              : indicator().tone === "attention"
                              ? "text-attention"
                              : indicator().tone === "fault"
                              ? "text-fault"
                              : props.selected
                              ? "text-foreground"
                              : "text-muted"
                          }
                        />
                      )}
                    </Show>
                    <span
                      class={`absolute -top-0.5 -right-0.5 size-2 rounded-full border-2 border-surface ${
                        indicator().tone === "signal"
                          ? "bg-signal ring-1 ring-signal/30"
                          : indicator().tone === "attention"
                          ? "bg-attention ring-1 ring-attention/30 animate-pulse"
                          : indicator().tone === "fault"
                          ? "bg-fault ring-1 ring-fault/30"
                          : "bg-muted/40"
                      }`}
                      aria-hidden="true"
                    />
                  </>
                }
              >
                <button
                  type="button"
                  class="focus-ring flex size-6 items-center justify-center rounded-md text-muted hover:bg-raised hover:text-foreground transition-colors"
                  onClick={(e) => {
                    e.stopPropagation();
                    props.toggleCollapse?.();
                  }}
                  title="Minimize inactive lane"
                  aria-label={`Minimize inactive lane ${title()}`}
                >
                  <IconChevronDown size={11} />
                </button>
              </Show>
            </div>

            {/* 2. Middle Content Area (Title & Branch Name) */}
            <div class="min-w-0 flex-1 text-left">
              <div class="flex items-center gap-1">
                <span
                  class={`truncate text-xs font-medium ${
                    props.selected ? "text-foreground font-semibold" : "text-foreground/90"
                  }`}
                >
                  {title()}
                </span>
                <Show when={props.lane.pinned}>
                  <span class="shrink-0 text-signal" title="Pinned lane" aria-label="Pinned">
                    <IconPin size={10} />
                  </span>
                </Show>
              </div>

              <div class="mt-0.5 flex min-w-0 items-center gap-1 font-mono text-[11px] text-muted">
                <IconGitBranch size={10} class="shrink-0 text-muted/60" />
                <span class="truncate">
                  {branchName()}
                </span>
              </div>
            </div>

            {/* 3. Trailing Metadata & Badges Column (Fixed Right Alignment) */}
            <div class="shrink-0 flex flex-col items-end justify-center gap-0.5 text-right font-mono">
              {/* Top slot: Multi-session badge + Status indicator */}
              <div class="flex items-center gap-1">
                <Show when={sessionCount() > 1}>
                  <span
                    class="inline-flex items-center gap-1 rounded border border-line bg-raised/80 px-1.5 py-0.5 text-[9px] font-medium leading-none text-muted transition-colors hover:bg-raised hover:text-foreground"
                    aria-label={`${sessionCount()} active agent sessions open`}
                  >
                    <IconLayers size={10} class="text-muted/80 shrink-0" />
                    <span>{sessionCount()} agents</span>
                  </span>
                </Show>
                <Show when={indicator().label}>
                  <span
                    class={`lane-badge is-${indicator().tone}`}
                    title={
                      indicator().label === "external"
                        ? "External session running outside repomon. Select lane to adopt into tmux management."
                        : undefined
                    }
                  >
                    {indicator().label}
                  </span>
                </Show>
              </div>

              {/* Bottom slot: Telemetry in fixed order: Divergence (ahead/behind), Dirty count */}
              <div class="flex items-center gap-1.5 text-[10px] text-muted min-h-[14px]">
                <Show when={props.lane.state.ahead || props.lane.state.behind}>
                  <span
                    class="inline-flex items-center gap-0.5 leading-none"
                    title={`Git tracking: ${props.lane.state.ahead} ahead, ${props.lane.state.behind} behind upstream`}
                  >
                    <Show when={props.lane.state.ahead}>
                      <span class="text-signal inline-flex items-center"><IconArrowUp size={9} />{props.lane.state.ahead}</span>
                    </Show>
                    <Show when={props.lane.state.behind}>
                      <span class="text-muted inline-flex items-center"><IconArrowDown size={9} />{props.lane.state.behind}</span>
                    </Show>
                  </span>
                </Show>
                <Show when={dirty() > 0}>
                  <span
                    class="inline-flex items-center gap-0.5 leading-none text-attention font-semibold"
                    title={`${dirty()} uncommitted file${dirty() === 1 ? "" : "s"} (${props.lane.state.dirty.staged} staged, ${props.lane.state.dirty.unstaged} unstaged, ${props.lane.state.dirty.untracked} untracked)`}
                  >
                    <span class="size-1.5 rounded-full bg-attention" />
                    <span>{dirty()}</span>
                  </span>
                </Show>
              </div>
            </div>
          </button>
          <LaneAgentRosterPopover
            lane={props.lane}
            anchorRect={anchorRect()}
            visible={isHovered()}
            onSelectAgent={handleSelectAgent}
            onMouseEnter={onPopoverMouseEnter}
            onMouseLeave={onPopoverMouseLeave}
            reorderable={props.tabsReorderable}
            onReorderTabs={(ids) => props.onReorderTabs?.(props.lane.id, ids)}
            onRenameAgent={props.onRenameAgent}
          />
        </>
      }
    >
      <div
        class={`group/collapsed-row fleet-row focus-ring h-7 min-h-0 py-0.5 px-2 flex items-center justify-between text-muted hover:text-foreground cursor-pointer transition-colors ${
          props.selected ? "is-selected" : ""
        }`}
        onClick={props.select}
        role="button"
        tabIndex={0}
        aria-current={props.selected ? "true" : undefined}
        title={`${title()} (${branchName()}) - Inactive (minimized)`}
      >
        <div class="flex items-center gap-1.5 min-w-0">
          <button
            type="button"
            class="focus-ring flex size-4 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
            onClick={(e) => {
              e.stopPropagation();
              props.toggleCollapse?.();
            }}
            title="Expand lane"
            aria-label={`Expand lane ${title()}`}
          >
            <IconChevronRight size={10} />
          </button>
          <IconGitBranch size={10} class="shrink-0 text-muted/60" />
          <span class="truncate text-xs font-medium text-muted hover:text-foreground">
            {title()}
          </span>
        </div>
        <div class="flex items-center gap-1 shrink-0 font-mono text-[10px] text-muted">
          <Show when={dirty() > 0}>
            <span
              class="inline-flex items-center gap-0.5 text-attention font-semibold"
              title={`${dirty()} uncommitted file${dirty() === 1 ? "" : "s"}`}
            >
              <span class="size-1.5 rounded-full bg-attention" />
              <span>{dirty()}</span>
            </span>
          </Show>
        </div>
      </div>
    </Show>
  );
}

function loadCollapsedLanes(): Set<number> {
  try {
    const raw = typeof localStorage !== "undefined" ? localStorage.getItem("repomon:collapsed-lanes") : null;
    return raw ? new Set(JSON.parse(raw)) : new Set();
  } catch {
    return new Set();
  }
}

function loadHiddenSectionCollapsed(): boolean {
  try {
    const raw = typeof localStorage !== "undefined" ? localStorage.getItem("repomon:collapsed-hidden-repos") : null;
    return raw !== null ? JSON.parse(raw) : true;
  } catch {
    return true;
  }
}

/// The name shown for a repo: the custom label when set, else the folder name.
export function repoDisplayName(repo: Pick<Repo, "name" | "label">): string {
  const label = repo.label?.trim();
  return label || repo.name;
}

/// Rename a repo's sidebar display. The folder name on disk never changes — this sets a label
/// override; clearing the field falls back to it.
function RepoRenameModal(props: {
  repo: Repo;
  onClose: () => void;
  onSubmit: (label: string) => Promise<void>;
}) {
  const [label, setLabel] = createSignal(props.repo.label ?? "");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function rename() {
    setBusy(true);
    setError(null);
    try {
      await props.onSubmit(label());
      props.onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  const footer = (
    <>
      <button
        type="button"
        class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
        onClick={props.onClose}
      >
        Cancel
      </button>
      <button
        type="button"
        class="focus-ring rounded-lg bg-signal px-4 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-50"
        disabled={busy()}
        onClick={() => void rename()}
      >
        {busy() ? "Saving…" : "Save"}
      </button>
    </>
  );

  return (
    <Modal
      title={`Rename ${props.repo.name}`}
      subtitle="Set a display name for the sidebar. The folder on disk keeps its real name."
      onClose={props.onClose}
      footer={footer}
    >
      <label class="block">
        <span class="section-label">Display Name</span>
        <input
          class="focus-ring mt-1.5 h-9 w-full rounded-lg border border-line bg-surface px-3 text-xs text-foreground outline-none placeholder:text-muted/60"
          value={label()}
          placeholder={props.repo.name}
          onInput={(event) => setLabel(event.currentTarget.value)}
          autofocus
        />
      </label>
      <Show when={error()}>
        <p class="mt-3 rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault">{error()}</p>
      </Show>
    </Modal>
  );
}

export default function FleetSidebar(props: FleetSidebarProps) {
  const [extMenu, setExtMenu] = createSignal<{ repoId: number; x: number; y: number } | null>(null);
  const [renameRepoId, setRenameRepoId] = createSignal<number | null>(null);
  // Manual-mode drag state: which repo is being dragged, and which header is the current
  // insertion target (drives the drop indicator line).
  const [dragRepoId, setDragRepoId] = createSignal<number | null>(null);
  const [dropTargetId, setDropTargetId] = createSignal<number | null>(null);
  const [autoCollapse, setAutoCollapse] = createSignal<boolean>(readAutoCollapseEmptyLanes());
  const [manuallyExpandedLanes, setManuallyExpandedLanes] = createSignal<Set<number>>(new Set());
  const [manuallyCollapsedLanes, setManuallyCollapsedLanes] = createSignal<Set<number>>(loadCollapsedLanes());
  const [hiddenCollapsed, setHiddenCollapsed] = createSignal<boolean>(loadHiddenSectionCollapsed());
  // E9: local spin state for the Rate Limits manual refresh button. Scoped here rather than reusing
  // `fleet.loading()`, which also flips on every 1.2s poll tick and would make the icon flicker
  // continuously instead of spinning only for the click the user actually made.
  const [usageRefreshing, setUsageRefreshing] = createSignal(false);

  const refreshUsage = async () => {
    if (usageRefreshing()) return;
    setUsageRefreshing(true);
    try {
      await props.fleet.refreshUsage();
    } finally {
      setUsageRefreshing(false);
    }
  };

  const toggleHiddenCollapsed = () => {
    setHiddenCollapsed((prev) => {
      const next = !prev;
      try {
        if (typeof localStorage !== "undefined") {
          localStorage.setItem("repomon:collapsed-hidden-repos", JSON.stringify(next));
        }
      } catch {}
      return next;
    });
    notifyLayoutChanged();
  };

  onMount(() => {
    const unsub = onAutoCollapseChanged((enabled) => {
      setAutoCollapse(enabled);
      notifyLayoutChanged();
    });
    onCleanup(unsub);
  });

  const isLaneCollapsed = (lane: Lane) => {
    const sessionCount = lane.agent_sessions.length;
    if (sessionCount > 0) return false;
    if (autoCollapse()) {
      return !manuallyExpandedLanes().has(lane.id);
    }
    return manuallyCollapsedLanes().has(lane.id);
  };

  const toggleLaneCollapsed = (laneId: number) => {
    if (autoCollapse()) {
      setManuallyExpandedLanes((prev) => {
        const next = new Set(prev);
        if (next.has(laneId)) {
          next.delete(laneId);
        } else {
          next.add(laneId);
        }
        return next;
      });
    } else {
      setManuallyCollapsedLanes((prev) => {
        const next = new Set(prev);
        if (next.has(laneId)) {
          next.delete(laneId);
        } else {
          next.add(laneId);
        }
        try {
          if (typeof localStorage !== "undefined") {
            localStorage.setItem("repomon:collapsed-lanes", JSON.stringify(Array.from(next)));
          }
        } catch {}
        return next;
      });
    }
    notifyLayoutChanged();
  };

  const handleSelectAgent = (lane: Lane, session: AgentSession) => {
    props.fleet.setSelectedLaneId(lane.id);
    if (session.tmux_window && props.workspace) {
      props.workspace.setActiveWindow(session.tmux_window);
    }
    props.onSelectAgent?.(lane, session);
  };

  // Manual ordering is only draggable in that mode; activity/default orders are daemon-computed.
  const manualMode = () => props.fleet.sortMode() === "manual";
  const tabsReorderable = () => props.fleet.tabSortMode() === "manual";

  const handleReorderTabs = (laneId: number, orderedSessionIds: string[]) => {
    void props.actions.setAgentTabOrder(laneId, orderedSessionIds);
  };

  const handleRenameAgent = (session: AgentSession) => {
    if (!session.session_id) return;
    props.actions.rename({
      sessionId: session.session_id,
      current: agentSessionTitle(session),
    });
  };

  const onRepoDragStart = (repo: Repo, event: DragEvent) => {
    if (!manualMode()) return;
    event.dataTransfer?.setData("text/plain", String(repo.id));
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
    setDragRepoId(repo.id);
  };

  const onRepoDragOver = (repo: Repo, event: DragEvent) => {
    if (dragRepoId() === null || repo.id === dragRepoId()) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
    setDropTargetId(repo.id);
  };

  const onRepoDrop = (repo: Repo, event: DragEvent) => {
    event.preventDefault();
    const dragged = dragRepoId();
    setDragRepoId(null);
    setDropTargetId(null);
    if (dragged === null || dragged === repo.id) return;
    // Drop position follows the pointer relative to the target header's midpoint, so dragging
    // downward past a repo moves the dragged one below it rather than being swallowed by
    // insert-before.
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const after = rect.height > 0 && event.clientY > rect.top + rect.height / 2;
    const next = reorderAround(
      props.fleet.visibleRepos().map((r) => r.id),
      dragged,
      repo.id,
      after,
    );
    if (next) void props.actions.reorderRepos(next);
  };

  const endRepoDrag = () => {
    setDragRepoId(null);
    setDropTargetId(null);
  };

  const renameTargetRepo = createMemo(
    () => props.fleet.repos().find((repo) => repo.id === renameRepoId()) ?? null,
  );

  return (
    <>
      <div class="space-y-2 border-b border-line p-2.5">
        <label class="relative block">
          <span class="sr-only">Filter fleet</span>
          <input
            ref={props.searchRef}
            class="focus-ring h-8 w-full rounded-lg border border-line bg-background pl-8 pr-7 font-sans text-xs text-foreground outline-none placeholder:text-muted/60"
            value={props.fleet.query()}
            onInput={(event) => props.fleet.setQuery(event.currentTarget.value)}
            placeholder="Filter fleet"
          />
          <span class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted">
            <IconSearch size={13} />
          </span>
          <kbd class="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 rounded border border-line bg-raised px-1 py-0.5 font-mono text-[9px] text-muted">
            /
          </kbd>
        </label>
        <div class="flex items-center gap-1.5">
          <button
            type="button"
            class={`focus-ring flex h-7 min-w-0 flex-1 items-center justify-between gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors ${
              props.fleet.urgentOnly()
                ? "border-attention/60 bg-attention/20 text-attention font-semibold shadow-xs ring-1 ring-attention/25"
                : props.fleet.counts().urgent > 0
                  ? "border-attention/30 bg-attention/6 hover:border-attention/50 hover:bg-attention/12"
                  : "border-line bg-raised/60 text-muted hover:border-line hover:bg-raised hover:text-foreground"
            }`}
            onClick={() => props.fleet.setUrgentOnly(!props.fleet.urgentOnly())}
            aria-pressed={props.fleet.urgentOnly()}
            title={props.fleet.urgentOnly() ? "Show all lanes" : "Filter to lanes needing attention"}
          >
            <span
              class={`truncate ${
                props.fleet.urgentOnly()
                  ? "text-attention font-semibold"
                  : props.fleet.counts().urgent > 0
                    ? "text-foreground/85"
                    : "text-muted"
              }`}
            >
              Needs attention
            </span>
            <span
              class={`font-mono text-[11px] ${
                props.fleet.urgentOnly()
                  ? "text-attention font-bold"
                  : props.fleet.counts().urgent > 0
                    ? "text-attention font-semibold"
                    : "text-muted/70 font-medium"
              }`}
            >
              {props.fleet.counts().urgent}
            </span>
          </button>
          <button
            type="button"
            class={`focus-ring flex h-7 shrink-0 items-center justify-between gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors ${
              props.fleet.runningOnly()
                ? "border-signal/60 bg-signal/20 text-signal font-semibold shadow-xs ring-1 ring-signal/25"
                : props.fleet.counts().running > 0
                  ? "border-signal/30 bg-signal/6 hover:border-signal/50 hover:bg-signal/12"
                  : "border-line bg-raised/60 text-muted hover:border-line hover:bg-raised hover:text-foreground"
            }`}
            onClick={() => props.fleet.setRunningOnly(!props.fleet.runningOnly())}
            aria-pressed={props.fleet.runningOnly()}
            title={props.fleet.runningOnly() ? "Show all lanes" : "Filter to lanes with a running agent"}
          >
            <span
              class={`truncate ${
                props.fleet.runningOnly()
                  ? "text-signal font-semibold"
                  : props.fleet.counts().running > 0
                    ? "text-foreground/85"
                    : "text-muted"
              }`}
            >
              Running
            </span>
            <span
              class={`font-mono text-[11px] ${
                props.fleet.runningOnly()
                  ? "text-signal font-bold"
                  : props.fleet.counts().running > 0
                    ? "text-signal font-semibold"
                    : "text-muted/70 font-medium"
              }`}
            >
              {props.fleet.counts().running}
            </span>
          </button>
          <button
            type="button"
            class="focus-ring flex size-7 shrink-0 items-center justify-center rounded-lg border border-line bg-raised/60 text-muted transition-colors hover:border-line hover:bg-raised hover:text-foreground"
            onClick={() => void props.actions.addRepo()}
            title="Add a repository"
            aria-label="Add a repository"
          >
            <IconPlus size={13} />
          </button>
        </div>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto px-2 py-2">
        <Show when={!props.fleet.loading() || props.fleet.lanes().length} fallback={<p class="p-3 text-xs text-muted">Syncing fleet…</p>}>
          <For each={props.fleet.visibleRepos()}>
            {(repo) => {
              const laneList = createMemo(() =>
                props.fleet.visibleLanes().filter((lane) => lane.repo.id === repo.id),
              );
              return (
                <Show when={laneList().length > 0 || !props.fleet.query()}>
                  <section class="mb-2.5" aria-label={repoDisplayName(repo)}>
                    <div
                      class={`group/repo-header flex items-center justify-between rounded px-2 py-1 text-muted transition-colors hover:bg-raised/40 ${
                        manualMode() ? "cursor-grab active:cursor-grabbing" : ""
                      } ${dropTargetId() === repo.id && dragRepoId() !== null ? "border-t border-signal" : ""}`}
                      draggable={manualMode()}
                      onDragStart={(event) => onRepoDragStart(repo, event)}
                      onDragOver={(event) => onRepoDragOver(repo, event)}
                      onDrop={(event) => onRepoDrop(repo, event)}
                      onDragEnd={endRepoDrag}
                      onDragLeave={() => setDropTargetId((current) => (current === repo.id ? null : current))}
                      onContextMenu={(event) => {
                        event.preventDefault();
                        setExtMenu({ repoId: repo.id, x: event.clientX, y: event.clientY });
                      }}
                    >
                      <span
                        class="truncate font-mono text-[11px] font-semibold uppercase tracking-wider text-muted hover:text-foreground transition-colors cursor-default"
                        title={
                          repo.label
                            ? `${repoDisplayName(repo)} — repository: ${repo.name} (${repo.path})`
                            : `Repository: ${repo.name} (${repo.path})`
                        }
                      >
                        {repoDisplayName(repo)}
                      </span>
                      <span class="flex items-center gap-1 shrink-0">
                        <div class="flex items-center gap-0.5 opacity-0 transition-opacity group-hover/repo-header:opacity-100 focus-within:opacity-100">
                          <button
                            type="button"
                            class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-signal"
                            onClick={() => props.actions.newLane(repo.id)}
                            title={`New lane in ${repoDisplayName(repo)}`}
                            aria-label={`New lane in ${repoDisplayName(repo)}`}
                          >
                            <IconPlus size={12} />
                          </button>
                          <button
                            type="button"
                            class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                            onClick={() => void props.actions.setRepoHidden(repo, true)}
                            title={`Hide ${repoDisplayName(repo)} (stays registered)`}
                            aria-label={`Hide ${repoDisplayName(repo)}`}
                          >
                            <IconHide size={12} />
                          </button>
                          <button
                            type="button"
                            class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-fault"
                            onClick={() => props.actions.removeRepo(repo)}
                            title={`Remove ${repoDisplayName(repo)}`}
                            aria-label={`Remove ${repoDisplayName(repo)}`}
                          >
                            <IconClose size={12} />
                          </button>
                        </div>
                        <span
                          class="ml-0.5 rounded bg-raised px-1 font-mono text-[10px] text-muted"
                          title={`${laneList().length} active lane${laneList().length === 1 ? "" : "s"}`}
                        >
                          {laneList().length}
                        </span>
                      </span>
                    </div>
                    <div class="space-y-0.5">
                      <For each={laneList()}>
                        {(lane) => (
                          <LaneRow
                            lane={lane}
                            selected={props.fleet.selectedLaneId() === lane.id}
                            select={() => props.fleet.setSelectedLaneId(lane.id)}
                            collapsed={isLaneCollapsed(lane)}
                            toggleCollapse={() => toggleLaneCollapsed(lane.id)}
                            onSelectAgent={handleSelectAgent}
                            tabsReorderable={tabsReorderable()}
                            onReorderTabs={handleReorderTabs}
                            onRenameAgent={handleRenameAgent}
                          />
                        )}
                      </For>
                    </div>
                  </section>
                </Show>
              );
            }}
          </For>
          <Show when={props.fleet.hiddenRepos().length}>
            <section class="mt-2 border-t border-line pt-2" aria-label="Hidden projects">
              <button
                type="button"
                class="group/hidden-header focus-ring flex w-full items-center justify-between rounded px-2 py-1 text-muted transition-colors hover:bg-raised/40 cursor-pointer"
                onClick={toggleHiddenCollapsed}
                aria-expanded={!hiddenCollapsed()}
                aria-label={hiddenCollapsed() ? `Expand Hidden (${props.fleet.hiddenRepos().length})` : `Collapse Hidden (${props.fleet.hiddenRepos().length})`}
                title={hiddenCollapsed() ? `Expand Hidden (${props.fleet.hiddenRepos().length})` : `Collapse Hidden (${props.fleet.hiddenRepos().length})`}
              >
                <div class="flex items-center gap-1.5 min-w-0">
                  <span class="flex size-4 shrink-0 items-center justify-center rounded text-muted group-hover/hidden-header:text-foreground">
                    <Show when={hiddenCollapsed()} fallback={<IconChevronDown size={10} strokeWidth={2} />}>
                      <IconChevronRight size={10} strokeWidth={2} />
                    </Show>
                  </span>
                  <span class="truncate font-mono text-[10px] font-semibold uppercase tracking-wider text-muted group-hover/hidden-header:text-foreground">
                    Hidden ({props.fleet.hiddenRepos().length})
                  </span>
                </div>
              </button>
              <Show when={!hiddenCollapsed()}>
                <div class="mt-1 space-y-0.5">
                  <For each={props.fleet.hiddenRepos()}>
                    {(repo) => (
                      <button
                        type="button"
                        class="focus-ring flex w-full items-center justify-between rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-raised"
                        onClick={() => void props.actions.setRepoHidden(repo, false)}
                        title={`Show ${repo.name} again`}
                      >
                        <span class="truncate font-mono text-[11px] uppercase tracking-wider text-muted">
                          {repo.name}
                        </span>
                        <span class="ml-2 shrink-0 text-xs text-signal font-medium">Unhide</span>
                      </button>
                    )}
                  </For>
                </div>
              </Show>
            </section>
          </Show>
          <Show when={!props.fleet.visibleLanes().length}>
            <div class="m-2 rounded-xl border border-line bg-surface/40 p-3.5 text-xs leading-relaxed text-muted">
              <Show
                when={props.fleet.query() || props.fleet.urgentOnly()}
                fallback={
                  <Show
                    when={!props.fleet.hiddenRepos().length}
                    fallback={<p>Every project is hidden. Use the list above to bring one back.</p>}
                  >
                    <div class="space-y-2.5 text-center">
                      <p class="text-foreground font-medium">No repositories yet</p>
                      <p class="text-xs text-muted">Register a git repository to start tracking lanes and agents.</p>
                      <button
                        type="button"
                        class="focus-ring inline-flex items-center gap-1.5 rounded-lg border border-signal/40 bg-signal/10 px-3 py-1.5 text-xs font-medium text-signal transition-colors hover:bg-signal/20"
                        onClick={() => void props.actions.addRepo()}
                      >
                        <IconPlus size={13} />
                        <span>Add repository</span>
                      </button>
                    </div>
                  </Show>
                }
              >
                No lanes match this filter.
              </Show>
            </div>
          </Show>
        </Show>
      </div>

      <Show keyed when={extMenu()}>
        {(menu) => (
          <RepoExtMenu
            repoId={menu.repoId}
            x={menu.x}
            y={menu.y}
            onOpenExtensions={() => props.onOpenExtensions?.(menu.repoId)}
            onOpenNotes={() => {
              const repo = props.fleet.repos().find((r) => r.id === menu.repoId);
              if (repo) props.actions.openRepoNotes(repo);
            }}
            onRename={() => setRenameRepoId(menu.repoId)}
            onClose={() => setExtMenu(null)}
          />
        )}
      </Show>

      <Show when={renameTargetRepo()}>
        {(repo) => (
          <RepoRenameModal
            repo={repo()}
            onClose={() => setRenameRepoId(null)}
            onSubmit={async (label) => {
              await props.actions.renameRepo(repo(), label);
            }}
          />
        )}
      </Show>

      <Show when={props.fleet.focusedUsage()}>
        {(usage) => (
          <div class="border-t border-line bg-surface/50 p-2.5">
            {/* Rate Limits & Usage Quota */}
            <div class="rounded-lg border border-line/60 bg-raised/30 p-2">
              <div class="mb-1.5 flex items-center justify-between font-mono text-[10px] text-muted">
                <span class="font-semibold uppercase tracking-wider text-muted/90 flex items-center gap-1">
                  <IconCpu size={11} class="text-muted/70" />
                  <span>Rate Limits ({usage().label})</span>
                </span>
                {/* E9: staleness + manual refresh button */}
                <span class="flex items-center gap-1">
                  <span class="text-muted/60" title={`Updated ${usage().age_secs} seconds ago`}>
                    {usage().age_secs < 60 ? "just now" : `${Math.floor(usage().age_secs / 60)}m ago`}
                  </span>
                  <button
                    type="button"
                    class="focus-ring ml-0.5 flex items-center justify-center rounded p-0.5 text-muted/50 hover:bg-raised hover:text-muted transition-colors disabled:opacity-40"
                    title="Refresh usage data"
                    aria-label="Refresh rate limit data"
                    disabled={usageRefreshing()}
                    onClick={() => void refreshUsage()}
                  >
                    <IconRefresh size={9} class={usageRefreshing() ? "animate-spin" : ""} />
                  </button>
                </span>
              </div>
              <div class="space-y-1">
                <For each={usage().report.windows}>
                  {(window) => {
                    // E9: show reset time in tooltip when the daemon captured it; omit otherwise.
                    const resetStr = formatResetAt(window.reset_at);
                    const tooltipText = resetStr
                      ? `${formatUsageWindow(window.label)}: ${window.pct_used}% used · resets ${resetStr}`
                      : `${formatUsageWindow(window.label)}: ${window.pct_used}% used`;
                    return (
                      <div
                        class="flex items-center justify-between font-mono text-[10px] text-muted py-0.5"
                        title={tooltipText}
                      >
                        <span class="text-muted/80">{formatUsageWindow(window.label)}</span>
                        <span class={`rounded bg-raised px-1.5 py-0.2 ${usageTone(window.pct_used)}`}>
                          {window.pct_used}%
                        </span>
                      </div>
                    );
                  }}
                </For>
              </div>
            </div>
          </div>
        )}
      </Show>
    </>
  );
}
