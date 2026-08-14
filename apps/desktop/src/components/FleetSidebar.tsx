import { For, Show, createMemo, createSignal } from "solid-js";

import type { Lane } from "../bindings";
import { laneIndicator, type FleetStore } from "../stores/fleet";
import type { ActionsStore } from "../stores/actions";
import { primarySession } from "./agentLabel";
import RepoExtMenu from "./RepoExtMenu";
import {
  AgentIcon,
  IconArrowDown,
  IconArrowUp,
  IconClose,
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

function LaneRow(props: { lane: Lane; selected: boolean; select: () => void }) {
  const indicator = () => laneIndicator(props.lane);
  const primary = () => primarySession(props.lane.agent_sessions);
  const title = () => primary()?.custom_label ?? props.lane.worktree.name;

  return (
    <button
      type="button"
      class={`fleet-row focus-ring ${props.selected ? "is-selected" : ""}`}
      onClick={props.select}
      aria-current={props.selected ? "true" : undefined}
    >
      <span class={`lane-pulse is-${indicator().tone}`} aria-hidden="true" />
      <span class="min-w-0 flex-1 text-left">
        <span class="flex items-center gap-1.5">
          <Show when={primary()}>
            <span class="shrink-0 text-muted/80">
              <AgentIcon agent={primary()?.agent} size={12} />
            </span>
          </Show>
          <span class="truncate text-xs font-medium text-foreground">{title()}</span>
          <Show when={props.lane.agent_sessions.length > 1}>
            <span
              class="inline-flex shrink-0 items-center gap-0.5 rounded border border-line bg-raised px-1 py-0.5 font-mono text-[10px] leading-none text-muted"
              title={`${props.lane.agent_sessions.length} agents open in this lane`}
              aria-label={`${props.lane.agent_sessions.length} agents open`}
            >
              <IconLayers size={10} />
              <span>{props.lane.agent_sessions.length}</span>
            </span>
          </Show>
          <Show when={props.lane.pinned}>
            <span class="text-signal" aria-label="Pinned">
              <IconPin size={11} />
            </span>
          </Show>
        </span>
        <span class="mt-0.5 flex min-w-0 items-center gap-1.5 font-mono text-[11px] text-muted">
          <span class="flex items-center gap-1 truncate">
            <IconGitBranch size={11} />
            <span class="truncate">{props.lane.worktree.branch ?? "detached"}</span>
          </span>
          <Show when={props.lane.state.ahead || props.lane.state.behind}>
            <span class="inline-flex items-center gap-0.5 text-[10px]">
              <Show when={props.lane.state.ahead}><IconArrowUp size={10} />{props.lane.state.ahead}</Show>
              <Show when={props.lane.state.behind}><IconArrowDown size={10} />{props.lane.state.behind}</Show>
            </span>
          </Show>
          <Show when={dirtyCount(props.lane) > 0}>
            <span class="inline-flex items-center gap-1 text-[10px] text-attention font-semibold">
              <span class="size-1.5 rounded-full bg-attention" />
              <span>{dirtyCount(props.lane)}</span>
            </span>
          </Show>
        </span>
      </span>
      <span class={`lane-badge is-${indicator().tone}`}>{indicator().label}</span>
    </button>
  );
}

export default function FleetSidebar(props: FleetSidebarProps) {
  const [extMenu, setExtMenu] = createSignal<{ repoId: number; x: number; y: number } | null>(null);

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
        <div class="flex gap-1.5">
          <button
            type="button"
            class={`focus-ring flex h-7 flex-1 items-center justify-between rounded-lg border px-2.5 text-xs font-medium transition-colors ${props.fleet.urgentOnly() ? "border-attention/50 bg-attention/10 text-attention" : "border-line bg-raised/70 text-muted hover:text-foreground hover:bg-raised"}`}
            onClick={() => props.fleet.setUrgentOnly(!props.fleet.urgentOnly())}
            aria-pressed={props.fleet.urgentOnly()}
          >
            <span>Needs attention</span>
            <span class="font-mono text-[11px] font-semibold">{props.fleet.counts().urgent}</span>
          </button>
          <button
            type="button"
            class="focus-ring flex h-7 items-center gap-1 rounded-lg border border-line bg-raised/70 px-2 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
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
                  <section class="group/repo mb-2.5" aria-label={repo.name}>
                    <div
                      class="flex items-center justify-between px-2 py-1"
                      onContextMenu={(event) => {
                        event.preventDefault();
                        setExtMenu({ repoId: repo.id, x: event.clientX, y: event.clientY });
                      }}
                    >
                      <span class="truncate font-mono text-[11px] font-semibold uppercase tracking-wider text-muted">
                        {repo.name}
                      </span>
                      <span class="flex items-center gap-1">
                        <button
                          type="button"
                          class="focus-ring flex size-5 items-center justify-center rounded text-muted opacity-0 transition-opacity hover:bg-raised hover:text-signal focus-visible:opacity-100 group-focus-within/repo:opacity-100 group-hover/repo:opacity-100"
                          onClick={() => props.actions.newLane(repo.id)}
                          title={`New lane in ${repo.name}`}
                          aria-label={`New lane in ${repo.name}`}
                        >
                          <IconPlus size={12} />
                        </button>
                        <button
                          type="button"
                          class="focus-ring flex size-5 items-center justify-center rounded text-muted opacity-0 transition-opacity hover:bg-raised hover:text-foreground focus-visible:opacity-100 group-focus-within/repo:opacity-100 group-hover/repo:opacity-100"
                          onClick={() => void props.actions.setRepoHidden(repo, true)}
                          title={`Hide ${repo.name} (stays registered)`}
                          aria-label={`Hide ${repo.name}`}
                        >
                          <IconHide size={12} />
                        </button>
                        <button
                          type="button"
                          class="focus-ring flex size-5 items-center justify-center rounded text-muted opacity-0 transition-opacity hover:bg-raised hover:text-fault focus-visible:opacity-100 group-focus-within/repo:opacity-100 group-hover/repo:opacity-100"
                          onClick={() => props.actions.removeRepo(repo)}
                          title={`Remove ${repo.name}`}
                          aria-label={`Remove ${repo.name}`}
                        >
                          <IconClose size={12} />
                        </button>
                        <span class="ml-0.5 rounded bg-raised px-1 font-mono text-[10px] text-muted">{laneList().length}</span>
                      </span>
                    </div>
                    <div class="space-y-0.5">
                      <For each={laneList()}>
                        {(lane) => (
                          <LaneRow
                            lane={lane}
                            selected={props.fleet.selectedLaneId() === lane.id}
                            select={() => props.fleet.setSelectedLaneId(lane.id)}
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
              <p class="px-2 py-1 font-mono text-[10px] uppercase tracking-wider text-muted">
                Hidden ({props.fleet.hiddenRepos().length})
              </p>
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

      <div class="border-t border-line bg-surface/50 p-3">
        <div class="grid grid-cols-2 gap-2 text-xs text-muted">
          <span class="flex items-center justify-between rounded-md bg-raised/50 px-2 py-1 font-mono text-[11px]">
            <span>Needs you</span>
            <b class="text-attention">{props.fleet.counts().urgent}</b>
          </span>
          <span class="flex items-center justify-between rounded-md bg-raised/50 px-2 py-1 font-mono text-[11px]">
            <span>Running</span>
            <b class="text-signal">{props.fleet.counts().running}</b>
          </span>
        </div>
        <Show when={props.fleet.focusedUsage()}>
          {(usage) => (
            <div class="mt-2.5 border-t border-line/60 pt-2">
              <div class="mb-1.5 flex items-center justify-between font-mono text-[10px] text-muted">
                <span class="font-medium text-foreground">{usage().label}</span>
                <span>{usage().age_secs}s ago</span>
              </div>
              <div class="flex flex-wrap gap-1">
                <For each={usage().report.windows}>
                  {(window) => (
                    <span class="rounded border border-line bg-raised px-1.5 py-0.5 font-mono text-[10px] text-muted">
                      {window.label} {window.pct_used}%
                    </span>
                  )}
                </For>
              </div>
            </div>
          )}
        </Show>
      </div>
    </>
  );
}
