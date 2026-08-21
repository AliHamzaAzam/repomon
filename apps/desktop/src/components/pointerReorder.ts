import { reorderAround } from "./ordering";

/// Chrome-style single-axis drag-to-reorder, hand-rolled on pointer events.
///
/// Native HTML5 DnD can't feel like Chrome's tabs: dragover arrives at low frequency, the ghost
/// is a static snapshot, and neighbors snap instead of sliding. This primitive replaces that
/// interaction layer (the array math stays in `reorderAround`):
///
/// - `pointerdown` arms a drag; once the cursor moves past a small threshold the element is
///   captured and follows the cursor every animation frame via a direct CSS transform — no
///   per-pixel framework renders.
/// - When the cursor crosses an adjacent sibling's midpoint, the order swaps immediately and the
///   displaced siblings animate to their new slots via FLIP (invert → play), which is most of
///   what makes Chrome dragging feel smooth.
/// - The dragged element's transform is compensated after each swap so its visual position never
///   jumps when its layout slot changes underneath it.
/// - `pointerup` commits the final order exactly once; a drag that never crossed the threshold
///   leaves clicks untouched.
export interface PointerReorderOptions<T extends string | number> {
  /** Axis the items are laid out along. */
  axis: "x" | "y";
  /** The live item order (accessor so swaps read fresh). */
  ids: () => T[];
  /** Apply an intermediate order while dragging (drives rendering). */
  reorder: (order: T[]) => void;
  /** Commit the final order on release. Called only when the order actually changed. */
  commit: (order: T[]) => void;
  /** Whether a given id participates in dragging (shells/placeholders opt out). */
  enabled: (id: T) => boolean;
}

const DRAG_THRESHOLD_PX = 4;

interface ActiveDrag<T> {
  id: T;
  el: HTMLElement;
  startX: number;
  startY: number;
  /** Last raw cursor position (drives per-frame deltas). */
  lastX: number;
  lastY: number;
  active: boolean;
  /** Set when a real drag happened, so the trailing click can be swallowed. */
  suppressClick: boolean;
  // Order at activation; release only commits when this actually changed.
  initialOrder: T[];
  cleanup: () => void;
}

