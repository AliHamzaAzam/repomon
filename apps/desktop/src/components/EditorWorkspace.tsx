import {
  For,
  Match,
  Show,
  Switch,
  createMemo,
  createSignal,
  type Component,
} from "solid-js";
import { Dynamic } from "solid-js/web";

import type { FleetStore } from "../stores/fleet";
import type { EditorStore, FileConflict } from "../stores/editor";
import CodeEditor from "./CodeEditor";
import ImageViewer from "./ImageViewer";
import BinaryViewer from "./BinaryViewer";
import ConfirmDialog from "./ConfirmDialog";
import {
  IconChevronDown,
  IconChevronRight,
  IconClose,
  IconFile,
  IconFileBinary,
  IconFileCode,
  IconFileImage,
  IconFileText,
  IconFolder,
  IconFolderOpen,
  IconLocate,
  IconRefresh,
  IconSearch,
  type IconProps,
} from "./icons";

export interface EditorWorkspaceProps {
  fleet: FleetStore;
  editor: EditorStore;
  actions?: unknown;
}

function basename(path: string): string {
  return path.split("/").pop() || path;
}

function getFileIcon(path: string, kind?: string): Component<IconProps> {
  if (kind === "image") return IconFileImage;
  if (kind === "binary") return IconFileBinary;
  const ext = path.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "rs":
    case "ts":
    case "tsx":
    case "js":
    case "jsx":
    case "go":
    case "py":
    case "c":
    case "cpp":
    case "h":
    case "css":
    case "html":
    case "sh":
    case "bash":
    case "zsh":
      return IconFileCode;
    case "md":
    case "txt":
    case "doc":
      return IconFileText;
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
    case "svg":
    case "bmp":
    case "ico":
      return IconFileImage;
    default:
      return IconFile;
  }
}

function ConflictBanner(props: {
  conflict: FileConflict;
  onReload: () => void;
  onKeepMine: () => void;
  onSaveAsNew: () => void;
}) {
  return (
    <div
      role="alert"
      class="flex items-center justify-between border-b border-attention/40 bg-attention/10 px-3 py-1.5 text-xs text-attention"
    >
      <div class="flex items-center gap-2">
        <span class="size-2 rounded-full bg-attention" />
        <span class="font-medium">
          {props.conflict.deleted
            ? "File was deleted on disk"
            : "File changed on disk since last read"}
        </span>
      </div>
      <div class="flex items-center gap-1">
        <button
          type="button"
          class="focus-ring rounded border border-attention/40 bg-attention/10 px-2 py-0.5 text-xs font-medium text-attention hover:bg-attention/20"
          onClick={props.onReload}
        >
          {props.conflict.deleted ? "Close tab" : "Reload disk version"}
        </button>
        <Show when={props.conflict.deleted}>
          <button
            type="button"
            class="focus-ring rounded border border-attention/40 bg-attention/10 px-2 py-0.5 text-xs font-medium text-attention hover:bg-attention/20"
            onClick={props.onSaveAsNew}
          >
            Save as new content
          </button>
        </Show>
        <Show when={!props.conflict.deleted}>
          <button
            type="button"
            class="focus-ring rounded px-2 py-0.5 text-xs text-muted hover:text-foreground"
            onClick={props.onKeepMine}
          >
            Keep mine
          </button>
        </Show>
      </div>
    </div>
  );
}

const COMMON_LANGUAGES = [
  { id: "rust", label: "Rust" },
  { id: "typescript", label: "TypeScript" },
  { id: "javascript", label: "JavaScript" },
  { id: "python", label: "Python" },
  { id: "json", label: "JSON" },
  { id: "toml", label: "TOML" },
  { id: "yaml", label: "YAML" },
  { id: "markdown", label: "Markdown" },
  { id: "css", label: "CSS" },
  { id: "html", label: "HTML" },
  { id: "shell", label: "Shell Script" },
  { id: "go", label: "Go" },
  { id: "c", label: "C" },
  { id: "cpp", label: "C++" },
  { id: "dockerfile", label: "Dockerfile" },
];

