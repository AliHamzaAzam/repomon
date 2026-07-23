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

  async function mutate(op: () => Promise<unknown>) {
    setBusy(true);
    try {
      await op();
      setError(null);
      await refresh();
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }

  async function setEnabled(id: string, enabled: boolean) {
    await mutate(() => source.setEnabled(id, enabled, scope()));
  }

  async function install(ref: string) {
    await mutate(() => source.install(ref, scope()));
  }

  async function remove(id: string) {
    await mutate(() => source.remove(id, scope()));
  }

  async function update(id?: string) {
    await mutate(() => source.update(id));
  }

  async function marketplaceAdd(value: string) {
    await mutate(() => source.marketplaceAdd(value));
  }

  async function marketplaceRemove(name: string) {
    await mutate(() => source.marketplaceRemove(name));
  }

  async function marketplaceRefresh(name?: string) {
    await mutate(() => source.marketplaceRefresh(name));
  }

  async function details(id: string): Promise<string> {
    return source.details(id);
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
    details,
    marketplaceAdd,
    marketplaceRemove,
    marketplaceRefresh,
    cliAvailable,
  };
}

export type ExtensionsStore = ReturnType<typeof createExtensionsStore>;
