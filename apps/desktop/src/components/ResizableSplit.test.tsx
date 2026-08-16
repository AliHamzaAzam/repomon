import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import { LAYOUT_CHANGED_EVENT } from "../stores/uiSettings";
import { ResizableSplit, type ResizableSplitProps } from "./ResizableSplit";

const STORAGE_KEY = "test:resizable-split-width";

afterEach(() => {
  cleanup();
  localStorage.removeItem(STORAGE_KEY);
  document.documentElement.style.removeProperty("--split-width");
  vi.useRealTimers();
});

function renderSplit(overrides: Partial<ResizableSplitProps> = {}) {
  const onWidthChange = vi.fn();
  const props: ResizableSplitProps = {
    storageKey: STORAGE_KEY,
    defaultWidth: 320,
    minWidth: 200,
    maxWidth: 480,
    label: "Resize panel",
    onWidthChange,
    ...overrides,
  };
  render(() => <ResizableSplit {...props} />);
  const handle = screen.getByRole("separator", { name: props.label });
  return { handle, onWidthChange, props };
}

// jsdom has no PointerEvent constructor, so @testing-library/dom's fireEvent.pointerX helpers
// silently drop clientX/pointerId (they fall back to the plain Event constructor, which ignores
// unrecognized init keys). Build MouseEvents instead — the component only reads clientX,
// pointerId, pointerType, and button, all of which we attach manually below.
function pointerEvent(
  type: string,
  init: { clientX: number; pointerId?: number; button?: number; pointerType?: string },
): Event {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: init.clientX,
    button: init.button ?? 0,
  });
  Object.defineProperty(event, "pointerId", { value: init.pointerId ?? 1, configurable: true });
  Object.defineProperty(event, "pointerType", { value: init.pointerType ?? "mouse", configurable: true });
  return event;
}

function drag(handle: HTMLElement, startX: number, endX: number) {
  fireEvent(handle, pointerEvent("pointerdown", { clientX: startX }));
  fireEvent(handle, pointerEvent("pointermove", { clientX: endX }));
  fireEvent(handle, pointerEvent("pointerup", { clientX: endX }));
}

describe("ResizableSplit", () => {
  it("restores the default width, sets the CSS custom property, and fires layout-changed on mount", () => {
    const layoutSpy = vi.fn();
    window.addEventListener(LAYOUT_CHANGED_EVENT, layoutSpy);

    const { handle, onWidthChange } = renderSplit();

    expect(handle.getAttribute("aria-valuenow")).toBe("320");
    expect(document.documentElement.style.getPropertyValue("--split-width")).toBe("320px");
    expect(onWidthChange).toHaveBeenCalledWith(320);
    expect(layoutSpy).toHaveBeenCalled();

    window.removeEventListener(LAYOUT_CHANGED_EVENT, layoutSpy);
  });

  it("drags via pointer events, growing an 'after' panel when the handle moves left", () => {
    const layoutSpy = vi.fn();
    window.addEventListener(LAYOUT_CHANGED_EVENT, layoutSpy);
    const { handle, onWidthChange } = renderSplit({ panelSide: "after" });
    layoutSpy.mockClear();

    // Handle moves 40px left -> panel (which sits after/right of the handle) grows by 40px.
    drag(handle, 500, 460);

    expect(handle.getAttribute("aria-valuenow")).toBe("360");
    expect(document.documentElement.style.getPropertyValue("--split-width")).toBe("360px");
    expect(onWidthChange).toHaveBeenLastCalledWith(360);
    expect(layoutSpy).toHaveBeenCalled();

    window.removeEventListener(LAYOUT_CHANGED_EVENT, layoutSpy);
  });

  it("drags via pointer events, growing a 'before' panel when the handle moves right", () => {
    const { handle, onWidthChange } = renderSplit({ panelSide: "before" });

    drag(handle, 200, 250);

    expect(handle.getAttribute("aria-valuenow")).toBe("370");
    expect(onWidthChange).toHaveBeenLastCalledWith(370);
  });

  it("clamps drag movement at the minimum width", () => {
    const { handle, onWidthChange } = renderSplit({ panelSide: "after" });

    // Dragging the handle far to the right shrinks an "after" panel well past its floor.
    drag(handle, 0, 1000);

    expect(handle.getAttribute("aria-valuenow")).toBe("200");
    expect(onWidthChange).toHaveBeenLastCalledWith(200);
  });

  it("clamps drag movement at the maximum width", () => {
    const { handle, onWidthChange } = renderSplit({ panelSide: "after" });

    drag(handle, 1000, 0);

    expect(handle.getAttribute("aria-valuenow")).toBe("480");
    expect(onWidthChange).toHaveBeenLastCalledWith(480);
  });

  it("adjusts width with arrow keys and fires layout-changed", () => {
    const layoutSpy = vi.fn();
    window.addEventListener(LAYOUT_CHANGED_EVENT, layoutSpy);
    const { handle, onWidthChange } = renderSplit({ panelSide: "after", step: 10 });
    layoutSpy.mockClear();

    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(onWidthChange).toHaveBeenLastCalledWith(330);
    expect(layoutSpy).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(onWidthChange).toHaveBeenLastCalledWith(320);

    fireEvent.keyDown(handle, { key: "Home" });
    expect(onWidthChange).toHaveBeenLastCalledWith(200);

    fireEvent.keyDown(handle, { key: "End" });
    expect(onWidthChange).toHaveBeenLastCalledWith(480);

    window.removeEventListener(LAYOUT_CHANGED_EVENT, layoutSpy);
  });

  it("resets to the default width on double-click", () => {
    const { handle, onWidthChange } = renderSplit({ panelSide: "after" });

    drag(handle, 500, 400); // grows to 420
    expect(onWidthChange).toHaveBeenLastCalledWith(420);

    fireEvent.dblClick(handle);
    expect(handle.getAttribute("aria-valuenow")).toBe("320");
    expect(onWidthChange).toHaveBeenLastCalledWith(320);
  });

  it("persists width to localStorage debounced, and restores it across a remount", async () => {
    vi.useFakeTimers();
    const { handle } = renderSplit({ panelSide: "after" });

    drag(handle, 500, 460); // -> 360

    expect(localStorage.getItem(STORAGE_KEY)).toBeNull();
    await vi.advanceTimersByTimeAsync(200);
    expect(localStorage.getItem(STORAGE_KEY)).toBe("360");

    cleanup();
    vi.useRealTimers();

    const { handle: remounted } = renderSplit({ panelSide: "after" });
    expect(remounted.getAttribute("aria-valuenow")).toBe("360");
  });

  it("clamps a stale out-of-range persisted value on restore", () => {
    localStorage.setItem(STORAGE_KEY, "9999");
    const { handle, onWidthChange } = renderSplit({ minWidth: 200, maxWidth: 480 });
    expect(handle.getAttribute("aria-valuenow")).toBe("480");
    expect(onWidthChange).toHaveBeenCalledWith(480);

    cleanup();
    localStorage.setItem(STORAGE_KEY, "-50");
    const { handle: handle2 } = renderSplit({ minWidth: 200, maxWidth: 480 });
    expect(handle2.getAttribute("aria-valuenow")).toBe("200");
  });
});
