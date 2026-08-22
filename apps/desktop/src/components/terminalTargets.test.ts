import { describe, expect, it } from "vitest";

import {
  dedupe,
  stableVisibleTargets,
  stabilizeTargets,
  warmTargetWindows,
  type PaneTarget,
} from "./terminalTargets";

function target(window: string, overrides: Partial<PaneTarget> = {}): PaneTarget {
  return { laneId: 1, window, label: window, shell: false, sessionId: null, targetId: null, ...overrides };
}

describe("stabilizeTargets", () => {
  it("reuses the previous reference for a window that still exists", () => {
    const cache = new Map<string, PaneTarget>();
    const first = stabilizeTargets(cache, [target("lane-7")]);
    // A fresh poll builds a brand-new object for the same window.
    const second = stabilizeTargets(cache, [target("lane-7")]);
    expect(second[0]).toBe(first[0]);
  });

  it("keeps the reference stable across a label change", () => {
    const cache = new Map<string, PaneTarget>();
    const first = stabilizeTargets(cache, [target("lane-7", { label: "claude 1" })]);
    const second = stabilizeTargets(cache, [target("lane-7", { label: "renamed" })]);
    expect(second[0]).toBe(first[0]);
    expect(second[0].label).toBe("renamed");
  });

  it("prunes windows that disappear so the cache does not leak", () => {
    const cache = new Map<string, PaneTarget>();
    stabilizeTargets(cache, [target("lane-7"), target("lane-8")]);
    stabilizeTargets(cache, [target("lane-7")]);
    expect([...cache.keys()]).toEqual(["lane-7"]);
  });

  it("mints a new reference for a genuinely new window", () => {
    const cache = new Map<string, PaneTarget>();
    const first = stabilizeTargets(cache, [target("lane-7")]);
    const second = stabilizeTargets(cache, [target("lane-7"), target("lane-9")]);
    expect(second[0]).toBe(first[0]);
    expect(second[1].window).toBe("lane-9");
  });
});

describe("dedupe", () => {
  it("drops repeated windows, keeping first occurrence", () => {
    const out = dedupe([target("a"), target("a", { label: "dup" }), target("b")]);
    expect(out.map((t) => t.window)).toEqual(["a", "b"]);
  });
});

describe("warmTargetWindows", () => {
  it("moves visible windows to the front and retains recent live windows", () => {
    const available = ["a", "b", "c", "d"].map((window) => target(window));
    expect(warmTargetWindows(["a", "b", "c"], [target("d")], available)).toEqual([
      "d",
      "a",
      "b",
      "c",
    ]);
  });

  it("prewarms unvisited windows up to capacity", () => {
    const available = ["a", "b", "c", "d"].map((window) => target(window));
    expect(warmTargetWindows([], [target("c")], available, 3)).toEqual([
      "c",
      "a",
      "b",
    ]);
  });

  it("evicts the least recent window at capacity", () => {
    const available = ["a", "b", "c", "d"].map((window) => target(window));
    expect(warmTargetWindows(["a", "b", "c"], [target("d")], available, 3)).toEqual([
      "d",
      "a",
      "b",
    ]);
  });

  it("drops windows that no longer exist", () => {
    expect(warmTargetWindows(["gone", "a"], [target("b")], [target("a"), target("b")])).toEqual([
      "b",
      "a",
    ]);
  });
});

describe("stableVisibleTargets", () => {
  const panes = ["a", "b", "c", "d"].map((window) => target(window));

  it("keeps the fleet order in grid mode regardless of which pane is active", () => {
    expect(stableVisibleTargets(panes, "c", "grid").map((t) => t.window)).toEqual([
      "a",
      "b",
      "c",
      "d",
    ]);
  });

  it("does not reorder when the selection moves between visible panes", () => {
    const before = stableVisibleTargets(panes, "a", "grid");
    const after = stableVisibleTargets(panes, "d", "grid");
    expect(after.map((t) => t.window)).toEqual(before.map((t) => t.window));
  });

  it("caps the grid at six panes and swaps an unseen selection into the last slot", () => {
    const many = ["a", "b", "c", "d", "e", "f", "g"].map((window) => target(window));
    const visible = stableVisibleTargets(many, "g", "grid");
    // "g" was beyond the cap, so it takes the last slot instead of being invisible.
    expect(visible.map((t) => t.window)).toEqual(["a", "b", "c", "d", "e", "g"]);
  });

  it("caps split view at two panes with the same stability rule", () => {
    expect(stableVisibleTargets(panes, "b", "split").map((t) => t.window)).toEqual(["a", "b"]);
    const swapped = stableVisibleTargets(panes, "d", "split");
    expect(swapped.map((t) => t.window)).toEqual(["a", "d"]);
  });

  it("returns panes without the active one when no window matches", () => {
    expect(stableVisibleTargets(panes, null, "grid").map((t) => t.window)).toEqual([
      "a",
      "b",
      "c",
      "d",
    ]);
  });
});
