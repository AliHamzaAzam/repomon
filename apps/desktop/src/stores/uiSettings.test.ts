import { describe, expect, it, vi } from "vitest";
import {
  notifyLayoutChanged,
  onLayoutChanged,
  readAutoCollapseEmptyLanes,
  saveAutoCollapseEmptyLanes,
  onAutoCollapseChanged,
  readOnboardingCompleted,
  saveOnboardingCompleted,
  onOnboardingCompletedChanged,
  readShortcutsHintLaunchCount,
  recordShortcutsHintLaunch,
  SHORTCUTS_HINT_LAUNCH_COUNT_KEY,
  SHORTCUTS_HINT_MAX_LAUNCHES,
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

  it("persists onboarding completed setting across storage and notifies listeners", () => {
    localStorage.removeItem("repomon:onboarding-completed");
    expect(readOnboardingCompleted()).toBe(false);

    const listener = vi.fn();
    const unsub = onOnboardingCompletedChanged(listener);

    saveOnboardingCompleted(true);
    expect(readOnboardingCompleted()).toBe(true);
    expect(listener).toHaveBeenCalledWith(true);

    saveOnboardingCompleted(false);
    expect(readOnboardingCompleted()).toBe(false);
    expect(listener).toHaveBeenCalledWith(false);

    unsub();
  });

  it("counts shortcuts-hint launches and stops mattering past the max", () => {
    localStorage.removeItem(SHORTCUTS_HINT_LAUNCH_COUNT_KEY);
    expect(readShortcutsHintLaunchCount()).toBe(0);

    recordShortcutsHintLaunch();
    expect(readShortcutsHintLaunchCount()).toBe(1);

    recordShortcutsHintLaunch();
    recordShortcutsHintLaunch();
    expect(readShortcutsHintLaunchCount()).toBe(3);
    expect(readShortcutsHintLaunchCount()).toBeGreaterThanOrEqual(SHORTCUTS_HINT_MAX_LAUNCHES);
  });
});
