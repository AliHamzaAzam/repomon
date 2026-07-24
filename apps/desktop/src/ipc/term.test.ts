import { describe, expect, it } from "vitest";

import { DaemonRpcError } from "./rpc";
import {
  asTransportError,
  takeWheelBatch,
  terminalPointerCell,
  translateKeyboardKey,
  wheelLines,
} from "./term";

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

describe("wheelLines", () => {
  it("returns signed fractional lines by delta mode", () => {
    expect(wheelLines(-14, 0, 30, 14)).toBe(-1);
    expect(wheelLines(35, 0, 30, 14)).toBeCloseTo(2.5);
    expect(wheelLines(-2, 1, 30, 14)).toBe(-2);
    expect(wheelLines(1, 2, 30, 14)).toBe(30);
  });

  it("returns 0 for empty or invalid deltas", () => {
    expect(wheelLines(0, 0, 30, 14)).toBe(0);
    expect(wheelLines(Number.NaN, 0, 30, 14)).toBe(0);
  });

  it("a tiny trackpad delta is a fraction of a line (not a full line)", () => {
    expect(Math.abs(wheelLines(3, 0, 30, 14))).toBeLessThan(1);
  });
});

describe("takeWheelBatch", () => {
  it("caps one send without discarding the queued tail", () => {
    expect(takeWheelBatch(95.5)).toEqual({ ticks: 40, remainder: 55.5 });
    expect(takeWheelBatch(-95.5)).toEqual({ ticks: -40, remainder: -55.5 });
  });

  it("keeps sub-line movement for a later frame", () => {
    expect(takeWheelBatch(0.75)).toEqual({ ticks: 0, remainder: 0.75 });
  });
});

describe("terminalPointerCell", () => {
  it("maps the pointer to a 1-based terminal cell", () => {
    expect(terminalPointerCell(150, 70, 50, 20, 200, 100, 80, 40)).toEqual({
      col: 41,
      row: 21,
    });
  });

  it("clamps outside positions and invalid geometry", () => {
    expect(terminalPointerCell(-20, 999, 0, 0, 200, 100, 80, 40)).toEqual({
      col: 1,
      row: 40,
    });
    expect(terminalPointerCell(1, 1, 0, 0, 0, 0, 0, 0)).toEqual({ col: 1, row: 1 });
  });
});
