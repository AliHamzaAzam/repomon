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
  install(ref: string, scope: ExtScopeParams): Promise<unknown>;
  remove(id: string, scope: ExtScopeParams): Promise<unknown>;
  update(id: string | undefined): Promise<unknown>;
  details(id: string): Promise<string>;
  marketplaceAdd(source: string): Promise<unknown>;
  marketplaceRemove(name: string): Promise<unknown>;
  marketplaceRefresh(name: string | undefined): Promise<unknown>;
  createSkill(name: string, description: string | undefined, scope: ExtScopeParams): Promise<unknown>;
  deleteSkill(name: string, scope: ExtScopeParams): Promise<unknown>;
  subscribe?(onEvent: (event: DaemonEvent) => void): Promise<() => void>;
}

export const daemonExtSource: ExtSource = {
  list: (scope) => daemonCall("ext.list", scope),
  setEnabled: (id, enabled, scope) =>
    daemonCall(enabled ? "plugin.enable" : "plugin.disable", { id, ...scope }),
  install: (ref, scope) => daemonCall("plugin.install", { ref, ...scope }),
  remove: (id, scope) => daemonCall("plugin.remove", { id, ...scope }),
  update: (id) => daemonCall("plugin.update", { id }),
  details: async (id) => (await daemonCall("plugin.details", { id })).text,
  marketplaceAdd: (source) => daemonCall("marketplace.add", { source }),
  marketplaceRemove: (name) => daemonCall("marketplace.remove", { name }),
  marketplaceRefresh: (name) => daemonCall("marketplace.refresh", { name }),
  createSkill: (name, description, scope) => daemonCall("skill.create", { name, description, ...scope }),
  deleteSkill: (name, scope) => daemonCall("skill.delete", { name, ...scope }),
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
  const [detailsCache, setDetailsCache] = createSignal<Record<string, string>>({});
  const [detailsErrorCache, setDetailsErrorCache] = createSignal<Record<string, string>>({});

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
    setDetailsCache({});
    setDetailsErrorCache({});
    void refresh();
  }

  async function mutate(op: () => Promise<unknown>): Promise<boolean> {
    setBusy(true);
    try {
      await op();
      setError(null);
      await refresh();
      return true;
    } catch (cause) {
      setError(message(cause));
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function setEnabled(id: string, enabled: boolean): Promise<boolean> {
    return mutate(() => source.setEnabled(id, enabled, scope()));
  }

  async function install(ref: string): Promise<boolean> {
    return mutate(() => source.install(ref, scope()));
  }

  async function remove(id: string): Promise<boolean> {
    return mutate(() => source.remove(id, scope()));
  }

  async function update(id?: string): Promise<boolean> {
    return mutate(() => source.update(id));
  }

  async function marketplaceAdd(value: string): Promise<boolean> {
    return mutate(() => source.marketplaceAdd(value));
  }

  async function marketplaceRemove(name: string): Promise<boolean> {
    return mutate(() => source.marketplaceRemove(name));
  }

  async function marketplaceRefresh(name?: string): Promise<boolean> {
    return mutate(() => source.marketplaceRefresh(name));
  }

  async function createSkill(name: string, description?: string): Promise<boolean> {
    return mutate(() => source.createSkill(name, description, scope()));
  }

  async function deleteSkill(name: string): Promise<boolean> {
    return mutate(() => source.deleteSkill(name, scope()));
  }

  async function loadDetails(id: string): Promise<void> {
    try {
      const text = await source.details(id);
      setDetailsCache((prev) => ({ ...prev, [id]: text }));
      setDetailsErrorCache((prev) => {
        if (!(id in prev)) return prev;
        const next = { ...prev };
        delete next[id];
        return next;
      });
    } catch (cause) {
      setDetailsErrorCache((prev) => ({ ...prev, [id]: message(cause) }));
    }
  }

  function detailsFor(id: string): { text: string | null; error: string | null } {
    return { text: detailsCache()[id] ?? null, error: detailsErrorCache()[id] ?? null };
  }

  function cliAvailable(): boolean {
    return snapshot()?.cli_version != null;
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

  return {
    scope,
    setScope,
    query,
    setQuery,
    filter,
    setFilter,
    snapshot,
    rows,
    busy,
    error,
    refresh,
    setEnabled,
    install,
    remove,
    update,
    detailsFor,
    loadDetails,
    marketplaceAdd,
    marketplaceRemove,
    marketplaceRefresh,
    createSkill,
    deleteSkill,
    cliAvailable,
  };
}

export type ExtensionsStore = ReturnType<typeof createExtensionsStore>;
