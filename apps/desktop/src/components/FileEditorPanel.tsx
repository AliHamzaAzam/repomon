import {
  For,
  Match,
  Show,
  Switch,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";

import type { FileEntry } from "../bindings";
import CodeEditor from "./CodeEditor";
import ConfirmDialog from "./ConfirmDialog";
import { translateError, type TranslatedError } from "../ipc/errors";
import { DaemonRpcError, daemonCall, subscribeDaemon } from "../ipc/rpc";
import type { FleetStore } from "../stores/fleet";
import { IconChevronDown, IconChevronRight, IconClose, IconRefresh } from "./icons";

/// D4: a lane-scoped file tree + multi-tab editor for the right rail, built entirely on the D1/D2
/// worktree file RPCs and the D3 `CodeEditor` wrapper. Layout choice (see the panel's top-level
/// return below): the tree collapses to a one-line breadcrumb the moment a file is open, rather
/// than staying pinned open beside the editor - this rail tops out at 40rem (`RIGHT_PANEL_MAX_WIDTH_PX`
/// in RightPanelHost), and a persistent split at that width leaves either half too cramped to be
/// useful. The brief this panel serves is "quick edit next to the agent terminal" (glance at a
/// file, nudge a value, get back to watching the agent), not a standalone IDE, so the tree reads
/// as a picker you summon rather than a sidebar you maintain.

interface FileEditorPanelProps {
  /// Mirrors GitExplorerPanel's `fleet` prop: the panel only reads `selectedLane()` off the
  /// store, but that memo lives there, not as a standalone prop.
  fleet?: FleetStore;
}

/// One open tab's state. `content`/`savedContent` diverging is this panel's only definition of
/// "dirty" - no separate boolean to keep in sync.
interface OpenFile {
  path: string;
  content: string;
  savedContent: string;
  /// `null` only while the initial `file.read` is still in flight (see `loading`) - every other
  /// state (including the deleted-on-disk conflict) leaves the last-known real value in place so
  /// "Keep mine" always has something to compare against.
  mtimeMs: number | null;
  loading: boolean;
  loadError: TranslatedError | null;
  saving: boolean;
  saveError: TranslatedError | null;
  conflict: FileConflict | null;
}

/// `deleted` mirrors `file.write`'s `actual_mtime_ms === null` - the one signal the daemon gives
/// for "the file is gone", surfaced whether this conflict was discovered by an explicit save
/// hitting -32011 or by `syncExternalChange` failing to re-read the file after an
/// `event.file.changed` / tab-focus check.
interface FileConflict {
  deleted: boolean;
  actualMtimeMs: number | null;
}

type DirCacheEntry =
  | { status: "loading" }
  | { status: "loaded"; entries: FileEntry[]; truncated: boolean }
  | { status: "error"; error: TranslatedError };

/// Splits a path into its muted directory prefix (trailing slash kept) and emphasized basename -
/// same convention as GitExplorerPanel's and DiffView's own local copies; kept here too so this
/// file stays a self-contained, independently testable unit.
function splitPath(path: string): { dir: string; base: string } {
  const idx = path.lastIndexOf("/");
  return idx === -1 ? { dir: "", base: path } : { dir: path.slice(0, idx + 1), base: path.slice(idx + 1) };
}

function basename(path: string): string {
  return splitPath(path).base;
}

function TreeSkeleton(props: { depth: number; rows?: number }) {
  return (
    <div class="animate-pulse space-y-1 py-0.5" style={{ "padding-left": `${props.depth * 12 + 8}px` }}>
      <For each={Array.from({ length: props.rows ?? 3 })}>
        {() => <div class="h-3.5 w-28 rounded bg-line/30" />}
      </For>
    </div>
  );
}

function TreeEntryRow(props: {
  entry: FileEntry;
  depth: number;
  isExpanded: boolean;
  isActive: boolean;
  onToggleDir: () => void;
  onOpenFile: () => void;
}) {
  return (
    <button
      type="button"
      class={`focus-ring flex w-full items-center gap-1 rounded px-1.5 py-0.5 text-left hover:bg-raised/50 ${
        props.isActive ? "bg-raised" : ""
      } ${props.entry.ignored ? "opacity-50" : ""}`}
      style={{ "padding-left": `${props.depth * 12 + 4}px` }}
      onClick={props.entry.is_dir ? props.onToggleDir : props.onOpenFile}
      aria-expanded={props.entry.is_dir ? props.isExpanded : undefined}
    >
      <Show when={props.entry.is_dir} fallback={<span class="size-2.5 shrink-0" aria-hidden="true" />}>
        <Show when={props.isExpanded} fallback={<IconChevronRight size={10} class="shrink-0 text-muted/50" />}>
          <IconChevronDown size={10} class="shrink-0 text-muted/50" />
        </Show>
      </Show>
      <span
        class={`min-w-0 flex-1 truncate font-mono text-[11px] ${
          props.entry.is_dir ? "font-medium text-foreground" : "text-foreground/90"
        }`}
      >
        {props.entry.name}
      </span>
    </button>
  );
}

/// One directory level, recursing into itself for any expanded child directory. Lazy: a level's
/// `file.list` call only happens the first time it's expanded (root is the exception - see the
/// panel's lane-switch effect, which loads it eagerly the same way GitExplorerPanel eagerly loads
/// its own data on mount).
function TreeLevel(props: {
  dirPath: string;
  depth: number;
  dirCache: () => Map<string, DirCacheEntry>;
  expanded: () => Set<string>;
  activePath: () => string | null;
  onToggleDir: (path: string) => void;
  onOpenFile: (path: string) => void;
}) {
  const state = createMemo(() => props.dirCache().get(props.dirPath));

  return (
    <Show when={state()} fallback={<TreeSkeleton depth={props.depth} />}>
      {(s) => (
        <Switch>
          <Match when={s().status === "loading"}>
            <TreeSkeleton depth={props.depth} />
          </Match>
          <Match when={s().status === "error"}>
            <p class="px-1.5 py-1 text-[10px] text-fault" style={{ "padding-left": `${props.depth * 12 + 8}px` }}>
              {(s() as { status: "error"; error: TranslatedError }).error.friendly}
            </p>
          </Match>
          <Match when={s().status === "loaded"}>
            {(() => {
              const loaded = s() as { status: "loaded"; entries: FileEntry[]; truncated: boolean };
              return (
                <>
                  <Show when={loaded.entries.length === 0}>
                    <p class="px-1.5 py-1 text-[10px] text-muted/60" style={{ "padding-left": `${props.depth * 12 + 8}px` }}>
                      Empty folder
                    </p>
                  </Show>
                  <For each={loaded.entries}>
                    {(entry) => (
                      <>
                        <TreeEntryRow
                          entry={entry}
                          depth={props.depth}
                          isExpanded={props.expanded().has(entry.path)}
                          isActive={props.activePath() === entry.path}
                          onToggleDir={() => props.onToggleDir(entry.path)}
                          onOpenFile={() => props.onOpenFile(entry.path)}
                        />
                        <Show when={entry.is_dir && props.expanded().has(entry.path)}>
                          <TreeLevel
                            dirPath={entry.path}
                            depth={props.depth + 1}
                            dirCache={props.dirCache}
                            expanded={props.expanded}
                            activePath={props.activePath}
                            onToggleDir={props.onToggleDir}
                            onOpenFile={props.onOpenFile}
                          />
                        </Show>
                      </>
                    )}
                  </For>
                  <Show when={loaded.truncated}>
                    <p class="px-1.5 py-1 text-[10px] text-muted/70" style={{ "padding-left": `${props.depth * 12 + 8}px` }}>
                      Showing a partial listing - this folder has more entries than fit.
                    </p>
                  </Show>
                </>
              );
            })()}
          </Match>
        </Switch>
      )}
    </Show>
  );
}

function ConflictBanner(props: {
  conflict: FileConflict;
  onReload: () => void;
  onKeepMine: () => void;
  onSaveAsNew: () => void;
  onCloseDeleted: () => void;
}) {
  return (
    <div
      role="alert"
      class="flex shrink-0 items-center justify-between gap-3 border-b border-attention/40 bg-attention/10 px-3 py-2 text-[11px] text-attention"
    >
      <span class="font-medium">
        {props.conflict.deleted ? "This file was deleted on disk." : "File changed on disk."}
      </span>
      <div class="flex shrink-0 items-center gap-2">
        <Show
          when={!props.conflict.deleted}
          fallback={
            <>
              <button
                type="button"
                class="focus-ring rounded-lg border border-attention/40 bg-surface px-2.5 py-1 font-medium text-foreground hover:bg-attention/15"
                onClick={props.onSaveAsNew}
              >
                Save as new content
              </button>
              <button
                type="button"
                class="focus-ring rounded-lg px-2.5 py-1 font-medium text-muted hover:text-foreground"
                onClick={props.onCloseDeleted}
              >
                Close
              </button>
            </>
          }
        >
          <button
            type="button"
            class="focus-ring rounded-lg border border-attention/40 bg-surface px-2.5 py-1 font-medium text-foreground hover:bg-attention/15"
            onClick={props.onReload}
          >
            Reload
          </button>
          <button
            type="button"
            class="focus-ring rounded-lg px-2.5 py-1 font-medium text-muted hover:text-foreground"
            onClick={props.onKeepMine}
          >
            Keep mine
          </button>
        </Show>
      </div>
    </div>
  );
}

export default function FileEditorPanel(props: FileEditorPanelProps) {
  const lane = () => props.fleet?.selectedLane() ?? null;

  const [dirCache, setDirCache] = createSignal<Map<string, DirCacheEntry>>(new Map());
  const [expandedDirs, setExpandedDirs] = createSignal<Set<string>>(new Set());
  const [treeExpanded, setTreeExpanded] = createSignal(true);
  const [openFiles, setOpenFiles] = createSignal<OpenFile[]>([]);
  const [activePath, setActivePath] = createSignal<string | null>(null);
  const [pendingLaneSwitch, setPendingLaneSwitch] = createSignal<
    { fromLaneId: number; toLaneId: number; dirtyPaths: string[] } | null
  >(null);
  const [closeConfirmPath, setCloseConfirmPath] = createSignal<string | null>(null);

  // Which lane's worktree `dirCache`/`openFiles` currently belong to - not a signal, since it's
  // only ever read/written from the lane-switch effect and the async helpers it kicks off, never
  // rendered directly.
  let currentLaneId: number | null = null;
  // Set just before re-dispatching `setSelectedLaneId` from the "discard and switch" confirm, so
  // the lane-switch effect's dirty check is skipped exactly once instead of re-prompting forever.
  let forceNextSwitch = false;

  const effectiveTreeExpanded = createMemo(() => treeExpanded() || openFiles().length === 0);
  const activeFile = createMemo(() => {
    const p = activePath();
    return p ? openFiles().find((f) => f.path === p) ?? null : null;
  });

  function findOpenFile(path: string): OpenFile | undefined {
    return openFiles().find((f) => f.path === path);
  }

  function updateOpenFile(path: string, updater: (file: OpenFile) => OpenFile) {
    setOpenFiles((files) => files.map((f) => (f.path === path ? updater(f) : f)));
  }

  async function loadDir(laneId: number, dirPath: string) {
    setDirCache((cache) => {
      const next = new Map(cache);
      next.set(dirPath, { status: "loading" });
      return next;
    });
    try {
      const result = await daemonCall("file.list", { lane_id: laneId, path: dirPath });
      if (currentLaneId !== laneId) return; // lane switched away while this was in flight
      setDirCache((cache) => {
        const next = new Map(cache);
        next.set(dirPath, { status: "loaded", entries: result.entries, truncated: result.truncated });
        return next;
      });
    } catch (cause) {
      if (currentLaneId !== laneId) return;
      setDirCache((cache) => {
        const next = new Map(cache);
        next.set(dirPath, { status: "error", error: translateError(cause) });
        return next;
      });
    }
  }

  function toggleDir(path: string) {
    const laneId = lane()?.id;
    if (laneId == null) return;
    setExpandedDirs((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
        if (!dirCache().has(path)) void loadDir(laneId, path);
      }
      return next;
    });
  }

  function refreshTree() {
    const laneId = lane()?.id;
    if (laneId == null) return;
    void loadDir(laneId, "");
    for (const path of expandedDirs()) void loadDir(laneId, path);
  }

  function activateTab(path: string) {
    setActivePath(path);
    setTreeExpanded(false);
    void syncExternalChange(path);
  }

  async function openFile(path: string) {
    const existing = findOpenFile(path);
    if (existing) {
      activateTab(path);
      return;
    }
    const laneId = lane()?.id;
    if (laneId == null) return;
    setTreeExpanded(false);
    const placeholder: OpenFile = {
      path,
      content: "",
      savedContent: "",
      mtimeMs: null,
      loading: true,
      loadError: null,
      saving: false,
      saveError: null,
      conflict: null,
    };
    setOpenFiles((files) => [...files, placeholder]);
    setActivePath(path);
    try {
      const result = await daemonCall("file.read", { lane_id: laneId, path });
      updateOpenFile(path, (f) => ({
        ...f,
        content: result.content,
        savedContent: result.content,
        mtimeMs: result.mtime_ms,
        loading: false,
      }));
    } catch (cause) {
      updateOpenFile(path, (f) => ({ ...f, loading: false, loadError: translateError(cause) }));
    }
  }

  function closeFile(path: string) {
    const current = openFiles();
    const idx = current.findIndex((f) => f.path === path);
    const next = current.filter((f) => f.path !== path);
    setOpenFiles(next);
    if (activePath() === path) {
      const neighbor = next[idx] ?? next[idx - 1] ?? null;
      setActivePath(neighbor ? neighbor.path : null);
    }
    setCloseConfirmPath(null);
  }

  function requestCloseFile(path: string) {
    const f = findOpenFile(path);
    if (f && f.content !== f.savedContent) {
      setCloseConfirmPath(path);
    } else {
      closeFile(path);
    }
  }

  async function saveFile(path: string) {
    const laneId = lane()?.id;
    const file = findOpenFile(path);
    if (!file || laneId == null) return;
    if (file.content === file.savedContent && !file.conflict) return;
    updateOpenFile(path, (f) => ({ ...f, saving: true, saveError: null }));
    try {
      const result = await daemonCall("file.write", {
        lane_id: laneId,
        path,
        content: file.content,
        expected_mtime_ms: file.mtimeMs ?? undefined,
      });
      updateOpenFile(path, (f) => ({
        ...f,
        savedContent: f.content,
        mtimeMs: result.mtime_ms,
        saving: false,
        saveError: null,
        conflict: null,
      }));
    } catch (cause) {
      if (cause instanceof DaemonRpcError && cause.code === -32011) {
        const data = cause.data as { expected_mtime_ms?: number; actual_mtime_ms?: number | null } | null;
        updateOpenFile(path, (f) => ({
          ...f,
          saving: false,
          conflict: { deleted: data?.actual_mtime_ms == null, actualMtimeMs: data?.actual_mtime_ms ?? null },
        }));
      } else {
        updateOpenFile(path, (f) => ({ ...f, saving: false, saveError: translateError(cause) }));
      }
    }
  }

  async function reloadFile(path: string) {
    const laneId = lane()?.id;
    if (laneId == null || !findOpenFile(path)) return;
    try {
      const result = await daemonCall("file.read", { lane_id: laneId, path });
      updateOpenFile(path, (f) => ({
        ...f,
        content: result.content,
        savedContent: result.content,
        mtimeMs: result.mtime_ms,
        conflict: null,
        loadError: null,
      }));
    } catch (cause) {
      updateOpenFile(path, (f) => ({ ...f, loadError: translateError(cause) }));
    }
  }

  /// "Keep mine" per the D4 brief: don't touch the buffer (it stays dirty, still holding the
  /// user's edits) - just re-read the file's current mtime so the *next* explicit save's
  /// `expected_mtime_ms` matches what's actually on disk and succeeds deliberately, instead of
  /// bouncing off -32011 a second time.
  async function keepMine(path: string) {
    const laneId = lane()?.id;
    if (laneId == null || !findOpenFile(path)) return;
    try {
      const result = await daemonCall("file.read", { lane_id: laneId, path });
      updateOpenFile(path, (f) => ({ ...f, mtimeMs: result.mtime_ms, conflict: null }));
    } catch {
      updateOpenFile(path, (f) => ({ ...f, conflict: { deleted: true, actualMtimeMs: null } }));
    }
  }

  /// The deleted-on-disk conflict's affirmative action: write the buffer back out with no
  /// `expected_mtime_ms` at all, i.e. plain create - there is nothing on disk to conflict with.
  async function saveAsNew(path: string) {
    const laneId = lane()?.id;
    const file = findOpenFile(path);
    if (!file || laneId == null) return;
    updateOpenFile(path, (f) => ({ ...f, saving: true, saveError: null }));
    try {
      const result = await daemonCall("file.write", { lane_id: laneId, path, content: file.content });
      updateOpenFile(path, (f) => ({
        ...f,
        savedContent: f.content,
        mtimeMs: result.mtime_ms,
        saving: false,
        conflict: null,
      }));
    } catch (cause) {
      updateOpenFile(path, (f) => ({ ...f, saving: false, saveError: translateError(cause) }));
    }
  }

  /// Shared by the `event.file.changed` subscription and the tab-focus recheck: re-reads `path`
  /// and reconciles it against the open tab's last-known state. A clean buffer adopts the new
  /// content silently; a dirty one gets the non-blocking conflict banner instead, per the D4
  /// brief - the buffer itself is never touched by anything other than the user or an explicit
  /// Reload, so an in-progress edit can never be clobbered out from under the typist.
  async function syncExternalChange(path: string) {
    const laneId = lane()?.id;
    const file = findOpenFile(path);
    if (!file || laneId == null) return;
    try {
      const result = await daemonCall("file.read", { lane_id: laneId, path });
      if (result.mtime_ms === file.mtimeMs) return; // nothing actually changed
      const isDirty = file.content !== file.savedContent;
      if (isDirty) {
        updateOpenFile(path, (f) => ({ ...f, conflict: { deleted: false, actualMtimeMs: result.mtime_ms } }));
      } else {
        updateOpenFile(path, (f) => ({
          ...f,
          content: result.content,
          savedContent: result.content,
          mtimeMs: result.mtime_ms,
          conflict: null,
          loadError: null,
        }));
      }
    } catch {
      // Most likely deleted out from under us (could also be a read that now fails for some other
      // reason, e.g. grew past the size cap) - either way there is nothing left to silently
      // reconcile, so surface the same deleted-conflict banner `file.write`'s -32011 path uses.
      updateOpenFile(path, (f) => ({ ...f, conflict: { deleted: true, actualMtimeMs: null } }));
    }
  }

  function performLaneSwitch(newLaneId: number | null, rootPath: string) {
    void rootPath; // not needed directly - file.list paths are lane-relative, not host-absolute
    currentLaneId = newLaneId;
    setDirCache(new Map<string, DirCacheEntry>());
    setExpandedDirs(new Set<string>());
    setTreeExpanded(true);
    setOpenFiles([]);
    setActivePath(null);
    setPendingLaneSwitch(null);
    if (newLaneId !== null) void loadDir(newLaneId, "");
  }

  // Resets the tree and open tabs whenever the selected lane changes. A lane with dirty open
  // files first reverts the selection (via `fleet.setSelectedLaneId`) and asks for confirmation
  // instead - see the `pendingLaneSwitch` ConfirmDialog below, whose "Discard and switch" sets
  // `forceNextSwitch` and re-dispatches the same selection to let this effect through the second
  // time. A lane with no dirty files (including the very first mount, when `openFiles` is
  // necessarily empty) switches immediately, same as GitExplorerPanel's own lane-change effect.
  createEffect(() => {
    const l = lane();
    const newLaneId = l?.id ?? null;
    if (newLaneId === currentLaneId) return;
    const dirty = openFiles().filter((f) => f.content !== f.savedContent);
    if (dirty.length > 0 && currentLaneId !== null && !forceNextSwitch) {
      setPendingLaneSwitch({ fromLaneId: currentLaneId, toLaneId: newLaneId ?? -1, dirtyPaths: dirty.map((f) => f.path) });
      props.fleet?.setSelectedLaneId(currentLaneId);
      return;
    }
    forceNextSwitch = false;
    performLaneSwitch(newLaneId, l?.worktree.path ?? "");
  });

  onMount(() => {
    let active = true;
    let stop: (() => void) | undefined;
    void subscribeDaemon((event) => {
      if (!active || event.method !== "event.file.changed") return;
      const params = event.params as { lane_id?: number; path?: string } | null;
      const laneId = lane()?.id;
      if (laneId == null || !params || params.lane_id !== laneId || typeof params.path !== "string") return;
      if (!findOpenFile(params.path)) return;
      void syncExternalChange(params.path);
    })
      .then((unsub) => {
        if (active) stop = unsub;
        else unsub();
      })
      .catch(() => undefined);
    onCleanup(() => {
      active = false;
      stop?.();
    });
  });

  return (
    <div class="flex h-full flex-col bg-surface">
      <div class="flex h-10 shrink-0 items-center justify-between border-b border-line bg-surface/95 px-3.5">
        <div class="flex min-w-0 items-center gap-2">
          <span class="text-xs font-semibold text-foreground">Editor</span>
          <Show when={lane()} keyed>
            {(l) => (
              <>
                <span class="h-3 w-px shrink-0 bg-line/60" aria-hidden="true" />
                <span class="min-w-0 truncate font-mono text-[11px] text-muted">{l.worktree.name}</span>
              </>
            )}
          </Show>
        </div>
        <button
          type="button"
          class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
          onClick={refreshTree}
          disabled={!lane()}
          title="Refresh file tree"
          aria-label="Refresh file tree"
        >
          <IconRefresh size={12} />
        </button>
      </div>

      <Show
        when={lane()}
        fallback={
          <div class="flex flex-1 items-center justify-center p-4">
            <div class="max-w-[220px] space-y-2 rounded-xl border border-line bg-surface/40 p-3.5 text-center">
              <p class="text-xs font-medium text-foreground">No lane selected</p>
              <p class="text-xs text-muted">Select a lane in the fleet to browse and edit its files.</p>
            </div>
          </div>
        }
      >
        <div class="flex min-h-0 flex-1 flex-col">
          <Show when={openFiles().length > 0}>
            <button
              type="button"
              class="focus-ring flex h-7 w-full shrink-0 items-center gap-1.5 border-b border-line px-3 text-left hover:bg-raised/40"
              onClick={() => setTreeExpanded((v) => !v)}
              aria-expanded={effectiveTreeExpanded()}
              aria-label="Toggle file tree"
            >
              <Show when={effectiveTreeExpanded()} fallback={<IconChevronRight size={9} class="shrink-0 text-muted/50" />}>
                <IconChevronDown size={9} class="shrink-0 text-muted/50" />
              </Show>
              <span class="section-label shrink-0">Files</span>
              <Show when={activeFile()} keyed>
                {(f) => {
                  const parts = () => splitPath(f.path);
                  return (
                    <span class="min-w-0 flex-1 truncate font-mono text-[10px]">
                      <span class="text-muted/60">{parts().dir}</span>
                      <span class="text-muted">{parts().base}</span>
                    </span>
                  );
                }}
              </Show>
            </button>
          </Show>

          <Show when={effectiveTreeExpanded()}>
            <div
              class={
                openFiles().length > 0
                  ? "max-h-56 shrink-0 overflow-y-auto border-b border-line p-1.5"
                  : "min-h-0 flex-1 overflow-y-auto p-1.5"
              }
            >
              <TreeLevel
                dirPath=""
                depth={0}
                dirCache={dirCache}
                expanded={expandedDirs}
                activePath={activePath}
                onToggleDir={toggleDir}
                onOpenFile={openFile}
              />
            </div>
          </Show>

          <Show when={openFiles().length > 0}>
            <div class="flex h-8 shrink-0 items-center border-b border-line bg-surface/95">
              {/* min-w-0 + overflow-x-auto + no-scrollbar/scroll-smooth: same horizontal-overflow
                  pattern as TerminalWorkspace's tab strip, so a lot of open files scrolls (each
                  tab keeps shrink-0) instead of wrapping or squeezing tabs unreadably thin. */}
              <div class="flex min-w-0 flex-1 items-center overflow-x-auto no-scrollbar scroll-smooth">
                <For each={openFiles()}>
                  {(file) => {
                    const isActive = () => activePath() === file.path;
                    const dirty = () => file.content !== file.savedContent;
                    return (
                      <div
                        class={`group flex h-8 shrink-0 items-center border-r border-line ${
                          isActive() ? "bg-raised" : "hover:bg-raised/40"
                        }`}
                        onMouseDown={(event) => {
                          if (event.button === 1) {
                            event.preventDefault();
                            requestCloseFile(file.path);
                          }
                        }}
                      >
                        <button
                          type="button"
                          class={`focus-ring flex items-center gap-1.5 px-2 py-1 text-xs ${
                            isActive() ? "font-medium text-foreground" : "text-muted hover:text-foreground"
                          }`}
                          onClick={() => activateTab(file.path)}
                          title={file.path}
                        >
                          <span class="max-w-[8rem] truncate font-mono">{basename(file.path)}</span>
                          <Show when={dirty()}>
                            <span class="size-1.5 shrink-0 rounded-full bg-attention" title="Unsaved changes" />
                          </Show>
                        </button>
                        <button
                          type="button"
                          class="focus-ring mr-1 flex size-4 shrink-0 items-center justify-center rounded text-muted/60 opacity-0 hover:bg-line/60 hover:text-foreground group-hover:opacity-100"
                          onClick={() => requestCloseFile(file.path)}
                          aria-label={`Close ${basename(file.path)}`}
                        >
                          <IconClose size={9} />
                        </button>
                      </div>
                    );
                  }}
                </For>
              </div>
              <button
                type="button"
                class="focus-ring flex h-8 shrink-0 items-center gap-1 border-l border-line px-2.5 text-[11px] font-medium text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={!activeFile() || activeFile()!.content === activeFile()!.savedContent || activeFile()!.saving}
                onClick={() => {
                  const p = activePath();
                  if (p) void saveFile(p);
                }}
              >
                {activeFile()?.saving ? "Saving…" : "Save"}
              </button>
            </div>

            <div class="relative flex min-h-0 flex-1 flex-col">
              <Show when={activeFile()?.conflict} keyed>
                {(conflict) => (
                  <ConflictBanner
                    conflict={conflict}
                    onReload={() => void reloadFile(activePath()!)}
                    onKeepMine={() => void keepMine(activePath()!)}
                    onSaveAsNew={() => void saveAsNew(activePath()!)}
                    onCloseDeleted={() => closeFile(activePath()!)}
                  />
                )}
              </Show>
              <Show when={activeFile()?.saveError} keyed>
                {(err) => (
                  <div role="alert" class="shrink-0 border-b border-fault/30 bg-fault/10 px-3 py-1.5 text-[11px] text-fault">
                    Couldn't save: {err.friendly}
                  </div>
                )}
              </Show>
              <Show when={activeFile()} keyed>
                {(file) => (
                  <Show
                    when={!file.loading}
                    fallback={
                      <div class="flex flex-1 items-center justify-center">
                        <p class="text-xs text-muted">Loading file…</p>
                      </div>
                    }
                  >
                    <Show
                      when={!file.loadError}
                      fallback={
                        <div class="flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center">
                          <p class="text-xs font-medium text-foreground">Can't open this file</p>
                          <p class="max-w-[220px] text-xs text-muted">{file.loadError?.friendly}</p>
                          <button
                            type="button"
                            class="focus-ring rounded-lg border border-line px-2.5 py-1 text-xs font-medium text-foreground hover:bg-raised"
                            onClick={() => requestCloseFile(file.path)}
                          >
                            Close tab
                          </button>
                        </div>
                      }
                    >
                      <CodeEditor
                        value={file.content}
                        path={file.path}
                        onChange={(content) => updateOpenFile(file.path, (f) => ({ ...f, content }))}
                        onSave={() => void saveFile(file.path)}
                        class="min-h-0 flex-1"
                      />
                    </Show>
                  </Show>
                )}
              </Show>
            </div>
          </Show>
        </div>
      </Show>

      <Show when={pendingLaneSwitch()} keyed>
        {(pending) => (
          <ConfirmDialog
            options={{
              title: "Unsaved changes",
              message: `You have unsaved changes in ${pending.dirtyPaths.length} file${
                pending.dirtyPaths.length === 1 ? "" : "s"
              } (${pending.dirtyPaths.join(", ")}). Switching lanes will discard them.`,
              confirmLabel: "Discard and switch",
              danger: true,
              onConfirm: () => {
                forceNextSwitch = true;
                props.fleet?.setSelectedLaneId(pending.toLaneId === -1 ? null : pending.toLaneId);
              },
            }}
            onClose={() => setPendingLaneSwitch(null)}
          />
        )}
      </Show>

      <Show when={closeConfirmPath()} keyed>
        {(path) => (
          <ConfirmDialog
            options={{
              title: "Unsaved changes",
              message: `Close ${path} without saving your changes?`,
              confirmLabel: "Discard and close",
              danger: true,
              onConfirm: () => closeFile(path),
            }}
            onClose={() => setCloseConfirmPath(null)}
          />
        )}
      </Show>
    </div>
  );
}
