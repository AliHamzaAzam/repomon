import { createEffect, createSignal, onCleanup } from "solid-js";
import { daemonCall, type InputHistory } from "../ipc/rpc";

const EMPTY_HISTORY: InputHistory = { entries: [], source: "none" };

// Per-pane client cache, keyed by (lane_id, window): Up/Down must feel instant once the operator
// starts browsing, so a pane's history is fetched once on mount (same gate as the command
// catalog) and reused for the rest of that mount's life. The daemon reads the agent's own
// history file fresh per call; this is only about not re-issuing that read on every keystroke.
const cache = new Map<string, InputHistory>();

export function resetInputHistoryCacheForTests(): void {
  cache.clear();
}

/// Fetches and caches the agent's own recall history for one target - never a desktop-local
/// list. A failed fetch is distinct from a real answer (including the real answer "source: none",
/// which means this kind has no readable history store at all): same swallowed-error pattern as
/// the command catalog and the clipboard write before it, fixed the same way.
export function createInputHistory(target: () => { lane_id: number; window?: string } | null) {
  const [history, setHistory] = createSignal<InputHistory>(EMPTY_HISTORY);
  const [error, setError] = createSignal<string | null>(null);
  let epoch = 0;

  createEffect(() => {
    const params = target();
    const run = ++epoch;
    if (!params) { setHistory(EMPTY_HISTORY); setError(null); return; }
    const key = `${params.lane_id}:${params.window ?? ""}`;
    const cached = cache.get(key);
    if (cached) { setHistory(cached); setError(null); return; }
    let disposed = false;
    setError(null);
    void daemonCall("agent.input_history", params)
      .then((result) => {
        if (disposed || run !== epoch) return;
        const safe = result ?? EMPTY_HISTORY;
        cache.set(key, safe);
        setHistory(safe);
      })
      .catch((cause) => {
        if (disposed || run !== epoch) return;
        // Deliberately not cached: a later mount of the same (lane_id, window) gets a fresh
        // attempt instead of being stuck on this failure for the process lifetime.
        setHistory(EMPTY_HISTORY);
        setError(`Could not load input history: ${String(cause)}`);
      });
    onCleanup(() => { disposed = true; });
  });

  return { history, error };
}
