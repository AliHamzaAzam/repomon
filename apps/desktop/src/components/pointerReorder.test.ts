import { describe, expect, it, vi } from "vitest";

import { createPointerReorder } from "./pointerReorder";

/// Minimal DOM harness: a horizontal container of pill divs wired through the primitive, the
/// same shape both call sites (tab strip, roster popover) use.
function harness(order: string[], enabled: (id: string) => boolean = () => true) {
  const container = document.createElement("div");
  container.setAttribute("data-reorder-container", "");
  document.body.appendChild(container);
  const commit = vi.fn();
  const drag = createPointerReorder<string>({
    axis: "x",
    ids: () => [...order],
    reorder: (next) => {
      order.splice(0, order.length, ...next);
      render();
    },
    commit,
    enabled,
  });

  function render() {
    for (const el of Array.from(container.querySelectorAll("[data-reorder-id]"))) el.remove();
    for (const id of order) {
      const el = document.createElement("div");
      el.dataset.reorderId = id;
      Object.defineProperty(el, "getBoundingClientRect", {
        value: () => new DOMRect(order.indexOf(id) * 100, 0, 100, 30),
      });
      // Swap-detection reads the layout box (offsetLeft/offsetWidth), not getBoundingClientRect
      // — see pointerReorder.ts's maybeSwap doc comment — so it stays correct while a sibling's
      // getBoundingClientRect is mid-FLIP-transition. Keep these in lockstep with the rect above.
      Object.defineProperty(el, "offsetLeft", { value: order.indexOf(id) * 100, configurable: true });
      Object.defineProperty(el, "offsetTop", { value: 0, configurable: true });
      Object.defineProperty(el, "offsetWidth", { value: 100, configurable: true });
      Object.defineProperty(el, "offsetHeight", { value: 30, configurable: true });
      // Solid spreads itemHandlers onto the element; simulate that with a native listener.
      const handlers = drag.itemHandlers(id);
      el.addEventListener("pointerdown", (e) =>
        handlers.onPointerDown(e as PointerEvent),
      );
      container.appendChild(el);
    }
  }
  render();

  const pointer = (type: string, x: number, y: number, el: Element) =>
    el.dispatchEvent(new MouseEvent(type, { bubbles: true, clientX: x, clientY: y }));

  return {
    container,
    commit,
    pill: (id: string) => container.querySelector(`[data-reorder-id="${id}"]`) as HTMLElement,
    move: (x: number, y: number) => pointer("pointermove", x, y, container),
    up: (x: number, y: number) => pointer("pointerup", x, y, container),
    cancel: () => pointer("pointercancel", 0, 0, container),
    renderedOrder: () =>
      [...container.querySelectorAll("[data-reorder-id]")].map((el) =>
        el.getAttribute("data-reorder-id"),
      ),
  };
}

const flushFrames = () =>
  new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));

describe("createPointerReorder", () => {
  it("swaps with the adjacent sibling when the cursor crosses its midpoint", async () => {
    const h = harness(["a", "b", "c"]);
    // Pill "c" occupies [200,300); grab it at x=250 and drag left past b's midpoint (150).
    h.pill("c").dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, clientX: 250, clientY: 10 }),
    );
    h.move(240, 10); // past threshold
    await flushFrames();
    h.move(140, 10); // crosses b's midpoint
    await flushFrames();

    expect(h.renderedOrder()).toEqual(["a", "c", "b"]);

    h.up(140, 10);
    expect(h.commit).toHaveBeenCalledWith(["a", "c", "b"]);
  });

  it("ignores movement under the drag threshold and never commits a plain click", async () => {
    const h = harness(["a", "b"]);
    h.pill("a").dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, clientX: 50, clientY: 10 }),
    );
    h.move(52, 11); // under the 4px threshold
    await flushFrames();
    h.up(52, 11);

    expect(h.commit).not.toHaveBeenCalled();
    expect(h.renderedOrder()).toEqual(["a", "b"]);
  });

  it("does not start a drag for items whose enabled() is false", async () => {
    const h = harness(["a", "b"], (id) => id !== "a");
    h.pill("a").dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, clientX: 50, clientY: 10 }),
    );
    h.move(500, 10);
    await flushFrames();
    h.up(500, 10);

    expect(h.commit).not.toHaveBeenCalled();
    expect(h.renderedOrder()).toEqual(["a", "b"]);
  });

  it("clears drag state on pointercancel without committing", async () => {
    const h = harness(["a", "b"]);
    h.pill("a").dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, clientX: 50, clientY: 10 }),
    );
    h.move(60, 10);
    await flushFrames();
    h.cancel();
    expect(h.commit).not.toHaveBeenCalled();
  });

  it("ignores non-primary-button pointerdown entirely (right-click must not drag)", async () => {
    const h = harness(["a", "b", "c"]);
    // A right-click that drifts past the threshold before release must never arm a drag —
    // contextmenu owns that gesture.
    h.pill("a").dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, clientX: 50, clientY: 10, button: 2 }),
    );
    h.move(400, 10);
    await flushFrames();
    h.up(400, 10);

    expect(h.commit).not.toHaveBeenCalled();
    expect(h.renderedOrder()).toEqual(["a", "b", "c"]);
  });

  it("resolves the target slot from one large jump across several midpoints", async () => {
    const h = harness(["a", "b", "c", "d"]);
    // Grab "a" (slot [0,100), midpoint of its own irrelevant) and flick it all the way past
    // b's midpoint (150) AND c's midpoint (250) in a single native pointermove. The committed
    // order must match the visual position — not land one adjacent swap short.
    h.pill("a").dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, clientX: 50, clientY: 10 }),
    );
    h.move(60, 10); // arm
    await flushFrames();
    h.move(260, 10); // single jump past two midpoints
    await flushFrames();

    expect(h.renderedOrder()).toEqual(["b", "c", "a", "d"]);

    h.up(260, 10);
    expect(h.commit).toHaveBeenCalledWith(["b", "c", "a", "d"]);
  });

  it("abort() tears down an in-flight drag without committing", async () => {
    let api: ReturnType<typeof createPointerReorder<string>> | null = null;
    const container = document.createElement("div");
    container.setAttribute("data-reorder-container", "");
    document.body.appendChild(container);
    const order = ["a", "b"];
    const commit = vi.fn();
    const drag = createPointerReorder<string>({
      axis: "x",
      ids: () => [...order],
      reorder: () => {},
      commit,
      enabled: () => true,
    });
    api = drag;
    const el = document.createElement("div");
    el.dataset.reorderId = "a";
    el.getBoundingClientRect = () => new DOMRect(0, 0, 100, 30);
    const handlers = drag.itemHandlers("a");
    el.addEventListener("pointerdown", (e) => handlers.onPointerDown(e as PointerEvent));
    container.appendChild(el);

    el.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 50, clientY: 10 }));
    container.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientX: 90, clientY: 10 }));
    await flushFrames();
    expect(api!.isDragging()).toBe(true);

    // Simulates the surface unmounting mid-drag.
    api!.abort();
    expect(api!.isDragging()).toBe(false);
    container.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, clientX: 90, clientY: 10 }));
    expect(commit).not.toHaveBeenCalled();
  });
});
