import { For, Show, createEffect, createMemo, createSignal } from "solid-js";

import type { Commit, Lane } from "../bindings";
import DiffView from "./DiffView";
import { translateError, type TranslatedError } from "../ipc/errors";
import { daemonCall, type LaneDiff } from "../ipc/rpc";
import type { FleetStore } from "../stores/fleet";
import { formatRelativeTime } from "./relativeTime";
import {
  IconArrowDown,
  IconArrowUp,
  IconChevronDown,
  IconChevronRight,
  IconGitBranch,
  IconGitCommit,
  IconRefresh,
} from "./icons";

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

export interface StatFileRow {
  /// Post-rename path (or the only path, for a non-rename). Brace-form renames
  /// ("src/{old => new}/mod.rs") are expanded back to the full new path.
  path: string;
  /// The pre-rename path, present only when the line described a rename/move.
  renamedFrom?: string;
  /// Count of literal `+`/`-` characters in the stat bar. For small diffs this equals the real
  /// insertion/deletion counts; `git diff --stat` scales the bar for large diffs to fit its
  /// column width, so past that point these become proportional, not exact. `--stat` is what the
  /// daemon sends (see `LaneDiff.uncommitted_stat` in crates/repomon-core/src/git/diff.rs) — an
  /// exact split would need `--numstat` instead, which isn't part of the RPC surface today.
  adds: number;
  dels: number;
  binary: boolean;
}

/// Splits a rename's `raw` path column into its `{ path, renamedFrom }`, handling both the plain
/// form (`old/path.ts => new/path.ts`) and the brace form git uses when a common prefix and/or
/// suffix survive the move (`src/{old => new}/mod.rs`). Returns `raw` unchanged when it isn't a
/// rename at all.
function parseStatPath(raw: string): { path: string; renamedFrom?: string } {
  const brace = raw.match(/^(.*)\{(.*) => (.*)\}(.*)$/);
  if (brace) {
    const [, prefix, from, to, suffix] = brace;
    return { path: `${prefix}${to}${suffix}`, renamedFrom: `${prefix}${from}${suffix}` };
  }
  const plain = raw.match(/^(.*) => (.*)$/);
  if (plain) {
    const [, from, to] = plain;
    return { path: to, renamedFrom: from };
  }
  return { path: raw };
}

/// Parses `git diff --stat`'s per-file lines (as sent verbatim in `LaneDiff.uncommitted_stat` and
/// `committed_stat`) into structured rows. Every file line has the shape `path | rest` (a literal
/// " | " column separator that git always emits, even though the padding around it varies with the
/// longest path in the block); the trailing summary line ("3 files changed, ...") never contains
/// "|", so filtering on that separator is enough to drop it without a second pass. `rest` is either
/// `Bin <old> -> <new> bytes` for a binary file or `N <bar>` for text, where `<bar>` is `+`/`-`
/// characters (see `StatFileRow.adds`/`dels` for the scaling caveat); a content-free rename reports
/// `0` with an empty bar, which naturally parses to zero adds/dels.
export function parseStatFiles(stat: string): StatFileRow[] {
  return stat
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.includes(" | "))
    .map((line) => {
      const sep = line.indexOf(" | ");
      const rawPath = line.slice(0, sep).trim();
      const rest = line.slice(sep + 3).trim();
      const { path, renamedFrom } = parseStatPath(rawPath);
      const binary = rest.startsWith("Bin");
      const adds = binary ? 0 : (rest.match(/\+/g) ?? []).length;
      const dels = binary ? 0 : (rest.match(/-/g) ?? []).length;
      return { path, renamedFrom, adds, dels, binary };
    });
}

/// Splits a path into its muted directory prefix (trailing slash kept) and emphasized basename,
/// for the dir-muted/basename-emphasized row treatment.
function splitPath(path: string): { dir: string; base: string } {
  const idx = path.lastIndexOf("/");
  return idx === -1 ? { dir: "", base: path } : { dir: path.slice(0, idx + 1), base: path.slice(idx + 1) };
}

