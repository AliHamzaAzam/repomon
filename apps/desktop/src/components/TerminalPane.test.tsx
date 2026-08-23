import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import TerminalPane from "./TerminalPane";

const watchTerminalMock = vi.hoisted(() => vi.fn());
const daemonCallMock = vi.hoisted(() => vi.fn().mockResolvedValue(null));

vi.mock("../ipc/term", async () => {
  const actual = await vi.importActual<typeof import("../ipc/term")>("../ipc/term");
  return {
    ...actual,
    watchTerminal: (...args: unknown[]) => watchTerminalMock(...args),
  };
});

vi.mock("../ipc/rpc", async () => {
  const actual = await vi.importActual<typeof import("../ipc/rpc")>("../ipc/rpc");
  return {
    ...actual,
    daemonCall: (...args: unknown[]) => daemonCallMock(...args),
  };
});

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    options: Record<string, unknown>;
    unicode = { activeVersion: "" };
    buffer = { active: { type: "normal" } };
    element: HTMLElement | null = null;

    constructor(options: Record<string, unknown>) {
      this.options = options;
    }

    loadAddon() {}
    open(element: HTMLElement) { this.element = element; }
    attachCustomKeyEventHandler() {}
    onData() { return { dispose() {} }; }
    write() {}
    resize(cols: number, rows: number) { this.cols = cols; this.rows = rows; }
    refresh() {}
    focus() {}
    blur() {}
    scrollLines() {}
    dispose() {}
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
    proposeDimensions() { return undefined; }
  },
}));
vi.mock("@xterm/addon-search", () => ({
  SearchAddon: class {
    findNext() {}
    findPrevious() {}
  },
}));
vi.mock("@xterm/addon-clipboard", () => ({ ClipboardAddon: class {} }));
vi.mock("@xterm/addon-unicode11", () => ({ Unicode11Addon: class {} }));
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    onContextLoss() {}
    dispose() {}
  },
}));

class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}

async function flushMicrotasks() {
  for (let i = 0; i < 6; i += 1) await Promise.resolve();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("ResizeObserver", ResizeObserverMock);
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    queueMicrotask(() => callback(0));
    return 1;
  });
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  watchTerminalMock.mockReset();
  daemonCallMock.mockClear();
});

afterEach(() => {
  cleanup();
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("TerminalPane transport recovery", () => {
  it("retries a failed watch twice before offering a manual retry", async () => {
    const stop = vi.fn().mockResolvedValue(undefined);
    watchTerminalMock
      .mockRejectedValueOnce(new Error("boot screen still changing"))
      .mockRejectedValueOnce(new Error("capture temporarily unavailable"))
      .mockRejectedValueOnce(new Error("watch could not open"))
      .mockResolvedValue({
        ack: { cols: 120, rows: 40, generation: 1, sequence: 9 },
        stop,
      });

    render(() => <TerminalPane laneId={7} window="lane-7-1" label="Codex" />);
    await flushMicrotasks();
    expect(watchTerminalMock).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await vi.advanceTimersByTimeAsync(300);
    await flushMicrotasks();
    expect(watchTerminalMock).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await vi.advanceTimersByTimeAsync(900);
    await flushMicrotasks();
    expect(watchTerminalMock).toHaveBeenCalledTimes(3);
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Terminal transport unavailable: watch could not open",
    );

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(screen.getByRole("button", { name: "Retrying…" })).toBeDisabled();
    await flushMicrotasks();
    expect(watchTerminalMock).toHaveBeenCalledTimes(4);
    expect(watchTerminalMock).toHaveBeenLastCalledWith(
      { laneId: 7, window: "lane-7-1" },
      expect.any(Function),
      expect.any(Function),
      expect.any(Function),
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
