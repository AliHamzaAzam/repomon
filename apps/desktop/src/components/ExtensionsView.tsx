import { For, Show, createSignal } from "solid-js";

import type { FleetStore } from "../stores/fleet";
import type { ExtensionsStore, ExtFilter, ExtRow } from "../stores/extensions";
import ExtensionDrawer from "./ExtensionDrawer";
import SkillEditorModal from "./SkillEditorModal";

interface ExtensionsViewProps {
  store: ExtensionsStore;
  fleet: FleetStore;
}

const filters: ExtFilter[] = ["all", "plugins", "skills", "marketplaces"];

// Mirrors the daemon's valid_skill_name check (repomon-daemon/src/ext.rs) so bad names are
// rejected client-side instead of round-tripping to skill.create for an invalid_params error.
const skillNamePattern = /^[A-Za-z0-9_-]{1,64}$/;

function rowKey(row: ExtRow): string {
  return row.kind === "plugin" ? `p:${row.plugin.id}` : `s:${row.skill.path}`;
}

export default function ExtensionsView(props: ExtensionsViewProps) {
  const [selectedKey, setSelectedKey] = createSignal<string | null>(null);
  const [installOpen, setInstallOpen] = createSignal(false);
  const [installRef, setInstallRef] = createSignal("");
  const [marketplaceSource, setMarketplaceSource] = createSignal("");
  const [newSkillOpen, setNewSkillOpen] = createSignal(false);
  const [newSkillName, setNewSkillName] = createSignal("");
  const [newSkillDescription, setNewSkillDescription] = createSignal("");
  const [editorPath, setEditorPath] = createSignal<string | null>(null);
  const selected = () => props.store.rows().find((row) => rowKey(row) === selectedKey()) ?? null;
  const newSkillNameValid = () => skillNamePattern.test(newSkillName().trim());
  const scopeIsRepo = (repoId: number) => {
    const scope = props.store.scope();
    return scope.scope === "repo" && scope.repo_id === repoId;
  };
  const cliTitle = () => (props.store.cliAvailable() ? undefined : "Requires the claude CLI");

  async function submitInstall(event: Event) {
    event.preventDefault();
    const ref = installRef().trim();
    if (!ref) return;
    const ok = await props.store.install(ref);
    if (ok) {
      setInstallRef("");
      setInstallOpen(false);
    }
  }

  async function submitMarketplaceAdd(event: Event) {
    event.preventDefault();
    const source = marketplaceSource().trim();
    if (!source) return;
    const ok = await props.store.marketplaceAdd(source);
    if (ok) {
      setMarketplaceSource("");
    }
  }

  async function submitNewSkill(event: Event) {
    event.preventDefault();
    const name = newSkillName().trim();
    if (!skillNamePattern.test(name)) return;
    const description = newSkillDescription().trim() || undefined;
    const ok = await props.store.createSkill(name, description);
    if (ok) {
      setNewSkillName("");
      setNewSkillDescription("");
      setNewSkillOpen(false);
    }
  }

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
          <button
            type="button"
            class="focus-ring rounded-full border border-line bg-raised px-2.5 py-1 font-mono text-[0.58rem] uppercase text-muted disabled:opacity-40"
            disabled={props.store.busy() || !props.store.cliAvailable()}
            title={cliTitle()}
            onClick={() => setInstallOpen((open) => !open)}
          >+ Install</button>
          <button
            type="button"
            class="focus-ring rounded-full border border-line bg-raised px-2.5 py-1 font-mono text-[0.58rem] uppercase text-muted disabled:opacity-40"
            disabled={props.store.busy()}
            onClick={() => setNewSkillOpen((open) => !open)}
          >+ New skill</button>
        </div>
        <Show when={installOpen()}>
          <form class="flex items-center gap-2" onSubmit={submitInstall}>
            <input
              class="focus-ring min-w-0 flex-1 rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.7rem]"
              placeholder="plugin@marketplace"
              value={installRef()}
              onInput={(event) => setInstallRef(event.currentTarget.value)}
            />
            <button
              type="submit"
              class="focus-ring rounded-md border border-signal/40 bg-signal/10 px-2.5 py-1.5 font-mono text-[0.6rem] uppercase text-signal disabled:opacity-40"
              disabled={props.store.busy() || !props.store.cliAvailable()}
              title={cliTitle()}
            >Install</button>
          </form>
        </Show>
        <Show when={newSkillOpen()}>
          <form class="flex flex-col gap-1.5" onSubmit={submitNewSkill}>
            <div class="flex items-center gap-2">
              <input
                class="focus-ring min-w-0 flex-1 rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.7rem]"
                placeholder="skill-name"
                value={newSkillName()}
                onInput={(event) => setNewSkillName(event.currentTarget.value)}
              />
              <input
                class="focus-ring min-w-0 flex-[2] rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.7rem]"
                placeholder="Description (optional)"
                value={newSkillDescription()}
                onInput={(event) => setNewSkillDescription(event.currentTarget.value)}
              />
              <button
                type="submit"
                class="focus-ring rounded-md border border-signal/40 bg-signal/10 px-2.5 py-1.5 font-mono text-[0.6rem] uppercase text-signal disabled:opacity-40"
                disabled={props.store.busy() || !newSkillNameValid()}
              >Create</button>
            </div>
            <Show when={newSkillName().length > 0 && !newSkillNameValid()}>
              <p class="font-mono text-[0.6rem] text-fault">Use 1-64 letters, digits, dashes, or underscores.</p>
            </Show>
          </form>
        </Show>
        <Show when={props.store.error()}>
          {(error) => <p class="rounded-md border border-fault/40 bg-fault/10 px-3 py-2 font-mono text-[0.66rem] text-fault">{error()}</p>}
        </Show>
        <div class={`flex min-h-0 flex-1 flex-col gap-2 ${props.store.busy() ? "pointer-events-none opacity-60" : ""}`}>
          <Show
            when={props.store.filter() !== "marketplaces"}
            fallback={
              <>
                <ul class="min-h-0 flex-1 space-y-1 overflow-y-auto">
                  <For each={props.store.snapshot()?.marketplaces ?? []}>
                    {(marketplace) => (
                      <li class="flex items-center justify-between gap-2 rounded-md border border-line bg-raised px-3 py-2 font-mono text-[0.7rem]">
                        <span class="min-w-0 flex-1 truncate">
                          <span>{marketplace.name}</span>
                          <span class="ml-2 text-muted">{marketplace.kind} · {marketplace.reference}</span>
                        </span>
                        <span class="flex shrink-0 items-center gap-1">
                          <button
                            type="button"
                            class="focus-ring rounded border border-line px-2 py-0.5 font-mono text-[0.58rem] uppercase text-muted hover:text-foreground disabled:opacity-40"
                            disabled={!props.store.cliAvailable()}
                            title={cliTitle()}
                            onClick={() => void props.store.marketplaceRefresh(marketplace.name)}
                          >Refresh</button>
                          <button
                            type="button"
                            class="focus-ring rounded border border-line px-2 py-0.5 font-mono text-[0.58rem] uppercase text-muted hover:text-foreground disabled:opacity-40"
                            disabled={!props.store.cliAvailable()}
                            title={cliTitle()}
                            onClick={() => void props.store.marketplaceRemove(marketplace.name)}
                          >Remove</button>
                        </span>
                      </li>
                    )}
                  </For>
                </ul>
                <form class="flex items-center gap-2" onSubmit={submitMarketplaceAdd}>
                  <input
                    class="focus-ring min-w-0 flex-1 rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.7rem]"
                    placeholder="owner/repo or url"
                    value={marketplaceSource()}
                    onInput={(event) => setMarketplaceSource(event.currentTarget.value)}
                  />
                  <button
                    type="submit"
                    class="focus-ring rounded-md border border-signal/40 bg-signal/10 px-2.5 py-1.5 font-mono text-[0.6rem] uppercase text-signal disabled:opacity-40"
                    disabled={!props.store.cliAvailable()}
                    title={cliTitle()}
                  >+ Add marketplace</button>
                </form>
              </>
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
      </div>
      <Show when={selected()}>
        {(row) => (
          <ExtensionDrawer
            row={row()}
            store={props.store}
            onClose={() => setSelectedKey(null)}
            onEdit={(path) => setEditorPath(path)}
          />
        )}
      </Show>
      <Show when={editorPath()}>
        {(path) => <SkillEditorModal path={path()} onClose={() => setEditorPath(null)} />}
      </Show>
    </div>
  );
}
