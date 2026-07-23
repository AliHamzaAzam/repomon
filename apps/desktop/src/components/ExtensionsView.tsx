import { For, Show, createSignal } from "solid-js";

import type { FleetStore } from "../stores/fleet";
import type { ExtensionsStore, ExtFilter, ExtRow } from "../stores/extensions";
import ExtensionDrawer from "./ExtensionDrawer";

interface ExtensionsViewProps {
  store: ExtensionsStore;
  fleet: FleetStore;
}

const filters: ExtFilter[] = ["all", "plugins", "skills", "marketplaces"];

function rowKey(row: ExtRow): string {
  return row.kind === "plugin" ? `p:${row.plugin.id}` : `s:${row.skill.path}`;
}

export default function ExtensionsView(props: ExtensionsViewProps) {
  const [selectedKey, setSelectedKey] = createSignal<string | null>(null);
  const selected = () => props.store.rows().find((row) => rowKey(row) === selectedKey()) ?? null;
  const scopeIsRepo = (repoId: number) => {
    const scope = props.store.scope();
    return scope.scope === "repo" && scope.repo_id === repoId;
  };

  return (
    <div class="flex h-full min-h-0">
      <div class="flex min-w-0 flex-1 flex-col gap-3 p-4">
        <div class="flex flex-wrap items-center gap-2">
          <button
            type="button"
            class={`focus-ring rounded-md border px-2.5 py-1 font-mono text-[0.62rem] uppercase tracking-[0.1em] ${props.store.scope().scope === "global" ? "border-signal/40 bg-signal/10 text-signal" : "border-line bg-raised text-muted"}`}
            onClick={() => props.store.setScope({ scope: "global" })}
          >Global</button>
          <For each={props.fleet.repos()}>
            {(repo) => (
              <button
                type="button"
                class={`focus-ring rounded-md border px-2.5 py-1 font-mono text-[0.62rem] ${scopeIsRepo(repo.id) ? "border-signal/40 bg-signal/10 text-signal" : "border-line bg-raised text-muted"}`}
                onClick={() => props.store.setScope({ scope: "repo", repo_id: repo.id })}
              >{repo.name}</button>
            )}
          </For>
        </div>
        <div class="flex items-center gap-2">
          <input
            class="focus-ring min-w-0 flex-1 rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.7rem]"
            placeholder="Search extensions"
            value={props.store.query()}
            onInput={(event) => props.store.setQuery(event.currentTarget.value)}
          />
          <For each={filters}>
            {(filter) => (
              <button
                type="button"
                class={`focus-ring rounded-full border px-2.5 py-1 font-mono text-[0.58rem] uppercase ${props.store.filter() === filter ? "border-signal/40 bg-signal/10 text-signal" : "border-line bg-raised text-muted"}`}
                onClick={() => props.store.setFilter(filter)}
              >{filter}</button>
            )}
          </For>
        </div>
        <Show when={props.store.error()}>
          {(error) => <p class="rounded-md border border-fault/40 bg-fault/10 px-3 py-2 font-mono text-[0.66rem] text-fault">{error()}</p>}
        </Show>
        <Show
          when={props.store.filter() !== "marketplaces"}
          fallback={
            <ul class="min-h-0 flex-1 space-y-1 overflow-y-auto">
              <For each={props.store.snapshot()?.marketplaces ?? []}>
                {(marketplace) => (
                  <li class="flex items-center justify-between rounded-md border border-line bg-raised px-3 py-2 font-mono text-[0.7rem]">
                    <span>{marketplace.name}</span>
                    <span class="text-muted">{marketplace.kind} · {marketplace.reference}</span>
                  </li>
                )}
              </For>
            </ul>
          }
        >
          <ul class="min-h-0 flex-1 space-y-1 overflow-y-auto" aria-label="Extensions">
            <For each={props.store.rows()}>
              {(row) => (
                <li>
                  <button
                    type="button"
                    class={`focus-ring flex w-full items-center justify-between gap-2 rounded-md border px-3 py-2 text-left font-mono text-[0.72rem] ${selectedKey() === rowKey(row) ? "border-signal/40 bg-signal/10" : "border-line bg-raised hover:border-signal/30"}`}
                    onClick={() => setSelectedKey(rowKey(row))}
                  >
                    <span class="flex min-w-0 items-center gap-2 truncate">
                      <span class="truncate">{row.kind === "plugin" ? row.plugin.name : row.skill.name}</span>
                      <span class="rounded-full border border-line px-1.5 text-[0.55rem] uppercase text-muted">{row.kind}</span>
                      <span class="text-[0.58rem] text-muted">
                        {row.kind === "plugin" ? row.plugin.marketplace : row.skill.source}
                      </span>
                    </span>
                    <Show when={row.kind === "plugin" ? row : null} keyed>
                      {(pluginRow) => (
                        <span class={`text-[0.6rem] ${pluginRow.plugin.enabled ? "text-signal" : "text-muted"}`}>
                          {pluginRow.plugin.enabled ? "on" : "off"}
                        </span>
                      )}
                    </Show>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
      <Show when={selected()}>
        {(row) => <ExtensionDrawer row={row()} store={props.store} onClose={() => setSelectedKey(null)} />}
      </Show>
    </div>
  );
}
