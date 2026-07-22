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
}

export interface TranslatedKey {
  key: string;
  literal: boolean;
}

export interface WheelStep {
  up: boolean;
  ticks: number;
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

/// Turn browser wheel units into the small integer steps expected by the daemon. Pixel-mode
/// trackpads can emit tiny deltas while conventional wheels often emit roughly 100 pixels.
export function wheelStep(deltaY: number, deltaMode: number, pageRows: number): WheelStep | null {
  if (!Number.isFinite(deltaY) || deltaY === 0) return null;
  const magnitude = Math.abs(deltaY);
  const rawTicks = deltaMode === 1
    ? magnitude
    : deltaMode === 2
      ? magnitude * Math.max(1, pageRows)
      : magnitude / 40;
  return {
    up: deltaY < 0,
    ticks: Math.max(1, Math.min(40, Math.ceil(rawTicks))),
  };
}

/// Normalize whatever `invoke` rejected with into a real `Error`, so callers surface the
/// daemon's message instead of stringifying a `{code, message}` object into `[object Object]`.
export function asTransportError(error: unknown): Error {
  if (error instanceof Error) return error;
  if (isRpcFailure(error)) return new DaemonRpcError(error);
  return new Error(typeof error === "string" ? error : "terminal transport unavailable");
}

export async function watchTerminal(
  target: TerminalTarget,
  onBytes: (bytes: Uint8Array) => void,
): Promise<{ ack: TermWatchAck; stop: () => Promise<void> }> {
  const channel = new Channel<ArrayBuffer>();
  let active = true;
  channel.onmessage = (buffer) => {
    if (active) onBytes(new Uint8Array(buffer));
  };
  let ack: TermWatchAck;
  try {
    ack = await invoke<TermWatchAck>("term_watch", {
      laneId: target.laneId,
      window: target.window,
      onBytes: channel,
    });
  } catch (error) {
    throw asTransportError(error);
  }
  return {
    ack,
    async stop() {
      active = false;
      await invoke("term_unwatch", { window: target.window });
    },
  };
}

export function createInputCoalescer(target: TerminalTarget) {
  let pending = "";
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function flush() {
    if (timer) clearTimeout(timer);
    timer = undefined;
    if (!pending) return;
    const text = pending;
    pending = "";
    await daemonCall("agent.send_input", {
      lane_id: target.laneId,
      window: target.window,
      text,
      enter: false,
    });
  }

  function push(text: string) {
    pending += text;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void flush(), 8);
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
    if (timer) clearTimeout(timer);
    timer = undefined;
    void flush();
  }

  return { push, flush, key, dispose };
}
