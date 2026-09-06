import { formatBytes } from "../formatBytes";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  type JSX,
} from "solid-js";
import { openPath } from "@tauri-apps/plugin-opener";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  GlobalWorkerOptions,
  getDocument,
  type PDFDocumentProxy,
  type PDFPageProxy,
  type RenderTask,
} from "pdfjs-dist";
// Vite's `?url` import turns the worker module into a fingerprinted asset URL and bundles it as
// its own chunk, so pdf.js's parsing/rendering work runs off the main thread without fetching
// anything from a CDN.
import pdfWorkerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

import { ensureWorktreeAssetsAllowed } from "../ipc/assets";
import {
  IconChevronLeft,
  IconChevronRight,
  IconClose,
  IconExternalLink,
  IconFitPage,
  IconFitWidth,
  IconSearch,
  IconZoomIn,
  IconZoomOut,
} from "./icons";

GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

export interface PdfViewerState {
  numPages: number;
  zoomPercent: number;
}

export interface PdfViewerProps {
  worktreeRoot: string;
  path: string;
  size?: number;
  // Lets the host (EditorWorkspace's status line) show page count and zoom without owning any
  // pdf.js state itself. Called with `null` while there is nothing meaningful to show (loading,
  // error, password, or on unmount).
  onStateChange?: (state: PdfViewerState | null) => void;
}

type Status = "granting" | "loading" | "loaded" | "error" | "password";
type FitMode = "width" | "page" | "custom";

interface TextLayoutItem {
  str: string;
  // Store geometry at scale one so zoom multiplies a stable page layout.
  left: number;
  top: number;
  width: number;
  fontHeight: number;
}

// Marked-content entries lack str and must be skipped when building text geometry.
interface PdfTextItem {
  str?: string;
  transform: number[];
  width: number;
}

interface PageMatch {
  itemIndex: number;
  charStart: number;
  charEnd: number;
}

interface GlobalMatch extends PageMatch {
  page: number;
  globalIndex: number;
}

const MIN_SCALE = 0.25;
const MAX_SCALE = 4;
const ZOOM_STEP = 1.15;
const RESIZE_DEBOUNCE_MS = 100;
// One viewport height of slack above and below the scroll port before a page is torn down -
// scrolling stays smooth because the next page is already painted by the time it comes into view.
const VIRTUALIZE_MARGIN = "100% 0px 100% 0px";
// The padding around the page stack (see the scroll container's `p-4`), subtracted out of fit
// calculations so a fit-width/fit-page page doesn't butt up against the pane edges.
const PAGE_GUTTER = 32;

function basename(path: string): string {
  return path.split("/").pop() || path;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || target.isContentEditable;
}

// Compose viewport and text-item transforms using pdf.js’s matrix convention.
function combineTransforms(m1: number[], m2: number[]): number[] {
  return [
    m1[0] * m2[0] + m1[2] * m2[1],
    m1[1] * m2[0] + m1[3] * m2[1],
    m1[0] * m2[2] + m1[2] * m2[3],
    m1[1] * m2[2] + m1[3] * m2[3],
    m1[0] * m2[4] + m1[2] * m2[5] + m1[4],
    m1[1] * m2[4] + m1[3] * m2[5] + m1[5],
  ];
}

function layoutTextItems(items: PdfTextItem[], baseViewportTransform: number[]): TextLayoutItem[] {
  const layout: TextLayoutItem[] = [];
  for (const item of items) {
    if (!item.str) continue;
    const tx = combineTransforms(baseViewportTransform, item.transform);
    const fontHeight = Math.max(Math.hypot(tx[2], tx[3]), 1);
    const scaleX = Math.hypot(tx[0], tx[1]) || fontHeight;
    layout.push({
      str: item.str,
      left: tx[4],
      top: tx[5] - fontHeight,
      width: Math.max(item.width * scaleX, 1),
      fontHeight,
    });
  }
  return layout;
}

