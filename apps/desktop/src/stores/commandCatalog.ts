import { createEffect, createSignal, onCleanup } from "solid-js";
import { daemonCall } from "../ipc/rpc";
import type { CommandCatalog } from "../bindings";

const EMPTY_CATALOG: CommandCatalog = { commands: [], models: [], model_command: null };

// Per-pane client cache, keyed by (lane_id, window): the palette opens on a keystroke and must
// feel instant, so a pane's catalog is fetched once on mount and reused for the rest of that
// mount's life. The daemon keeps its own cache (with its own invalidation) behind the RPC; this
// is only about not re-issuing the call every time the operator toggles the palette open.
const cache = new Map<string, CommandCatalog>();

export function resetCommandCatalogCacheForTests(): void {
  cache.clear();
}

/// Fetches and caches the command/model catalog for one target. An empty result the daemon
/// actually returned is a legitimate answer ("no commands known"), per the contract's own rule
/// that a fabricated command is worse than none - but a *failed* fetch is not that, and must
/// never be reported as one. Same pattern as the clipboard-write failure this mirrors: surface
/// the failure distinctly via `error`, and never fold it into a signal that also means "asked
/// and got nothing back".
export function createCommandCatalog(target: () => { lane_id: number; window?: string } | null) {
  const [catalog, setCatalog] = createSignal<CommandCatalog>(EMPTY_CATALOG);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let epoch = 0;

  createEffect(() => {
    const params = target();
    const run = ++epoch;
    if (!params) { setCatalog(EMPTY_CATALOG); setError(null); return; }
    const key = `${params.lane_id}:${params.window ?? ""}`;
    const cached = cache.get(key);
    if (cached) { setCatalog(cached); setError(null); return; }
    let disposed = false;
    setLoading(true);
    setError(null);
    void daemonCall("agent.command_catalog", params)
      .then((result) => {
        if (disposed || run !== epoch) return;
        const safe = result ?? EMPTY_CATALOG;
        cache.set(key, safe);
        setCatalog(safe);
      })
      .catch((cause) => {
        if (disposed || run !== epoch) return;
        // Deliberately not cached: a later mount of the same (lane_id, window) gets a fresh
        // attempt instead of being stuck on this failure for the process lifetime.
        setCatalog(EMPTY_CATALOG);
        setError(`Could not load commands: ${String(cause)}`);
      })
      .finally(() => {
        if (!disposed && run === epoch) setLoading(false);
      });
    onCleanup(() => { disposed = true; });
  });

  return { catalog, loading, error };
}