export function createPointerReorder<T extends string | number>(
  options: PointerReorderOptions<T>,
) {
  let drag: ActiveDrag<T> | null = null;

  /// The dragged element's visual top-left, kept stable across layout-slot changes.
  let visualX = 0;
  let visualY = 0;

  const coord = (e: { clientX: number; clientY: number }) =>
    options.axis === "x" ? e.clientX : e.clientY;

  function applyDraggedTransform() {
    if (!drag) return;
    const rect = drag.el.getBoundingClientRect();
    const dx = visualX - rect.left;
    const dy = visualY - rect.top;
    drag.el.style.transform = `translate3d(${dx}px, ${dy}px, 0)`;
  }

  function setVisualFromCursor(e: { clientX: number; clientY: number }) {
    if (!drag) return;
    // visual tracks the cursor minus the grab offset within the element.
    visualX += e.clientX - (drag.lastX ?? e.clientX);
    visualY += e.clientY - (drag.lastY ?? e.clientY);
    drag.lastX = e.clientX;
    drag.lastY = e.clientY;
  }

  /// FLIP the siblings (not the dragged one): invert their post-swap positions, then play them
  /// back to zero over a short transition.
  function flipSiblings(container: HTMLElement, draggedId: T) {
    const items = Array.from(
      container.querySelectorAll<HTMLElement>("[data-reorder-id]"),
    );
    for (const el of items) {
      if (el.dataset.reorderId === String(draggedId)) continue;
      const current = el.dataset.flipFrom;
      if (!current) continue;
      delete el.dataset.flipFrom;
      const [fromLeft, fromTop] = current.split(",").map(Number);
      const now = el.getBoundingClientRect();
      const dx = fromLeft - now.left;
      const dy = fromTop - now.top;
      if (dx === 0 && dy === 0) continue;
      el.style.transition = "none";
      el.style.transform = `translate3d(${dx}px, ${dy}px, 0)`;
      requestAnimationFrame(() => {
        el.style.transition = "transform 150ms ease";
        el.style.transform = "";
        const clear = () => {
          el.style.transition = "";
          el.removeEventListener("transitionend", clear);
        };
        el.addEventListener("transitionend", clear);
        // Safety: transitions may not fire in tests/headless; clear anyway.
        setTimeout(clear, 200);
      });
    }
  }

  function rememberRects(container: HTMLElement) {
    for (const el of container.querySelectorAll<HTMLElement>("[data-reorder-id]")) {
      const rect = el.getBoundingClientRect();
      el.dataset.flipFrom = `${rect.left},${rect.top}`;
    }
  }

  function begin(event: PointerEvent, id: T) {
    // Only the primary button drags: pointerdown fires for every mouse button, and a
    // right-click that drifts past the threshold would otherwise arm a real drag and race the
    // context menu's rename flow. Real pointer events always carry `button`; treat `undefined`
    // (synthetic/test environments) as primary.
    if (event.button !== 0 && event.button !== undefined) return;
    if (drag || !options.enabled(id)) return;
    const target = event.currentTarget as HTMLElement | null;
    if (!target) return;
    const container = target.closest("[data-reorder-container]");
    if (!container) return;

    drag = {
      id,
      el: target,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      active: false,
      suppressClick: false,
      initialOrder: options.ids(),
      cleanup: () => {},
    };
    const rect = target.getBoundingClientRect();
    visualX = rect.left;
    visualY = rect.top;

    const onMove = (e: PointerEvent) => tick(e);
    const onUp = (e: PointerEvent) => finish(e);
    const onCancel = () => abort();
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp, { once: true });
    window.addEventListener("pointercancel", onCancel, { once: true });
    drag.cleanup = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
    };
  }

  let rafPending = false;
  let lastRawEvent: { clientX: number; clientY: number } | null = null;

  function tick(e: PointerEvent) {
    if (!drag) return;
    lastRawEvent = e;
    if (!drag.active) {
      const moved = Math.max(
        Math.abs(e.clientX - drag.startX),
        Math.abs(e.clientY - drag.startY),
      );
      if (moved < DRAG_THRESHOLD_PX) return;
      drag.active = true;
      drag.suppressClick = true;
      try {
        drag.el.setPointerCapture?.(e.pointerId);
      } catch {
        /* jsdom/tests have no capture; window listeners still drive the drag */
      }
      drag.el.style.zIndex = "50";
      drag.el.style.cursor = "grabbing";
      drag.el.style.willChange = "transform";
    }
    if (rafPending) return;
    rafPending = true;
    requestAnimationFrame(() => {
      rafPending = false;
      if (!drag || !lastRawEvent) return;
      setVisualFromCursor(lastRawEvent);
      applyDraggedTransform();
      maybeSwap();
    });
  }

  /// Move the dragged item to the slot under the cursor, then FLIP the others out of the way.
  ///
  /// The target slot is resolved directly from the cursor position against every sibling's
  /// midpoint — not by stepping one adjacent swap at a time — so a single fast pointermove that
  /// flicks across several items lands in the right slot in one tick. (Adjacent-only evaluation
  /// desynced the committed array from the visual position when a jump crossed more than one
  /// midpoint between two paint frames.)
  function maybeSwap() {
    if (!drag || !drag.active) return;
    const container = drag.el.closest("[data-reorder-container]") as HTMLElement | null;
    if (!container) return;
    const ids = options.ids();
    const index = ids.indexOf(drag.id);
    if (index < 0 || ids.length < 2) return;
    const cursor = coord({
      clientX: drag.lastX ?? 0,
      clientY: drag.lastY ?? 0,
    });

    let passed = 0;
    for (let i = 0; i < ids.length; i++) {
      if (i === index) continue;
      const el = container.querySelector<HTMLElement>(
        `[data-reorder-id="${CSS.escape(String(ids[i]))}"]`,
      );
      if (!el) continue;
      const rect = el.getBoundingClientRect();
      const midpoint =
        options.axis === "x" ? rect.left + rect.width / 2 : rect.top + rect.height / 2;
      if (cursor > midpoint) passed += 1;
    }
    // The cursor sits in the `passed`-th gap (skipping the dragged element's own slot).
    const targetIndex = Math.min(passed, ids.length - 1);

    const next = reorderAround(ids, drag.id, ids[targetIndex], targetIndex > index);
    if (!next) return;
    rememberRects(container);
    options.reorder(next as T[]);
    // Solid re-renders synchronously on signal writes, so rects measured here are post-swap.
    flipSiblings(container, drag.id);
    // The dragged element's untransformed slot just moved; keep its visual position continuous.
    applyDraggedTransform();
  }

  function settle() {
    if (!drag) return;
    drag.el.style.transform = "";
    drag.el.style.zIndex = "";
    drag.el.style.cursor = "";
    drag.el.style.willChange = "";
  }

  function finish(_e?: PointerEvent) {
    if (!drag) return;
    const wasActive = drag.active;
    const initialOrder = drag.initialOrder;
    drag.cleanup();
    settle();
    drag = null;
    // Commit only when the live order actually diverged from the initial one — a drag that
    // never crossed a midpoint has nothing to persist.
    if (
      wasActive &&
      (() => {
        const now = options.ids();
        return now.length !== initialOrder.length || now.some((id, i) => id !== initialOrder[i]);
      })()
    ) {
      options.commit(options.ids());
    }
  }

  function abort() {
    if (!drag) return;
    // Snap back visually by clearing transforms; the order stays wherever swaps left it —
    // matching native DnD semantics where a cancel mid-drop keeps the last arrangement only if
    // committed. We choose the simpler contract: cancel commits nothing beyond live swaps.
    drag.cleanup();
    settle();
    drag = null;
  }

  return {
    /// Spread onto each reorderable item element.
    itemHandlers(id: T) {
      return {
        "data-reorder-id": String(id),
        onPointerDown: (event: PointerEvent) => begin(event, id),
        // A real drag ends with a synthetic click on the item; swallow exactly that one.
        onClickCapture: (event: MouseEvent) => {
          if (drag?.suppressClick) {
            drag.suppressClick = false;
            event.preventDefault();
            event.stopPropagation();
          }
        },
      };
    },
    /// True while a drag is in flight (exposed for tests and aria states).
    isDragging: () => drag !== null,
    /// Tear down an in-flight drag without committing — call from `onCleanup` so a surface that
    /// unmounts mid-drag (lane switch, popover close) never leaves window listeners or a
    /// captured pointer behind, and never commits against gone state.
    abort,
  };
}
