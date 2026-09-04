import { createSignal } from "solid-js";

import type { RepomindStatus } from "../bindings";
import { daemonCall, subscribeDaemon, type DaemonEvent } from "../ipc/rpc";

/// How often `repomind.status` is re-read. Matches the fleet heartbeat so the pinned Repomind row
/// never lags the lane rows beside it by a beat.
export const REPOMIND_POLL_MS = 1200;

export interface RepomindSource {
  status(): Promise<RepomindStatus>;
  boot(): Promise<void>;
  export(): Promise<void>;
  subscribe(onEvent: (event: DaemonEvent) => void): Promise<() => void>;
}

export const daemonRepomindSource: RepomindSource = {
  status: () => daemonCall("repomind.status"),
  boot: async () => {
    await daemonCall("repomind.boot");
  },
  export: async () => {
    await daemonCall("repomind.export");
  },
  subscribe: subscribeDaemon,
};

/// The `event.agent.status` payload the daemon broadcasts when a session changes state. Only the
/// window matters here: it says whether the change happened inside the controller lane.
interface AgentStatusEvent {
  window?: unknown;
}

/// Reads `repomind.status` for every surface that shows the home: the pinned sidebar row (state,
/// controller count, active goals) and the Repomind panel (boot context, export state, counts).
///
/// One store rather than a poller per surface, so the row and the panel can never disagree about
/// the same home. On top of the heartbeat it refreshes immediately when an agent in the controller
/// lane changes state, which is when the numbers actually move.
export function createRepomindStore(source: RepomindSource = daemonRepomindSource) {
  const [status, setStatus] = createSignal<RepomindStatus | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<"boot" | "export" | null>(null);

  let active = false;
  let inFlight = false;
  let token = 0;
  let interval: ReturnType<typeof setInterval> | undefined;
  let unsubscribe: (() => void) | undefined;
  // Windows the controller lane owns, kept up to date by the caller so the event filter does not
  // need its own copy of the fleet.
  let controllerWindows: ReadonlySet<string> = new Set();

  function message(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
  }

  async function refresh() {
    // `repomind.status` walks the home's directories; overlapping calls would queue disk work
    // behind a heartbeat that has already moved on.
    if (inFlight) return;
    inFlight = true;
    const mine = ++token;
    try {
      const next = await source.status();
      // A slower call that resolves after a newer one must not roll the state back.
      if (!active || mine !== token) return;
      setStatus(next);
      setError(null);
    } catch (cause) {
      if (active && mine === token) setError(message(cause));
    } finally {
      inFlight = false;
    }
  }

  /// Tell the store which tmux windows belong to the controller lane, so `event.agent.status` for
  /// one of them can trigger an immediate re-read.
  function setControllerWindows(windows: string[]) {
    controllerWindows = new Set(windows);
  }

  function onEvent(event: DaemonEvent) {
    if (event.method !== "event.agent.status") return;
    const window = (event.params as AgentStatusEvent).window;
    if (typeof window === "string" && controllerWindows.has(window)) void refresh();
  }

  /// Rewrite the boot document, then re-read the status so the panel shows the new size and trim
  /// list rather than the one it was assembled from.
  async function regenerateBoot() {
    setBusy("boot");
    setError(null);
    try {
      await source.boot();
      await refresh();
    } catch (cause) {
      if (active) setError(message(cause));
    } finally {
      setBusy(null);
    }
  }

  /// Run the daemon's one-way export now instead of waiting out its debounce.
  async function runExport() {
    setBusy("export");
    setError(null);
    try {
      await source.export();
      await refresh();
    } catch (cause) {
      if (active) setError(message(cause));
    } finally {
      setBusy(null);
    }
  }

  function start() {
    if (active) return;
    active = true;
    void refresh();
    interval = setInterval(() => void refresh(), REPOMIND_POLL_MS);
    void source
      .subscribe(onEvent)
      .then((stop) => {
        if (active) unsubscribe = stop;
        else stop();
      })
      .catch(() => undefined);
  }

  function stop() {
    active = false;
    if (interval) clearInterval(interval);
    interval = undefined;
    unsubscribe?.();
    unsubscribe = undefined;
  }

  return {
    status,
    error,
    busy,
    dismissError: () => setError(null),
    setControllerWindows,
    refresh,
    regenerateBoot,
    runExport,
    start,
    stop,
  };
}

export type RepomindStore = ReturnType<typeof createRepomindStore>;
