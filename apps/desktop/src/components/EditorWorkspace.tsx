import {
  For,
  Match,
  Show,
  Switch,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  type Component,
} from "solid-js";
import { Dynamic } from "solid-js/web";

import type { FleetStore } from "../stores/fleet";
import type { EditorStore, FileConflict } from "../stores/editor";
import { daemonCall } from "../ipc/rpc";
import CodeEditor, { type CodeEditorReplaceRequest } from "./CodeEditor";
import ProjectSearchPanel from "./ProjectSearchPanel";
import ImageViewer from "./ImageViewer";
import BinaryViewer from "./BinaryViewer";
import PdfViewer from "./PdfViewer";
import ConfirmDialog from "./ConfirmDialog";
import {
  MarkdownPreview,
  parseMarkdown,
  findNearestHeading,
} from "./markdown";

export function isMarkdownFile(path: string | null | undefined): boolean {
  if (!path) return false;
  const lower = path.toLowerCase();
  return lower.endsWith(".md") || lower.endsWith(".markdown");
}
import {
  IconChevronDown,
  IconChevronRight,
  IconClose,
  IconFile,
  IconFileBinary,
  IconFileCode,
  IconFileImage,
  IconFilePdf,
  IconFileText,
  IconFolder,
  IconFolderOpen,
  IconLocate,
  IconMoreVertical,
  IconRefresh,
  IconSearch,
  type IconProps,
} from "./icons";

export interface EditorWorkspaceProps {
  fleet: FleetStore;
  editor: EditorStore;
  actions?: unknown;
  onOpenFinder?: () => void;
}

function basename(path: string): string {
  return path.split("/").pop() || path;
}

