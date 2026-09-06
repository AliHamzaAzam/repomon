import { Channel, invoke } from "@tauri-apps/api/core";

import { DaemonRpcError, daemonCall, isRpcFailure } from "./rpc";

export type TerminalRenderer = "auto" | "webgl" | "dom";

export interface TerminalTarget {
  laneId: number;
  window: string;
}

export interface TermWatchAck {
  cols: number | null;
  rows: number | null;
  generation: number | null;
  sequence: number | null;
}

export interface TerminalGrid {
  cols: number;
  rows: number;
}

export interface TranslatedKey {
  key: string;
  literal: boolean;
}

const namedKeys: Record<string, string> = {
  Escape: "Escape",
  Enter: "Enter",
  Backspace: "BSpace",
  Tab: "Tab",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Delete: "DC",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
};

export function translateKeyboardKey(event: KeyboardEvent): TranslatedKey | null {
  const control = event.ctrlKey;
  const alt = event.altKey;
  if (event.key.length === 1) {
    if (control) return { key: `C-${event.key.toLowerCase()}`, literal: false };
    if (alt) return { key: `M-${event.key}`, literal: false };
    return null;
  }
  let base = namedKeys[event.key];
  if (!base) return null;
  if (event.key === "Tab" && event.shiftKey) base = "BTab";
  return { key: `${control ? "C-" : alt ? "M-" : ""}${base}`, literal: false };
}

/// True when a key event should release focus from the terminal back to the app shell.
/// Shift is required: plain Escape must keep reaching the agent, since Claude Code uses it to
/// interrupt its own work.
export function isTerminalReleaseChord(event: KeyboardEvent): boolean {
  return event.key === "Escape" && event.shiftKey;
}

