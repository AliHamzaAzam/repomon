import { For, Show, createEffect, createMemo, createSignal } from "solid-js";

import type { Commit, CommitShow, Lane } from "../bindings";
import DiffView, { findFileFirstChangedLine, parseDiff } from "./DiffView";
import { translateError, type TranslatedError } from "../ipc/errors";
import { daemonCall, type LaneDiff } from "../ipc/rpc";
import type { EditorStore } from "../stores/editor";
import type { FleetStore } from "../stores/fleet";
import type { WorkspaceStore } from "../stores/workspace";
import { formatRelativeTime } from "./relativeTime";
import {
  IconArrowDown,
  IconArrowUp,
  IconChevronDown,
  IconChevronRight,
  IconClose,
  IconFileCode,
  IconGitBranch,
  IconGitCommit,
  IconRefresh,
} from "./icons";

const HISTORY_LIMIT = 30;

export interface ParsedCommit {
  oid: string;
  summary: string;
}

/// Parses newest-first oid/summary lines from the daemon’s raw commit log.
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
  /// Counts stat-bar symbols, which are proportional rather than exact when git scales a large
  /// diff.
  adds: number;
  dels: number;
  binary: boolean;
}

/// Expand plain and brace-form rename paths, leaving non-renames unchanged.
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

/// Parses text and binary git --stat rows, excluding the summary and preserving zero-change
/// renames.
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

/// Open the selected working-tree file inside the lane diff.
function StatFileRowView(props: {
  file: StatFileRow;
  onSelect: (path: string) => void;
  onOpenInEditor?: (path: string) => void;
  onContextMenu?: (e: MouseEvent, path: string) => void;
}) {
  const parts = () => splitPath(props.file.path);
  return (
    <li class="group flex items-center justify-between rounded-lg hover:bg-raised/60">
      <button
        type="button"
        class="focus-ring flex min-w-0 flex-1 items-center gap-1.5 rounded-lg px-1.5 py-1 text-left"
        onClick={() => props.onSelect(props.file.path)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            e.stopPropagation();
            props.onOpenInEditor?.(props.file.path);
          }
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          props.onContextMenu?.(e, props.file.path);
        }}
      >
        <span class="flex min-w-0 flex-1 font-mono text-xs" title={props.file.renamedFrom ? `${props.file.path} (from ${props.file.renamedFrom})` : props.file.path}>
          <span class="min-w-0 truncate text-muted/70">{parts().dir}</span>
          <span class="max-w-full shrink-0 truncate text-foreground">{parts().base}</span>
          <Show when={props.file.renamedFrom} keyed>
            {(from) => <span class="min-w-0 truncate text-muted/50"> ← {from}</span>}
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
      <Show when={props.onOpenInEditor}>
        <button
          type="button"
          class="focus-ring mr-1 hidden size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground group-hover:flex group-focus-within:flex"
          onClick={(e) => {
            e.stopPropagation();
            props.onOpenInEditor!(props.file.path);
          }}
          title="Open in editor"
          aria-label="Open in editor"
        >
          <IconFileCode size={12} />
        </button>
      </Show>
    </li>
  );
}

/// A muted "not tracked" marker for the untracked-files summary row - see the comment on the
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
  /// Supplies the selected lane, with absence rendering the empty state.
  fleet?: FleetStore;
  editor?: EditorStore;
  workspace?: WorkspaceStore;
  onEnsureEditorOpen?: () => void;
}

