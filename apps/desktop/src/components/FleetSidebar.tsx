import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import type { Lane } from "../bindings";
import { laneIndicator, type FleetStore } from "../stores/fleet";
import type { ActionsStore } from "../stores/actions";
import {
  readAutoCollapseEmptyLanes,
  onAutoCollapseChanged,
} from "../stores/uiSettings";
import { primarySession } from "./agentLabel";
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
  IconSearch,
} from "./icons";

interface FleetSidebarProps {
  fleet: FleetStore;
  actions: ActionsStore;
  searchRef?: (element: HTMLInputElement) => void;
  onOpenExtensions?: (repoId: number) => void;
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
}) {
  const indicator = () => laneIndicator(props.lane);
  const primary = () => primarySession(props.lane.agent_sessions);
  const title = () => primary()?.custom_label ?? props.lane.worktree.name;
  const branchName = () => props.lane.worktree.branch ?? "detached";
  const dirty = () => dirtyCount(props.lane);
  const sessionCount = () => props.lane.agent_sessions.length;

  return (
    <Show
      when={props.collapsed}
      fallback={
        <button
          type="button"
          class={`group/lane-row fleet-row focus-ring ${props.selected ? "is-selected" : ""}`}
          onClick={props.select}
          aria-current={props.selected ? "true" : undefined}
          title={`${title()} (${branchName()})`}
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
                title={title()}
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
              <span class="truncate" title={`Branch: ${branchName()}`}>
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
                  title={`${sessionCount()} active agent sessions open in this lane`}
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

export default function FleetSidebar(props: FleetSidebarProps) {
  const [extMenu, setExtMenu] = createSignal<{ repoId: number; x: number; y: number } | null>(null);
  const [autoCollapse, setAutoCollapse] = createSignal<boolean>(readAutoCollapseEmptyLanes());
  const [manuallyExpandedLanes, setManuallyExpandedLanes] = createSignal<Set<number>>(new Set());
  const [manuallyCollapsedLanes, setManuallyCollapsedLanes] = createSignal<Set<number>>(loadCollapsedLanes());
  const [hiddenCollapsed, setHiddenCollapsed] = createSignal<boolean>(loadHiddenSectionCollapsed());

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
  };

  onMount(() => {
    const unsub = onAutoCollapseChanged((enabled) => {
      setAutoCollapse(enabled);
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
  };

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
                ? "border-attention/50 bg-attention/10 text-attention font-semibold"
                : props.fleet.counts().urgent > 0
                  ? "border-attention/30 bg-attention/5 text-attention hover:bg-attention/10"
                  : "border-line bg-raised/70 text-muted hover:text-foreground hover:bg-raised"
            }`}
            onClick={() => props.fleet.setUrgentOnly(!props.fleet.urgentOnly())}
            aria-pressed={props.fleet.urgentOnly()}
            title={props.fleet.urgentOnly() ? "Show all lanes" : "Filter to lanes needing attention"}
          >
            <span class="truncate">Needs attention</span>
            <span class="font-mono text-[11px] font-semibold">{props.fleet.counts().urgent}</span>
          </button>
          <span
            class="flex h-7 shrink-0 items-center gap-1.5 rounded-lg border border-line bg-raised/70 px-2 font-mono text-[11px] text-muted"
            title={`${props.fleet.counts().running} active agent session${props.fleet.counts().running === 1 ? "" : "s"} currently running`}
          >
            <span class={`size-1.5 rounded-full ${props.fleet.counts().running > 0 ? "bg-signal animate-pulse" : "bg-muted/40"}`} />
            <span class="text-[10px] font-sans font-medium text-muted/70">Running</span>
            <b class={props.fleet.counts().running > 0 ? "text-signal font-semibold" : "text-muted"}>
              {props.fleet.counts().running}
            </b>
          </span>
          <button
            type="button"
            class="focus-ring flex h-7 shrink-0 items-center gap-1 rounded-lg border border-line bg-raised/70 px-2 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
            onClick={() => void props.actions.addRepo()}
            title="Add a repository"
          >
            <IconPlus size={12} />
            <span>Repo</span>
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
                  <section class="mb-2.5" aria-label={repo.name}>
                    <div
                      class="group/repo-header flex items-center justify-between rounded px-2 py-1 text-muted transition-colors hover:bg-raised/40"
                      onContextMenu={(event) => {
                        event.preventDefault();
                        setExtMenu({ repoId: repo.id, x: event.clientX, y: event.clientY });
                      }}
                    >
                      <span
                        class="truncate font-mono text-[11px] font-semibold uppercase tracking-wider text-muted hover:text-foreground transition-colors cursor-default"
                        title={`Repository: ${repo.name} (${repo.path})`}
                      >
                        {repo.name}
                      </span>
                      <span class="flex items-center gap-1 shrink-0">
                        <div class="flex items-center gap-0.5 opacity-0 transition-opacity group-hover/repo-header:opacity-100 focus-within:opacity-100">
                          <button
                            type="button"
                            class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-signal"
                            onClick={() => props.actions.newLane(repo.id)}
                            title={`New lane in ${repo.name}`}
                            aria-label={`New lane in ${repo.name}`}
                          >
                            <IconPlus size={12} />
                          </button>
                          <button
                            type="button"
                            class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                            onClick={() => void props.actions.setRepoHidden(repo, true)}
                            title={`Hide ${repo.name} (stays registered)`}
                            aria-label={`Hide ${repo.name}`}
                          >
                            <IconHide size={12} />
                          </button>
                          <button
                            type="button"
                            class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-fault"
                            onClick={() => props.actions.removeRepo(repo)}
                            title={`Remove ${repo.name}`}
                            aria-label={`Remove ${repo.name}`}
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
            onClose={() => setExtMenu(null)}
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
                <span class="text-muted/60" title={`Updated ${usage().age_secs} seconds ago`}>
                  {usage().age_secs < 60 ? "just now" : `${Math.floor(usage().age_secs / 60)}m ago`}
                </span>
              </div>
              <div class="space-y-1">
                <For each={usage().report.windows}>
                  {(window) => (
                    <div
                      class="flex items-center justify-between font-mono text-[10px] text-muted py-0.5"
                      title={`${formatUsageWindow(window.label)}: ${window.pct_used}% used`}
                    >
                      <span class="text-muted/80">{formatUsageWindow(window.label)}</span>
                      <span class={`rounded bg-raised px-1.5 py-0.2 ${usageTone(window.pct_used)}`}>
                        {window.pct_used}%
                      </span>
                    </div>
                  )}
                </For>
              </div>
            </div>
          </div>
        )}
      </Show>
    </>
  );
}
