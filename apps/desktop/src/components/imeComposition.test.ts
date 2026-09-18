import { beforeEach, describe, expect, it, vi } from "vitest";

import { SAFARI_IME_RACE_WINDOW_MS, __ime, isImeConfirmation } from "./imeComposition";

let clock = 1000;
const ev = (isComposing = false) => ({ isComposing });

beforeEach(() => {
  __ime.reset();
  clock = 1000;
  __ime.setNow(() => clock);
});

describe("isImeConfirmation", () => {
  it("is false when nothing is being composed", () => {
    expect(isImeConfirmation(ev())).toBe(false);
  });

  it("is false in the page's first milliseconds, before anything has been composed", async () => {
    // `performance.now()` counts from the page's time origin, so a module that starts its
    // "last composition ended" at 0 is inside the window until the page is 30ms old. This has
    // to read a FRESHLY LOADED module: beforeEach's reset would otherwise supply the value
    // being tested, and the assertion would hold whatever the module was born with.
    vi.resetModules();
    const fresh = await import("./imeComposition");
    fresh.__ime.setNow(() => 16);

    expect(fresh.isImeConfirmation({ isComposing: false })).toBe(false);
  });

  it("is true while a composition is open, which is Chrome and Firefox", () => {
    expect(isImeConfirmation(ev(true))).toBe(true);
  });

  it("is true just after compositionend, which is Safari and WKWebView", () => {
    // Safari fires compositionend BEFORE the confirming Enter, so isComposing is already false.
    __ime.start();
    __ime.end();
    clock += 3; // the 3ms the maintainer measured
    expect(isImeConfirmation(ev(false))).toBe(true);
  });

  it("stops guarding once a human could plausibly have pressed Enter themselves", () => {
    __ime.start();
    __ime.end();
    clock += SAFARI_IME_RACE_WINDOW_MS;
    expect(isImeConfirmation(ev(false))).toBe(false);
  });

  it("covers the whole window and not just its edges", () => {
    for (const delay of [0, 1, 5, 15, 29]) {
      __ime.reset();
      __ime.setNow(() => clock);
      __ime.start();
      __ime.end();
      clock += delay;
      expect(isImeConfirmation(ev(false)), `${delay}ms after compositionend`).toBe(true);
      clock += 1000;
    }
  });

  it("closes an abandoned composition on blur rather than leaving it open forever", () => {
    __ime.start();
    expect(isImeConfirmation(ev(false))).toBe(true);

    __ime.blur();
    clock += SAFARI_IME_RACE_WINDOW_MS;
    expect(isImeConfirmation(ev(false))).toBe(false);
  });

  it("is safe on an element that can never hold a composition", () => {
    // A button or a row: no composition ever starts, so the guard is inert there.
    expect(isImeConfirmation(ev(false))).toBe(false);
    clock += 10_000;
    expect(isImeConfirmation(ev(false))).toBe(false);
  });
});
