import { For, Show, createMemo, createSignal } from "solid-js";

import type { Lane } from "../bindings";
import { laneIndicator, type FleetStore } from "../stores/fleet";
import type { ActionsStore } from "../stores/actions";
import RepoExtMenu from "./RepoExtMenu";

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
  const title = () => props.lane.agent_sessions[0]?.custom_label
    ?? props.lane.agent_sessions[0]?.title
    ?? props.lane.worktree.name;

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
          <span class="truncate text-xs font-medium text-foreground">{title()}</span>
          <Show when={props.lane.pinned}>
            <span class="text-[0.6rem] text-signal" aria-label="Pinned">◆</span>
          </Show>
        </span>
        <span class="mt-0.5 flex min-w-0 items-center gap-1.5 font-mono text-[0.58rem] text-muted">
          <span class="truncate">{props.lane.worktree.branch ?? "detached"}</span>
          <Show when={props.lane.state.ahead || props.lane.state.behind}>
            <span>↑{props.lane.state.ahead} ↓{props.lane.state.behind}</span>
          </Show>
          <Show when={dirtyCount(props.lane) > 0}>
            <span class="text-attention">●{dirtyCount(props.lane)}</span>
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
      <div class="space-y-2 border-b border-line p-3">
        <label class="relative block">
          <span class="sr-only">Filter fleet</span>
          <input
            ref={props.searchRef}
            class="focus-ring h-8 w-full rounded-md border border-line bg-background pl-7 pr-2 font-mono text-[0.65rem] outline-none placeholder:text-muted/70"
            value={props.fleet.query()}
            onInput={(event) => props.fleet.setQuery(event.currentTarget.value)}
            placeholder="Filter fleet  /"
          />
          <span class="absolute left-2.5 top-1/2 -translate-y-1/2 font-mono text-xs text-muted">⌕</span>
        </label>
        <div class="flex gap-2">
          <button
            type="button"
            class={`focus-ring flex h-7 flex-1 items-center justify-between rounded-md border px-2 font-mono text-[0.58rem] uppercase tracking-[0.1em] ${props.fleet.urgentOnly() ? "border-attention/50 bg-attention/10 text-attention" : "border-line bg-raised text-muted"}`}
            onClick={() => props.fleet.setUrgentOnly(!props.fleet.urgentOnly())}
            aria-pressed={props.fleet.urgentOnly()}
          >
            <span>Urgent only</span>
            <span>{props.fleet.counts().urgent}</span>
          </button>
          <button
            type="button"
            class="focus-ring flex h-7 items-center gap-1 rounded-md border border-line bg-raised px-2 font-mono text-[0.58rem] uppercase tracking-[0.1em] text-muted hover:text-foreground"
            onClick={() => void props.actions.addRepo()}
            title="Add a repository"
          >
            <span>+ Repo</span>
          </button>
        </div>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto px-2 py-2">
        <Show when={!props.fleet.loading() || props.fleet.lanes().length} fallback={<p class="p-3 text-xs text-muted">Syncing fleet…</p>}>
          <For each={props.fleet.repos()}>
            {(repo) => {
              // Per-repo lane slice, recomputed reactively but reusing the store's stable lane
              // rows — so the section and its rows persist across polls and hover holds.
              const laneList = createMemo(() =>
                props.fleet.visibleLanes().filter((lane) => lane.repo.id === repo.id),
              );
              return (
                <Show when={laneList().length > 0 || !props.fleet.query()}>
                  <section class="group/repo mb-2" aria-label={repo.name}>
                    <div
                      class="flex items-center justify-between px-2 py-1.5"
                      onContextMenu={(event) => {
                        event.preventDefault();
                        setExtMenu({ repoId: repo.id, x: event.clientX, y: event.clientY });
                      }}
                    >
                      <span class="truncate font-mono text-[0.61rem] font-semibold uppercase tracking-[0.08em] text-muted">
                        {repo.name}
                      </span>
                      <span class="flex items-center gap-1.5">
                        <button
                          type="button"
                          class="focus-ring rounded px-1 font-mono text-[0.7rem] leading-none text-muted opacity-0 transition-opacity hover:text-signal focus-visible:opacity-100 group-focus-within/repo:opacity-100 group-hover/repo:opacity-100"
                          onClick={() => props.actions.newLane(repo.id)}
                          title={`New lane in ${repo.name}`}
                          aria-label={`New lane in ${repo.name}`}
                        >+</button>
                        <button
                          type="button"
                          class="focus-ring rounded px-1 font-mono text-[0.7rem] leading-none text-muted opacity-0 transition-opacity hover:text-fault focus-visible:opacity-100 group-focus-within/repo:opacity-100 group-hover/repo:opacity-100"
                          onClick={() => props.actions.removeRepo(repo)}
                          title={`Remove ${repo.name}`}
                          aria-label={`Remove ${repo.name}`}
                        >×</button>
                        <span class="font-mono text-[0.55rem] text-muted/70">{laneList().length}</span>
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
          <Show when={!props.fleet.visibleLanes().length}>
            <div class="m-2 rounded-lg border border-dashed border-line p-3 text-xs leading-relaxed text-muted">
              <Show
                when={props.fleet.query() || props.fleet.urgentOnly()}
                fallback={
                  <div class="space-y-2">
                    <p>No repositories yet.</p>
                    <button
                      type="button"
                      class="focus-ring rounded-md border border-signal/40 bg-signal/10 px-3 py-1.5 font-mono text-[0.58rem] uppercase tracking-[0.1em] text-signal"
                      onClick={() => void props.actions.addRepo()}
                    >Add a repository</button>
                  </div>
                }
              >
                No lanes match this view.
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
            onClose={() => setExtMenu(null)}
          />
        )}
      </Show>

      <div class="border-t border-line p-3">
        <div class="grid grid-cols-2 gap-2 font-mono text-[0.58rem] uppercase tracking-[0.08em] text-muted">
          <span>Needs you <b class="ml-1 text-attention">{props.fleet.counts().urgent}</b></span>
          <span>Running <b class="ml-1 text-signal">{props.fleet.counts().running}</b></span>
        </div>
        <Show when={props.fleet.usage()[0]}>
          {(usage) => (
            <div class="mt-2 border-t border-line pt-2">
              <div class="mb-1 flex items-center justify-between font-mono text-[0.56rem] text-muted">
                <span>{usage().label}</span>
                <span>{usage().age_secs}s ago</span>
              </div>
              <div class="flex gap-1">
                <For each={usage().report.windows}>
                  {(window) => (
                    <span class="rounded border border-line bg-raised px-1.5 py-0.5 font-mono text-[0.55rem] text-muted">
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