export default function EditorWorkspace(props: EditorWorkspaceProps) {
  const lane = () => props.editor.selectedLane() ?? props.fleet.selectedLane() ?? null;
  const openFiles = () => props.editor.openFiles();
  const activePath = () => props.editor.activePath();
  const activeFile = () => props.editor.activeFile();
  const expandedDirs = () => props.editor.expandedDirs();
  const dirCache = () => props.editor.dirCache();

  const [filterQuery, setFilterQuery] = createSignal("");
  const [closeConfirmPath, setCloseConfirmPath] = createSignal<string | null>(null);
  const [langMenuOpen, setLangMenuOpen] = createSignal(false);
  const [cursorLine, setCursorLine] = createSignal(1);
  const [cursorCol, setCursorCol] = createSignal(1);
  const [isResizing, setIsResizing] = createSignal(false);

  // Compute line and column from doc length and head
  function updateCursorPos(head: number) {
    const file = activeFile();
    if (!file) {
      setCursorLine(1);
      setCursorCol(1);
      return;
    }
    const content = file.content;
    const bounded = Math.min(head, content.length);
    let line = 1;
    let col = 1;
    for (let i = 0; i < bounded; i++) {
      if (content[i] === "\n") {
        line++;
        col = 1;
      } else {
        col++;
      }
    }
    setCursorLine(line);
    setCursorCol(col);
  }

  function handleResizeStart(e: MouseEvent) {
    e.preventDefault();
    setIsResizing(true);
    const startX = e.clientX;
    const startWidth = props.editor.treeColumnWidth();

    function onMouseMove(moveEvent: MouseEvent) {
      const delta = moveEvent.clientX - startX;
      const newWidth = Math.max(180, Math.min(600, startWidth + delta));
      props.editor.setTreeColumnWidth(newWidth);
    }

    function onMouseUp() {
      setIsResizing(false);
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    }

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
  }

  function requestClose(path: string) {
    const f = openFiles().find((item) => item.path === path);
    if (f && f.content !== f.savedContent) {
      setCloseConfirmPath(path);
    } else {
      props.editor.closeFile(path);
    }
  }

  // Detect indent unit text
  const currentIndentUnit = createMemo(() => {
    const f = activeFile();
    if (!f) return "Spaces: 2";
    const head = f.content.slice(0, 4000);
    if (/^\t/m.test(head)) return "Tabs";
    if (/^ {4}[^\s]/m.test(head)) return "Spaces: 4";
    return "Spaces: 2";
  });

  // Flat list of visible tree items for keyboard navigation
  const visibleTreeItems = createMemo(() => {
    const result: Array<{ path: string; name: string; isDir: boolean; depth: number }> = [];
    const query = filterQuery().toLowerCase().trim();

    function traverse(dirPath: string, depth: number) {
      const cache = dirCache().get(dirPath);
      if (!cache || cache.status !== "loaded") return;
      for (const entry of cache.entries) {
        const matches = !query || entry.name.toLowerCase().includes(query) || entry.path.toLowerCase().includes(query);
        if (entry.is_dir) {
          if (matches || query) {
            result.push({ path: entry.path, name: entry.name, isDir: true, depth });
          }
          if (expandedDirs().has(entry.path) || query) {
            traverse(entry.path, depth + 1);
          }
        } else if (matches) {
          result.push({ path: entry.path, name: entry.name, isDir: false, depth });
        }
      }
    }

    traverse("", 0);
    return result;
  });

  const [focusedTreeIndex, setFocusedTreeIndex] = createSignal(0);

  function handleTreeKeyDown(e: KeyboardEvent) {
    const items = visibleTreeItems();
    if (items.length === 0) return;
    const currentIdx = focusedTreeIndex();

    if (e.key === "ArrowDown") {
      e.preventDefault();
      setFocusedTreeIndex((prev) => Math.min(items.length - 1, prev + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setFocusedTreeIndex((prev) => Math.max(0, prev - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[currentIdx];
      if (item) {
        if (item.isDir) {
          props.editor.toggleDir(item.path);
        } else {
          void props.editor.openFile(item.path);
        }
      }
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      const item = items[currentIdx];
      if (item && item.isDir) {
        if (!expandedDirs().has(item.path)) {
          props.editor.expandDir(item.path);
        } else {
          setFocusedTreeIndex((prev) => Math.min(items.length - 1, prev + 1));
        }
      }
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      const item = items[currentIdx];
      if (item && item.isDir && expandedDirs().has(item.path)) {
        props.editor.collapseDir(item.path);
      } else {
        // Move to parent
        const parentPath = item?.path.split("/").slice(0, -1).join("/");
        const parentIdx = items.findIndex((i) => i.path === parentPath);
        if (parentIdx >= 0) setFocusedTreeIndex(parentIdx);
      }
    }
  }

  return (
    <div class="flex h-full w-full select-none overflow-hidden bg-background">
      {/* Resizable Tree Column */}
      <div
        class="flex flex-col border-r border-line bg-surface"
        style={{ width: `${props.editor.treeColumnWidth()}px`, "min-width": "180px" }}
      >
        {/* Tree Column Header */}
        <div class="flex h-9 shrink-0 items-center justify-between border-b border-line px-3">
          <div class="flex items-center gap-1.5 min-w-0">
            <span class="font-mono text-xs font-semibold uppercase tracking-wider text-muted">
              {lane()?.worktree.name ?? "Files"}
            </span>
            <Show when={lane()?.worktree.branch}>
              <span class="truncate font-mono text-[10px] text-muted/60">
                ({lane()?.worktree.branch})
              </span>
            </Show>
          </div>
          <div class="flex items-center gap-1">
            <button
              type="button"
              class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
              title="Reveal active file in tree"
              onClick={() => {
                const path = activePath();
                if (path) props.editor.revealFile(path);
              }}
            >
              <IconLocate size={13} />
            </button>
            <button
              type="button"
              class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
              title="Refresh file tree"
              onClick={() => props.editor.refreshTree()}
            >
              <IconRefresh size={12} />
            </button>
          </div>
        </div>

        {/* Filter input */}
        <div class="border-b border-line p-2">
          <div class="relative flex items-center">
            <IconSearch size={12} class="pointer-events-none absolute left-2 text-muted" />
            <input
              type="text"
              class="focus-ring w-full rounded border border-line bg-background py-1 pr-2 pl-7 font-mono text-xs text-foreground placeholder:text-muted/60"
              placeholder="Filter files..."
              value={filterQuery()}
              onInput={(e) => setFilterQuery(e.currentTarget.value)}
            />
            <Show when={filterQuery().length > 0}>
              <button
                type="button"
                class="absolute right-1.5 text-muted hover:text-foreground"
                onClick={() => setFilterQuery("")}
              >
                <IconClose size={10} />
              </button>
            </Show>
          </div>
        </div>

        {/* Tree List View */}
        <div
          class="flex-1 overflow-y-auto p-1 outline-none"
          tabIndex={0}
          onKeyDown={handleTreeKeyDown}
        >
          <For each={visibleTreeItems()}>
            {(item, idx) => {
              const isFocused = () => idx() === focusedTreeIndex();
              const isActive = () => !item.isDir && item.path === activePath();
              const isExpanded = () => item.isDir && expandedDirs().has(item.path);
              const Icon = () => getFileIcon(item.path);

              return (
                <button
                  type="button"
                  class={`focus-ring flex w-full items-center gap-1.5 rounded px-1.5 py-0.5 text-left text-xs transition-colors ${
                    isActive()
                      ? "bg-accent/15 font-medium text-accent"
                      : isFocused()
                      ? "bg-raised/70 text-foreground"
                      : "text-foreground/80 hover:bg-raised/40 hover:text-foreground"
                  }`}
                  style={{ "padding-left": `${item.depth * 14 + 6}px` }}
                  onClick={() => {
                    setFocusedTreeIndex(idx());
                    if (item.isDir) {
                      props.editor.toggleDir(item.path);
                    } else {
                      void props.editor.openFile(item.path);
                    }
                  }}
                  title={item.path}
                >
                  <Show
                    when={item.isDir}
                    fallback={
                      <span class="size-3.5 shrink-0 text-muted">
                        <Dynamic component={Icon()} size={12} />
                      </span>
                    }
                  >
                    <span class="size-3 shrink-0 text-muted/60">
                      <Show when={isExpanded()} fallback={<IconChevronRight size={10} />}>
                        <IconChevronDown size={10} />
                      </Show>
                    </span>
                    <span class="size-3.5 shrink-0 text-accent/70">
                      <Show when={isExpanded()} fallback={<IconFolder size={12} />}>
                        <IconFolderOpen size={12} />
                      </Show>
                    </span>
                  </Show>
                  <span class="truncate font-mono text-[11px]">{item.name}</span>
                </button>
              );
            }}
          </For>
        </div>
      </div>

      {/* Draggable Divider */}
      <div
        class={`relative flex w-1 cursor-col-resize items-center justify-center transition-colors hover:bg-accent/40 ${
          isResizing() ? "bg-accent" : "bg-transparent"
        }`}
        onMouseDown={handleResizeStart}
        aria-hidden="true"
      />

      {/* Right Column: Tab strip, Editor view, and Status line */}
      <div class="flex min-w-0 flex-1 flex-col overflow-hidden bg-surface">
        {/* Tab strip */}
        <div class="flex h-9 shrink-0 items-center overflow-x-auto border-b border-line bg-surface/90 px-1">
          <For each={openFiles()}>
            {(file) => {
              const isActive = () => file.path === activePath();
              const isDirty = () => file.content !== file.savedContent;
              const Icon = () => getFileIcon(file.path, file.kind);

              return (
                <div
                  class={`group flex h-7 shrink-0 items-center gap-1.5 border-r border-line/60 px-2.5 text-xs transition-colors ${
                    isActive()
                      ? "bg-background font-medium text-foreground"
                      : "bg-surface text-muted hover:bg-raised/40 hover:text-foreground"
                  }`}
                >
                  <button
                    type="button"
                    class="focus-ring flex items-center gap-1.5"
                    onClick={() => props.editor.activateTab(file.path)}
                    title={file.path}
                  >
                    <Dynamic component={Icon()} size={12} class="shrink-0 text-muted" />
                    <span class="max-w-[140px] truncate font-mono text-[11px]">
                      {basename(file.path)}
                    </span>
                    <Show when={isDirty()}>
                      <span
                        class="size-1.5 shrink-0 rounded-full bg-attention"
                        title="Unsaved changes"
                      />
                    </Show>
                  </button>
                  <button
                    type="button"
                    class="focus-ring ml-1 flex size-4 shrink-0 items-center justify-center rounded text-muted/60 opacity-0 hover:bg-line hover:text-foreground group-hover:opacity-100"
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

        {/* Center Editor Container */}
        <div class="relative flex min-h-0 flex-1 flex-col overflow-hidden bg-background">
          <Show
            when={activeFile()}
            fallback={
              <div class="flex h-full flex-col items-center justify-center p-6 text-center text-muted select-none">
                <div class="mb-3 flex size-12 items-center justify-center rounded-xl border border-line bg-surface/50 text-muted/60">
                  <IconFile size={24} />
                </div>
                <p class="font-mono text-xs font-semibold text-foreground">No file open</p>
                <p class="mt-1 max-w-xs text-xs text-muted">
                  Select a file from the tree on the left to view or edit.
                </p>
              </div>
            }
          >
            {(file) => (
              <div class="flex h-full flex-col overflow-hidden">
                <Show when={file().conflict} keyed>
                  {(conflict) => (
                    <ConflictBanner
                      conflict={conflict}
                      onReload={() => void props.editor.reloadFile(file().path)}
                      onKeepMine={() => void props.editor.keepMine(file().path)}
                      onSaveAsNew={() => void props.editor.saveAsNew(file().path)}
                    />
                  )}
                </Show>

                <Show
                  when={!file().loadError}
                  fallback={
                    <div class="flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center">
                      <p class="text-xs font-medium text-foreground">Cannot open file</p>
                      <p class="max-w-[260px] text-xs text-muted">{file().loadError?.friendly}</p>
                      <button
                        type="button"
                        class="focus-ring rounded border border-line px-3 py-1 text-xs font-medium text-foreground hover:bg-raised"
                        onClick={() => requestClose(file().path)}
                      >
                        Close tab
                      </button>
                    </div>
                  }
                >
                  <Switch>
                    <Match when={file().kind === "image"}>
                      <ImageViewer
                        laneId={lane()?.id ?? 0}
                        path={file().path}
                        size={file().size}
                      />
                    </Match>
                    <Match when={file().kind === "binary"}>
                      <BinaryViewer path={file().path} size={file().size} />
                    </Match>
                    <Match when={true}>
                      <CodeEditor
                        value={file().content}
                        path={file().path}
                        languageOverride={props.editor.languageOverrides()[file().path]}
                        wrap={props.editor.wrap()}
                        whitespace={props.editor.whitespace()}
                        initialCursor={file().cursor}
                        initialScrollTop={file().scrollTop}
                        onCursorActivity={(cursor, scrollTop) => {
                          props.editor.updateCursor(file().path, cursor, scrollTop);
                          updateCursorPos(cursor);
                        }}
                        onChange={(content) => props.editor.updateContent(file().path, content)}
                        onSave={() => void props.editor.saveFile(file().path)}
                        class="min-h-0 flex-1"
                      />
                    </Match>
                  </Switch>
                </Show>
              </div>
            )}
          </Show>
        </div>

        {/* Status Line */}
        <div class="flex h-6 shrink-0 items-center justify-between border-t border-line bg-surface/95 px-3 font-mono text-[11px] text-muted select-none">
          <div class="flex items-center gap-3">
            {/* Language override button */}
            <div class="relative">
              <button
                type="button"
                class="focus-ring flex items-center gap-1 text-muted hover:text-foreground"
                onClick={() => setLangMenuOpen((v) => !v)}
                title="Click to override syntax language"
              >
                <span>
                  {props.editor.languageOverrides()[activePath() ?? ""] ?? "Auto Language"}
                </span>
                <IconChevronDown size={9} />
              </button>

              <Show when={langMenuOpen()}>
                <div
                  class="absolute bottom-6 left-0 z-50 max-h-60 w-40 overflow-y-auto rounded-lg border border-line bg-surface p-1 shadow-lg"
                  role="menu"
                >
                  <button
                    type="button"
                    class="focus-ring flex w-full rounded px-2 py-1 text-left text-xs text-muted hover:bg-raised hover:text-foreground"
                    onClick={() => {
                      const p = activePath();
                      if (p) props.editor.setLanguageOverride(p, null);
                      setLangMenuOpen(false);
                    }}
                  >
                    Auto (default)
                  </button>
                  <div class="my-1 border-t border-line/60" />
                  <For each={COMMON_LANGUAGES}>
                    {(lang) => (
                      <button
                        type="button"
                        class="focus-ring flex w-full rounded px-2 py-1 text-left text-xs text-foreground/90 hover:bg-raised hover:text-foreground"
                        onClick={() => {
                          const p = activePath();
                          if (p) props.editor.setLanguageOverride(p, lang.id);
                          setLangMenuOpen(false);
                        }}
                      >
                        {lang.label}
                      </button>
                    )}
                  </For>
                </div>
              </Show>
            </div>

            <span class="text-line">|</span>
            <span>
              Ln {cursorLine()}, Col {cursorCol()}
            </span>

            <span class="text-line">|</span>
            <span>{currentIndentUnit()}</span>
          </div>

          <div class="flex items-center gap-2">
            <button
              type="button"
              class={`focus-ring rounded px-1.5 py-0.5 transition-colors ${
                props.editor.wrap()
                  ? "bg-accent/15 font-medium text-accent"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={props.editor.toggleWrap}
              title="Toggle line wrapping"
            >
              Wrap: {props.editor.wrap() ? "On" : "Off"}
            </button>

            <span class="text-line">|</span>

            <button
              type="button"
              class={`focus-ring rounded px-1.5 py-0.5 transition-colors ${
                props.editor.whitespace()
                  ? "bg-accent/15 font-medium text-accent"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={props.editor.toggleWhitespace}
              title="Toggle render whitespace"
            >
              Whitespace: {props.editor.whitespace() ? "On" : "Off"}
            </button>
          </div>
        </div>
      </div>

      {/* Discard confirmation modal */}
      <Show when={closeConfirmPath()} keyed>
        {(path) => (
          <ConfirmDialog
            options={{
              title: "Unsaved changes",
              message: `Close ${basename(path)} without saving your changes?`,
              confirmLabel: "Discard and close",
              danger: true,
              onConfirm: () => {
                setCloseConfirmPath(null);
                props.editor.closeFile(path);
              },
            }}
            onClose={() => setCloseConfirmPath(null)}
          />
        )}
      </Show>
    </div>
  );
}
