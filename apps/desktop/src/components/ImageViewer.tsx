import { formatBytes } from "../formatBytes";
import { Show, createEffect, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { openPath } from "@tauri-apps/plugin-opener";
import { convertFileSrc } from "@tauri-apps/api/core";

import { ensureWorktreeAssetsAllowed } from "../ipc/assets";
import { daemonCall } from "../ipc/rpc";
import { translateError } from "../ipc/errors";
import { IconActualSize, IconExternalLink, IconFitFrame, IconZoomIn, IconZoomOut } from "./icons";

export interface ImageViewerState {
  format: string;
  width: number | null;
  height: number | null;
  sizeBytes: number;
  zoomPercent: number;
  animated: boolean;
}

export interface ImageViewerProps {
  worktreeRoot: string;
  laneId: number;
  path: string;
  size?: number;
  // Lets the host (EditorWorkspace's status line) show format, dimensions, size, and zoom
  // without owning any image-loading state itself. Called with `null` while there is nothing
  // meaningful to show (loading, error, too-large, or on unmount) - same contract as PdfViewer.
  onStateChange?: (state: ImageViewerState | null) => void;
}

// A 1x1 transparent placeholder so the persistent <img> element (see the stage markup below)
// never sits with an empty/missing `src` while a real one is pending - `bindImageHandlers`
// overwrites this with the real asset URL once the load token is confirmed current.
const TRANSPARENT_PIXEL = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==";

type Status = "granting" | "loading" | "loaded" | "error" | "too-large";
type ZoomMode = "fit" | "actual" | "custom";

const MIN_SCALE = 0.1;
const MAX_SCALE = 8;
const ZOOM_STEP = 1.25;
const PIXELATED_THRESHOLD = 2;
const TOO_LARGE_BYTES = 50 * 1024 * 1024;
// Padding around the image inside the stage, subtracted out of the fit calculation so a fitted
// image never butts up against the pane edges (mirrors PdfViewer's PAGE_GUTTER).
const STAGE_GUTTER = 32;
const RESIZE_DEBOUNCE_MS = 100;


function basename(path: string): string {
  return path.split("/").pop() || path;
}

function extensionLabel(path: string): string {
  const ext = path.split(".").pop();
  return ext && ext !== path ? ext.toUpperCase() : "IMAGE";
}

function isGifPath(path: string): boolean {
  return path.toLowerCase().endsWith(".gif");
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || target.isContentEditable;
}

export default function ImageViewer(props: ImageViewerProps): JSX.Element {
  const [status, setStatus] = createSignal<Status>("granting");
  const [errorMessage, setErrorMessage] = createSignal("");
  const [dimensions, setDimensions] = createSignal<{ width: number; height: number } | null>(null);
  const [fileSize, setFileSize] = createSignal<number>(props.size ?? 0);
  const [zoomMode, setZoomMode] = createSignal<ZoomMode>("fit");
  const [customScale, setCustomScale] = createSignal(1);
  const [fitScale, setFitScale] = createSignal(1);
  const [isPanning, setIsPanning] = createSignal(false);
  const [stageSize, setStageSize] = createSignal({ width: 0, height: 0 });

  let loadGeneration = 0;
  let usedFallback = false;
  let rootEl: HTMLDivElement | undefined;
  let stageEl: HTMLDivElement | undefined;
  let imgEl: HTMLImageElement | undefined;
  let resizeObserver: ResizeObserver | undefined;
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;

  const absolutePath = createMemo(() => `${props.worktreeRoot}/${props.path}`);

  const effectiveScale = createMemo(() => {
    const mode = zoomMode();
    if (mode === "fit") return fitScale();
    if (mode === "actual") return 1;
    return customScale();
  });

  function reportState() {
    if (status() !== "loaded") {
      props.onStateChange?.(null);
      return;
    }
    const dims = dimensions();
    props.onStateChange?.({
      format: extensionLabel(props.path),
      width: dims?.width ?? null,
      height: dims?.height ?? null,
      sizeBytes: fileSize(),
      zoomPercent: Math.round(effectiveScale() * 100),
      animated: isGifPath(props.path),
    });
  }

  function computeFitScale(): number {
    const dims = dimensions();
    if (!dims || !stageEl) return 1;
    const availWidth = Math.max(stageEl.clientWidth - STAGE_GUTTER, 1);
    const availHeight = Math.max(stageEl.clientHeight - STAGE_GUTTER, 1);
    const scale = Math.min(availWidth / dims.width, availHeight / dims.height, 1);
    return clamp(scale, MIN_SCALE, MAX_SCALE);
  }

  function applyFitScale() {
    setFitScale(computeFitScale());
  }

  function measureStage() {
    if (!stageEl) return;
    setStageSize({ width: stageEl.clientWidth, height: stageEl.clientHeight });
  }

  // Binds fresh onload/onerror handlers straight onto the persistent <img> element rather than
  // through JSX props: assigning `.src` supersedes any in-flight request for the previous src
  // (the browser never fires load/error against an element for a resource it no longer points
  // at), and closing over `token` here - captured at the moment this exact src was set - is what
  // lets a handler tell whether it is still the current load when it eventually fires.
  function bindImageHandlers(token: number, url: string) {
    const img = imgEl;
    if (!img) return;
    img.onload = () => {
      if (loadGeneration !== token) return;
      setDimensions({ width: img.naturalWidth, height: img.naturalHeight });
      applyFitScale();
      setStatus("loaded");
    };
    img.onerror = () => {
      if (loadGeneration !== token) return;
      if (!usedFallback) {
        usedFallback = true;
        void loadFallback(token);
        return;
      }
      setStatus("error");
      setErrorMessage("This image could not be displayed here.");
    };
    img.src = url;
  }

  async function loadFallback(token: number) {
    try {
      const res = await daemonCall("file.read_raw", { lane_id: props.laneId, path: props.path });
      if (loadGeneration !== token) return;
      setFileSize(res.size);
      bindImageHandlers(token, `data:${res.mime};base64,${res.base64}`);
    } catch (cause) {
      if (loadGeneration !== token) return;
      setStatus("error");
      setErrorMessage(translateError(cause).friendly);
    }
  }

  async function load() {
    const token = ++loadGeneration;
    usedFallback = false;
    setStatus("granting");
    setErrorMessage("");
    setDimensions(null);
    setZoomMode("fit");
    setCustomScale(1);
    setFileSize(props.size ?? 0);

    const knownSize = props.size ?? 0;
    if (knownSize > TOO_LARGE_BYTES) {
      setStatus("too-large");
      return;
    }

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
    bindImageHandlers(token, convertFileSrc(absolutePath()));
  }

  onMount(() => {
    rootEl?.focus();
  });

  createEffect(() => {
    // Initial run covers the mount load; re-runs cover a file switch.
    void props.worktreeRoot;
    void props.path;
    void props.laneId;
    void load();
  });

  createEffect(reportState);

  onMount(() => {
    if (!stageEl) return;
    measureStage();
    if (typeof ResizeObserver === "undefined") return;
    resizeObserver = new ResizeObserver(() => {
      measureStage();
      if (resizeTimer !== undefined) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        resizeTimer = undefined;
        if (zoomMode() === "fit") applyFitScale();
      }, RESIZE_DEBOUNCE_MS);
    });
    resizeObserver.observe(stageEl);
    onCleanup(() => resizeObserver?.disconnect());
  });

  onCleanup(() => {
    if (resizeTimer !== undefined) clearTimeout(resizeTimer);
    if (imgEl) {
      imgEl.onload = null;
      imgEl.onerror = null;
    }
    props.onStateChange?.(null);
  });

  function zoomIn() {
    setCustomScale(clamp(effectiveScale() * ZOOM_STEP, MIN_SCALE, MAX_SCALE));
    setZoomMode("custom");
  }

  function zoomOut() {
    setCustomScale(clamp(effectiveScale() / ZOOM_STEP, MIN_SCALE, MAX_SCALE));
    setZoomMode("custom");
  }

  function setFit() {
    setZoomMode("fit");
  }

  function setActual() {
    setZoomMode("actual");
  }

  function toggleFitActual() {
    setZoomMode((mode) => (mode === "actual" ? "fit" : "actual"));
  }

  function onWheel(e: WheelEvent) {
    if (!(e.ctrlKey || e.metaKey)) return;
    const stage = stageEl;
    const dims = dimensions();
    if (!stage || !dims) return;
    e.preventDefault();
    const rect = stage.getBoundingClientRect();
    const pointerX = e.clientX - rect.left + stage.scrollLeft;
    const pointerY = e.clientY - rect.top + stage.scrollTop;
    const prevScale = effectiveScale();
    const factor = Math.exp(-e.deltaY * 0.0025);
    const nextScale = clamp(prevScale * factor, MIN_SCALE, MAX_SCALE);
    if (nextScale === prevScale) return;
    const ratio = nextScale / prevScale;
    setCustomScale(nextScale);
    setZoomMode("custom");
    queueMicrotask(() => {
      if (!stageEl) return;
      stageEl.scrollLeft = pointerX * ratio - (e.clientX - rect.left);
      stageEl.scrollTop = pointerY * ratio - (e.clientY - rect.top);
    });
  }

  function onStageMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    const stage = stageEl;
    if (!stage) return;
    const scrollable = stage.scrollWidth > stage.clientWidth + 1 || stage.scrollHeight > stage.clientHeight + 1;
    if (!scrollable) return;
    e.preventDefault();
    const startX = e.clientX;
    const startY = e.clientY;
    const startLeft = stage.scrollLeft;
    const startTop = stage.scrollTop;
    setIsPanning(true);

    const onMove = (moveEvent: MouseEvent) => {
      stage.scrollLeft = startLeft - (moveEvent.clientX - startX);
      stage.scrollTop = startTop - (moveEvent.clientY - startY);
    };
    const onUp = () => {
      setIsPanning(false);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  function onRootKeyDown(e: KeyboardEvent) {
    if (isEditableTarget(e.target)) return;
    const mod = e.metaKey || e.ctrlKey;
    if (!mod) return;
    if (e.key === "=" || e.key === "+") {
      e.preventDefault();
      zoomIn();
    } else if (e.key === "-" || e.key === "_") {
      e.preventDefault();
      zoomOut();
    } else if (e.key === "0") {
      e.preventDefault();
      setFit();
    } else if (e.key === "1") {
      e.preventDefault();
      setActual();
    }
  }

  async function openExternally() {
    try {
      await openPath(absolutePath());
    } catch (err) {
      console.warn("openPath failed:", err);
    }
  }

  const isOnlyWayToView = () => status() === "error" || status() === "too-large";
  const pixelated = () => effectiveScale() > PIXELATED_THRESHOLD;
  const canPan = createMemo(() => {
    const dims = dimensions();
    if (!dims) return false;
    const scale = effectiveScale();
    const size = stageSize();
    return dims.width * scale > size.width || dims.height * scale > size.height;
  });
  const stageCursor = () => {
    if (isPanning()) return "cursor-grabbing";
    return canPan() ? "cursor-grab" : "cursor-default";
  };

  return (
    <div
      ref={rootEl}
      data-testid="image-viewer-root"
      tabIndex={-1}
      class="flex h-full w-full min-h-0 min-w-0 flex-col bg-background select-none focus:outline-none"
      onKeyDown={onRootKeyDown}
    >
      {/* Toolbar */}
      <div class="flex min-h-9 shrink-0 flex-wrap items-center gap-2 border-b border-line bg-surface/95 px-3 py-1.5">
        <div class="flex min-w-0 max-w-full items-center gap-2 font-mono text-[11px] text-muted">
          <span class="min-w-0 max-w-[220px] truncate font-medium text-foreground" title={props.path}>{basename(props.path)}</span>
          <Show when={status() === "loaded" && dimensions()}>
            <span class="shrink-0 text-line">|</span>
            <span class="shrink-0">
              {dimensions()!.width} x {dimensions()!.height}
            </span>
          </Show>
          <Show when={fileSize() > 0}>
            <span class="shrink-0 text-line">|</span>
            <span class="shrink-0">{formatBytes(fileSize())}</span>
          </Show>
        </div>

        <Show when={status() === "loaded"}>
          <div class="ml-auto flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2 font-mono text-[11px] text-muted">
            <div class="flex items-center gap-1">
              <button
                type="button"
                aria-label="Zoom out"
                title="Zoom out (Mod+-)"
                class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={effectiveScale() <= MIN_SCALE}
                onClick={zoomOut}
              >
                <IconZoomOut size={13} />
              </button>
              <button
                type="button"
                title="Reset zoom to fit"
                class="focus-ring w-11 rounded px-1 py-0.5 text-center text-muted hover:bg-raised hover:text-foreground"
                onClick={setFit}
              >
                {Math.round(effectiveScale() * 100)}%
              </button>
              <button
                type="button"
                aria-label="Zoom in"
                title="Zoom in (Mod+=)"
                class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={effectiveScale() >= MAX_SCALE}
                onClick={zoomIn}
              >
                <IconZoomIn size={13} />
              </button>
            </div>

            <span class="text-line">|</span>

            <div class="flex items-center gap-0.5">
              <button
                type="button"
                aria-label="Fit to pane"
                aria-pressed={zoomMode() === "fit"}
                title="Fit to pane (Mod+0)"
                class={`focus-ring flex size-6 items-center justify-center rounded ${
                  zoomMode() === "fit" ? "bg-signal/15 text-signal" : "text-muted hover:bg-raised hover:text-foreground"
                }`}
                onClick={setFit}
              >
                <IconFitFrame size={13} />
              </button>
              <button
                type="button"
                aria-label="Actual size"
                aria-pressed={zoomMode() === "actual"}
                title="Actual size, 100% (Mod+1)"
                class={`focus-ring flex size-6 items-center justify-center rounded ${
                  zoomMode() === "actual" ? "bg-signal/15 text-signal" : "text-muted hover:bg-raised hover:text-foreground"
                }`}
                onClick={setActual}
              >
                <IconActualSize size={13} />
              </button>
            </div>
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

      {/* Stage */}
      <div class="relative min-h-0 min-w-0 flex-1 overflow-hidden">
        <Show when={status() === "granting" || status() === "loading"}>
          <div class="flex h-full items-center justify-center overflow-auto p-4">
            <div
              class="animate-pulse rounded border border-line bg-raised/60 shadow-[0_1px_3px_var(--shadow)]"
              style={{ width: "min(60vw, 420px)", height: "min(70vh, 420px)" }}
            />
          </div>
        </Show>

        <Show when={status() === "error" || status() === "too-large"}>
          <div class="flex h-full flex-col items-center justify-center gap-1.5 p-6 text-center">
            <p class="text-xs font-medium text-fault">
              {status() === "too-large" ? "Too large to preview here" : "Couldn't load preview"}
            </p>
            <p class="max-w-xs text-xs text-muted">
              {status() === "too-large"
                ? `This image is over ${formatBytes(TOO_LARGE_BYTES)}. Use Open in system viewer above.`
                : errorMessage() || "This image could not be displayed here. Use Open in system viewer above."}
            </p>
          </div>
        </Show>

        {/* The image element stays mounted across every non-terminal status so its onload/onerror
            handlers (bound imperatively in bindImageHandlers, not via JSX props - see the request
            token guard there) can fire the loading -> loaded transition; it is only made visible
            once loaded. */}
        <div
          ref={stageEl}
          data-testid="image-stage"
          class={`h-full w-full overflow-auto ${stageCursor()}`}
          classList={{ hidden: status() !== "loaded" && status() !== "loading" }}
          style={{
            "background-color": "var(--background)",
            "background-image": "radial-gradient(var(--line) 1px, transparent 1px)",
            "background-size": "14px 14px",
          }}
          onWheel={onWheel}
          onMouseDown={onStageMouseDown}
          onDblClick={toggleFitActual}
        >
          <div class="flex min-h-full min-w-full items-center justify-center p-4">
            <img
              ref={imgEl}
              data-testid="image-viewer-img"
              alt={basename(props.path)}
              src={TRANSPARENT_PIXEL}
              class={`block max-w-none max-h-none rounded shadow-[0_1px_3px_var(--shadow)] ${
                status() === "loaded" ? "" : "invisible"
              }`}
              style={{
                width: dimensions() ? `${dimensions()!.width * effectiveScale()}px` : undefined,
                height: dimensions() ? `${dimensions()!.height * effectiveScale()}px` : undefined,
                "image-rendering": pixelated() ? "pixelated" : "auto",
              }}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
