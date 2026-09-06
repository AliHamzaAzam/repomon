import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import TerminalPane, { devicePixelAlignedInsets } from "./TerminalPane";

const watchTerminalMock = vi.hoisted(() => vi.fn());
const daemonCallMock = vi.hoisted(() => vi.fn().mockResolvedValue(null));
const terminalInstances = vi.hoisted(() => [] as Array<{
  rows: number;
  refresh: ReturnType<typeof vi.fn>;
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
    open(element: HTMLElement) {
      this.element = element;
      const screen = document.createElement("div");
      screen.className = "xterm-screen";
      Object.defineProperty(screen, "getBoundingClientRect", {
        configurable: true,
        value: () => new DOMRect(0, 28, 1200, this.rows * 16.5),
      });
      element.appendChild(screen);
    }
    attachCustomKeyEventHandler() {}
    onData() { return { dispose() {} }; }
    write(_data: string | Uint8Array, callback?: () => void) { callback?.(); }
    resize(cols: number, rows: number) { this.cols = cols; this.rows = rows; }
    refresh = vi.fn();
    focus() {}
    blur() {}
    scrollLines() {}
    scrollToBottom = vi.fn();
    linkProviders: Array<{ provideLinks: (bufferLineNumber: number, callback: (links: any[] | undefined) => void) => void }> = [];
    registerLinkProvider(provider: { provideLinks: (bufferLineNumber: number, callback: (links: any[] | undefined) => void) => void }) {
      this.linkProviders.push(provider);
      return { dispose: vi.fn() };
    }
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
  it("preserves host size while snapping fractional grid positions to device pixels", () => {
    const original = { left: 8, right: 8, top: 28, bottom: 0 };
    const aligned = devicePixelAlignedInsets(original, 677.6640625, 68.5, 2);

    expect((677.6640625 + aligned.left - original.left) * 2).toBe(1355);
    expect((68.5 + aligned.top - original.top) * 2).toBe(137);
    expect(aligned.left + aligned.right).toBe(original.left + original.right);
    expect(aligned.top + aligned.bottom).toBe(original.top + original.bottom);
  });

  it("realigns a reused pane when its wrapper moves without changing size", async () => {
    watchTerminalMock.mockResolvedValue({
      ack: { cols: 40, rows: 5, generation: 1, sequence: 9 },
      stop: vi.fn().mockResolvedValue(undefined),
    });
    daemonCallMock.mockImplementation(async (method: string) => (
      method === "agent.fit" ? { cols: 40, rows: 5 } : null
    ));

    const { container } = render(() => (
      <TerminalPane laneId={7} window="lane-7-1" label="Codex" visible followTail />
    ));
    await flushMicrotasks();
    const host = container.querySelector<HTMLElement>(".terminal-host")!;
    host.style.left = "8px";
    host.style.right = "8px";
    host.style.top = "28px";
    host.style.bottom = "0px";
    Object.defineProperties(host, {
      clientWidth: { configurable: true, value: 314 },
      clientHeight: { configurable: true, value: 232 },
      getBoundingClientRect: {
        configurable: true,
        value: () => {
          const left = 677.6640625 + (Number.parseFloat(host.style.left || "8") - 8);
          return new DOMRect(left, 68.5, 314, 232);
        },
      },
    });

    const wrapper = host.closest("section")!.parentElement!;
    wrapper.style.order = "2";
    await flushMicrotasks();

    const alignedDeviceX = host.getBoundingClientRect().left * 2;
    expect(alignedDeviceX).toBe(Math.round(alignedDeviceX));
    expect(terminalInstances[0].refresh).toHaveBeenCalled();
  });

  it("reports enough grid height for every daemon-authoritative row", async () => {
    const [visible, setVisible] = createSignal(false);
    const onMinimumHeight = vi.fn();
    watchTerminalMock.mockImplementation(async (
      _target: unknown,
      _bytes: unknown,
      onAck: (ack: { cols: number; rows: number }) => void,
    ) => {
      // A warm pane can still carry its old full-screen grid. It must not make that stale height
      // the multitasking row floor before the visible cell gets its own fit.
      onAck({ cols: 211, rows: 60 });
      return {
        ack: { cols: 211, rows: 60, generation: 1, sequence: 9 },
        stop: vi.fn().mockResolvedValue(undefined),
      };
    });
    daemonCallMock.mockImplementation(async (method: string) => (
      method === "agent.fit" ? { cols: 166, rows: 24 } : null
    ));

    const { container } = render(() => (
      <TerminalPane
        laneId={7}
        window="lane-7-1"
        label="Codex"
        visible={visible()}
        followTail
        onMinimumHeight={onMinimumHeight}
      />
    ));
    await flushMicrotasks();
    expect(terminalInstances[0].rows).toBe(60);
    expect(onMinimumHeight).not.toHaveBeenCalled();
    const pane = container.querySelector<HTMLElement>("section")!;
    const host = container.querySelector<HTMLElement>(".terminal-host")!;
    Object.defineProperties(host, {
      clientWidth: { configurable: true, value: 1184 },
      clientHeight: { configurable: true, value: 196 },
      getBoundingClientRect: {
        configurable: true,
        value: () => new DOMRect(8, 28, 1184, 196),
      },
    });
    Object.defineProperty(pane, "getBoundingClientRect", {
      configurable: true,
      value: () => new DOMRect(0, 0, 1200, 224),
    });

    setVisible(true);
    await flushMicrotasks();

    // 24 actual xterm rows × 16.5px plus the measured 28px header/chrome.
    expect(onMinimumHeight).toHaveBeenCalledWith(424);
    expect(terminalInstances[0].rows).toBe(24);
  });

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

describe("TerminalPane clickable path links", () => {
  it("registers link provider that verifies against file.index and opens in editor on Cmd-click", async () => {
    watchTerminalMock.mockResolvedValue({
      ack: { cols: 80, rows: 24, generation: 1, sequence: 1 },
      stop: vi.fn().mockResolvedValue(undefined),
    });

    const openAt = vi.fn();
    const isPathInIndex = vi.fn((_laneId: number, path: string) => path === "src/foo.rs");
    const ensureIndex = vi.fn();
    const onEnsureEditorOpen = vi.fn();

    const mockFleet = {
      lanes: () => [{
        id: 1,
        worktree: { path: "/tmp/worktree", name: "worktree", branch: "main", is_main: true, id: 1, repo_id: 1 },
      }],
    } as any;

    const mockEditor = {
      openAt,
      isPathInIndex,
      ensureIndex,
    } as any;

    render(() => (
      <TerminalPane
        laneId={1}
        window="lane-1-1"
        label="Terminal"
        fleet={mockFleet}
        editor={mockEditor}
        onEnsureEditorOpen={onEnsureEditorOpen}
      />
    ));
    await vi.waitFor(() => {
      expect(ensureIndex).toHaveBeenCalledWith(1);
      expect((terminalInstances[terminalInstances.length - 1] as any)?.linkProviders.length).toBeGreaterThan(0);
    });

    const termInstance = terminalInstances[terminalInstances.length - 1] as any;

    const provider = termInstance.linkProviders[0];

    // Mock buffer active line
    termInstance.buffer = {
      active: {
        getLine: (lineIdx: number) => {
          if (lineIdx === 0) {
            return {
              translateToString: () => "error at src/foo.rs:12:4 and src/missing.rs:1",
            };
          }
          return null;
        },
      },
    };

    let providedLinks: any[] | undefined;
    provider.provideLinks(1, (links: any[] | undefined) => {
      providedLinks = links;
    });

    // Only src/foo.rs exists in index, src/missing.rs does not
    expect(providedLinks).toBeDefined();
    expect(providedLinks).toHaveLength(1);
    expect(providedLinks![0].text).toBe("src/foo.rs:12:4");

    // Plain click should not trigger openAt
    const plainClickEvent = { metaKey: false, ctrlKey: false } as MouseEvent;
    providedLinks![0].activate(plainClickEvent, "src/foo.rs:12:4");
    expect(openAt).not.toHaveBeenCalled();
    expect(onEnsureEditorOpen).not.toHaveBeenCalled();

    // Cmd-click triggers openAt and ensureEditorOpen
    const cmdClickEvent = { metaKey: true, ctrlKey: false } as MouseEvent;
    providedLinks![0].activate(cmdClickEvent, "src/foo.rs:12:4");
    expect(onEnsureEditorOpen).toHaveBeenCalled();
    expect(openAt).toHaveBeenCalledWith("src/foo.rs", 12, 4);
  });
});
