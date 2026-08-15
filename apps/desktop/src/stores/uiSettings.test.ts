import { describe, expect, it, vi } from "vitest";
import {
  notifyLayoutChanged,
  onLayoutChanged,
  readAutoCollapseEmptyLanes,
  saveAutoCollapseEmptyLanes,
  onAutoCollapseChanged,
} from "./uiSettings";

describe("uiSettings layout and preferences", () => {
  it("dispatches layout changed event and invokes subscribers", () => {
    const callback = vi.fn();
    const unsubscribe = onLayoutChanged(callback);

    notifyLayoutChanged();
    expect(callback).toHaveBeenCalledTimes(1);

    unsubscribe();
    notifyLayoutChanged();
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("persists auto-collapse setting and notifies listeners", () => {
    const listener = vi.fn();
    const unsub = onAutoCollapseChanged(listener);

    saveAutoCollapseEmptyLanes(false);
    expect(readAutoCollapseEmptyLanes()).toBe(false);
    expect(listener).toHaveBeenCalledWith(false);

    saveAutoCollapseEmptyLanes(true);
    expect(readAutoCollapseEmptyLanes()).toBe(true);
    expect(listener).toHaveBeenCalledWith(true);

    unsub();
  });
});
