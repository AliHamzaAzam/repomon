import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import TerminalPane from "./TerminalPane";

const watchTerminalMock = vi.hoisted(() => vi.fn());
const daemonCallMock = vi.hoisted(() => vi.fn().mockResolvedValue(null));
const terminalInstances = vi.hoisted(() => [] as Array<{
  scrollToBottom: ReturnType<typeof vi.fn>;
}>);

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
      terminalInstances.push(this);
    }

    loadAddon() {}
    open(element: HTMLElement) { this.element = element; }
    attachCustomKeyEventHandler() {}
    onData() { return { dispose() {} }; }
    write(_data: string | Uint8Array, callback?: () => void) { callback?.(); }
    resize(cols: number, rows: number) { this.cols = cols; this.rows = rows; }
    refresh() {}
    focus() {}
    blur() {}
    scrollLines() {}
    scrollToBottom = vi.fn();
    dispose() {}
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
    proposeDimensions() { return { cols: 40, rows: 5 }; }
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
  terminalInstances.splice(0);
});

describe("TerminalPane multitasking tail follow", () => {
  it("retries a zero-size warm pane until its visible grid cell is measurable", async () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    });
    const [visible, setVisible] = createSignal(false);
    watchTerminalMock.mockResolvedValue({
      ack: { cols: 120, rows: 40, generation: 1, sequence: 9 },
      stop: vi.fn().mockResolvedValue(undefined),
    });
    daemonCallMock.mockImplementation(async (method: string) => (
      method === "agent.fit" ? { cols: 40, rows: 5 } : null
    ));

    const { container } = render(() => (
      <TerminalPane
        laneId={7}
        window="lane-7-1"
        label="Codex"
        visible={visible()}
        followTail
      />
    ));
    await flushMicrotasks();
    expect(daemonCallMock).not.toHaveBeenCalledWith("agent.fit", expect.anything());
    expect(terminalInstances[0].scrollToBottom).not.toHaveBeenCalled();
    const host = container.querySelector<HTMLElement>(".terminal-host")!;
    let laidOut = false;
    Object.defineProperties(host, {
      clientWidth: { configurable: true, get: () => (laidOut ? 400 : 0) },
      clientHeight: { configurable: true, get: () => (laidOut ? 100 : 0) },
    });

    setVisible(true);
    await flushMicrotasks();
    expect(frames).toHaveLength(1);

    frames.shift()!(0);
    await flushMicrotasks();
    expect(daemonCallMock).not.toHaveBeenCalledWith("agent.fit", expect.anything());
    expect(frames).toHaveLength(1);

    laidOut = true;
    frames.shift()!(16);
    await flushMicrotasks();

    expect(daemonCallMock).toHaveBeenCalledWith("agent.fit", {
      lane_id: 7,
      window: "lane-7-1",
      cols: 40,
      rows: 5,
    });
    expect(terminalInstances[0].scrollToBottom).toHaveBeenCalled();
  });

  it("keeps a visible multitasking pane at the tail after long output", async () => {
    let onBytes: ((bytes: Uint8Array) => void) | undefined;
    watchTerminalMock.mockImplementation(async (_target, bytes) => {
      onBytes = bytes;
      return {
        ack: { cols: 40, rows: 5, generation: 1, sequence: 9 },
        stop: vi.fn().mockResolvedValue(undefined),
      };
    });
    daemonCallMock.mockImplementation(async (method: string) => (
      method === "agent.fit" ? { cols: 40, rows: 5 } : null
    ));

    render(() => (
      <TerminalPane laneId={7} window="lane-7-1" label="Codex" visible followTail />
    ));
    await flushMicrotasks();
    terminalInstances[0].scrollToBottom.mockClear();

    onBytes?.(new TextEncoder().encode(
      `${Array.from({ length: 80 }, (_, index) => `line-${index}`).join("\r\n")}\r\nPROMPT> `,
    ));

    expect(terminalInstances[0].scrollToBottom).toHaveBeenCalledTimes(1);
  });

  it("preserves manual scrollback outside multitasking", async () => {
    let onBytes: ((bytes: Uint8Array) => void) | undefined;
    watchTerminalMock.mockImplementation(async (_target, bytes) => {
      onBytes = bytes;
      return {
        ack: { cols: 40, rows: 5, generation: 1, sequence: 9 },
        stop: vi.fn().mockResolvedValue(undefined),
      };
    });

    render(() => <TerminalPane laneId={7} window="lane-7-1" label="Codex" visible />);
    await flushMicrotasks();
    onBytes?.(new TextEncoder().encode("new output while reviewing history"));

    expect(terminalInstances[0].scrollToBottom).not.toHaveBeenCalled();
  });
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

describe("TerminalPane header containment (bug 5: header can disappear under high-throughput panes)", () => {
  it("always mounts the header bar, and clips the terminal host to its own box below it", async () => {
    watchTerminalMock.mockResolvedValue({
      ack: { cols: 120, rows: 40, generation: 1, sequence: 9 },
      stop: vi.fn().mockResolvedValue(undefined),
    });

    const { container } = render(() => <TerminalPane laneId={7} window="lane-7-1" label="Codex" />);
    await flushMicrotasks();

    // Header render is unconditional in TerminalPane.tsx — no <Show> gates it — so it must
    // always be present regardless of transport/view state.
    expect(screen.getByText("Codex")).toBeInTheDocument();

    const section = container.querySelector("section[aria-label='Codex']");
    expect(section).not.toBeNull();
    // Each pane gets its own stacking context, so a sibling pane's header/canvas can never win a
    // paint-order tie against this one regardless of DOM position.
    expect(section!.classList.contains("isolate")).toBe(true);

    const host = container.querySelector(".terminal-host");
    expect(host).not.toBeNull();
    // The terminal host is clipped at its own box (starting below the h-7 header, `top-7`), not
    // just at the section's full-pane bounds — so an oversized/mis-sized xterm canvas during a
    // burst of live output can't paint upward over the header strip.
    expect(host!.classList.contains("overflow-hidden")).toBe(true);
    expect(host!.classList.contains("top-7")).toBe(true);
  });
});
