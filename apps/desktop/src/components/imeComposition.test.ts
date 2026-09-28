import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { isImeConfirmation } from "./imeComposition";

// Ported from qa/pr94/ime-audit.test.ts: exercise event identity and missing lifecycle events.
let input: HTMLInputElement;
beforeEach(() => {
  input = document.createElement("input");
  document.body.append(input);
});
afterEach(() => { input.remove(); vi.restoreAllMocks(); });

const start = () => input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
const end = () => input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
const enter = (keyCode = 13, isComposing = false) =>
  new KeyboardEvent("keydown", { key: "Enter", keyCode, isComposing, bubbles: true });

describe("IME event-local detection", () => {
  it.each([
    { isComposing: false, keyCode: 13, expected: false },
    { isComposing: true, keyCode: 13, expected: true },
    { isComposing: false, keyCode: 229, expected: true },
    { isComposing: true, keyCode: 229, expected: true },
  ])("classifies isComposing=$isComposing keyCode=$keyCode", ({ isComposing, keyCode, expected }) => {
    expect(isImeConfirmation(enter(keyCode, isComposing))).toBe(expected);
  });

  it("guards the WebKit confirmation repeatedly and permits the next ordinary Enter", () => {
    start();
    end();
    const confirmation = enter(229);
    expect(isImeConfirmation(confirmation)).toBe(true);
    // The composer asks the same predicate for both slash selection and ordinary send.
    expect(isImeConfirmation(confirmation)).toBe(true);
    input.dispatchEvent(new KeyboardEvent("keyup", { key: "Enter", bubbles: true }));
    expect(isImeConfirmation(enter())).toBe(false);
  });

  it("does not stick closed when compositionend and blur are both missing", () => {
    start();
    expect(isImeConfirmation(enter(13, true))).toBe(true);
    expect(isImeConfirmation(enter())).toBe(false);
  });

  it("keeps an IME key guarded after blur even if compositionend is missing", () => {
    start();
    input.dispatchEvent(new FocusEvent("blur", { bubbles: false }));
    expect(isImeConfirmation(enter(229))).toBe(true);
    expect(isImeConfirmation(enter())).toBe(false);
  });

  it.each([false, true])("does not transfer composition state to a button, ended=%s", (ended) => {
    start();
    if (ended) end();
    const button = document.createElement("button");
    document.body.append(button);
    try {
      let confirmation: boolean | undefined;
      button.addEventListener("keydown", (event) => { confirmation = isImeConfirmation(event); });
      button.dispatchEvent(enter());
      expect(confirmation).toBe(false);
    } finally {
      button.remove();
    }
  });

  it("adds no global composition or blur listeners when the module is reevaluated", async () => {
    const added = vi.spyOn(window, "addEventListener");
    const types = ["compositionstart", "compositionend", "blur"];
    vi.resetModules();
    const first = await import("./imeComposition");
    vi.resetModules();
    const second = await import("./imeComposition");
    start();
    expect(first.isImeConfirmation(enter())).toBe(false);
    expect(second.isImeConfirmation(enter())).toBe(false);
    expect(added.mock.calls.filter(([type]) => types.includes(type))).toHaveLength(0);
  });
});
