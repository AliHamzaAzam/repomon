import { describe, expect, it } from "vitest";

import { DaemonRpcError } from "./rpc";
import { asTransportError, translateKeyboardKey, wheelStep } from "./term";

function key(value: string, modifiers: Partial<KeyboardEvent> = {}): KeyboardEvent {
  return new KeyboardEvent("keydown", { key: value, ...modifiers });
}

describe("asTransportError", () => {
  it("surfaces the daemon message from a structured RpcFailure", () => {
    const error = asTransportError({ code: -32009, message: "terminal 'lane-7' is already watched", data: null });
    expect(error).toBeInstanceOf(DaemonRpcError);
    expect(error.message).toBe("terminal 'lane-7' is already watched");
  });

  it("passes through a plain string rejection", () => {
    expect(asTransportError("boom").message).toBe("boom");
  });

  it("keeps an existing Error untouched", () => {
    const original = new Error("already an error");
    expect(asTransportError(original)).toBe(original);
  });
});

describe("terminal key translation", () => {
  it("lets xterm collect printable runs", () => {
    expect(translateKeyboardKey(key("a"))).toBeNull();
  });

  it("matches tmux control and navigation syntax", () => {
    expect(translateKeyboardKey(key("o", { ctrlKey: true }))).toEqual({ key: "C-o", literal: false });
    expect(translateKeyboardKey(key("ArrowLeft", { altKey: true }))).toEqual({ key: "M-Left", literal: false });
    expect(translateKeyboardKey(key("Tab", { shiftKey: true }))).toEqual({ key: "BTab", literal: false });
    expect(translateKeyboardKey(key("Escape"))).toEqual({ key: "Escape", literal: false });
  });
});

describe("wheelStep", () => {
  it("normalizes pixel, line, and page wheel units", () => {
    expect(wheelStep(-1, 0, 30)).toEqual({ up: true, ticks: 1 });
    expect(wheelStep(100, 0, 30)).toEqual({ up: false, ticks: 3 });
    expect(wheelStep(-2, 1, 30)).toEqual({ up: true, ticks: 2 });
    expect(wheelStep(1, 2, 30)).toEqual({ up: false, ticks: 30 });
  });

  it("ignores empty deltas and caps daemon work", () => {
    expect(wheelStep(0, 0, 30)).toBeNull();
    expect(wheelStep(Number.NaN, 0, 30)).toBeNull();
    expect(wheelStep(10_000, 0, 30)).toEqual({ up: false, ticks: 40 });
  });
});
