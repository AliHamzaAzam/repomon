import { For, Show, createEffect, createMemo, createSignal } from "solid-js";

import type { Commit, Lane } from "../bindings";
import { translateError, type TranslatedError } from "../ipc/errors";
import { daemonCall, type LaneDiff } from "../ipc/rpc";
import type { FleetStore } from "../stores/fleet";
import { formatRelativeTime } from "./relativeTime";
import { IconArrowDown, IconArrowUp, IconGitBranch, IconGitCommit, IconRefresh } from "./icons";

const HISTORY_LIMIT = 30;

export interface ParsedCommit {
  oid: string;
  summary: string;
}

/// `lane.diff`'s `commits` field is raw `git log --oneline <merge_base>..HEAD` text — one
/// "oid summary" line per commit, newest first (see LaneDiff in crates/repomon-core/src/git/diff.rs).
/// Parsed here, at the edge, rather than shaping it server-side, since the daemon also forwards
/// the same text verbatim to the orchestrator as a human-readable log.
export function parseCommits(raw: string): ParsedCommit[] {
  return raw
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const sep = line.indexOf(" ");
      return sep === -1
        ? { oid: line, summary: "" }
        : { oid: line.slice(0, sep), summary: line.slice(sep + 1).trim() };
    });
}

/// The final line of a `git diff --stat` block is its summary ("3 files changed, 40
/// insertions(+), 2 deletions(-)"); the per-file lines above it don't fit this rail in v1.
function statSummary(stat: string): string | null {
  const lines = stat.split("\n").map((line) => line.trim()).filter(Boolean);
  return lines.length ? lines[lines.length - 1] : null;
}

function dirtyCount(lane: Lane): number {
  const dirty = lane.state.dirty;
  return dirty.staged + dirty.unstaged + dirty.untracked;
}

function RowSkeleton(props: { rows: number }) {
  return (
    <div class="animate-pulse space-y-1.5">
      <For each={Array.from({ length: props.rows })}>
        {() => <div class="h-5 rounded-lg bg-line/30" />}
      </For>
    </div>
  );
}

interface GitExplorerPanelProps {
  /// Full fleet store, mirroring FleetSidebar/TerminalWorkspace's prop threading — the panel
  /// only reads `selectedLane()` from it, but that memo lives on the store, not as a standalone
  /// prop. Optional so RightPanelHost's default-registry wiring degrades to the empty state
  /// instead of a type error if a future caller ever mounts this without a live fleet.
  fleet?: FleetStore;
}

