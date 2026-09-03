import {
  createMemo,
  createRenderEffect,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";

import type { FileEntry, Lane } from "../bindings";
import { translateError, type TranslatedError } from "../ipc/errors";
import { DaemonRpcError, daemonCall, subscribeDaemon } from "../ipc/rpc";
import type { FleetStore } from "./fleet";

export interface OpenFile {
  path: string;
  content: string;
  savedContent: string;
  mtimeMs: number | null;
  size?: number;
  kind?: "text" | "binary" | "image" | string;
  cursor: number;
  scrollTop: number;
  loading: boolean;
  loadError: TranslatedError | null;
  saving: boolean;
  saveError: TranslatedError | null;
  conflict: FileConflict | null;
}

export interface FileConflict {
  deleted: boolean;
  actualMtimeMs: number | null;
}

export type DirCacheEntry =
  | { status: "loading" }
  | { status: "loaded"; entries: FileEntry[]; truncated: boolean }
  | { status: "error"; error: TranslatedError };

export interface LaneEditorState {
  openFiles: OpenFile[];
  activePath: string | null;
  expandedDirs: Set<string>;
  dirCache: Map<string, DirCacheEntry>;
}

interface PersistedLane {
  openPaths: string[];
  activePath: string | null;
  expandedDirs: string[];
  cursors?: Record<string, number>;
  scrollTops?: Record<string, number>;
}

interface PersistedEditorStorage {
  lanes: Record<string, PersistedLane>;
  treeColumnWidth?: number;
  wrap?: boolean;
  whitespace?: boolean;
}

export const EDITOR_STORAGE_KEY = "repomon.editor.v1";
export const DEFAULT_TREE_WIDTH_PX = 240;
export const MIN_TREE_WIDTH_PX = 180;

function readPersistedStorage(): PersistedEditorStorage {
  try {
    const raw = localStorage.getItem(EDITOR_STORAGE_KEY);
    if (!raw) return { lanes: {} };
    const parsed = JSON.parse(raw) as PersistedEditorStorage;
    return parsed && typeof parsed === "object" && typeof parsed.lanes === "object"
      ? parsed
      : { lanes: {} };
  } catch {
    return { lanes: {} };
  }
}

function writePersistedStorage(data: PersistedEditorStorage) {
  try {
    localStorage.setItem(EDITOR_STORAGE_KEY, JSON.stringify(data));
  } catch {}
}

export function createEditorStore(fleet: FleetStore) {
  const persisted = readPersistedStorage();

  const [treeColumnWidth, setTreeColumnWidthSignal] = createSignal<number>(
    typeof persisted.treeColumnWidth === "number" && persisted.treeColumnWidth >= MIN_TREE_WIDTH_PX
      ? persisted.treeColumnWidth
      : DEFAULT_TREE_WIDTH_PX,
  );
  const [wrap, setWrapSignal] = createSignal<boolean>(Boolean(persisted.wrap));
  const [whitespace, setWhitespaceSignal] = createSignal<boolean>(Boolean(persisted.whitespace));

  const [treeExpanded, setTreeExpandedSignal] = createSignal<boolean>(true);

  // In-memory per-lane state cache
  const laneStates = new Map<number, LaneEditorState>();

  // Current active lane state signals
  const [openFiles, setOpenFiles] = createSignal<OpenFile[]>([]);
  const [activePath, setActivePathSignal] = createSignal<string | null>(null);
  const [expandedDirs, setExpandedDirs] = createSignal<Set<string>>(new Set());
  const [dirCache, setDirCache] = createSignal<Map<string, DirCacheEntry>>(new Map());

  let activeLaneId: number | null = null;

  const selectedLane = createMemo<Lane | null>(() => {
    if (typeof fleet.selectedLane === "function") {
      return fleet.selectedLane();
    }
    const id = typeof fleet.selectedLaneId === "function" ? fleet.selectedLaneId() : null;
    if (id == null) return null;
    return fleet.lanes?.().find((l) => l.id === id) ?? null;
  });

  const currentLaneId = () => selectedLane()?.id ?? null;

  function persistCurrentLane() {
    const laneId = activeLaneId;
    if (laneId == null) return;
    const current = readPersistedStorage();
    const files = openFiles();
    const openPaths = files.map((f) => f.path);
    const cursors: Record<string, number> = {};
    const scrollTops: Record<string, number> = {};
    for (const f of files) {
      if (f.cursor > 0) cursors[f.path] = f.cursor;
      if (f.scrollTop > 0) scrollTops[f.path] = f.scrollTop;
    }

    current.lanes = current.lanes || {};
    current.lanes[String(laneId)] = {
      openPaths,
      activePath: activePath(),
      expandedDirs: Array.from(expandedDirs()),
      cursors,
      scrollTops,
    };
    current.treeColumnWidth = treeColumnWidth();
    current.wrap = wrap();
    current.whitespace = whitespace();
    writePersistedStorage(current);
  }

  function saveActiveLaneToMemory() {
    if (activeLaneId != null) {
      laneStates.set(activeLaneId, {
        openFiles: openFiles(),
        activePath: activePath(),
        expandedDirs: new Set(expandedDirs()),
        dirCache: new Map(dirCache()),
      });
      persistCurrentLane();
    }
  }

  // Updates the live signal only - safe to call on every `mousemove` of a column-resize drag.
  // Callers persist the final width once, via `persistTreeColumnWidth`, on `mouseup` - reading
  // and rewriting the whole localStorage blob on every mousemove event would otherwise thrash
  // storage dozens of times a second for the length of a single drag.
  function setTreeColumnWidth(width: number) {
    const clamped = Math.max(MIN_TREE_WIDTH_PX, width);
    setTreeColumnWidthSignal(clamped);
  }

  // Commits the current tree column width to localStorage. Call once, e.g. on drag `mouseup`,
  // not on every `setTreeColumnWidth` call.
  function persistTreeColumnWidth() {
    const current = readPersistedStorage();
    current.treeColumnWidth = treeColumnWidth();
    writePersistedStorage(current);
  }

  function setWrap(value: boolean | ((prev: boolean) => boolean)) {
    const next = typeof value === "function" ? value(wrap()) : value;
    setWrapSignal(next);
    const current = readPersistedStorage();
    current.wrap = next;
    writePersistedStorage(current);
  }

  function toggleWrap() {
    setWrap((v) => !v);
  }

  function setWhitespace(value: boolean | ((prev: boolean) => boolean)) {
    const next = typeof value === "function" ? value(whitespace()) : value;
    setWhitespaceSignal(next);
    const current = readPersistedStorage();
    current.whitespace = next;
    writePersistedStorage(current);
  }

  function toggleWhitespace() {
    setWhitespace((v) => !v);
  }

  function setTreeExpanded(value: boolean | ((prev: boolean) => boolean)) {
    setTreeExpandedSignal(value);
  }

  function findOpenFile(path: string): OpenFile | undefined {
    return openFiles().find((f) => f.path === path);
  }

  function updateOpenFile(path: string, updater: (file: OpenFile) => OpenFile) {
    setOpenFiles((files) => files.map((f) => (f.path === path ? updater(f) : f)));
    persistCurrentLane();
  }

  async function loadDir(laneId: number, dirPath: string) {
    setDirCache((cache) => {
      const next = new Map(cache);
      next.set(dirPath, { status: "loading" });
      return next;
    });
    try {
      const result = await daemonCall("file.list", { lane_id: laneId, path: dirPath });
      if (activeLaneId !== laneId) return;
      setDirCache((cache) => {
        const next = new Map(cache);
        next.set(dirPath, { status: "loaded", entries: result.entries, truncated: result.truncated });
        return next;
      });
    } catch (cause) {
      if (activeLaneId !== laneId) return;
      setDirCache((cache) => {
        const next = new Map(cache);
        next.set(dirPath, { status: "error", error: translateError(cause) });
        return next;
      });
    }
  }

  function toggleDir(path: string) {
    const laneId = currentLaneId();
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
    persistCurrentLane();
  }

  function expandDir(path: string) {
    const laneId = currentLaneId();
    if (laneId == null) return;
    setExpandedDirs((prev) => {
      if (prev.has(path)) return prev;
      const next = new Set(prev);
      next.add(path);
      if (!dirCache().has(path)) void loadDir(laneId, path);
      return next;
    });
    persistCurrentLane();
  }

  function collapseDir(path: string) {
    setExpandedDirs((prev) => {
      if (!prev.has(path)) return prev;
      const next = new Set(prev);
      next.delete(path);
      return next;
    });
    persistCurrentLane();
  }

  function refreshTree() {
    const laneId = currentLaneId();
    if (laneId == null) return;
    void loadDir(laneId, "");
    for (const path of expandedDirs()) void loadDir(laneId, path);
  }

  function revealFile(filePath: string) {
    const laneId = currentLaneId();
    if (laneId == null) return;
    const parts = filePath.split("/").slice(0, -1);
    let current = "";
    for (const part of parts) {
      current = current ? `${current}/${part}` : part;
      expandDir(current);
    }
  }

  const [languageOverrides, setLanguageOverrides] = createSignal<Record<string, string>>({});
  function setLanguageOverride(path: string, lang: string | null) {
    setLanguageOverrides((prev) => {
      const next = { ...prev };
      if (lang) next[path] = lang;
      else delete next[path];
      return next;
    });
  }

  function activateTab(path: string) {
    setActivePathSignal(path);
    setTreeExpandedSignal(false);
    persistCurrentLane();
    void syncExternalChange(path);
  }

  async function openFile(path: string, options?: { cursor?: number; scrollTop?: number }) {
    const existing = findOpenFile(path);
    if (existing) {
      if (options?.cursor !== undefined) {
        updateOpenFile(path, (f) => ({
          ...f,
          cursor: options.cursor ?? f.cursor,
          scrollTop: options.scrollTop ?? f.scrollTop,
        }));
      }
      activateTab(path);
      if (existing.mtimeMs == null && !existing.loading) {
        await reloadFile(path);
      }
      return;
    }

    const laneId = currentLaneId();
    if (laneId == null) return;
    setTreeExpandedSignal(false);
    const placeholder: OpenFile = {
      path,
      content: "",
      savedContent: "",
      mtimeMs: null,
      cursor: options?.cursor ?? 0,
      scrollTop: options?.scrollTop ?? 0,
      loading: true,
      loadError: null,
      saving: false,
      saveError: null,
      conflict: null,
    };
    setOpenFiles((files) => [...files, placeholder]);
    setActivePathSignal(path);
    persistCurrentLane();

    try {
      const result = await daemonCall("file.read", { lane_id: laneId, path });
      updateOpenFile(path, (f) => ({
        ...f,
        content: result.content,
        savedContent: result.content,
        mtimeMs: result.mtime_ms,
        size: result.size,
        kind: result.kind || "text",
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
      setActivePathSignal(neighbor ? neighbor.path : null);
    }
    persistCurrentLane();
  }

  function requestCloseFile(path: string): boolean {
    const f = findOpenFile(path);
    if (f && f.content !== f.savedContent) {
      return false;
    }
    closeFile(path);
    return true;
  }

  function updateContent(path: string, content: string) {
    updateOpenFile(path, (f) => ({ ...f, content }));
  }

  function updateCursor(path: string, cursor: number, scrollTop?: number) {
    updateOpenFile(path, (f) => ({
      ...f,
      cursor,
      scrollTop: scrollTop !== undefined ? scrollTop : f.scrollTop,
    }));
  }

  async function saveFile(path: string) {
    const laneId = currentLaneId();
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
      if (
        (cause instanceof DaemonRpcError || (cause && typeof cause === "object" && "code" in cause)) &&
        (cause as { code: number }).code === -32011
      ) {
        const data = (cause as { data?: unknown }).data as
          | { expected_mtime_ms?: number; actual_mtime_ms?: number | null }
          | null;
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
    const laneId = currentLaneId();
    if (laneId == null || !findOpenFile(path)) return;
    updateOpenFile(path, (f) => ({ ...f, loading: true, loadError: null }));
    try {
      const result = await daemonCall("file.read", { lane_id: laneId, path });
      updateOpenFile(path, (f) => ({
        ...f,
        content: result.content,
        savedContent: result.content,
        mtimeMs: result.mtime_ms,
        size: result.size,
        kind: result.kind || "text",
        loading: false,
        conflict: null,
        loadError: null,
      }));
    } catch (cause) {
      updateOpenFile(path, (f) => ({ ...f, loading: false, loadError: translateError(cause) }));
    }
  }

  async function keepMine(path: string) {
    const laneId = currentLaneId();
    if (laneId == null || !findOpenFile(path)) return;
    try {
      const result = await daemonCall("file.read", { lane_id: laneId, path });
      updateOpenFile(path, (f) => ({ ...f, mtimeMs: result.mtime_ms, conflict: null }));
    } catch {
      updateOpenFile(path, (f) => ({ ...f, conflict: { deleted: true, actualMtimeMs: null } }));
    }
  }

  async function saveAsNew(path: string) {
    const laneId = currentLaneId();
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

  async function syncExternalChange(path: string, specificLaneId?: number) {
    const laneId = specificLaneId ?? currentLaneId();
    if (laneId == null) return;
    const file = laneId === activeLaneId
      ? findOpenFile(path)
      : laneStates.get(laneId)?.openFiles.find((f) => f.path === path);
    if (!file) return;

    try {
      const result = await daemonCall("file.read", { lane_id: laneId, path });
      if (result.mtime_ms === file.mtimeMs) return;
      const isDirty = file.content !== file.savedContent;
      if (laneId === activeLaneId) {
        if (isDirty) {
          updateOpenFile(path, (f) => ({ ...f, conflict: { deleted: false, actualMtimeMs: result.mtime_ms } }));
        } else {
          updateOpenFile(path, (f) => ({
            ...f,
            content: result.content,
            savedContent: result.content,
            mtimeMs: result.mtime_ms,
            size: result.size,
            kind: result.kind || "text",
            conflict: null,
            loadError: null,
          }));
        }
      } else {
        const memState = laneStates.get(laneId);
        if (memState) {
          const target = memState.openFiles.find((f) => f.path === path);
          if (target) {
            if (isDirty) {
              target.conflict = { deleted: false, actualMtimeMs: result.mtime_ms };
            } else {
              target.content = result.content;
              target.savedContent = result.content;
              target.mtimeMs = result.mtime_ms;
              target.conflict = null;
              target.loadError = null;
            }
          }
        }
      }
    } catch {
      if (laneId === activeLaneId) {
        updateOpenFile(path, (f) => ({ ...f, conflict: { deleted: true, actualMtimeMs: null } }));
      } else {
        const memState = laneStates.get(laneId);
        if (memState) {
          const target = memState.openFiles.find((f) => f.path === path);
          if (target) {
            target.conflict = { deleted: true, actualMtimeMs: null };
          }
        }
      }
    }
  }

  function switchLane(newLaneId: number | null) {
    if (newLaneId === activeLaneId) return;
    saveActiveLaneToMemory();

    activeLaneId = newLaneId;
    if (newLaneId === null) {
      setOpenFiles([]);
      setActivePathSignal(null);
      setExpandedDirs(new Set<string>());
      setDirCache(new Map());
      return;
    }

    const cached = laneStates.get(newLaneId);
    if (cached) {
      setOpenFiles(cached.openFiles);
      setActivePathSignal(cached.activePath);
      setExpandedDirs(new Set<string>(cached.expandedDirs));
      setDirCache(new Map(cached.dirCache));
      return;
    }

    // Restore from localStorage if available
    const saved = readPersistedStorage().lanes?.[String(newLaneId)];
    const restoredExpanded = new Set<string>(saved?.expandedDirs ?? []);
    const restoredActive = saved?.activePath ?? null;
    const restoredOpenPaths = saved?.openPaths ?? [];
    const restoredCursors = saved?.cursors ?? {};
    const restoredScrollTops = saved?.scrollTops ?? {};

    const initialOpenFiles: OpenFile[] = restoredOpenPaths.map((p) => ({
      path: p,
      content: "",
      savedContent: "",
      mtimeMs: null,
      cursor: restoredCursors[p] ?? 0,
      scrollTop: restoredScrollTops[p] ?? 0,
      loading: p === restoredActive,
      loadError: null,
      saving: false,
      saveError: null,
      conflict: null,
    }));

    setOpenFiles(initialOpenFiles);
    setActivePathSignal(restoredActive);
    setExpandedDirs(restoredExpanded);
    setDirCache(new Map());

    // Eagerly load root directory and active file
    void loadDir(newLaneId, "");
    if (restoredActive) {
      void (async () => {
        try {
          const result = await daemonCall("file.read", { lane_id: newLaneId, path: restoredActive });
          updateOpenFile(restoredActive, (f) => ({
            ...f,
            content: result.content,
            savedContent: result.content,
            mtimeMs: result.mtime_ms,
            loading: false,
          }));
        } catch (cause) {
          updateOpenFile(restoredActive, (f) => ({ ...f, loading: false, loadError: translateError(cause) }));
        }
      })();
    }
  }

  // Initialize immediately
  switchLane(currentLaneId());

  // Sync with selected lane changes
  createRenderEffect(() => {
    const lane = selectedLane();
    const newLaneId = lane?.id ?? null;
    switchLane(newLaneId);
  });

  const [finderOpen, setFinderOpen] = createSignal(false);
  function openFinder() { setFinderOpen(true); }
  function closeFinder() { setFinderOpen(false); }
  const [openAtTarget, setOpenAtTarget] = createSignal<{
    path: string;
    line: number;
    column: number;
    token: number;
  } | null>(null);
  let openAtToken = 0;

  async function openAt(path: string, line: number, column: number) {
    const token = ++openAtToken;
    const target = { path, line, column, token };
    setOpenAtTarget(target);
    await openFile(path);
    // Refresh token after openFile ensures CodeEditor's effect fires even if doc was just created
    setOpenAtTarget({ ...target, token: ++openAtToken });
  }

  function handleFileRenamed(from: string, to: string) {
    const normFrom = from.trim().replace(/\\/g, "/");
    const normTo = to.trim().replace(/\\/g, "/");

    setOpenFiles((files) =>
      files.map((f) => {
        if (f.path === normFrom) {
          return { ...f, path: normTo };
        }
        if (f.path.startsWith(normFrom + "/")) {
          return { ...f, path: normTo + f.path.slice(normFrom.length) };
        }
        return f;
      })
    );

    const curActive = activePath();
    if (curActive === normFrom) {
      setActivePathSignal(normTo);
    } else if (curActive && curActive.startsWith(normFrom + "/")) {
      setActivePathSignal(normTo + curActive.slice(normFrom.length));
    }

    setExpandedDirs((prev) => {
      const next = new Set<string>();
      for (const dir of prev) {
        if (dir === normFrom) {
          next.add(normTo);
        } else if (dir.startsWith(normFrom + "/")) {
          next.add(normTo + dir.slice(normFrom.length));
        } else {
          next.add(dir);
        }
      }
      return next;
    });

    persistCurrentLane();
  }

  function handleFileDeleted(path: string) {
    const normPath = path.trim().replace(/\\/g, "/");
    setOpenFiles((files) =>
      files.map((f) => {
        if (f.path === normPath || f.path.startsWith(normPath + "/")) {
          return {
            ...f,
            conflict: { deleted: true, actualMtimeMs: null },
          };
        }
        return f;
      })
    );
    persistCurrentLane();
  }

  let pendingReloadDirs = new Set<string>();
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  function queueDirReload(laneId: number, dir: string) {
    pendingReloadDirs.add(dir);
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      const toReload = Array.from(pendingReloadDirs);
      pendingReloadDirs.clear();
      for (const d of toReload) {
        void loadDir(laneId, d);
      }
    }, 300);
  }

  onMount(() => {
    let active = true;
    let stop: (() => void) | undefined;
    void subscribeDaemon((event) => {
      if (!active || event.method !== "event.file.changed") return;
      const params = event.params as {
        lane_id?: number;
        path?: string;
        op?: "created" | "modified" | "removed" | "renamed";
        from?: string;
      } | null;
      if (!params || typeof params.lane_id !== "number" || typeof params.path !== "string") return;

      const laneId = params.lane_id;
      const path = params.path;
      const op = params.op;
      const from = params.from;

      // Reload affected directory levels
      const parentDir = path.split("/").slice(0, -1).join("/");
      queueDirReload(laneId, parentDir);

      if (op === "renamed" && from) {
        const fromParentDir = from.split("/").slice(0, -1).join("/");
        if (fromParentDir !== parentDir) {
          queueDirReload(laneId, fromParentDir);
        }
        handleFileRenamed(from, path);
      } else if (op === "removed") {
        handleFileDeleted(path);
      } else {
        void syncExternalChange(path, laneId);
      }
    })
      .then((unsub) => {
        if (active) stop = unsub;
        else unsub();
      })
      .catch(() => undefined);

    onCleanup(() => {
      active = false;
      if (debounceTimer) clearTimeout(debounceTimer);
      stop?.();
    });
  });

  const activeFile = createMemo(() => {
    const p = activePath();
    return p ? openFiles().find((f) => f.path === p) ?? null : null;
  });

  return {
    currentLaneId,
    selectedLane,
    openFiles,
    activePath,
    activeFile,
    expandedDirs,
    dirCache,
    treeColumnWidth,
    setTreeColumnWidth,
    persistTreeColumnWidth,
    wrap,
    setWrap,
    toggleWrap,
    whitespace,
    setWhitespace,
    toggleWhitespace,
    treeExpanded,
    setTreeExpanded,
    openFile,
    closeFile,
    requestCloseFile,
    activateTab,
    updateContent,
    updateCursor,
    saveFile,
    reloadFile,
    keepMine,
    saveAsNew,
    syncExternalChange,
    loadDir,
    toggleDir,
    expandDir,
    collapseDir,
    refreshTree,
    revealFile,
    languageOverrides,
    setLanguageOverride,
    openAt,
    openAtTarget,
    finderOpen,
    openFinder,
    closeFinder,
    handleFileRenamed,
    handleFileDeleted,
    getLaneState: (id: number) => laneStates.get(id),
  };
}

export type EditorStore = ReturnType<typeof createEditorStore>;
