import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DaemonRpcError } from "./rpc";
import {
  asTransportError,
  createInputCoalescer,
  createTerminalFrameGate,
  decodeTerminalChannelFrame,
  isTerminalFindChord,
  isTerminalReleaseChord,
  takeWheelBatch,
  terminalPointerCell,
  translateKeyboardKey,
  wheelLines,
} from "./term";

const daemonCallMock = vi.hoisted(() => vi.fn().mockResolvedValue(null));
vi.mock("./rpc", async () => {
  const actual = await vi.importActual<typeof import("./rpc")>("./rpc");
  return {
    ...actual,
    daemonCall: (...args: unknown[]) => daemonCallMock(...args),
  };
});

afterEach(() => {
  clearMocks();
  daemonCallMock.mockClear();
});

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

describe("createTerminalFrameGate", () => {
  it("holds the initial repaint until the caller has applied the acknowledged grid", () => {
    const seen: number[] = [];
    const gate = createTerminalFrameGate((bytes) => seen.push(...bytes));
    gate.push({ type: "bytes", bytes: Uint8Array.of(1, 2) });
    expect(seen).toEqual([]);
    gate.open();
    gate.push({ type: "bytes", bytes: Uint8Array.of(3) });
    expect(seen).toEqual([1, 2, 3]);
  });

  it("drops queued and future frames after close", () => {
    const seen: number[] = [];
    const gate = createTerminalFrameGate((bytes) => seen.push(...bytes));
    gate.push({ type: "bytes", bytes: Uint8Array.of(1) });
    gate.close();
    gate.open();
    gate.push({ type: "bytes", bytes: Uint8Array.of(2) });
    expect(seen).toEqual([]);
  });

  it("preserves grid and byte ordering while the initial checkpoint is gated", () => {
    const seen: string[] = [];
    const gate = createTerminalFrameGate(
      (bytes) => seen.push(`bytes:${bytes[0]}`),
      ({ cols, rows }) => seen.push(`grid:${cols}x${rows}`),
    );
    gate.push({ type: "bytes", bytes: Uint8Array.of(1) });
    gate.push({ type: "grid", cols: 120, rows: 40 });
    gate.push({ type: "bytes", bytes: Uint8Array.of(2) });
    gate.open();
    expect(seen).toEqual(["bytes:1", "grid:120x40", "bytes:2"]);
  });
});

describe("decodeTerminalChannelFrame", () => {
  it("decodes tagged output and grid frames", () => {
    expect(decodeTerminalChannelFrame(Uint8Array.of(0, 27, 91, 72).buffer)).toEqual({
      type: "bytes",
      bytes: Uint8Array.of(27, 91, 72),
    });
    expect(decodeTerminalChannelFrame(Uint8Array.of(1, 0, 120, 0, 40).buffer)).toEqual({
      type: "grid",
      cols: 120,
      rows: 40,
    });
    expect(decodeTerminalChannelFrame(Uint8Array.of(1, 0).buffer)).toBeNull();
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

describe("isTerminalReleaseChord", () => {
  it("releases on shift+escape", () => {
    expect(isTerminalReleaseChord(new KeyboardEvent("keydown", { key: "Escape", shiftKey: true }))).toBe(true);
  });

  it("leaves a plain escape for the agent, which uses it to interrupt", () => {
    expect(isTerminalReleaseChord(new KeyboardEvent("keydown", { key: "Escape" }))).toBe(false);
  });

  it("ignores other shifted keys", () => {
    expect(isTerminalReleaseChord(new KeyboardEvent("keydown", { key: "Enter", shiftKey: true }))).toBe(false);
  });
});

describe("isTerminalFindChord", () => {
  it("opens find on Cmd+Shift+F", () => {
    expect(
      isTerminalFindChord(new KeyboardEvent("keydown", { key: "f", metaKey: true, shiftKey: true })),
    ).toBe(true);
  });

  it("also opens find on Ctrl+Shift+F, even on a platform where mod is Cmd", () => {
    expect(
      isTerminalFindChord(new KeyboardEvent("keydown", { key: "f", ctrlKey: true, shiftKey: true })),
    ).toBe(true);
  });

  it("requires shift", () => {
    expect(isTerminalFindChord(new KeyboardEvent("keydown", { key: "f", metaKey: true }))).toBe(false);
  });

  it("ignores plain f", () => {
    expect(isTerminalFindChord(new KeyboardEvent("keydown", { key: "f" }))).toBe(false);
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

describe("createInputCoalescer", () => {
  const target = { laneId: 1, window: "lane-1" };

  it("sends a small paste as a single agent.send_input call", async () => {
    const coalescer = createInputCoalescer(target);
    coalescer.push("hello");
    await coalescer.flush();

    expect(daemonCallMock).toHaveBeenCalledTimes(1);
    expect(daemonCallMock).toHaveBeenCalledWith("agent.send_input", {
      lane_id: 1,
      window: "lane-1",
      text: "hello",
      enter: false,
    });
  });

  it("splits a paste larger than the chunk cap into multiple bounded sends", async () => {
    // Bound paste chunks so large input cannot exceed backend argument limits or stall a single
    // request.
    const big = "x".repeat(40 * 1024); // 40 KiB, > the 8 KiB chunk cap
    const coalescer = createInputCoalescer(target);
    coalescer.push(big);
    await coalescer.flush();

    expect(daemonCallMock).toHaveBeenCalledTimes(5);
    const sentTexts = daemonCallMock.mock.calls.map((call) => (call[1] as { text: string }).text);
    expect(sentTexts.every((text) => text.length <= 8 * 1024)).toBe(true);
    expect(sentTexts.join("")).toBe(big);
  });

  it("never splits a UTF-16 surrogate pair across a chunk boundary", async () => {
    // An astral character (e.g. an emoji) is two UTF-16 code units. Slicing between them
    // would hand the backend two lone, invalid surrogates instead of one valid character.
    const emoji = "\u{1F600}"; // one astral character = a high + low surrogate pair
    const padding = "y".repeat(8 * 1024 - 1); // chunk boundary lands exactly on the pair
    const big = padding + emoji + "z".repeat(10);
    const coalescer = createInputCoalescer(target);
    coalescer.push(big);
    await coalescer.flush();

    const sentTexts = daemonCallMock.mock.calls.map((call) => (call[1] as { text: string }).text);
    expect(sentTexts.join("")).toBe(big);
    for (const text of sentTexts) {
      // A lone surrogate at either edge means the pair was split.
      const firstCode = text.charCodeAt(0);
      const lastCode = text.charCodeAt(text.length - 1);
      expect(lastCode >= 0xd800 && lastCode <= 0xdbff).toBe(false);
      expect(firstCode >= 0xdc00 && firstCode <= 0xdfff).toBe(false);
    }
  });
});
