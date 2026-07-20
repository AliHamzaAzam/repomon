import { describe, expect, it } from "vitest";

import { dedupe, stabilizeTargets, type PaneTarget } from "./terminalTargets";

function target(window: string, overrides: Partial<PaneTarget> = {}): PaneTarget {
  return { laneId: 1, window, label: window, shell: false, ...overrides };
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
