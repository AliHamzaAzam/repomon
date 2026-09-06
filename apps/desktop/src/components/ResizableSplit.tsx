import { createEffect, createSignal, onCleanup, type JSX } from "solid-js";

import { notifyLayoutChanged } from "../stores/uiSettings";

/** Identifies the panel side controlled by a divider that reports width without wrapping the parent
 * grid’s columns. */

export type ResizableSplitPanelSide = "before" | "after";

export interface ResizableSplitProps {
  /** localStorage key the resolved width (px) is persisted under. Scope per-usage. */
  storageKey: string;
  /** Width restored/reset to when no valid stored value exists, or on double-click reset. */
  defaultWidth: number;
  /** Inclusive clamp, px. */
  minWidth: number;
  /** Inclusive clamp, px. */
  maxWidth: number;
  /** Selects which side grows when dragging, defaulting to the panel after the handle. */
  panelSide?: ResizableSplitPanelSide;
  /** Keyboard step size in px. Defaults to 16. */
  step?: number;
  /** CSS custom property name written on every width change. Defaults to "--split-width". */
  cssVar?: string;
  /** Selects the CSS-variable target, defaulting to the document root so ancestor layout rules can
   * read the width. */
  target?: HTMLElement | (() => HTMLElement | null);
  /** Required - a screen-reader label naming what this handle resizes (e.g. "Resize Repomind panel"). */
  label: string;
  /** Called synchronously on every resolved width change: initial restore, drag, keys, reset. */
  onWidthChange?: (widthPx: number) => void;
  class?: string;
}

const DEFAULT_CSS_VAR = "--split-width";
const DEFAULT_STEP = 16;
const PERSIST_DEBOUNCE_MS = 150;

function clamp(value: number, min: number, max: number): number {
  if (max < min) return min;
  return Math.min(max, Math.max(min, value));
}

function readStoredWidth(storageKey: string, min: number, max: number, fallback: number): number {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return clamp(fallback, min, max);
  }
  const raw = localStorage.getItem(storageKey);
  if (raw === null) return clamp(fallback, min, max);
  const parsed = Number(raw);
  if (!Number.isFinite(parsed)) return clamp(fallback, min, max);
  // Stale persisted values (from a previous min/max range, or corrupted storage) must not break
  // layout - always clamp against the current bounds.
  return clamp(parsed, min, max);
}

export function ResizableSplit(props: ResizableSplitProps): JSX.Element {
  const panelSide = () => props.panelSide ?? "after";
  const step = () => props.step ?? DEFAULT_STEP;
  const cssVar = () => props.cssVar ?? DEFAULT_CSS_VAR;

  const [width, setWidth] = createSignal(
    readStoredWidth(props.storageKey, props.minWidth, props.maxWidth, props.defaultWidth),
  );
  const [dragging, setDragging] = createSignal(false);

  let handleRef: HTMLDivElement | undefined;
  let persistTimer: ReturnType<typeof setTimeout> | undefined;

  function resolveTarget(): HTMLElement | null {
    if (props.target) {
      return typeof props.target === "function" ? props.target() : props.target;
    }
    return typeof document !== "undefined" ? document.documentElement : null;
  }

  function schedulePersist(value: number) {
    if (typeof window === "undefined" || typeof localStorage === "undefined") return;
    if (persistTimer !== undefined) clearTimeout(persistTimer);
    persistTimer = setTimeout(() => {
      persistTimer = undefined;
      try {
        localStorage.setItem(props.storageKey, String(value));
      } catch {
        // localStorage can throw (quota, private mode) — persistence is best-effort.
      }
    }, PERSIST_DEBOUNCE_MS);
  }

  // Notify every width change, including restoration; terminal resizing already coalesces these
  // events.
  createEffect(() => {
    const w = width();
    const varName = cssVar();
    const valuePx = `${w}px`;
    handleRef?.style.setProperty(varName, valuePx);
    const targetEl = resolveTarget();
    if (targetEl && targetEl !== handleRef) targetEl.style.setProperty(varName, valuePx);
    props.onWidthChange?.(w);
    notifyLayoutChanged();
    schedulePersist(w);
  });

  onCleanup(() => {
    if (persistTimer !== undefined) clearTimeout(persistTimer);
    const varName = cssVar();
    const targetEl = resolveTarget();
    targetEl?.style.removeProperty(varName);
  });

  function applyWidth(next: number) {
    setWidth(clamp(next, props.minWidth, props.maxWidth));
  }

  let dragStartClientX = 0;
  let dragStartWidth = 0;

  const handlePointerDown: JSX.EventHandler<HTMLDivElement, PointerEvent> = (event) => {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    dragStartClientX = event.clientX;
    dragStartWidth = width();
    setDragging(true);
    event.currentTarget.setPointerCapture?.(event.pointerId);
    event.preventDefault();
  };

  const handlePointerMove: JSX.EventHandler<HTMLDivElement, PointerEvent> = (event) => {
    if (!dragging()) return;
    const dx = event.clientX - dragStartClientX;
    const delta = panelSide() === "after" ? -dx : dx;
    applyWidth(dragStartWidth + delta);
  };

  const endDrag: JSX.EventHandler<HTMLDivElement, PointerEvent> = (event) => {
    if (!dragging()) return;
    setDragging(false);
    event.currentTarget.releasePointerCapture?.(event.pointerId);
  };

  const handleDoubleClick = () => {
    applyWidth(props.defaultWidth);
  };

  const handleKeyDown: JSX.EventHandler<HTMLDivElement, KeyboardEvent> = (event) => {
    const growKey = panelSide() === "after" ? "ArrowLeft" : "ArrowRight";
    const shrinkKey = panelSide() === "after" ? "ArrowRight" : "ArrowLeft";
    if (event.key === growKey) {
      event.preventDefault();
      applyWidth(width() + step());
    } else if (event.key === shrinkKey) {
      event.preventDefault();
      applyWidth(width() - step());
    } else if (event.key === "Home") {
      event.preventDefault();
      applyWidth(props.minWidth);
    } else if (event.key === "End") {
      event.preventDefault();
      applyWidth(props.maxWidth);
    }
  };

  return (
    <div
      ref={handleRef}
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      aria-valuenow={Math.round(width())}
      aria-valuemin={props.minWidth}
      aria-valuemax={props.maxWidth}
      tabIndex={0}
      class={`resizable-split-handle ${dragging() ? "is-dragging" : ""} ${props.class ?? ""}`}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDblClick={handleDoubleClick}
      onKeyDown={handleKeyDown}
    />
  );
}