function findMatchesInItems(items: TextLayoutItem[], query: string): PageMatch[] {
  if (!query) return [];
  const lowerQuery = query.toLowerCase();
  const out: PageMatch[] = [];
  items.forEach((item, itemIndex) => {
    const lower = item.str.toLowerCase();
    let from = 0;
    for (;;) {
      const idx = lower.indexOf(lowerQuery, from);
      if (idx === -1) break;
      out.push({ itemIndex, charStart: idx, charEnd: idx + lowerQuery.length });
      from = idx + lowerQuery.length;
    }
  });
  return out;
}

interface PdfPageSlotProps {
  pageNumber: number;
  scale: number;
  estimatedWidth: number;
  estimatedHeight: number;
  scrollRoot: HTMLDivElement | undefined;
  getPage: (pageNumber: number) => Promise<PDFPageProxy>;
  getPageText: (pageNumber: number) => Promise<TextLayoutItem[]>;
  registerRef: (pageNumber: number, el: HTMLDivElement | null) => void;
  onIntersect: (pageNumber: number, isIntersecting: boolean, ratio: number) => void;
  matches: () => Array<PageMatch & { active: boolean }>;
}

// Keep offscreen pages as sized placeholders so virtualization preserves scroll geometry.
function PdfPageSlot(props: PdfPageSlotProps): JSX.Element {
  let containerEl: HTMLDivElement | undefined;
  let canvasEl: HTMLCanvasElement | undefined;
  let renderTask: RenderTask | undefined;
  let renderGeneration = 0;
  let disposed = false;

  const [visible, setVisible] = createSignal(false);
  const [size, setSize] = createSignal({ width: props.estimatedWidth, height: props.estimatedHeight });
  const [textItems, setTextItems] = createSignal<TextLayoutItem[]>([]);

  onMount(() => {
    props.registerRef(props.pageNumber, containerEl ?? null);
    if (typeof IntersectionObserver === "undefined" || !containerEl) {
      // No observer support (or nothing to observe against): render unconditionally rather than
      // leave the page permanently blank.
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          setVisible(entry.isIntersecting);
          props.onIntersect(props.pageNumber, entry.isIntersecting, entry.intersectionRatio);
        }
      },
      { root: props.scrollRoot ?? null, rootMargin: VIRTUALIZE_MARGIN },
    );
    observer.observe(containerEl);
    onCleanup(() => observer.disconnect());
  });

  onCleanup(() => {
    disposed = true;
    props.registerRef(props.pageNumber, null);
    renderTask?.cancel();
  });

  createEffect(() => {
    const scale = props.scale;
    if (!visible()) {
      renderTask?.cancel();
      renderTask = undefined;
      return;
    }
    const myGeneration = ++renderGeneration;
    void (async () => {
      try {
        const page = await props.getPage(props.pageNumber);
        if (disposed || myGeneration !== renderGeneration) return;
        const viewport = page.getViewport({ scale });
        setSize({ width: viewport.width, height: viewport.height });
        if (!canvasEl) return;
        const dpr = (typeof window !== "undefined" && window.devicePixelRatio) || 1;
        canvasEl.width = Math.max(1, Math.ceil(viewport.width * dpr));
        canvasEl.height = Math.max(1, Math.ceil(viewport.height * dpr));
        canvasEl.style.width = `${viewport.width}px`;
        canvasEl.style.height = `${viewport.height}px`;
        // jsdom's canvas has no 2D backend (getContext returns null in tests); real webviews
        // always hand back a live context, so this cast only matters for the test environment,
        // where the render call below is a mock that never touches it.
        const ctx = canvasEl.getContext("2d") as CanvasRenderingContext2D;
        const task = page.render({
          canvasContext: ctx,
          viewport,
          transform: dpr !== 1 ? [dpr, 0, 0, dpr, 0, 0] : undefined,
        });
        renderTask = task;
        await task.promise;
        if (disposed || myGeneration !== renderGeneration) return;
        const items = await props.getPageText(props.pageNumber);
        if (disposed || myGeneration !== renderGeneration) return;
        setTextItems(items);
      } catch {
        // Cancelled render tasks reject by design (RenderingCancelledException) whenever the page
        // scrolls out of view or the scale changes again mid-paint - nothing to surface for that.
      }
    })();
  });

  const scaledItems = createMemo(() => {
    const scale = props.scale;
    return textItems().map((item) => ({
      str: item.str,
      left: item.left * scale,
      top: item.top * scale,
      width: item.width * scale,
      fontHeight: item.fontHeight * scale,
    }));
  });

  const highlightBoxes = createMemo(() => {
    const items = scaledItems();
    return props.matches().flatMap((match) => {
      const item = items[match.itemIndex];
      if (!item || item.str.length === 0) return [];
      const startFrac = match.charStart / item.str.length;
      const endFrac = match.charEnd / item.str.length;
      return [
        {
          left: item.left + startFrac * item.width,
          top: item.top,
          width: Math.max((endFrac - startFrac) * item.width, 1),
          height: item.fontHeight,
          active: match.active,
        },
      ];
    });
  });

  return (
    <div
      ref={containerEl}
      data-pdf-page={props.pageNumber}
      data-testid={`pdf-page-${props.pageNumber}`}
      class="relative shrink-0 overflow-hidden rounded border border-line bg-surface shadow-[0_1px_3px_var(--shadow)]"
      style={{ width: `${size().width}px`, height: `${size().height}px` }}
    >
      <Show when={visible()}>
        <canvas ref={canvasEl} class="block" />
        <div class="pdf-text-layer">
          <For each={scaledItems()}>
            {(item) => (
              <span
                class="pdf-text-span"
                style={{
                  left: `${item.left}px`,
                  top: `${item.top}px`,
                  "font-size": `${item.fontHeight}px`,
                  width: `${item.width}px`,
                }}
              >
                {item.str}
              </span>
            )}
          </For>
        </div>
        <div class="pointer-events-none absolute inset-0" aria-hidden="true">
          <For each={highlightBoxes()}>
            {(box) => (
              <mark
                class="pdf-find-mark"
                classList={{ "is-active": box.active }}
                style={{
                  left: `${box.left}px`,
                  top: `${box.top}px`,
                  width: `${box.width}px`,
                  height: `${box.height}px`,
                }}
              />
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

export default function PdfViewer(props: PdfViewerProps): JSX.Element {
  const [status, setStatus] = createSignal<Status>("granting");
  const [errorMessage, setErrorMessage] = createSignal<string>("");
  const [numPages, setNumPages] = createSignal(0);
  // Use a plain generation counter for async guards; reading and writing a tracked signal here
  // would make loading retrigger itself.
  let loadGeneration = 0;
  const [loadToken, setLoadToken] = createSignal(0);
  const [scale, setScale] = createSignal(1);
  const [fitMode, setFitMode] = createSignal<FitMode>("width");
  const [currentPage, setCurrentPage] = createSignal(1);
  const [pageDraft, setPageDraft] = createSignal("1");
  const [pageInputFocused, setPageInputFocused] = createSignal(false);
  const [baseSize, setBaseSize] = createSignal<{ width: number; height: number } | null>(null);
  const [pageTextCache, setPageTextCache] = createSignal<Map<number, TextLayoutItem[]>>(new Map());
  const [findOpen, setFindOpen] = createSignal(false);
  const [findQuery, setFindQuery] = createSignal("");
  const [activeMatchIndex, setActiveMatchIndex] = createSignal(0);

  const absolutePath = createMemo(() => `${props.worktreeRoot}/${props.path}`);

  let doc: PDFDocumentProxy | undefined;
  let pageCache = new Map<number, Promise<PDFPageProxy>>();
  let pendingLoadingTask: { destroy: () => Promise<void> } | undefined;
  let scrollContainerEl: HTMLDivElement | undefined;
  let rootEl: HTMLDivElement | undefined;
  let findInputEl: HTMLInputElement | undefined;
  const pageRefs = new Map<number, HTMLDivElement>();
  const visibilityRatios = new Map<number, number>();
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  let resizeObserver: ResizeObserver | undefined;

  function reportState() {
    if (status() !== "loaded") {
      props.onStateChange?.(null);
      return;
    }
    props.onStateChange?.({ numPages: numPages(), zoomPercent: Math.round(scale() * 100) });
  }

  function getPage(pageNumber: number): Promise<PDFPageProxy> {
    let cached = pageCache.get(pageNumber);
    if (!cached) {
      if (!doc) return Promise.reject(new Error("no document loaded"));
      cached = doc.getPage(pageNumber);
      pageCache.set(pageNumber, cached);
    }
    return cached;
  }

  async function getPageText(pageNumber: number): Promise<TextLayoutItem[]> {
    const existing = pageTextCache().get(pageNumber);
    if (existing) return existing;
    const page = await getPage(pageNumber);
    const content = await page.getTextContent();
    const viewport = page.getViewport({ scale: 1 });
    const items = layoutTextItems(content.items as PdfTextItem[], viewport.transform);
    setPageTextCache((prev) => {
      const next = new Map(prev);
      next.set(pageNumber, items);
      return next;
    });
    return items;
  }

  function ensureAllPagesTextFetched() {
    const total = numPages();
    for (let n = 1; n <= total; n++) {
      if (!pageTextCache().has(n)) void getPageText(n).catch(() => {});
    }
  }

  const allMatches = createMemo<GlobalMatch[]>(() => {
    const query = findQuery().trim();
    if (!query) return [];
    const cache = pageTextCache();
    const out: GlobalMatch[] = [];
    for (let page = 1; page <= numPages(); page++) {
      const items = cache.get(page);
      if (!items) continue;
      for (const match of findMatchesInItems(items, query)) {
        out.push({ page, globalIndex: out.length, ...match });
      }
    }
    return out;
  });

  function matchesForPage(pageNumber: number) {
    return () => {
      const active = activeMatchIndex();
      return allMatches()
        .filter((m) => m.page === pageNumber)
        .map((m) => ({ ...m, active: m.globalIndex === active }));
    };
  }

  function resetDocumentState() {
    doc = undefined;
    pageCache = new Map();
    pageRefs.clear();
    visibilityRatios.clear();
    setNumPages(0);
    setBaseSize(null);
    setPageTextCache(new Map());
    setFindOpen(false);
    setFindQuery("");
    setActiveMatchIndex(0);
    setCurrentPage(1);
    setPageDraft("1");
    setFitMode("width");
  }

  async function load() {
    const token = ++loadGeneration;
    setLoadToken(token);
    if (pendingLoadingTask) {
      void pendingLoadingTask.destroy().catch(() => {});
      pendingLoadingTask = undefined;
    }
    if (doc) {
      void doc.destroy().catch(() => {});
    }
    resetDocumentState();
    setStatus("granting");
    setErrorMessage("");

    try {
      await ensureWorktreeAssetsAllowed(props.worktreeRoot);
    } catch {
      if (loadGeneration !== token) return;
      setStatus("error");
      setErrorMessage("Couldn't access this file.");
      return;
    }
    if (loadGeneration !== token) return;

    setStatus("loading");
    try {
      const response = await fetch(convertFileSrc(absolutePath()));
      const data = await response.arrayBuffer();
      if (loadGeneration !== token) return;

      const loadingTask = getDocument({ data });
      pendingLoadingTask = loadingTask;
      const loadedDoc = await loadingTask.promise;
      if (loadGeneration !== token) {
        void loadedDoc.destroy().catch(() => {});
        return;
      }
      pendingLoadingTask = undefined;
      doc = loadedDoc;
      setNumPages(loadedDoc.numPages);

      const firstPage = await getPage(1);
      if (loadGeneration !== token) return;
      const viewport = firstPage.getViewport({ scale: 1 });
      setBaseSize({ width: viewport.width, height: viewport.height });
      applyFitMode();
      setStatus("loaded");
    } catch (err) {
      if (loadGeneration !== token) return;
      const name = (err as { name?: string } | undefined)?.name;
      if (name === "PasswordException") {
        setStatus("password");
      } else {
        setStatus("error");
        setErrorMessage("This PDF could not be displayed here.");
      }
    }
  }

  onMount(() => {
    if (rootEl) rootEl.focus();
  });

  createEffect(() => {
    // Initial run covers the mount load; re-runs cover a file switch.
    void props.worktreeRoot;
    void props.path;
    void load();
  });

  createEffect(reportState);

  onCleanup(() => {
    if (pendingLoadingTask) void pendingLoadingTask.destroy().catch(() => {});
    if (doc) void doc.destroy().catch(() => {});
    if (resizeTimer !== undefined) clearTimeout(resizeTimer);
    resizeObserver?.disconnect();
    props.onStateChange?.(null);
  });

  function computeFitScale(mode: "width" | "page"): number | null {
    const base = baseSize();
    if (!base || !scrollContainerEl) return null;
    const availableWidth = Math.max(scrollContainerEl.clientWidth - PAGE_GUTTER, 1);
    const widthScale = availableWidth / base.width;
    if (mode === "width") return clamp(widthScale, MIN_SCALE, MAX_SCALE);
    const availableHeight = Math.max(scrollContainerEl.clientHeight - PAGE_GUTTER, 1);
    const heightScale = availableHeight / base.height;
    return clamp(Math.min(widthScale, heightScale), MIN_SCALE, MAX_SCALE);
  }

  function applyFitMode() {
    const mode = fitMode();
    if (mode === "custom") return;
    const next = computeFitScale(mode);
    if (next !== null) setScale(next);
  }

  function setFitWidth() {
    setFitMode("width");
    applyFitMode();
  }

  function setFitPage() {
    setFitMode("page");
    applyFitMode();
  }

  function zoomIn() {
    setFitMode("custom");
    setScale((s) => clamp(s * ZOOM_STEP, MIN_SCALE, MAX_SCALE));
  }

  function zoomOut() {
    setFitMode("custom");
    setScale((s) => clamp(s / ZOOM_STEP, MIN_SCALE, MAX_SCALE));
  }

  function resetZoom() {
    setFitMode("custom");
    setScale(1);
  }

  onMount(() => {
    if (typeof ResizeObserver === "undefined" || !scrollContainerEl) return;
    resizeObserver = new ResizeObserver(() => {
      if (resizeTimer !== undefined) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        resizeTimer = undefined;
        applyFitMode();
      }, RESIZE_DEBOUNCE_MS);
    });
    resizeObserver.observe(scrollContainerEl);
    onCleanup(() => resizeObserver?.disconnect());
  });

  function registerPageRef(pageNumber: number, el: HTMLDivElement | null) {
    if (el) pageRefs.set(pageNumber, el);
    else pageRefs.delete(pageNumber);
  }

  function goToPage(pageNumber: number) {
    const total = numPages();
    if (total === 0) return;
    const clamped = clamp(Math.round(pageNumber), 1, total);
    const el = pageRefs.get(clamped);
    el?.scrollIntoView?.({ block: "start", behavior: "auto" });
    setCurrentPage(clamped);
    setPageDraft(String(clamped));
  }

  function onIntersect(pageNumber: number, isIntersecting: boolean, ratio: number) {
    if (isIntersecting) visibilityRatios.set(pageNumber, ratio);
    else visibilityRatios.delete(pageNumber);
    if (visibilityRatios.size === 0) return;
    let best = pageNumber;
    let bestRatio = -1;
    for (const [page, r] of visibilityRatios) {
      if (r > bestRatio || (r === bestRatio && page < best)) {
        best = page;
        bestRatio = r;
      }
    }
    setCurrentPage(best);
    if (!pageInputFocused()) setPageDraft(String(best));
  }

  function commitPageDraft() {
    const parsed = Number.parseInt(pageDraft(), 10);
    if (Number.isFinite(parsed)) goToPage(parsed);
    else setPageDraft(String(currentPage()));
    setPageInputFocused(false);
  }

  function openFind() {
    setFindOpen(true);
    ensureAllPagesTextFetched();
    setTimeout(() => findInputEl?.focus(), 0);
  }

  function closeFind() {
    setFindOpen(false);
    setFindQuery("");
    setActiveMatchIndex(0);
    rootEl?.focus();
  }

  function stepMatch(direction: 1 | -1) {
    const total = allMatches().length;
    if (total === 0) return;
    const next = (activeMatchIndex() + direction + total) % total;
    setActiveMatchIndex(next);
    const match = allMatches()[next];
    if (match) pageRefs.get(match.page)?.scrollIntoView?.({ block: "center", behavior: "auto" });
  }

  createEffect(() => {
    // Re-derive matches whenever the query changes and land on the first hit.
    findQuery();
    setActiveMatchIndex(0);
  });

  function onRootKeyDown(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === "f") {
      e.preventDefault();
      openFind();
      return;
    }
    if (mod && (e.key === "=" || e.key === "+")) {
      e.preventDefault();
      zoomIn();
      return;
    }
    if (mod && (e.key === "-" || e.key === "_")) {
      e.preventDefault();
      zoomOut();
      return;
    }
    if (mod && e.key === "0") {
      e.preventDefault();
      resetZoom();
      return;
    }
    if (isEditableTarget(e.target)) return;
    if (e.key === "PageDown") {
      e.preventDefault();
      goToPage(currentPage() + 1);
    } else if (e.key === "PageUp") {
      e.preventDefault();
      goToPage(currentPage() - 1);
    } else if (e.key === "Home") {
      e.preventDefault();
      goToPage(1);
    } else if (e.key === "End") {
      e.preventDefault();
      goToPage(numPages());
    }
  }

  async function openExternally() {
    try {
      await openPath(absolutePath());
    } catch (err) {
      console.warn("openPath failed:", err);
    }
  }

  const isOnlyWayToView = () => status() === "error" || status() === "password";
  const pageNumbers = createMemo(() => {
    const token = loadToken();
    return Array.from({ length: numPages() }, (_, i) => ({ key: `${token}-${i + 1}`, page: i + 1 }));
  });
  const totalMatches = () => allMatches().length;

  return (
    <div
      ref={rootEl}
      data-testid="pdf-viewer-root"
      tabIndex={-1}
      class="flex h-full w-full min-h-0 min-w-0 flex-col bg-background select-none focus:outline-none"
      onKeyDown={onRootKeyDown}
    >

      <div class="flex min-h-9 shrink-0 flex-wrap items-center gap-2 border-b border-line bg-surface/95 px-3 py-1.5">
        <div class="flex min-w-0 max-w-full items-center gap-2 font-mono text-[11px] text-muted">
          <span class="min-w-0 max-w-[220px] truncate font-medium text-foreground" title={props.path}>{basename(props.path)}</span>
          <Show when={props.size !== undefined && props.size > 0}>
            <span class="shrink-0 text-line">|</span>
            <span class="shrink-0">{formatBytes(props.size)}</span>
          </Show>
        </div>

        <Show when={status() === "loaded"}>
          <div class="ml-auto flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2 font-mono text-[11px] text-muted">
            <div class="flex items-center gap-1">
              <button
                type="button"
                aria-label="Previous page"
                title="Previous page"
                class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={currentPage() <= 1}
                onClick={() => goToPage(currentPage() - 1)}
              >
                <IconChevronLeft size={13} />
              </button>
              <input
                type="text"
                inputmode="numeric"
                aria-label="Page number"
                class="focus-ring w-8 rounded border border-line bg-background px-1 py-0.5 text-center text-foreground"
                value={pageDraft()}
                onFocus={() => setPageInputFocused(true)}
                onInput={(e) => setPageDraft(e.currentTarget.value)}
                onBlur={commitPageDraft}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    commitPageDraft();
                  } else if (e.key === "Escape") {
                    setPageDraft(String(currentPage()));
                    e.currentTarget.blur();
                  }
                }}
              />
              <span class="shrink-0">of {numPages()}</span>
              <button
                type="button"
                aria-label="Next page"
                title="Next page"
                class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={currentPage() >= numPages()}
                onClick={() => goToPage(currentPage() + 1)}
              >
                <IconChevronRight size={13} />
              </button>
            </div>

            <span class="text-line">|</span>

            <div class="flex items-center gap-1">
              <button
                type="button"
                aria-label="Zoom out"
                title="Zoom out (Mod+-)"
                class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={scale() <= MIN_SCALE}
                onClick={zoomOut}
              >
                <IconZoomOut size={13} />
              </button>
              <button
                type="button"
                title="Reset zoom to 100%"
                class="focus-ring w-11 rounded px-1 py-0.5 text-center text-muted hover:bg-raised hover:text-foreground"
                onClick={resetZoom}
              >
                {Math.round(scale() * 100)}%
              </button>
              <button
                type="button"
                aria-label="Zoom in"
                title="Zoom in (Mod+=)"
                class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={scale() >= MAX_SCALE}
                onClick={zoomIn}
              >
                <IconZoomIn size={13} />
              </button>
            </div>

            <span class="text-line">|</span>

            <div class="flex items-center gap-0.5">
              <button
                type="button"
                aria-label="Fit width"
                aria-pressed={fitMode() === "width"}
                title="Fit width"
                class={`focus-ring flex size-6 items-center justify-center rounded ${
                  fitMode() === "width" ? "bg-signal/15 text-signal" : "text-muted hover:bg-raised hover:text-foreground"
                }`}
                onClick={setFitWidth}
              >
                <IconFitWidth size={13} />
              </button>
              <button
                type="button"
                aria-label="Fit page"
                aria-pressed={fitMode() === "page"}
                title="Fit page"
                class={`focus-ring flex size-6 items-center justify-center rounded ${
                  fitMode() === "page" ? "bg-signal/15 text-signal" : "text-muted hover:bg-raised hover:text-foreground"
                }`}
                onClick={setFitPage}
              >
                <IconFitPage size={13} />
              </button>
            </div>

            <span class="text-line">|</span>

            <button
              type="button"
              aria-label="Find in document"
              title="Find in document (Mod+F)"
              class={`focus-ring flex size-6 items-center justify-center rounded ${
                findOpen() ? "bg-signal/15 text-signal" : "text-muted hover:bg-raised hover:text-foreground"
              }`}
              onClick={() => (findOpen() ? closeFind() : openFind())}
            >
              <IconSearch size={13} />
            </button>
          </div>
        </Show>

        <button
          type="button"
          class={
            isOnlyWayToView()
              ? "ml-auto shrink-0 focus-ring rounded bg-signal px-2.5 py-1 text-[11px] font-semibold text-background transition-colors hover:bg-signal/90"
              : `shrink-0 focus-ring rounded border border-line bg-surface px-2 py-0.5 text-[11px] font-medium text-muted transition-colors hover:border-line/80 hover:text-foreground ${
                  status() === "loaded" ? "" : "ml-auto"
                }`
          }
          onClick={() => void openExternally()}
        >
          <span class="inline-flex items-center gap-1">
            <IconExternalLink size={11} />
            Open in system viewer
          </span>
        </button>
      </div>

      <div class="relative min-h-0 min-w-0 flex-1 overflow-hidden">
        <Show when={status() === "granting" || status() === "loading"}>
          <div class="flex h-full items-center justify-center overflow-auto p-4">
            <div
              class="animate-pulse rounded border border-line bg-raised/60 shadow-[0_1px_3px_var(--shadow)]"
              style={{ width: "min(70vw, 560px)", height: "min(80vh, 720px)" }}
            />
          </div>
        </Show>

        <Show when={status() === "error" || status() === "password"}>
          <div class="flex h-full flex-col items-center justify-center gap-1.5 p-6 text-center">
            <p class="text-xs font-medium text-fault">
              {status() === "password" ? "This PDF is password protected" : "Couldn't load preview"}
            </p>
            <p class="max-w-xs text-xs text-muted">
              {status() === "password"
                ? "Repomon doesn't prompt for PDF passwords. Use Open in system viewer above."
                : errorMessage() || "This PDF could not be displayed here. Use Open in system viewer above."}
            </p>
          </div>
        </Show>

        <Show when={status() === "loaded"}>
          <div
            ref={scrollContainerEl}
            data-testid="pdf-scroll-container"
            class="flex h-full min-h-0 min-w-0 flex-col items-center gap-3 overflow-y-auto overflow-x-hidden p-4"
          >
            <For each={pageNumbers()}>
              {(item) => (
                <PdfPageSlot
                  pageNumber={item.page}
                  scale={scale()}
                  estimatedWidth={(baseSize()?.width ?? 612) * scale()}
                  estimatedHeight={(baseSize()?.height ?? 792) * scale()}
                  scrollRoot={scrollContainerEl}
                  getPage={getPage}
                  getPageText={getPageText}
                  registerRef={registerPageRef}
                  onIntersect={onIntersect}
                  matches={matchesForPage(item.page)}
                />
              )}
            </For>
          </div>

          <Show when={findOpen()}>
            <div class="absolute top-2 right-2 left-2 z-20 ml-auto flex max-w-sm items-center gap-1.5 rounded-lg border border-line bg-surface/95 px-2 py-1.5 font-mono text-[11px] shadow-lg backdrop-blur">
              <IconSearch size={12} class="shrink-0 text-muted" />
              <input
                ref={findInputEl}
                type="text"
                placeholder="Find in document"
                aria-label="Find in document"
                class="focus-ring min-w-0 flex-1 rounded border border-line bg-background px-1.5 py-0.5 text-foreground"
                value={findQuery()}
                onInput={(e) => setFindQuery(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    stepMatch(e.shiftKey ? -1 : 1);
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    closeFind();
                  }
                }}
              />
              <span class="w-16 shrink-0 text-muted">
                {findQuery().trim() === ""
                  ? ""
                  : totalMatches() === 0
                    ? "No results"
                    : `${activeMatchIndex() + 1} of ${totalMatches()}`}
              </span>
              <button
                type="button"
                aria-label="Previous match"
                title="Previous match (Shift+Enter)"
                class="focus-ring flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={totalMatches() === 0}
                onClick={() => stepMatch(-1)}
              >
                <IconChevronLeft size={11} />
              </button>
              <button
                type="button"
                aria-label="Next match"
                title="Next match (Enter)"
                class="focus-ring flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={totalMatches() === 0}
                onClick={() => stepMatch(1)}
              >
                <IconChevronRight size={11} />
              </button>
              <button
                type="button"
                aria-label="Close find bar"
                title="Close (Esc)"
                class="focus-ring flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                onClick={closeFind}
              >
                <IconClose size={10} />
              </button>
            </div>
          </Show>
        </Show>
      </div>
    </div>
  );
}
