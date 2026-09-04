import {
  For,
  Match,
  Show,
  Switch,
  createMemo,
  createSignal,
} from "solid-js";

import type { FileEntry } from "../bindings";
import CodeEditor, { type CodeEditorReplaceRequest } from "./CodeEditor";
import ProjectSearchPanel from "./ProjectSearchPanel";
import ImageViewer from "./ImageViewer";
import BinaryViewer from "./BinaryViewer";
import PdfViewer from "./PdfViewer";
import ConfirmDialog from "./ConfirmDialog";
import type { TranslatedError } from "../ipc/errors";
import type { FleetStore } from "../stores/fleet";
import {
  createEditorStore,
  type DirCacheEntry,
  type EditorStore,
  type FileConflict,
} from "../stores/editor";
import {
  IconChevronDown,
  IconChevronRight,
  IconClose,
  IconFolder,
  IconRefresh,
  IconSearch,
} from "./icons";

interface FileEditorPanelProps {
  fleet?: FleetStore;
  editor?: EditorStore;
  onOpenFinder?: () => void;
}

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
                      Showing a partial listing (this folder has more entries than fit).
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
  const fallbackStore = !props.editor && props.fleet ? createEditorStore(props.fleet) : null;
  const editor = () => props.editor ?? fallbackStore;

  const lane = () => editor()?.selectedLane() ?? props.fleet?.selectedLane() ?? null;
  const openFiles = () => editor()?.openFiles() ?? [];
  const activePath = () => editor()?.activePath() ?? null;
  const activeFile = () => editor()?.activeFile() ?? null;
  const expandedDirs = () => editor()?.expandedDirs() ?? new Set<string>();
  const dirCache = () => editor()?.dirCache() ?? new Map();

  const [panelTreeExpanded, setPanelTreeExpanded] = createSignal(true);
  const [closeConfirmPath, setCloseConfirmPath] = createSignal<string | null>(null);
  const [treeMode, setTreeMode] = createSignal<"files" | "search">("files");

  const [replaceRequest, setReplaceRequest] = createSignal<CodeEditorReplaceRequest | null>(null);
  let replaceToken = 0;

  function handleReplaceInActiveFile(
    query: string,
    replacement: string,
    regex: boolean,
    caseSensitive: boolean,
    all?: boolean
  ) {
    setReplaceRequest({
      query,
      replacement,
      regex,
      caseSensitive,
      all,
      token: ++replaceToken,
    });
  }

  const effectiveTreeExpanded = createMemo(() => panelTreeExpanded() || openFiles().length === 0);

  function requestClose(path: string) {
    const f = openFiles().find((item) => item.path === path);
    if (f && f.content !== f.savedContent) {
      setCloseConfirmPath(path);
    } else {
      editor()?.closeFile(path);
    }
  }

  function handleCloseConfirmed(path: string) {
    editor()?.closeFile(path);
    setCloseConfirmPath(null);
  }

  return (
    <div class="flex h-full flex-col bg-surface">
      <div class="flex h-10 min-w-0 shrink-0 items-center justify-between border-b border-line bg-surface/95 px-3.5 backdrop-blur">
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
        <div class="flex items-center gap-1">
          <div class="flex items-center gap-0.5 rounded border border-line bg-background p-0.5">
            <button
              type="button"
              class={`flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[10px] font-medium transition-colors ${
                treeMode() === "files"
                  ? "bg-raised text-foreground font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => setTreeMode("files")}
              title="Files explorer"
            >
              <IconFolder size={11} />
            </button>
            <button
              type="button"
              class={`flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[10px] font-medium transition-colors ${
                treeMode() === "search"
                  ? "bg-raised text-foreground font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => setTreeMode("search")}
              title="Search in project"
            >
              <IconSearch size={11} />
            </button>
          </div>
          <button
            type="button"
            class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
            onClick={() => props.onOpenFinder?.()}
            disabled={!lane()}
            title="Find file"
            aria-label="Find file"
          >
            <IconSearch size={12} />
          </button>
          <button
            type="button"
            class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
            onClick={() => editor()?.refreshTree()}
            disabled={!lane()}
            title="Refresh file tree"
            aria-label="Refresh file tree"
          >
            <IconRefresh size={12} />
          </button>
        </div>
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
              onClick={() => setPanelTreeExpanded((v) => !v)}
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

          <Show
            when={treeMode() === "files"}
            fallback={
              <div class="min-h-0 flex-1 overflow-hidden">
                <ProjectSearchPanel
                  editor={editor()!}
                  compact={true}
                  onReplace={handleReplaceInActiveFile}
                />
              </div>
            }
          >
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
                  onToggleDir={(path) => editor()?.toggleDir(path)}
                  onOpenFile={(path) => {
                    setPanelTreeExpanded(false);
                    void editor()?.openFile(path);
                  }}
                />
              </div>
            </Show>
          </Show>

          <Show when={openFiles().length > 0}>
            <div class="flex h-8 shrink-0 items-center border-b border-line bg-surface/95">
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
                            requestClose(file.path);
                          }
                        }}
                      >
                        <button
                          type="button"
                          class={`focus-ring flex items-center gap-1.5 px-2 py-1 text-xs ${
                            isActive() ? "font-medium text-foreground" : "text-muted hover:text-foreground"
                          }`}
                          onClick={() => {
                            setPanelTreeExpanded(false);
                            editor()?.activateTab(file.path);
                          }}
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
                          onClick={() => requestClose(file.path)}
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
                  if (p) void editor()?.saveFile(p);
                }}
              >
                {activeFile()?.saving ? "Saving..." : "Save"}
              </button>
            </div>

            <div class="relative flex min-h-0 min-w-0 flex-1 flex-col">
              <Show when={activeFile()?.conflict} keyed>
                {(conflict) => (
                  <ConflictBanner
                    conflict={conflict}
                    onReload={() => void editor()?.reloadFile(activePath()!)}
                    onKeepMine={() => void editor()?.keepMine(activePath()!)}
                    onSaveAsNew={() => void editor()?.saveAsNew(activePath()!)}
                    onCloseDeleted={() => editor()?.closeFile(activePath()!)}
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
              <Show when={activeFile()}>
                {(file) => (
                  <Show
                    when={!file().loading}
                    fallback={
                      <div class="flex flex-1 items-center justify-center">
                        <p class="text-xs text-muted">Loading file...</p>
                      </div>
                    }
                  >
                    <Show
                      when={!file().loadError}
                      fallback={
                        <div class="flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center">
                          <p class="text-xs font-medium text-foreground">Can't open this file</p>
                          <p class="max-w-[220px] text-xs text-muted">{file().loadError?.friendly}</p>
                          <button
                            type="button"
                            class="focus-ring rounded-lg border border-line px-2.5 py-1 text-xs font-medium text-foreground hover:bg-raised"
                            onClick={() => requestClose(file().path)}
                          >
                            Close tab
                          </button>
                        </div>
                      }
                    >
                      <Switch>
                        <Match when={file().kind === "image"}>
                          <ImageViewer laneId={lane()?.id ?? 0} path={file().path} size={file().size} />
                        </Match>
                        <Match when={file().kind === "binary"}>
                          <BinaryViewer path={file().path} size={file().size} />
                        </Match>
                        <Match when={file().kind === "pdf"}>
                          <PdfViewer
                            worktreeRoot={lane()?.worktree.path ?? ""}
                            path={file().path}
                            size={file().size}
                          />
                        </Match>
                        <Match when={true}>
                          <CodeEditor
                            value={file().content}
                            path={file().path}
                            laneId={lane()?.id}
                            large={Boolean(file().large)}
                            wrap={editor()?.wrap()}
                            whitespace={editor()?.whitespace()}
                            initialCursor={file().cursor}
                            initialScrollTop={file().scrollTop}
                            saveVersion={file().saveVersion}
                            openAtTarget={editor()?.openAtTarget()?.path === file().path ? editor()?.openAtTarget() : null}
                            replaceRequest={activePath() === file().path ? replaceRequest() : null}
                            onCursorActivity={(cursor, scrollTop) => editor()?.updateCursor(file().path, cursor, scrollTop)}
                            onChange={(content) => editor()?.updateContent(file().path, content)}
                            onSave={() => void editor()?.saveFile(file().path)}
                            class="min-h-0 flex-1"
                          />
                        </Match>
                      </Switch>
                    </Show>
                  </Show>
                )}
              </Show>
            </div>
          </Show>
        </div>
      </Show>

      <Show when={closeConfirmPath()} keyed>
        {(path) => (
          <ConfirmDialog
            options={{
              title: "Unsaved changes",
              message: `Close ${path} without saving your changes?`,
              confirmLabel: "Discard and close",
              danger: true,
              onConfirm: () => handleCloseConfirmed(path),
            }}
            onClose={() => setCloseConfirmPath(null)}
          />
        )}
      </Show>
    </div>
  );
}
