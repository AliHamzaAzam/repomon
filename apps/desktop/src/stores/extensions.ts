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
  update(id: string | undefined, account: string): Promise<unknown>;
  details(id: string, account: string): Promise<string>;
  marketplaceAdd(source: string, account: string): Promise<unknown>;
  marketplaceRemove(name: string, account: string): Promise<unknown>;
  marketplaceRefresh(name: string | undefined, account: string): Promise<unknown>;
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
  update: (id, account) => daemonCall("plugin.update", { id, account }),
  details: async (id, account) => (await daemonCall("plugin.details", { id, account })).text,
  marketplaceAdd: (source, account) => daemonCall("marketplace.add", { source, account }),
  marketplaceRemove: (name, account) => daemonCall("marketplace.remove", { name, account }),
  marketplaceRefresh: (name, account) => daemonCall("marketplace.refresh", { name, account }),
  createSkill: (name, description, scope) => daemonCall("skill.create", { name, description, ...scope }),
  deleteSkill: (name, scope) => daemonCall("skill.delete", { name, ...scope }),
  subscribe: subscribeDaemon,
};

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function createExtensionsStore(source: ExtSource = daemonExtSource) {
  const [scope, setScopeSignal] = createSignal<ExtScopeParams>({ scope: "global" });
  // Which Claude account (config dir) the view targets. Orthogonal to global/repo scope. Keyed the
  // same as the usage probe: "default" = ~/.claude, a config-dir path for a variant, "codex".
  const [account, setAccountSignal] = createSignal<string>("default");
  const [query, setQuery] = createSignal("");
  const [filter, setFilter] = createSignal<ExtFilter>("all");
  const [snapshot, setSnapshot] = createSignal<ExtSnapshot | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [detailsCache, setDetailsCache] = createSignal<Record<string, string>>({});
  const [detailsErrorCache, setDetailsErrorCache] = createSignal<Record<string, string>>({});

  // Every daemon call carries the scope and the selected account.
  const params = (): ExtScopeParams => ({ ...scope(), account: account() });

  // Accounts to offer in the picker, surfaced by the daemon in each snapshot.
  const accounts = createMemo(() => snapshot()?.accounts ?? []);

  async function refresh() {
    setBusy(true);
    try {
      setSnapshot(await source.list(params()));
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

  function setAccount(next: string) {
    setAccountSignal(next);
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
    return mutate(() => source.setEnabled(id, enabled, params()));
  }

  async function install(ref: string): Promise<boolean> {
    return mutate(() => source.install(ref, params()));
  }

  async function remove(id: string): Promise<boolean> {
    return mutate(() => source.remove(id, params()));
  }

  async function update(id?: string): Promise<boolean> {
    return mutate(() => source.update(id, account()));
  }

  async function marketplaceAdd(value: string): Promise<boolean> {
    return mutate(() => source.marketplaceAdd(value, account()));
  }

  async function marketplaceRemove(name: string): Promise<boolean> {
    return mutate(() => source.marketplaceRemove(name, account()));
  }

  async function marketplaceRefresh(name?: string): Promise<boolean> {
    return mutate(() => source.marketplaceRefresh(name, account()));
  }

  async function createSkill(name: string, description?: string): Promise<boolean> {
    return mutate(() => source.createSkill(name, description, params()));
  }

  async function deleteSkill(name: string): Promise<boolean> {
    return mutate(() => source.deleteSkill(name, params()));
  }

  async function loadDetails(id: string): Promise<void> {
    try {
      const text = await source.details(id, account());
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
    account,
    setAccount,
    accounts,
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