/// Working-tree file rows are the one clickable target that opens the Diff view (see
/// `openDiff`/`DiffView` below) - `onSelect` fires with the file's (post-rename) path. Commit
/// rows in Branch/History stay plain `<li>`s with no click handler: `lane.diff`'s patch is
/// working-tree-only (see the comment on `openDiff`), so there is no backing data for a
/// per-commit diff yet, and a hover/click affordance here would promise something that isn't
/// there.
function StatFileRowView(props: { file: StatFileRow; onSelect: (path: string) => void }) {
  const parts = () => splitPath(props.file.path);
  return (
    <li>
      <button
        type="button"
        class="focus-ring flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1 text-left hover:bg-raised/60"
        onClick={() => props.onSelect(props.file.path)}
      >
        <span class="min-w-0 flex-1 truncate font-mono text-xs">
          <span class="text-muted/70">{parts().dir}</span>
          <span class="text-foreground">{parts().base}</span>
          <Show when={props.file.renamedFrom} keyed>
            {(from) => <span class="text-muted/50"> ← {from}</span>}
          </Show>
        </span>
        <Show
          when={!props.file.binary}
          fallback={<span class="shrink-0 text-[10px] text-muted/60">binary</span>}
        >
          <span class="shrink-0 text-[10px] tabular-nums">
            <Show when={props.file.adds > 0}>
              <span class="text-signal">+{props.file.adds}</span>
            </Show>
            <Show when={props.file.adds > 0 && props.file.dels > 0}> </Show>
            <Show when={props.file.dels > 0}>
              <span class="text-fault">-{props.file.dels}</span>
            </Show>
          </span>
        </Show>
      </button>
    </li>
  );
}

/// A muted "not tracked" marker for the untracked-files summary row — see the comment on the
/// "Untracked" group in the Working tree section for why this is a count, not a file list.
function UntrackedGlyph() {
  return (
    <span
      class="flex size-4 shrink-0 items-center justify-center rounded bg-muted/10 font-mono text-[9px] font-semibold text-muted/70"
      aria-hidden="true"
    >
      U
    </span>
  );
}