export default function GitExplorerPanel(props: GitExplorerPanelProps) {
  const lane = () => props.fleet?.selectedLane() ?? null;

  const [branchData, setBranchData] = createSignal<LaneDiff | null>(null);
  const [history, setHistory] = createSignal<Commit[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<TranslatedError | null>(null);

  let epoch = 0;

  async function load(laneId: number) {
    const mine = ++epoch;
    setLoading(true);
    setError(null);
    try {
      const [diff, commits] = await Promise.all([
        daemonCall("lane.diff", { lane_id: laneId }),
        daemonCall("commit.recent", { lane_id: laneId, limit: HISTORY_LIMIT }),
      ]);
      if (mine !== epoch) return;
      setBranchData(diff);
      setHistory(commits);
    } catch (cause) {
      if (mine !== epoch) return;
      setError(translateError(cause, { binary: "git" }));
    } finally {
      if (mine === epoch) setLoading(false);
    }
  }

  // Piggybacks on the fleet store's own 1.2s poll instead of running a second timer: any change
  // to the selected lane's live git state (a new commit landing, files getting dirtied) already
  // updates `lane.state` through that poll, so tracking a signature derived from it here refetches
  // this panel's data in lockstep without repomon running two clocks for the same information.
  const signature = createMemo(() => {
    const l = lane();
    if (!l) return null;
    const s = l.state;
    return `${l.id}:${s.head}:${s.ahead}:${s.behind}:${s.dirty.staged}:${s.dirty.unstaged}:${s.dirty.untracked}`;
  });

  createEffect(() => {
    const sig = signature();
    const l = lane();
    if (!l || !sig) {
      epoch += 1; // invalidate any in-flight request from a lane we've since left
      setBranchData(null);
      setHistory([]);
      setError(null);
      setLoading(false);
      return;
    }
    void load(l.id);
  });

  function refresh() {
    const l = lane();
    if (l) void load(l.id);
  }

  return (
    <div class="flex h-full flex-col bg-surface">
      <div class="flex h-10 shrink-0 items-center justify-between border-b border-line bg-surface/95 px-3.5">
        <div class="flex min-w-0 items-center gap-2">
          <span class="text-xs font-semibold text-foreground">Git</span>
          <Show when={lane()} keyed>
            {(l) => (
              <>
                <span class="h-3 w-px shrink-0 bg-line/60" aria-hidden="true" />
                <span class="flex min-w-0 items-center gap-1 font-mono text-[11px] text-muted">
                  <IconGitBranch size={10} class="shrink-0 text-muted/60" />
                  <span class="min-w-0 truncate">{l.worktree.branch ?? "detached"}</span>
                </span>
                <Show when={l.state.ahead || l.state.behind}>
                  <span
                    class="inline-flex shrink-0 items-center gap-0.5 text-[10px] leading-none"
                    title={`Git tracking: ${l.state.ahead} ahead, ${l.state.behind} behind upstream`}
                  >
                    <Show when={l.state.ahead}>
                      <span class="inline-flex items-center text-signal"><IconArrowUp size={9} />{l.state.ahead}</span>
                    </Show>
                    <Show when={l.state.behind}>
                      <span class="inline-flex items-center text-muted"><IconArrowDown size={9} />{l.state.behind}</span>
                    </Show>
                  </span>
                </Show>
                <Show when={dirtyCount(l) > 0}>
                  <span
                    class="inline-flex shrink-0 items-center gap-0.5 text-[10px] font-semibold leading-none text-attention"
                    title={`${dirtyCount(l)} uncommitted file${dirtyCount(l) === 1 ? "" : "s"} (${l.state.dirty.staged} staged, ${l.state.dirty.unstaged} unstaged, ${l.state.dirty.untracked} untracked)`}
                  >
                    <span class="size-1.5 rounded-full bg-attention" />
                    <span>{dirtyCount(l)}</span>
                  </span>
                </Show>
              </>
            )}
          </Show>
        </div>
        <button
          type="button"
          class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
          onClick={refresh}
          disabled={!lane() || loading()}
          title="Refresh git status"
          aria-label="Refresh git status"
        >
          <IconRefresh size={12} class={loading() ? "animate-spin" : ""} />
        </button>
      </div>

      <Show when={error()} keyed>
        {(err) => (
          <div role="alert" class="m-3 mb-0 flex items-start justify-between gap-3 rounded-xl border border-fault/30 bg-fault/10 p-3 text-xs text-fault">
            <div class="min-w-0">
              <p class="font-semibold">Couldn't load git status</p>
              <p class="mt-0.5 break-words text-fault/80">{err.friendly}</p>
            </div>
            <button
              type="button"
              class="focus-ring shrink-0 rounded-lg border border-fault/40 bg-surface px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-fault/20"
              onClick={refresh}
            >
              Retry
            </button>
          </div>
        )}
      </Show>

      <Show
        when={lane()}
        fallback={
          <div class="flex flex-1 items-center justify-center p-4">
            <div class="max-w-[220px] space-y-2 rounded-xl border border-line bg-surface/40 p-3.5 text-center">
              <p class="text-xs font-medium text-foreground">No lane selected</p>
              <p class="text-xs text-muted">Select a lane in the fleet to see its branch and commit history.</p>
            </div>
          </div>
        }
      >
        <div class="min-h-0 flex-1 space-y-5 overflow-y-auto p-3">
          <section>
            <p class="section-label mb-2">Branch</p>
            <Show
              when={branchData()}
              keyed
              fallback={loading() ? <RowSkeleton rows={3} /> : <p class="text-xs text-muted">Git status unavailable.</p>}
            >
              {(diff) => {
                const commits = () => parseCommits(diff.commits);
                return (
                  <div class="space-y-2">
                    <Show
                      when={commits().length > 0}
                      fallback={
                        <p class="text-xs text-muted">
                          Nothing ahead of <span class="font-mono text-foreground">{diff.base}</span>
                        </p>
                      }
                    >
                      <p class="text-xs text-muted">
                        {commits().length} commit{commits().length === 1 ? "" : "s"} ahead of{" "}
                        <span class="font-mono text-foreground">{diff.base}</span>
                      </p>
                      <Show when={statSummary(diff.committed_stat)} keyed>
                        {(summary) => <p class="text-[10px] text-muted/70">{summary}</p>}
                      </Show>
                      <ul class="space-y-0.5">
                        <For each={commits()}>
                          {(commit) => (
                            <li class="flex items-center gap-1.5 rounded-lg px-1.5 py-1 hover:bg-raised/60">
                              <IconGitCommit size={10} class="shrink-0 text-muted/40" />
                              <span class="shrink-0 font-mono text-[10px] text-muted">{commit.oid}</span>
                              <span class="min-w-0 flex-1 truncate text-xs text-foreground">{commit.summary}</span>
                            </li>
                          )}
                        </For>
                      </ul>
                      <Show when={diff.commits_truncated}>
                        <p class="text-[10px] text-muted/70">Showing the most recent 20 commits.</p>
                      </Show>
                    </Show>
                  </div>
                );
              }}
            </Show>
          </section>

          <section>
            <p class="section-label mb-2">History</p>
            <Show when={!loading() || history().length > 0} fallback={<RowSkeleton rows={6} />}>
              <Show when={history().length > 0} fallback={<p class="text-xs text-muted">No commits recorded for this lane yet.</p>}>
                <ul class="space-y-0.5">
                  <For each={history()}>
                    {(commit) => (
                      <li class="flex items-center gap-1.5 rounded-lg px-1.5 py-1 hover:bg-raised/60">
                        <IconGitCommit size={10} class="shrink-0 text-muted/40" />
                        <span class="shrink-0 font-mono text-[10px] text-muted/70">{commit.oid.slice(0, 7)}</span>
                        <span class="min-w-0 flex-1 truncate text-xs text-foreground">{commit.summary}</span>
                        <span class="shrink-0 text-[10px] text-muted" title={new Date(commit.time).toLocaleString()}>
                          {formatRelativeTime(commit.time)}
                        </span>
                        <span class="max-w-[5.5rem] shrink-0 truncate text-[10px] text-muted/70">{commit.author_name}</span>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </Show>
          </section>
        </div>
      </Show>
    </div>
  );
}
