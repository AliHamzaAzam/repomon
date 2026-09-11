import { createEffect, createSignal, onCleanup } from "solid-js";
import { daemonCall, type CommandCatalog } from "../ipc/rpc";

const EMPTY_CATALOG: CommandCatalog = { commands: [], models: [], model_command: null };

// Per-pane client cache, keyed by (lane_id, window): the palette opens on a keystroke and must
// feel instant, so a pane's catalog is fetched once on mount and reused for the rest of that
// mount's life. The daemon keeps its own cache (with its own invalidation) behind the RPC; this
// is only about not re-issuing the call every time the operator toggles the palette open.
const cache = new Map<string, CommandCatalog>();

export function resetCommandCatalogCacheForTests(): void {
  cache.clear();
}

/// Fetches and caches the command/model catalog for one target. A failed or missing fetch is
/// treated the same as an empty catalog - never a guess, per the contract's own rule that a
/// fabricated command is worse than none.
export function createCommandCatalog(target: () => { lane_id: number; window?: string } | null) {
  const [catalog, setCatalog] = createSignal<CommandCatalog>(EMPTY_CATALOG);
  const [loading, setLoading] = createSignal(false);
  let epoch = 0;

  createEffect(() => {
    const params = target();
    const run = ++epoch;
    if (!params) { setCatalog(EMPTY_CATALOG); return; }
    const key = `${params.lane_id}:${params.window ?? ""}`;
    const cached = cache.get(key);
    if (cached) { setCatalog(cached); return; }
    let disposed = false;
    setLoading(true);
    void daemonCall("agent.command_catalog", params)
      .then((result) => {
        if (disposed || run !== epoch) return;
        const safe = result ?? EMPTY_CATALOG;
        cache.set(key, safe);
        setCatalog(safe);
      })
      .catch(() => {
        if (disposed || run !== epoch) return;
        setCatalog(EMPTY_CATALOG);
      })
      .finally(() => {
        if (!disposed && run === epoch) setLoading(false);
      });
    onCleanup(() => { disposed = true; });
  });

  return { catalog, loading };
}
