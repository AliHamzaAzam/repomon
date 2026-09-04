import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import type { AgentSession, Lane, Repo } from "../bindings";
import { fleetCounts, laneIndicator, laneIndicatorTitle, type FleetStore } from "../stores/fleet";
import type { ActionsStore } from "../stores/actions";
import type { WorkspaceStore } from "../stores/workspace";
import {
  readAutoCollapseEmptyLanes,
  onAutoCollapseChanged,
  notifyLayoutChanged,
} from "../stores/uiSettings";
import { primarySession } from "./agentLabel";
import { agentSessionTitle } from "./LaneAgentRosterPopover";
import { agentSessionTargetId } from "./agentIdentity";
import { formatResetAt } from "./resetTime";
import Modal from "./Modal";
import { reorderAround } from "./ordering";
import LaneRowMenu from "./LaneRowMenu";
import RepoExtMenu from "./RepoExtMenu";
import {
  AgentIcon,
  IconArrowDown,
  IconArrowUp,
  IconChevronDown,
  IconChevronRight,
  IconClose,
  IconCpu,
  IconHide,
  IconLayers,
  IconPin,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconShow,
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
  onContextMenu?: (lane: Lane, x: number, y: number) => void;
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
  // The daemon says why a status reads the way it does; the row never invents an explanation.
  const indicatorTitle = () => laneIndicatorTitle(props.lane);
  const primary = () => primarySession(props.lane.agent_sessions);
  const title = () => primary()?.custom_label ?? props.lane.worktree.name;
  const branchName = () => props.lane.worktree.branch ?? "detached";
  const dirty = () => dirtyCount(props.lane);
  const sessionCount = () => props.lane.agent_sessions.length;

  /// One change cell per row, so the numbers line up down a single column instead of moving
  /// with each row's content. Uncommitted work outranks divergence for the glyph; the tooltip
  /// still carries both, because that is where the detail belongs.
  const changeSummary = () => {
    const { ahead, behind } = props.lane.state;
    const d = props.lane.state.dirty;
    const parts: string[] = [];
    if (dirty() > 0) {
      parts.push(
        `${dirty()} uncommitted file${dirty() === 1 ? "" : "s"} (${d.staged} staged, ${d.unstaged} unstaged, ${d.untracked} untracked)`,
      );
    }
    if (ahead || behind) parts.push(`${ahead} ahead, ${behind} behind upstream`);
    if (!parts.length) return null;
    if (dirty() > 0) return { count: dirty(), icon: null, title: parts.join(" · ") };
    return {
      count: ahead || behind,
      icon: ahead ? <IconArrowUp size={9} /> : <IconArrowDown size={9} />,
      title: parts.join(" · "),
    };
  };

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
            onContextMenu={(event) => {
              event.preventDefault();
              clearTimers();
              setIsHovered(false);
              props.onContextMenu?.(props.lane, event.clientX, event.clientY);
            }}
            aria-current={props.selected ? "true" : undefined}
          >
            {/* Column 1: one health dot per row, so the left edge reads as a single strip. */}
            <span class="relative flex size-3 shrink-0 items-center justify-center">
              <Show
                when={sessionCount() === 0}
                fallback={
                  <span
                    class={`size-1.5 rounded-full ${
                      indicator().tone === "signal"
                        ? "bg-signal"
                        : indicator().tone === "attention"
                          ? "bg-attention"
                          : indicator().tone === "fault"
                            ? "bg-fault"
                            : "bg-muted/50"
                    }`}
                    aria-hidden="true"
                  />
                }
              >
                <button
                  type="button"
                  class="focus-ring flex size-3 items-center justify-center rounded text-muted/40 opacity-60 transition-opacity hover:text-foreground group-hover/lane-row:opacity-100"
                  onClick={(e) => {
                    e.stopPropagation();
                    props.toggleCollapse?.();
                  }}
                  title="Minimize inactive lane"
                  aria-label={`Minimize inactive lane ${title()}`}
                >
                  <IconChevronDown size={10} />
                </button>
              </Show>
            </span>

            {/* Column 2: identity. The branch is the part that gives way when space runs out. */}
            <span class="flex min-w-0 flex-1 items-baseline gap-1.5 text-left">
              <span
                class={`shrink-0 truncate text-xs ${
                  props.selected ? "font-semibold text-foreground" : "font-medium text-foreground/90"
                }`}
                style={{ "max-width": "60%" }}
              >
                {title()}
              </span>
              <Show when={props.lane.pinned}>
                <span
                  class="flex shrink-0 self-center text-signal"
                  title="Pinned lane"
                  aria-label="Pinned"
                >
                  <IconPin size={9} />
                </span>
              </Show>
              <span class="min-w-0 truncate font-mono text-[10px] text-muted/80" title={branchName()}>
                {branchName()}
              </span>
              <Show when={props.lane.state.merged && !props.lane.worktree.is_main}>
                <span
                  class="shrink-0 font-mono text-[9px] uppercase tracking-normal text-muted/60"
                  title="Every commit on this branch is already in the default branch. The worktree can be removed."
                >
                  merged
                </span>
              </Show>
            </span>

            {/* Column 3: how many agents, and what the most urgent one is doing. */}
            <span class="flex shrink-0 items-center gap-1">
              <Show when={sessionCount() > 1}>
                <span
                  class="inline-flex items-center gap-0.5 font-mono text-[10px] leading-none text-muted"
                  aria-label={`${sessionCount()} agents in this lane`}
                  title={`${sessionCount()} agents in this lane`}
                >
                  <IconLayers size={9} class="shrink-0 text-muted/70" />
                  {sessionCount()}
                </span>
              </Show>
              <Show when={sessionCount() === 1 ? primary() : null}>
                {(agentSession) => (
                  <AgentIcon
                    agent={agentSession().agent}
                    size={10}
                    class="shrink-0 text-muted/70"
                  />
                )}
              </Show>
              <Show when={indicator().label}>
                <span class={`lane-status is-${indicator().tone}`} title={indicatorTitle()}>
                  {indicator().label}
                </span>
              </Show>
            </span>

            {/* Column 4: one fixed-width change cell, so every row's numbers stack in a column. */}
            <span class="flex w-9 shrink-0 justify-end font-mono text-[10px] leading-none text-muted">
              <Show when={changeSummary()}>
                {(summary) => (
                  <span
                    class={`inline-flex items-center gap-0.5 ${dirty() > 0 ? "font-semibold text-attention" : ""}`}
                    title={summary().title}
                  >
                    <Show when={dirty() > 0} fallback={summary().icon}>
                      <span class="size-1.5 rounded-full bg-attention" />
                    </Show>
                    {summary().count}
                  </span>
                )}
              </Show>
            </span>
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
        class={`group/collapsed-row fleet-row focus-ring flex cursor-pointer items-center justify-between text-muted transition-colors hover:text-foreground ${
          props.selected ? "is-selected" : ""
        }`}
        onClick={props.select}
        role="button"
        tabIndex={0}
        aria-current={props.selected ? "true" : undefined}
        title={`${title()} (${branchName()}). Minimized: this lane has no agent.`}
      >
        <div class="flex min-w-0 items-center gap-1.5">
          <button
            type="button"
            class="focus-ring flex size-3 shrink-0 items-center justify-center rounded text-muted/50 hover:text-foreground"
            onClick={(e) => {
              e.stopPropagation();
              props.toggleCollapse?.();
            }}
            title="Expand lane"
            aria-label={`Expand lane ${title()}`}
          >
            <IconChevronRight size={10} />
          </button>
          <span class="truncate text-xs font-medium text-muted/80">{title()}</span>
          <Show when={props.lane.state.merged && !props.lane.worktree.is_main}>
            <span
              class="shrink-0 font-mono text-[9px] uppercase text-muted/60"
              title="Every commit on this branch is already in the default branch. The worktree can be removed."
            >
              merged
            </span>
          </Show>
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

/// One of the three fleet filters. They are toggles, so they look like toggles: pressed state is
/// carried by the whole chip, not by the number alone, and a chip with nothing to show recedes
/// rather than disappearing (the count itself is the answer to "is anything running?").
function FilterChip(props: {
  label: string;
  count: number;
  tone: "attention" | "signal" | "muted";
  pressed: boolean;
  title: string;
  onToggle: () => void;
}) {
  // Written out per tone rather than interpolated: Tailwind only emits classes it can read as
  // whole strings in the source.
  const pressedShell = {
    attention: "border-attention/60 bg-attention/20 text-attention font-semibold ring-1 ring-attention/25",
    signal: "border-signal/60 bg-signal/20 text-signal font-semibold ring-1 ring-signal/25",
    muted: "border-line bg-raised text-foreground font-semibold ring-1 ring-line",
  } as const;
  const liveShell = {
    attention: "border-attention/30 bg-attention/6 text-foreground/85 hover:border-attention/50 hover:bg-attention/12",
    signal: "border-signal/30 bg-signal/6 text-foreground/85 hover:border-signal/50 hover:bg-signal/12",
    muted: "border-line bg-raised/60 text-muted hover:bg-raised hover:text-foreground",
  } as const;
  const countTone = {
    attention: "text-attention font-semibold",
    signal: "text-signal font-semibold",
    muted: "text-muted font-medium",
  } as const;
  const shell = () =>
    props.pressed
      ? pressedShell[props.tone]
      : props.count > 0
        ? liveShell[props.tone]
        : "border-line bg-raised/60 text-muted hover:bg-raised hover:text-foreground";
  return (
    <button
      type="button"
      class={`focus-ring flex h-7 min-w-0 flex-1 items-center justify-between gap-1.5 rounded-lg border px-2 text-[11px] font-medium transition-colors ${shell()}`}
      onClick={props.onToggle}
      aria-pressed={props.pressed}
      title={props.title}
    >
      <span class="truncate">{props.label}</span>
      <span
        class={`font-mono text-[11px] ${
          props.pressed ? "font-bold" : props.count > 0 ? countTone[props.tone] : "text-muted/70 font-medium"
        }`}
      >
        {props.count}
      </span>
    </button>
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

/// Rename a repo's sidebar display. The folder name on disk never changes: this sets a label
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
  const [laneMenu, setLaneMenu] = createSignal<{ lane: Lane; x: number; y: number } | null>(null);
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
    const targetId = agentSessionTargetId(session);
    if (!targetId) return;
    props.actions.rename({
      targetId,
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
          <FilterChip
            label="Needs attention"
            count={props.fleet.counts().urgent}
            tone="attention"
            pressed={props.fleet.urgentOnly()}
            onToggle={() => props.fleet.setUrgentOnly(!props.fleet.urgentOnly())}
            title={
              props.fleet.urgentOnly()
                ? "Show all lanes"
                : "Show only lanes with an agent that needs you"
            }
          />
          <FilterChip
            label="Running"
            count={props.fleet.counts().running}
            tone="signal"
            pressed={props.fleet.runningOnly()}
            onToggle={() => props.fleet.setRunningOnly(!props.fleet.runningOnly())}
            title={props.fleet.runningOnly() ? "Show all lanes" : "Show only lanes with a running agent"}
          />
          <FilterChip
            label="Idle"
            count={props.fleet.counts().idle}
            tone="muted"
            pressed={props.fleet.idleOnly()}
            onToggle={() => props.fleet.setIdleOnly(!props.fleet.idleOnly())}
            title={props.fleet.idleOnly() ? "Show all lanes" : "Show only lanes with an idle agent"}
          />
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
              // The header answers "does anything in here want me?" without expanding the list.
              const repoUrgent = createMemo(() => fleetCounts(laneList()).urgent);
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
                      <span class="flex min-w-0 items-center gap-1.5">
                        <span
                          class="truncate font-mono text-[11px] font-semibold uppercase tracking-[0.02em] text-muted transition-colors hover:text-foreground cursor-default"
                          title={
                            repo.label
                              ? `${repoDisplayName(repo)}. Repository: ${repo.name} (${repo.path})`
                              : `Repository: ${repo.name} (${repo.path})`
                          }
                        >
                          {repoDisplayName(repo)}
                        </span>
                        <Show when={repoUrgent()}>
                          <span
                            class="inline-flex shrink-0 items-center gap-1 font-mono text-[10px] font-semibold leading-none text-attention"
                            title={`${repoUrgent()} agent${repoUrgent() === 1 ? "" : "s"} in this project need you`}
                          >
                            <span class="size-1.5 rounded-full bg-attention" />
                            {repoUrgent()}
                          </span>
                        </Show>
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
                          class="ml-0.5 rounded bg-raised px-1.5 font-mono text-[10px] text-muted"
                          title={`${laneList().length} lane${laneList().length === 1 ? "" : "s"} in ${repoDisplayName(repo)}`}
                        >
                          {laneList().length} lanes
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
                            onContextMenu={(target, x, y) => setLaneMenu({ lane: target, x, y })}
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
                <div class="flex min-w-0 items-center gap-1.5">
                  <span class="flex size-3 shrink-0 items-center justify-center rounded text-muted group-hover/hidden-header:text-foreground">
                    <Show when={hiddenCollapsed()} fallback={<IconChevronDown size={10} strokeWidth={2} />}>
                      <IconChevronRight size={10} strokeWidth={2} />
                    </Show>
                  </span>
                  <span class="truncate font-mono text-[10px] font-semibold uppercase tracking-[0.02em] text-muted group-hover/hidden-header:text-foreground">
                    Hidden ({props.fleet.hiddenRepos().length})
                  </span>
                </div>
              </button>
              {/* Expanded, these are still one line each: a hidden project is a parking space, not
                  a row that deserves the width of a live lane. */}
              <Show when={!hiddenCollapsed()}>
                <div class="mt-0.5">
                  <For each={props.fleet.hiddenRepos()}>
                    {(repo) => (
                      <div class="group/hidden-row flex items-center gap-1 rounded px-2 py-0.5 transition-colors hover:bg-raised/50">
                        <span
                          class="min-w-0 flex-1 truncate font-mono text-[10px] uppercase tracking-[0.02em] text-muted"
                          title={repo.path}
                        >
                          {repoDisplayName(repo)}
                        </span>
                        <button
                          type="button"
                          class="focus-ring flex size-4 shrink-0 items-center justify-center rounded text-muted/50 opacity-0 transition-opacity hover:text-signal group-hover/hidden-row:opacity-100 focus-visible:opacity-100"
                          onClick={() => void props.actions.setRepoHidden(repo, false)}
                          title={`Show ${repo.name} again`}
                          aria-label={`Show ${repo.name} again`}
                        >
                          <IconShow size={11} />
                        </button>
                      </div>
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

      <Show keyed when={laneMenu()}>
        {(menu) => (
          <LaneRowMenu
            lane={menu.lane}
            x={menu.x}
            y={menu.y}
            onPin={() => void props.actions.pinLane(menu.lane)}
            onRemoveWorktree={() => props.actions.deleteLane(menu.lane)}
            onClose={() => setLaneMenu(null)}
          />
        )}
      </Show>

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