function getFileIcon(path: string, kind?: string): Component<IconProps> {
  if (kind === "image") return IconFileImage;
  if (kind === "binary") return IconFileBinary;
  if (kind === "pdf") return IconFilePdf;
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
    case "pdf":
      return IconFilePdf;
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
  const [selectionCount, setSelectionCount] = createSignal(1);
  const [selectedChars, setSelectedChars] = createSignal(0);
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
      props.editor.persistTreeColumnWidth();
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

  const [treeMode, setTreeMode] = createSignal<"files" | "search">("files");

  // Below this tree column width, the header's mode-switcher labels ("Files" /
  // "Search") collapse to icon-only so the switcher and the icon button group
  // beside it both fit on one row without clipping. Reuses the resize signal
  // the column already tracks rather than a container query.
  const TREE_HEADER_LABEL_MIN_WIDTH_PX = 300;
  const compactTreeHeader = createMemo(
    () => props.editor.treeColumnWidth() < TREE_HEADER_LABEL_MIN_WIDTH_PX
  );

  // Context Menu State
  const [contextMenu, setContextMenu] = createSignal<{
    x: number;
    y: number;
    path: string;
    isDir: boolean;
    name: string;
    depth: number;
  } | null>(null);
  let contextMenuOpener: HTMLElement | null = null;

  createEffect(() => {
    const menu = contextMenu();
    if (!menu) return;

    const handleWindowKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setContextMenu(null);
        contextMenuOpener?.focus();
      }
    };

    window.addEventListener("keydown", handleWindowKeyDown, true);
    onCleanup(() => {
      window.removeEventListener("keydown", handleWindowKeyDown, true);
    });
  });

  // Inline Create State
  const [inlineCreate, setInlineCreate] = createSignal<{
    parentDir: string;
    isDir: boolean;
    depth: number;
  } | null>(null);
  const [inlineCreateInFlight, setInlineCreateInFlight] = createSignal(false);

  // Inline Rename State
  const [inlineRename, setInlineRename] = createSignal<{
    path: string;
    isDir: boolean;
    name: string;
  } | null>(null);
  const [inlineRenameInFlight, setInlineRenameInFlight] = createSignal(false);

  // Delete Target State
  const [deleteTarget, setDeleteTarget] = createSignal<{
    path: string;
    isDir: boolean;
  } | null>(null);

  // Replace in active file
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

  let editorContainerRef: HTMLDivElement | undefined;
  const [isSplitResizing, setIsSplitResizing] = createSignal(false);
  const [visibleLine, setVisibleLine] = createSignal(1);

  const nearestHeading = createMemo(() => {
    const path = activePath();
    if (!isMarkdownFile(path)) return null;
    const file = activeFile();
    if (!file) return null;
    const { headings } = parseMarkdown(file.content);
    return findNearestHeading(headings, visibleLine());
  });

  function handleSplitResizeStart(e: MouseEvent) {
    e.preventDefault();
    setIsSplitResizing(true);
    const container = editorContainerRef;
    if (!container) return;
    const rect = container.getBoundingClientRect();

    function onMouseMove(moveEvent: MouseEvent) {
      const offsetX = moveEvent.clientX - rect.left;
      const ratio = offsetX / rect.width;
      props.editor.setMarkdownSplitRatio(ratio);
    }

    function onMouseUp() {
      setIsSplitResizing(false);
      props.editor.persistMarkdownSplitRatio();
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    }

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
  }

  function startInlineCreate(targetPath: string, isDir: boolean, makeDir: boolean, depth: number) {
    if (isDir) {
      props.editor.expandDir(targetPath);
      setInlineCreate({ parentDir: targetPath, isDir: makeDir, depth: depth + 1 });
    } else {
      const parentDir = targetPath.split("/").slice(0, -1).join("/");
      setInlineCreate({ parentDir, isDir: makeDir, depth });
    }
  }

  async function commitInlineCreate(name: string) {
    if (inlineCreateInFlight()) return;
    const create = inlineCreate();
    if (!create || !name.trim()) {
      setInlineCreate(null);
      return;
    }
    const fullPath = create.parentDir ? `${create.parentDir}/${name.trim()}` : name.trim();
    const laneId = props.editor.currentLaneId();
    if (laneId == null) {
      setInlineCreate(null);
      return;
    }

    setInlineCreateInFlight(true);
    try {
      await daemonCall("file.create", {
        lane_id: laneId,
        path: fullPath,
        is_dir: create.isDir,
      });
      setInlineCreate(null);
      await props.editor.loadDir(laneId, create.parentDir);
      if (!create.isDir) {
        await props.editor.openFile(fullPath);
      }
    } catch (err) {
      console.error("file.create error:", err);
      setInlineCreate(null);
    } finally {
      setInlineCreateInFlight(false);
    }
  }

  async function commitInlineRename(newName: string) {
    if (inlineRenameInFlight()) return;
    const rename = inlineRename();
    if (!rename || !newName.trim() || newName.trim() === rename.name) {
      setInlineRename(null);
      return;
    }
    const parentDir = rename.path.split("/").slice(0, -1).join("/");
    const toPath = parentDir ? `${parentDir}/${newName.trim()}` : newName.trim();
    const laneId = props.editor.currentLaneId();
    if (laneId == null) {
      setInlineRename(null);
      return;
    }

    setInlineRenameInFlight(true);
    try {
      await daemonCall("file.rename", {
        lane_id: laneId,
        from: rename.path,
        to: toPath,
      });
      setInlineRename(null);
      props.editor.handleFileRenamed(rename.path, toPath, laneId);
      await props.editor.loadDir(laneId, parentDir);
    } catch (err) {
      console.error("file.rename error:", err);
      setInlineRename(null);
    } finally {
      setInlineRenameInFlight(false);
    }
  }

  async function commitDelete(target: { path: string; isDir: boolean }) {
    const laneId = props.editor.currentLaneId();
    if (laneId == null) return;
    try {
      await daemonCall("file.delete", {
        lane_id: laneId,
        path: target.path,
        recursive: true,
      });
      props.editor.handleFileDeleted(target.path, laneId);
      const parentDir = target.path.split("/").slice(0, -1).join("/");
      await props.editor.loadDir(laneId, parentDir);
    } catch (err) {
      console.error("file.delete error:", err);
    }
  }

  async function revealInFinder(relPath: string) {
    const l = lane();
    if (!l) return;
    const fullPath = `${l.worktree.path}/${relPath}`;
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(fullPath);
    } catch (err) {
      console.warn("revealItemInDir failed:", err);
    }
  }

  function copyRelativePath(path: string) {
    void navigator.clipboard.writeText(path);
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
        <div class="flex min-h-9 flex-wrap items-center justify-between gap-1 border-b border-line px-2 py-1">
          {/* Mode Switcher: Files vs Search. Shrinks first, and drops its
              labels to icon-only, so the icon button group never clips. */}
          <div class="flex min-w-0 items-center gap-0.5 rounded border border-line bg-background p-0.5">
            <button
              type="button"
              aria-label="Files explorer"
              class={`flex min-w-0 items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[11px] font-medium transition-colors ${
                treeMode() === "files"
                  ? "bg-raised text-foreground font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => setTreeMode("files")}
              title="Files explorer"
            >
              <IconFolder size={12} />
              <Show when={!compactTreeHeader()}>
                <span>Files</span>
              </Show>
            </button>
            <button
              type="button"
              aria-label="Search in project"
              class={`flex min-w-0 items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[11px] font-medium transition-colors ${
                treeMode() === "search"
                  ? "bg-raised text-foreground font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => setTreeMode("search")}
              title="Search in project"
            >
              <IconSearch size={12} />
              <Show when={!compactTreeHeader()}>
                <span>Search</span>
              </Show>
            </button>
          </div>

          {/* Icon actions never shrink below their tap target; the row wraps
              onto a second line rather than clipping this group. */}
          <div class="flex shrink-0 items-center gap-0.5">
            <Show when={treeMode() === "files"}>
              <button
                type="button"
                aria-label="New file in root"
                class="focus-ring flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                title="New file in root"
                onClick={() => setInlineCreate({ parentDir: "", isDir: false, depth: 0 })}
              >
                <IconFileCode size={13} />
              </button>
              <button
                type="button"
                aria-label="New folder in root"
                class="focus-ring flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                title="New folder in root"
                onClick={() => setInlineCreate({ parentDir: "", isDir: true, depth: 0 })}
              >
                <IconFolder size={13} />
              </button>
              <button
                type="button"
                aria-label="Find file"
                class="focus-ring flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                title="Find file"
                onClick={() => props.onOpenFinder?.()}
              >
                <IconSearch size={13} />
              </button>
              <button
                type="button"
                aria-label="Reveal active file in tree"
                class="focus-ring flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
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
                aria-label="Refresh file tree"
                class="focus-ring flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                title="Refresh file tree"
                onClick={() => props.editor.refreshTree()}
              >
                <IconRefresh size={12} />
              </button>
            </Show>
          </div>
        </div>

        <Show
          when={treeMode() === "files"}
          fallback={
            <ProjectSearchPanel
              editor={props.editor}
              onReplace={handleReplaceInActiveFile}
            />
          }
        >
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
            {/* Root level inline create */}
            <Show when={inlineCreate()?.parentDir === ""}>
              <div
                class="flex w-full items-center gap-1.5 px-1.5 py-0.5"
                style={{ "padding-left": "6px" }}
              >
                <span class="size-3.5 shrink-0 text-muted">
                  <Show when={inlineCreate()?.isDir} fallback={<IconFile size={12} />}>
                    <IconFolder size={12} />
                  </Show>
                </span>
                <input
                  type="text"
                  disabled={inlineCreateInFlight()}
                  class="focus-ring flex-1 rounded border border-signal bg-background px-1.5 py-0.5 font-mono text-[11px] text-foreground disabled:opacity-50"
                  placeholder={inlineCreate()?.isDir ? "Folder name..." : "File name..."}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void commitInlineCreate(e.currentTarget.value);
                    } else if (e.key === "Escape") {
                      e.preventDefault();
                      setInlineCreate(null);
                    }
                  }}
                  onBlur={(e) => void commitInlineCreate(e.currentTarget.value)}
                  ref={(el) => setTimeout(() => el?.focus(), 0)}
                />
              </div>
            </Show>

            <For each={visibleTreeItems()}>
              {(item, idx) => {
                const isFocused = () => idx() === focusedTreeIndex();
                const isActive = () => !item.isDir && item.path === activePath();
                const isExpanded = () => item.isDir && expandedDirs().has(item.path);
                const Icon = () => getFileIcon(item.path);
                const isRenaming = () => inlineRename()?.path === item.path;

                return (
                  <div class="group relative flex w-full items-center">
                    <Show
                      when={!isRenaming()}
                      fallback={
                        <div
                          class="flex w-full items-center gap-1.5 px-1.5 py-0.5"
                          style={{ "padding-left": `${item.depth * 14 + 6}px` }}
                        >
                          <span class="size-3.5 shrink-0 text-muted">
                            <Show when={item.isDir} fallback={<IconFile size={12} />}>
                              <IconFolder size={12} />
                            </Show>
                          </span>
                          <input
                            type="text"
                            disabled={inlineRenameInFlight()}
                            class="focus-ring flex-1 rounded border border-signal bg-background px-1.5 py-0.5 font-mono text-[11px] text-foreground disabled:opacity-50"
                            value={item.name}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") {
                                e.preventDefault();
                                void commitInlineRename(e.currentTarget.value);
                              } else if (e.key === "Escape") {
                                e.preventDefault();
                                setInlineRename(null);
                              }
                            }}
                            onBlur={(e) => void commitInlineRename(e.currentTarget.value)}
                            ref={(el) => setTimeout(() => el?.select(), 0)}
                          />
                        </div>
                      }
                    >
                      <button
                        type="button"
                        class={`focus-ring flex min-w-0 flex-1 items-center gap-1.5 rounded px-1.5 py-0.5 text-left text-xs transition-colors ${
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
                        onContextMenu={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          contextMenuOpener = e.currentTarget as HTMLElement;
                          setContextMenu({
                            x: e.clientX,
                            y: e.clientY,
                            path: item.path,
                            isDir: item.isDir,
                            name: item.name,
                            depth: item.depth,
                          });
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

                      {/* Row kebab menu trigger */}
                      <button
                        type="button"
                        class="focus-ring mr-1 flex size-5 shrink-0 items-center justify-center rounded text-muted opacity-0 transition-opacity group-hover:opacity-100 hover:bg-raised hover:text-foreground"
                        title="File actions"
                        onClick={(e) => {
                          e.stopPropagation();
                          contextMenuOpener = e.currentTarget as HTMLElement;
                          const rect = e.currentTarget.getBoundingClientRect();
                          setContextMenu({
                            x: rect.right,
                            y: rect.bottom,
                            path: item.path,
                            isDir: item.isDir,
                            name: item.name,
                            depth: item.depth,
                          });
                        }}
                      >
                        <IconMoreVertical size={12} />
                      </button>
                    </Show>
                  </div>
                );
              }}
            </For>

            {/* Nested inline create */}
            <Show when={inlineCreate() && inlineCreate()?.parentDir !== ""}>
              <div
                class="flex w-full items-center gap-1.5 px-1.5 py-0.5"
                style={{ "padding-left": `${(inlineCreate()?.depth ?? 0) * 14 + 6}px` }}
              >
                <span class="size-3.5 shrink-0 text-muted">
                  <Show when={inlineCreate()?.isDir} fallback={<IconFile size={12} />}>
                    <IconFolder size={12} />
                  </Show>
                </span>
                <input
                  type="text"
                  disabled={inlineCreateInFlight()}
                  class="focus-ring flex-1 rounded border border-signal bg-background px-1.5 py-0.5 font-mono text-[11px] text-foreground disabled:opacity-50"
                  placeholder={inlineCreate()?.isDir ? "Folder name..." : "File name..."}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void commitInlineCreate(e.currentTarget.value);
                    } else if (e.key === "Escape") {
                      e.preventDefault();
                      setInlineCreate(null);
                    }
                  }}
                  onBlur={(e) => void commitInlineCreate(e.currentTarget.value)}
                  ref={(el) => setTimeout(() => el?.focus(), 0)}
                />
              </div>
            </Show>
          </div>
        </Show>
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
                  <div
                    ref={editorContainerRef}
                    class="relative flex min-h-0 flex-1 overflow-hidden"
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
                      <Match when={file().kind === "pdf"}>
                        <PdfViewer
                          worktreeRoot={lane()?.worktree.path ?? ""}
                          path={file().path}
                          size={file().size}
                        />
                      </Match>
                      <Match when={true}>
                        <div
                          class="flex h-full min-w-0 flex-col overflow-hidden"
                          style={{
                            width:
                              isMarkdownFile(file().path) && props.editor.markdownPreview()
                                ? `${props.editor.markdownSplitRatio() * 100}%`
                                : "100%",
                          }}
                        >
                          <CodeEditor
                            value={file().content}
                            path={file().path}
                            laneId={lane()?.id}
                            large={Boolean(file().large)}
                            languageOverride={props.editor.languageOverrides()[file().path]}
                            wrap={props.editor.wrap()}
                            whitespace={props.editor.whitespace()}
                            initialCursor={file().cursor}
                            initialScrollTop={file().scrollTop}
                            openAtTarget={props.editor.openAtTarget()?.path === file().path ? props.editor.openAtTarget() : null}
                            replaceRequest={activePath() === file().path ? replaceRequest() : null}
                            onCursorActivity={(cursor, scrollTop, selection) => {
                              props.editor.updateCursor(file().path, cursor, scrollTop);
                              updateCursorPos(cursor);
                              setSelectionCount(selection.rangeCount);
                              setSelectedChars(selection.selectedChars);
                            }}
                            onVisibleLineChange={setVisibleLine}
                            onChange={(content) => props.editor.updateContent(file().path, content)}
                            onSave={() => void props.editor.saveFile(file().path)}
                            class="min-h-0 flex-1"
                          />
                        </div>

                        <Show when={isMarkdownFile(file().path) && props.editor.markdownPreview()}>
                          <div
                            class={`relative flex w-1 cursor-col-resize items-center justify-center border-l border-r border-line bg-surface hover:bg-accent/40 ${
                              isSplitResizing() ? "bg-accent" : ""
                            }`}
                            onMouseDown={handleSplitResizeStart}
                            aria-hidden="true"
                          />
                          <div class="flex min-w-0 flex-1 flex-col overflow-hidden border-l border-line bg-surface/30">
                            <MarkdownPreview
                              content={file().content}
                              filePath={file().path}
                              laneId={lane()?.id}
                              nearestHeading={nearestHeading()}
                            />
                          </div>
                        </Show>
                      </Match>
                    </Switch>
                  </div>
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

            <Show when={selectionCount() > 1 || selectedChars() > 0}>
              <span class="text-line">|</span>
              <span>
                {selectionCount() > 1
                  ? `${selectionCount()} selections`
                  : `${selectedChars()} char${selectedChars() === 1 ? "" : "s"}`}
              </span>
            </Show>

            <span class="text-line">|</span>
            <span>{currentIndentUnit()}</span>

            <Show when={activeFile()?.large}>
              <span class="text-line">|</span>
              <span class="font-medium text-attention">Large file: read-only</span>
            </Show>
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

            <Show when={isMarkdownFile(activePath())}>
              <span class="text-line">|</span>
              <button
                type="button"
                class={`focus-ring rounded px-1.5 py-0.5 transition-colors ${
                  props.editor.markdownPreview()
                    ? "bg-accent/15 font-medium text-accent"
                    : "text-muted hover:text-foreground"
                }`}
                onClick={props.editor.toggleMarkdownPreview}
                title="Toggle markdown preview (Mod+Shift+V)"
              >
                Preview: {props.editor.markdownPreview() ? "On" : "Off"}
              </button>
            </Show>
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

      {/* Context Menu floating popup */}
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
              class="fixed z-50 min-w-[160px] rounded-lg border border-line bg-surface py-1 text-xs shadow-xl backdrop-blur"
              style={{
                left: `${Math.min(menu.x, window.innerWidth - 170)}px`,
                top: `${Math.min(menu.y, window.innerHeight - 200)}px`,
              }}
            >
              <button
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-foreground/90 hover:bg-raised hover:text-foreground"
                onClick={() => {
                  setContextMenu(null);
                  startInlineCreate(menu.path, menu.isDir, false, menu.depth);
                }}
              >
                New File
              </button>
              <button
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-foreground/90 hover:bg-raised hover:text-foreground"
                onClick={() => {
                  setContextMenu(null);
                  startInlineCreate(menu.path, menu.isDir, true, menu.depth);
                }}
              >
                New Folder
              </button>
              <div class="my-1 h-px bg-line/60" />
              <button
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-foreground/90 hover:bg-raised hover:text-foreground"
                onClick={() => {
                  setContextMenu(null);
                  setInlineRename({ path: menu.path, isDir: menu.isDir, name: menu.name });
                }}
              >
                Rename
              </button>
              <button
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-fault hover:bg-fault/15 hover:text-fault"
                onClick={() => {
                  setContextMenu(null);
                  setDeleteTarget({ path: menu.path, isDir: menu.isDir });
                }}
              >
                Delete
              </button>
              <div class="my-1 h-px bg-line/60" />
              <button
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-foreground/90 hover:bg-raised hover:text-foreground"
                onClick={() => {
                  setContextMenu(null);
                  void revealInFinder(menu.path);
                }}
              >
                Reveal in Finder
              </button>
              <button
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-foreground/90 hover:bg-raised hover:text-foreground"
                onClick={() => {
                  setContextMenu(null);
                  copyRelativePath(menu.path);
                }}
              >
                Copy Relative Path
              </button>
            </div>
          </>
        )}
      </Show>

      {/* Delete Confirmation Dialog */}
      <Show when={deleteTarget()} keyed>
        {(target) => (
          <ConfirmDialog
            options={{
              title: `Delete ${target.isDir ? "folder" : "file"}`,
              message: `Are you sure you want to delete "${target.path}"${target.isDir ? " and all its contents" : ""}? This cannot be undone.`,
              confirmLabel: "Delete",
              danger: true,
              onConfirm: () => {
                const t = target;
                setDeleteTarget(null);
                void commitDelete(t);
              },
            }}
            onClose={() => setDeleteTarget(null)}
          />
        )}
      </Show>
    </div>
  );
}
