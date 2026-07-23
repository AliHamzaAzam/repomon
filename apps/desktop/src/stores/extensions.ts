import { createMemo, createSignal } from "solid-js";

import type { ExtSnapshot, PluginInfo, SkillInfo } from "../bindings";
import { daemonCall, subscribeDaemon, type DaemonEvent, type ExtScopeParams } from "../ipc/rpc";

export type ExtFilter = "all" | "plugins" | "skills" | "marketplaces";

export type ExtRow =
  | { kind: "plugin"; plugin: PluginInfo }
  | { kind: "skill"; skill: SkillInfo };

export interface ExtSource {
  list(scope: ExtScopeParams): Promise<ExtSnapshot>;
  setEnabled(id: string, enabled: boolean, scope: ExtScopeParams): Promise<unknown>;
  subscribe?(onEvent: (event: DaemonEvent) => void): Promise<() => void>;
}

export const daemonExtSource: ExtSource = {
  list: (scope) => daemonCall("ext.list", scope),
  setEnabled: (id, enabled, scope) =>
    daemonCall(enabled ? "plugin.enable" : "plugin.disable", { id, ...scope }),
  subscribe: subscribeDaemon,
};

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function createExtensionsStore(source: ExtSource = daemonExtSource) {
  const [scope, setScopeSignal] = createSignal<ExtScopeParams>({ scope: "global" });
  const [query, setQuery] = createSignal("");
  const [filter, setFilter] = createSignal<ExtFilter>("all");
  const [snapshot, setSnapshot] = createSignal<ExtSnapshot | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function refresh() {
    setBusy(true);
    try {
      setSnapshot(await source.list(scope()));
      setError(null);
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }

  function setScope(next: ExtScopeParams) {
    setScopeSignal(next);
    void refresh();
  }

  async function setEnabled(id: string, enabled: boolean) {
    setBusy(true);
    try {
      await source.setEnabled(id, enabled, scope());
      setError(null);
      await refresh();
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }

  const rows = createMemo<ExtRow[]>(() => {
    const snap = snapshot();
    if (!snap) return [];
    const q = query().trim().toLowerCase();
    const active = filter();
    const rows: ExtRow[] = [];
    if (active === "all" || active === "plugins") {
      for (const plugin of snap.plugins) rows.push({ kind: "plugin", plugin });
    }
    if (active === "all" || active === "skills") {
      for (const skill of snap.skills) rows.push({ kind: "skill", skill });
    }
    if (!q) return rows;
    return rows.filter((row) => {
      const text = row.kind === "plugin"
        ? `${row.plugin.id} ${row.plugin.name}`
        : `${row.skill.name} ${row.skill.description ?? ""}`;
      return text.toLowerCase().includes(q);
    });
  });

  void refresh();

  // Every client (this app, the TUI, iOS) refreshes on event.ext.changed so a toggle made
  // elsewhere shows up here without waiting on a poll. Fire-and-forget: this store is created
  // once for the app's lifetime, so there is no matching teardown to unsubscribe against.
  void source
    .subscribe?.((event) => {
      if (event.method === "event.ext.changed") void refresh();
    })
    ?.catch(() => undefined);

  return { scope, setScope, query, setQuery, filter, setFilter, snapshot, rows, busy, error, refresh, setEnabled };
}

export type ExtensionsStore = ReturnType<typeof createExtensionsStore>;