export default function GitExplorerPanel(props: GitExplorerPanelProps) {
  const lane = () => props.fleet?.selectedLane() ?? null;

  const [branchData, setBranchData] = createSignal<LaneDiff | null>(null);
  const [history, setHistory] = createSignal<Commit[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<TranslatedError | null>(null);
  const [contextMenu, setContextMenu] = createSignal<{ x: number; y: number; path: string } | null>(null);
  // Keep expansion state outside the data block so polling does not reset it.
  const [changesExpanded, setChangesExpanded] = createSignal(true);

  // Fetch full patches only while the diff replaces the overview, avoiding hidden payloads and
  // competing nested scroll areas.
  const [diffOpen, setDiffOpen] = createSignal(false);
  const [diffFocusPath, setDiffFocusPath] = createSignal<string | null>(null);

  function handleOpenInEditor(path: string, line: number = 1) {
    if (props.onEnsureEditorOpen) {
      props.onEnsureEditorOpen();
    } else if (props.workspace && !props.workspace.editorWorkspace()) {
      props.workspace.setEditorWorkspace(true);
    }
    void props.editor?.openAt(path, line, 1);
  }

  async function openFileInEditor(filePath: string, explicitLine?: number) {
    let targetLine = explicitLine;
    if (targetLine === undefined) {
      let patch = branchData()?.patch;
      if (!patch && lane()) {
        try {
          const res = await daemonCall("lane.diff", { lane_id: lane()!.id, include_patch: true });
          patch = res.patch;
          setBranchData((prev) => (prev ? { ...prev, patch: res.patch } : prev));
        } catch {
          // Fallback to line 1 if patch fetch fails
        }
      }
      if (patch) {
        const diffFiles = parseDiff(patch);
        const df = diffFiles.find((f) => f.path === filePath || f.newPath === filePath || f.oldPath === filePath);
        if (df && df.hunks.length > 0) {
          targetLine = findFileFirstChangedLine(df);
        }
      }
    }
    handleOpenInEditor(filePath, targetLine ?? 1);
  }

  // Keep commit detail separate from the working-tree patch because their fetches and result shapes
  // differ.
  const [commitOid, setCommitOid] = createSignal<string | null>(null);
  const [commitDetail, setCommitDetail] = createSignal<CommitShow | null>(null);
  const [commitLoading, setCommitLoading] = createSignal(false);
  const [commitError, setCommitError] = createSignal<TranslatedError | null>(null);
  let commitEpoch = 0;

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

  // Fetch the lane-wide patch once and select the requested file within it.
  function openDiff(path: string) {
    closeCommit(); // mutually exclusive with the commit view (see the field group's comment)
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

  // The daemon resolves abbreviated commit IDs against the lane repository.
  function openCommit(oid: string) {
    closeDiff(); // mutually exclusive with the working-tree Diff view
    const mine = ++commitEpoch;
    setCommitOid(oid);
    setCommitDetail(null);
    setCommitError(null);
    const l = lane();
    if (!l) {
      setCommitLoading(false);
      return;
    }
    setCommitLoading(true);
    daemonCall("commit.show", { lane_id: l.id, oid })
      .then((detail) => {
        if (mine !== commitEpoch) return;
        setCommitDetail(detail);
      })
      .catch((cause) => {
        if (mine !== commitEpoch) return;
        setCommitError(translateError(cause, { binary: "git" }));
      })
      .finally(() => {
        if (mine !== commitEpoch) return;
        setCommitLoading(false);
      });
  }

  function closeCommit() {
    commitEpoch += 1; // invalidate any in-flight commit.show for the commit we're leaving
    setCommitOid(null);
    setCommitDetail(null);
    setCommitError(null);
    setCommitLoading(false);
  }

  // `patch` is `undefined` both before the first patch fetch and after an error; an empty string
  // (a lane with no uncommitted changes) is a legitimate loaded-but-empty state, not "not yet
  // loaded" - so this checks presence, not truthiness, to tell the two apart.
  const patchLoaded = createMemo(() => branchData()?.patch !== undefined);

  // Track the fleet’s git-state signature to share its refresh cadence instead of starting another
  // poller.
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

  // Close lane-owned detail only on an ID change, since polling replaces otherwise identical lane
  // objects.
  let lastLaneId: number | null = null;
  createEffect(() => {
    const id = lane()?.id ?? null;
    if (id === lastLaneId) return;
    lastLaneId = id;
    setDiffOpen(false);
    setDiffFocusPath(null);
    closeCommit();
  });

  function refresh() {
    const l = lane();
    if (l) void load(l.id);
  }

  return (
    <div class="flex h-full flex-col bg-surface">
      <div class="panel-header">
        <div class="panel-header-lead">
          <span class="text-xs font-semibold text-foreground">Git</span>
          <Show when={lane()} keyed>
            {(l) => (
              <>
                <span class="h-3 w-px shrink-0 bg-line/60" aria-hidden="true" />
                <span class="flex min-w-0 items-center gap-1 font-mono text-[11px] text-muted">
                  <IconGitBranch size={10} class="shrink-0 text-muted/60" />
                  <span class="min-w-0 truncate" title={l.worktree.branch ?? "detached"}>{l.worktree.branch ?? "detached"}</span>
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
                onOpenInEditor={(path, line) => void openFileInEditor(path, line)}
              />
            </Show>
          </div>
        </Show>

        <Show when={commitOid()} keyed>
          {(oid) => (
            <div class="min-h-0 flex-1">
              <Show
                when={commitError()}
                keyed
                fallback={
                  <Show
                    when={commitDetail()}
                    keyed
                    fallback={
                      commitLoading() ? (
                        <div class="p-3">
                          <RowSkeleton rows={5} />
                        </div>
                      ) : (
                        <p class="p-3 text-xs text-muted">Commit not loaded yet.</p>
                      )
                    }
                  >
                    {(detail) => (
                      <DiffView
                        patch={detail.patch}
                        truncated={detail.patch_truncated}
                        header={
                          <div class="flex max-h-[40%] shrink-0 flex-col gap-1.5 overflow-y-auto border-b border-line px-3 py-2.5 [overflow-wrap:anywhere]">
                            <div class="flex items-center justify-between gap-2">
                              <span class="flex min-w-0 items-center gap-1.5 font-mono text-[10px] text-muted">
                                <IconGitCommit size={11} class="shrink-0 text-muted/50" />
                                <span class="truncate">{detail.oid.slice(0, 7)}</span>
                              </span>
                              <button
                                type="button"
                                class="focus-ring flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                                onClick={closeCommit}
                                title="Close commit"
                                aria-label="Close commit"
                              >
                                <IconClose size={11} />
                              </button>
                            </div>
                            <p class="text-xs font-medium text-foreground">{detail.summary}</p>
                            <Show when={detail.body.trim()} keyed>
                              {(body) => <p class="whitespace-pre-wrap text-xs text-muted">{body}</p>}
                            </Show>
                            <div class="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-[10px] text-muted/70">
                              <span>{detail.author_name}</span>
                              <span aria-hidden="true">·</span>
                              <span title={new Date(detail.time).toLocaleString()}>
                                {formatRelativeTime(detail.time)} ({new Date(detail.time).toLocaleString()})
                              </span>
                            </div>
                          </div>
                        }
                        onOpenInEditor={(path, line) => void openFileInEditor(path, line)}
                      />
                    )}
                  </Show>
                }
              >
                {(err) => (
                  <div role="alert" class="m-3 flex items-start justify-between gap-3 rounded-xl border border-fault/30 bg-fault/10 p-3 text-xs text-fault">
                    <div class="min-w-0">
                      <p class="font-semibold">Couldn't load commit</p>
                      <p class="mt-0.5 break-words text-fault/80">{err.friendly}</p>
                    </div>
                    <button
                      type="button"
                      class="focus-ring shrink-0 rounded-lg border border-fault/40 bg-surface px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-fault/20"
                      onClick={() => openCommit(oid)}
                    >
                      Retry
                    </button>
                  </div>
                )}
              </Show>
            </div>
          )}
        </Show>

        <Show when={!diffOpen() && !commitOid()}>
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
                              <li>
                                <button
                                  type="button"
                                  class="focus-ring flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1 text-left hover:bg-raised/60"
                                  onClick={() => openCommit(commit.oid)}
                                >
                                  <IconGitCommit size={10} class="shrink-0 text-muted/40" />
                                  <span class="shrink-0 font-mono text-[10px] text-muted">{commit.oid}</span>
                                  <span class="min-w-0 flex-1 truncate text-xs text-foreground">{commit.summary}</span>
                                </button>
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
                  // Use live dirty counts to match the lane badge; stat text combines staged and
                  // unstaged changes and exposes no untracked filenames.
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
                                    {(f) => (
                                      <StatFileRowView
                                        file={f}
                                        onSelect={openDiff}
                                        onOpenInEditor={(path) => void openFileInEditor(path)}
                                        onContextMenu={(e, path) => setContextMenu({ x: e.clientX, y: e.clientY, path })}
                                      />
                                    )}
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
                        <li>
                          <button
                            type="button"
                            class="focus-ring flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1 text-left hover:bg-raised/60"
                            onClick={() => openCommit(commit.oid)}
                          >
                            <IconGitCommit size={10} class="shrink-0 text-muted/40" />
                            <span class="shrink-0 font-mono text-[10px] text-muted/70">{commit.oid.slice(0, 7)}</span>
                            <span class="min-w-0 flex-1 truncate text-xs text-foreground">{commit.summary}</span>
                            <span class="shrink-0 text-[10px] text-muted" title={new Date(commit.time).toLocaleString()}>
                              {formatRelativeTime(commit.time)}
                            </span>
                            <span class="max-w-[5.5rem] shrink-0 truncate text-[10px] text-muted/70">{commit.author_name}</span>
                          </button>
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

      <Show when={contextMenu()} keyed>
        {(menu) => (
          <>
            <div
              class="fixed inset-0 z-40"
              onClick={() => setContextMenu(null)}
              onContextMenu={(e) => {
                e.preventDefault();
                setContextMenu(null);
              }}
            />
            <div
              role="menu"
              class="fixed z-50 min-w-[140px] rounded-lg border border-line bg-surface py-1 text-xs shadow-xl backdrop-blur"
              style={{
                left: `${Math.min(menu.x, window.innerWidth - 150)}px`,
                top: `${Math.min(menu.y, window.innerHeight - 100)}px`,
              }}
            >
              <button
                role="menuitem"
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-foreground/90 hover:bg-raised hover:text-foreground"
                onClick={() => {
                  setContextMenu(null);
                  void openFileInEditor(menu.path);
                }}
              >
                <IconFileCode size={12} class="text-muted" />
                <span>Open in editor</span>
              </button>
              <button
                role="menuitem"
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-foreground/90 hover:bg-raised hover:text-foreground"
                onClick={() => {
                  setContextMenu(null);
                  openDiff(menu.path);
                }}
              >
                <span>View diff</span>
              </button>
            </div>
          </>
        )}
      </Show>
    </div>
  );
}