function GroupCountBadge(props: { count: number; tone: "attention" | "muted" }) {
  const dot = () => (props.tone === "attention" ? "bg-attention" : "bg-muted/50");
  const text = () => (props.tone === "attention" ? "text-attention" : "text-muted");
  return (
    <span class={`inline-flex items-center gap-0.5 text-[10px] font-semibold leading-none ${text()}`}>
      <span class={`size-1.5 rounded-full ${dot()}`} />
      <span>{props.count}</span>
    </span>
  );
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
  // Hoisted above the branchData Show block (rather than declared inside it) so it survives the
  // panel's 1.2s poll-driven refetches instead of collapsing back open every time git state
  // changes; session-local only, per C3 — no localStorage persistence needed here.
  const [changesExpanded, setChangesExpanded] = createSignal(true);

  // C4: the Diff view replaces the Branch/Working tree/History sections while open (see the
  // `Show when={diffOpen()}` split below) rather than expanding inline - a lane-wide patch can
  // run to many files and hunks, and a second nested scroll region under History would fight the
  // panel's single outer scrollbar. `diffOpen` gates whether `load()` below asks for
  // `include_patch` at all, so idle polls (the common case) never pay for fetching patch text
  // nobody's looking at.
  const [diffOpen, setDiffOpen] = createSignal(false);
  const [diffFocusPath, setDiffFocusPath] = createSignal<string | null>(null);

  let epoch = 0;

  async function load(laneId: number) {
    const mine = ++epoch;
    setLoading(true);
    setError(null);
    try {
      const [diff, commits] = await Promise.all([
        daemonCall("lane.diff", { lane_id: laneId, include_patch: diffOpen() }),
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

  // Working-tree file rows are the only click target that opens the Diff view (see the comment
  // on `StatFileRowView`). `lane.diff`'s `patch` is lane-wide, not per-file, so opening it for
  // any one file fetches (or reuses an already-fetched) whole-lane patch and just tells DiffView
  // which file's card to land on/expand.
  function openDiff(path: string) {
    setDiffFocusPath(path);
    const wasOpen = diffOpen();
    setDiffOpen(true);
    // Reuse this refresh cycle's patch if the view was already open (e.g. a second file click) -
    // only kick off a fresh fetch the moment it's needed, not once per click.
    if (!wasOpen) {
      const l = lane();
      if (l) void load(l.id);
    }
  }

  function closeDiff() {
    setDiffOpen(false);
    setDiffFocusPath(null);
  }

  // `patch` is `undefined` both before the first patch fetch and after an error; an empty string
  // (a lane with no uncommitted changes) is a legitimate loaded-but-empty state, not "not yet
  // loaded" - so this checks presence, not truthiness, to tell the two apart.
  const patchLoaded = createMemo(() => branchData()?.patch !== undefined);

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

  // A patch belongs to one lane; switching lanes (not just a same-lane poll refresh) closes an
  // open Diff view rather than showing a stale or mismatched patch. Tracked by id, not object
  // identity, since the fleet store's poll produces a fresh Lane object every cycle even when
  // the selection hasn't moved.
  let lastLaneId: number | null = null;
  createEffect(() => {
    const id = lane()?.id ?? null;
    if (id === lastLaneId) return;
    lastLaneId = id;
    setDiffOpen(false);
    setDiffFocusPath(null);
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
        <Show when={diffOpen()}>
          <div class="min-h-0 flex-1">
            <Show
              when={patchLoaded()}
              fallback={
                loading() ? (
                  <div class="p-3">
                    <RowSkeleton rows={5} />
                  </div>
                ) : (
                  <p class="p-3 text-xs text-muted">Diff not loaded yet.</p>
                )
              }
            >
              <DiffView
                patch={branchData()?.patch ?? ""}
                truncated={branchData()?.patch_truncated ?? false}
                focusPath={diffFocusPath() ?? undefined}
                onClose={closeDiff}
              />
            </Show>
          </div>
        </Show>

        <Show when={!diffOpen()}>
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
              <p class="section-label mb-2">Working tree</p>
              <Show
                when={branchData()}
                keyed
                fallback={loading() ? <RowSkeleton rows={2} /> : <p class="text-xs text-muted">Git status unavailable.</p>}
              >
                {(diff) => {
                  // Counts here are sourced from `lane().state.dirty` (gix's live status walk,
                  // reader.rs `dirty_state`) rather than `diff.untracked`/parsed file rows, so this
                  // section always agrees with the header's dirty badge — same lane, same field.
                  //
                  // Grouping: `uncommitted_stat` is `git diff HEAD --stat`, which diffs the worktree
                  // straight against HEAD and so already mixes staged and unstaged hunks into one
                  // per-file line (see LaneDiff.uncommitted_stat in diff.rs) — there's no way to tell,
                  // from that text, which lines are staged. So file rows fall under a single honest
                  // "Changes" group, with the staged/unstaged split shown only as the header's
                  // aggregate counts, not as a per-file split the data can't support.
                  //
                  // "Untracked" is a count-only row, not a file list, for the same reason: neither
                  // `lane.diff` (LaneDiff.untracked, diff.rs) nor the live dirty walk exposes
                  // untracked *filenames* — both only ever return a usize. There is currently no RPC
                  // that lists them by name, so an honest UI shows the count and nothing invented.
                  const dirty = () => lane()?.state.dirty ?? { staged: 0, unstaged: 0, untracked: 0 };
                  const changesCount = () => dirty().staged + dirty().unstaged;
                  const files = () => parseStatFiles(diff.uncommitted_stat);
                  return (
                    <Show
                      when={changesCount() > 0 || dirty().untracked > 0}
                      fallback={<p class="text-xs text-muted">Working tree clean</p>}
                    >
                      <div class="space-y-3">
                        <Show when={changesCount() > 0}>
                          <div>
                            <button
                              type="button"
                              class="focus-ring flex w-full items-center gap-1.5 rounded px-1 py-0.5 text-left hover:bg-raised/40"
                              onClick={() => setChangesExpanded((v) => !v)}
                              aria-expanded={changesExpanded()}
                            >
                              <Show when={changesExpanded()} fallback={<IconChevronRight size={10} class="shrink-0 text-muted/50" />}>
                                <IconChevronDown size={10} class="shrink-0 text-muted/50" />
                              </Show>
                              <span class="section-label">Changes</span>
                              <GroupCountBadge count={changesCount()} tone="attention" />
                              <span class="text-[10px] text-muted/70">
                                {dirty().staged} staged · {dirty().unstaged} unstaged
                              </span>
                            </button>
                            <Show when={changesExpanded()}>
                              <Show
                                when={files().length > 0}
                                fallback={<p class="px-1 py-1 text-xs text-muted">No per-file details available.</p>}
                              >
                                <ul class="space-y-0.5">
                                  <For each={files()}>
                                    {(f) => <StatFileRowView file={f} onSelect={openDiff} />}
                                  </For>
                                </ul>
                              </Show>
                            </Show>
                          </div>
                        </Show>

                        <Show when={dirty().untracked > 0}>
                          <div>
                            <div class="flex items-center gap-1.5 px-1 py-0.5">
                              <span class="section-label">Untracked</span>
                              <GroupCountBadge count={dirty().untracked} tone="muted" />
                            </div>
                            <div class="flex items-center gap-1.5 rounded-lg px-1.5 py-1">
                              <UntrackedGlyph />
                              <span class="text-xs text-muted">
                                {dirty().untracked} file{dirty().untracked === 1 ? "" : "s"} not tracked by git
                              </span>
                            </div>
                          </div>
                        </Show>
                      </div>
                    </Show>
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
      </Show>
    </div>
  );
}