/// Recognizes Ctrl/Cmd+Shift+F on either platform because the focused terminal already owns both
/// modifiers.
export function isTerminalFindChord(event: KeyboardEvent): boolean {
  return (event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "f";
}

/// Converts wheel deltas to signed fractional lines so callers can accumulate trackpad movement
/// proportionally.
export function wheelLines(
  deltaY: number,
  deltaMode: number,
  pageRows: number,
  pixelsPerLine: number,
): number {
  if (!Number.isFinite(deltaY) || deltaY === 0) return 0;
  if (deltaMode === 1) return deltaY; // already in lines
  if (deltaMode === 2) return deltaY * Math.max(1, pageRows); // pages
  return deltaY / Math.max(1, pixelsPerLine); // pixels
}

/// Take one bounded whole-line batch while preserving unsent lines and the fractional remainder.
export function takeWheelBatch(accumulated: number, maxTicks = 40): {
  ticks: number;
  remainder: number;
} {
  const whole = Math.trunc(Number.isFinite(accumulated) ? accumulated : 0);
  const ticks = Math.sign(whole) * Math.min(Math.abs(whole), Math.max(0, maxTicks));
  return { ticks, remainder: accumulated - ticks };
}

/// Map a browser pointer position to a clamped, 1-based terminal cell.
export function terminalPointerCell(
  clientX: number,
  clientY: number,
  left: number,
  top: number,
  width: number,
  height: number,
  cols: number,
  rows: number,
): { col: number; row: number } {
  if (width <= 0 || height <= 0 || cols <= 0 || rows <= 0) return { col: 1, row: 1 };
  const col = Math.floor(((clientX - left) / width) * cols) + 1;
  const row = Math.floor(((clientY - top) / height) * rows) + 1;
  return {
    col: Math.min(cols, Math.max(1, col)),
    row: Math.min(rows, Math.max(1, row)),
  };
}

/// Normalize whatever `invoke` rejected with into a real `Error`, so callers surface the
/// daemon's message instead of stringifying a `{code, message}` object into `[object Object]`.
export function asTransportError(error: unknown): Error {
  if (error instanceof Error) return error;
  if (isRpcFailure(error)) return new DaemonRpcError(error);
  return new Error(typeof error === "string" ? error : "terminal transport unavailable");
}

export interface TermTraceItem {
  ts: number;
  type: string;
  window?: string;
  len: number;
  preview: string;
}

declare global {
  interface Window {
    __REPOMON_TERM_TRACE__?: TermTraceItem[];
  }
}

export function recordTrace(type: string, windowName: string | undefined, bytes: Uint8Array | string) {
  if (typeof window === "undefined") return;
  if (!window.__REPOMON_TERM_TRACE__) window.__REPOMON_TERM_TRACE__ = [];
  const raw = typeof bytes === "string" ? new TextEncoder().encode(bytes) : bytes;
  let preview = "";
  for (let i = 0; i < Math.min(raw.length, 128); i++) {
    const b = raw[i];
    if (b === 0x1b) preview += "\\e";
    else if (b === 0x0d) preview += "\\r";
    else if (b === 0x0a) preview += "\\n";
    else if (b === 0x09) preview += "\\t";
    else if (b >= 32 && b <= 126) preview += String.fromCharCode(b);
    else preview += `\\x${b.toString(16).padStart(2, "0")}`;
  }
  window.__REPOMON_TERM_TRACE__.push({
    ts: Date.now(),
    type,
    window: windowName,
    len: raw.length,
    preview,
  });
  if (window.__REPOMON_TERM_TRACE__.length > 4000) {
    window.__REPOMON_TERM_TRACE__.splice(0, 1000);
  }
}

export type TerminalChannelFrame =
  | { type: "bytes"; bytes: Uint8Array }
  | { type: "grid"; cols: number; rows: number };

export function decodeTerminalChannelFrame(buffer: ArrayBuffer): TerminalChannelFrame | null {
  const frame = new Uint8Array(buffer);
  if (frame[0] === 0) return { type: "bytes", bytes: frame.slice(1) };
  if (frame[0] === 1 && frame.length === 5) {
    const view = new DataView(buffer);
    return { type: "grid", cols: view.getUint16(1), rows: view.getUint16(3) };
  }
  return null;
}

export function createTerminalFrameGate(
  onBytes: (bytes: Uint8Array) => void,
  onGrid: (grid: TerminalGrid) => void = () => undefined,
) {
  let active = true;
  let streaming = false;
  let queued: TerminalChannelFrame[] = [];

  const dispatch = (frame: TerminalChannelFrame) => {
    if (frame.type === "bytes") onBytes(frame.bytes);
    else onGrid(frame);
  };

  return {
    push(frame: TerminalChannelFrame) {
      if (!active) return;
      if (streaming) dispatch(frame);
      else queued.push(frame);
    },
    open() {
      if (!active) return;
      streaming = true;
      for (const frame of queued) dispatch(frame);
      queued = [];
    },
    close() {
      active = false;
      queued = [];
    },
  };
}

export async function watchTerminal(
  target: TerminalTarget,
  onBytes: (bytes: Uint8Array) => void,
  onReady?: (ack: TermWatchAck) => void,
  onGrid?: (grid: TerminalGrid) => void,
): Promise<{ ack: TermWatchAck; stop: () => Promise<void> }> {
  const channel = new Channel<ArrayBuffer>();
  const gate = createTerminalFrameGate(onBytes, onGrid);
  channel.onmessage = (buffer) => {
    const frame = decodeTerminalChannelFrame(buffer);
    if (!frame) return;
    if (frame.type === "bytes") {
      recordTrace("CHANNEL_ONMESSAGE", target.window, frame.bytes);
    } else {
      recordTrace("CHANNEL_GRID", target.window, `${frame.cols}x${frame.rows}`);
    }
    gate.push(frame);
  };
  let ack: TermWatchAck;
  try {
    ack = await invoke<TermWatchAck>("term_watch", {
      laneId: target.laneId,
      window: target.window,
      onBytes: channel,
    });
  } catch (error) {
    gate.close();
    throw asTransportError(error);
  }
  // The host can deliver its authoritative repaint before the invoke promise resolves. Resize
  // the emulator to the acknowledged pane grid before releasing any queued frame, including for
  // proactively warmed panes that have never been visible.
  try {
    onReady?.(ack);
  } catch (error) {
    gate.close();
    await invoke("term_unwatch", { window: target.window }).catch(() => undefined);
    throw error;
  }
  gate.open();
  return {
    ack,
    async stop() {
      gate.close();
      await invoke("term_unwatch", { window: target.window });
    },
  };
}

export function createInputCoalescer(target: TerminalTarget, onError?: (error: unknown) => void) {
  // Empirically, a real tmux `send-keys` call on this machine accepts at most 16,331 bytes;
  // the target/window string consumes part of that same argv budget, so keep a wide margin
  // across varying lane/window names and prevent large pastes from stalling the daemon.
  const MAX_INPUT_CHUNK = 8 * 1024;
  let pending = "";
  let running: Promise<void> | null = null;
  const reportError = onError ?? (() => undefined);

  // Send the leading keystroke immediately and batch subsequent input during the request round
  // trip.
  function drain(): Promise<void> {
    if (!running) {
      running = (async () => {
        try {
          while (pending) {
            const size = Math.min(MAX_INPUT_CHUNK, pending.length);
            // Avoid splitting a UTF-16 surrogate pair at a chunk boundary.
            const end = size > 0 && size < pending.length && pending.charCodeAt(size - 1) >= 0xd800 && pending.charCodeAt(size - 1) <= 0xdbff ? size - 1 : size;
            const text = pending.slice(0, end || size);
            pending = pending.slice(text.length);
            await daemonCall("agent.send_input", {
              lane_id: target.laneId,
              window: target.window,
              text,
              enter: false,
            });
          }
        } finally {
          running = null;
        }
      })();
    }
    return running;
  }

  async function flush() {
    while (pending || running) await drain();
  }

  function push(text: string) {
    pending += text;
    void drain().catch(reportError);
  }

  async function key(translated: TranslatedKey) {
    await flush();
    await daemonCall("agent.key", {
      lane_id: target.laneId,
      window: target.window,
      key: translated.key,
      literal: translated.literal,
    });
  }

  function dispose() {
    void flush().catch(reportError);
  }

  return { push, flush, key, dispose };
}
