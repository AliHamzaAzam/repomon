import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import type { FleetStore } from "../stores/fleet";
import type { ExtensionsStore, ExtFilter, ExtRow } from "../stores/extensions";
import ExtensionDrawer from "./ExtensionDrawer";
import { AgentIcon, IconPlus, IconSearch } from "./icons";
import SkillEditorModal from "./SkillEditorModal";

interface ExtensionsViewProps {
  store: ExtensionsStore;
  fleet: FleetStore;
}

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
  const currentAccount = createMemo(
    () => props.store.accounts().find((a) => a.key === props.store.account())
  );
  const availableFilters = createMemo<ExtFilter[]>(() =>
    currentAccount()?.claude !== false
      ? ["all", "plugins", "skills", "marketplaces"]
      : ["all", "plugins", "skills"]
  );
  const scopeIsRepo = (repoId: number) => {
    const scope = props.store.scope();
    return scope.scope === "repo" && scope.repo_id === repoId;
  };
  const cliTitle = () => (props.store.cliAvailable() ? undefined : "Requires the claude CLI");

  onMount(() => {
    void props.store.refresh();
    const onFocus = () => void props.store.refresh();
    window.addEventListener("focus", onFocus);
    onCleanup(() => window.removeEventListener("focus", onFocus));
  });

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
    <div class="flex h-full min-h-0 bg-background">
      <div class="flex min-w-0 flex-1 flex-col gap-3 p-4">
        <Show when={props.store.accounts().length > 0}>
          <div class="flex flex-wrap items-center gap-1.5">
            <span class="section-label mr-1">Agent Ecosystem:</span>
            <For each={props.store.accounts()}>
              {(acct) => {
                const isSelected = () => props.store.account() === acct.key;
                return (
                  <button
                    type="button"
                    class={`focus-ring inline-flex items-center gap-1.5 rounded-lg border px-2.5 py-1 text-xs transition-colors ${
                      isSelected()
                        ? "border-signal/50 bg-signal/10 text-signal font-semibold shadow-xs"
                        : "border-line bg-surface text-muted hover:border-line hover:bg-raised hover:text-foreground"
                    }`}
                    onClick={() => {
                      props.store.setAccount(acct.key);
                      if (acct.claude === false && props.store.filter() === "marketplaces") {
                        props.store.setFilter("all");
                      }
                    }}
                  >
                    <AgentIcon agent={acct.agent_kind ?? acct.key} size={14} />
                    <span>{acct.label}</span>
                  </button>
                );
              }}
            </For>
          </div>
        </Show>
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="section-label mr-1">Scope:</span>
          <button
            type="button"
            class={`focus-ring rounded-lg border px-2.5 py-1 text-xs font-medium transition-colors ${
              props.store.scope().scope === "global"
                ? "border-signal/50 bg-signal/10 text-signal font-semibold"
                : "border-line bg-surface text-muted hover:text-foreground"
            }`}
            onClick={() => props.store.setScope({ scope: "global" })}
          >Global</button>
          <For each={props.fleet.visibleRepos()}>
            {(repo) => (
              <button
                type="button"
                class={`focus-ring rounded-lg border px-2.5 py-1 text-xs font-medium transition-colors ${
                  scopeIsRepo(repo.id)
                    ? "border-signal/50 bg-signal/10 text-signal font-semibold"
                    : "border-line bg-surface text-muted hover:text-foreground"
                }`}
                onClick={() => props.store.setScope({ scope: "repo", repo_id: repo.id })}
              >{repo.name}</button>
            )}
          </For>
        </div>
        <div class="flex items-center gap-2">
          <div class="relative min-w-0 flex-1">
            <input
              class="focus-ring h-8 w-full rounded-lg border border-line bg-surface pl-8 pr-3 text-xs text-foreground outline-none placeholder:text-muted/60"
              placeholder="Search extensions & skills…"
              value={props.store.query()}
              onInput={(event) => props.store.setQuery(event.currentTarget.value)}
            />
            <span class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted">
              <IconSearch size={13} />
            </span>
          </div>
          <div class="flex items-center rounded-lg border border-line bg-raised/50 p-0.5" role="group" aria-label="Extension filters">
            <For each={availableFilters()}>
              {(filter) => (
                <button
                  type="button"
                  class={`focus-ring rounded-md px-2.5 py-1 text-xs font-medium capitalize transition-colors ${
                    props.store.filter() === filter
                      ? "bg-surface text-foreground shadow-xs font-semibold"
                      : "text-muted hover:text-foreground"
                  }`}
                  onClick={() => props.store.setFilter(filter)}
                >{filter}</button>
              )}
            </For>
          </div>
          <Show when={currentAccount()?.claude !== false}>
            <button
              type="button"
              class="focus-ring inline-flex h-8 items-center gap-1.5 rounded-lg border border-line bg-surface px-3 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
              disabled={props.store.busy() || !props.store.cliAvailable()}
              title={cliTitle()}
              onClick={() => setInstallOpen((open) => !open)}
            >
              <IconPlus size={12} />
              <span>Install</span>
            </button>
          </Show>
          <button
            type="button"
            class="focus-ring inline-flex h-8 items-center gap-1.5 rounded-lg border border-line bg-surface px-3 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
            disabled={props.store.busy()}
            onClick={() => setNewSkillOpen((open) => !open)}
          >
            <IconPlus size={12} />
            <span>New Skill</span>
          </button>
        </div>
        <Show when={installOpen()}>
          <form class="flex items-center gap-2 rounded-xl border border-line bg-surface p-2" onSubmit={submitInstall}>
            <input
              class="focus-ring h-8 min-w-0 flex-1 rounded-lg border border-line bg-background px-3 font-mono text-xs text-foreground outline-none placeholder:text-muted/60"
              placeholder="plugin@marketplace"
              value={installRef()}
              onInput={(event) => setInstallRef(event.currentTarget.value)}
            />
            <button
              type="submit"
              class="focus-ring rounded-lg bg-signal px-3.5 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-40"
              disabled={props.store.busy() || !props.store.cliAvailable()}
              title={cliTitle()}
            >Install</button>
          </form>
        </Show>
        <Show when={newSkillOpen()}>
          <form class="flex flex-col gap-2 rounded-xl border border-line bg-surface p-3" onSubmit={submitNewSkill}>
            <div class="flex items-center gap-2">
              <input
                class="focus-ring h-8 min-w-0 flex-1 rounded-lg border border-line bg-background px-3 font-mono text-xs text-foreground outline-none placeholder:text-muted/60"
                placeholder="skill-name"
                value={newSkillName()}
                onInput={(event) => setNewSkillName(event.currentTarget.value)}
              />
              <input
                class="focus-ring h-8 min-w-0 flex-[2] rounded-lg border border-line bg-background px-3 text-xs text-foreground outline-none placeholder:text-muted/60"
                placeholder="Description (optional)"
                value={newSkillDescription()}
                onInput={(event) => setNewSkillDescription(event.currentTarget.value)}
              />
              <button
                type="submit"
                class="focus-ring rounded-lg bg-signal px-3.5 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-40"
                disabled={props.store.busy() || !newSkillNameValid()}
              >Create</button>
            </div>
            <Show when={newSkillName().length > 0 && !newSkillNameValid()}>
              <p class="font-mono text-xs text-fault">Use 1-64 letters, digits, dashes, or underscores.</p>
            </Show>
            <p class="text-[11px] text-muted">Changes apply to new agent sessions.</p>
          </form>
        </Show>
        <Show when={props.store.error()}>
          {(error) => <p class="rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault">{error()}</p>}
        </Show>
        <div class={`flex min-h-0 flex-1 flex-col gap-2 ${props.store.busy() ? "pointer-events-none opacity-60" : ""}`}>
          <Show
            when={props.store.filter() !== "marketplaces"}
            fallback={
              <>
                <ul class="min-h-0 flex-1 space-y-1.5 overflow-y-auto">
                  <For each={props.store.snapshot()?.marketplaces ?? []}>
                    {(marketplace) => (
                      <li class="flex items-center justify-between gap-2 rounded-xl border border-line bg-surface p-3 text-xs">
                        <span class="min-w-0 flex-1 truncate">
                          <span class="font-semibold text-foreground">{marketplace.name}</span>
                          <span class="ml-2 font-mono text-[11px] text-muted">{marketplace.kind} · {marketplace.reference}</span>
                        </span>
                        <span class="flex shrink-0 items-center gap-1.5">
                          <button
                            type="button"
                            class="focus-ring rounded-lg border border-line bg-surface px-2.5 py-1 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                            disabled={!props.store.cliAvailable()}
                            title={cliTitle()}
                            onClick={() => void props.store.marketplaceRefresh(marketplace.name)}
                          >Refresh</button>
                          <button
                            type="button"
                            class="focus-ring rounded-lg border border-fault/30 bg-fault/10 px-2.5 py-1 text-xs font-medium text-fault transition-colors hover:bg-fault/20 disabled:opacity-40"
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
                    class="focus-ring h-8 min-w-0 flex-1 rounded-lg border border-line bg-surface px-3 font-mono text-xs text-foreground outline-none placeholder:text-muted/60"
                    placeholder="owner/repo or https://..."
                    value={marketplaceSource()}
                    onInput={(event) => setMarketplaceSource(event.currentTarget.value)}
                  />
                  <button
                    type="submit"
                    class="focus-ring rounded-lg bg-signal px-3.5 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-40"
                    disabled={!props.store.cliAvailable()}
                    title={cliTitle()}
                  >+ Add Marketplace</button>
                </form>
              </>
            }
          >
            <ul class="min-h-0 flex-1 space-y-1.5 overflow-y-auto" aria-label="Extensions">
              <For each={props.store.rows()}>
                {(row) => (
                  <li>
                    <button
                      type="button"
                      class={`focus-ring flex w-full items-center justify-between gap-2 rounded-xl border p-3 text-left transition-all ${
                        selectedKey() === rowKey(row)
                          ? "border-signal bg-signal/5 ring-1 ring-signal/20"
                          : "border-line bg-surface hover:border-muted/50 hover:bg-raised/40"
                      }`}
                      onClick={() => setSelectedKey(rowKey(row))}
                    >
                      <span class="flex min-w-0 items-center gap-2.5 truncate">
                        <span class="truncate text-xs font-semibold text-foreground">
                          {row.kind === "plugin" ? row.plugin.name : row.skill.name}
                        </span>
                        <span class="rounded-full border border-line bg-raised px-2 py-0.5 font-mono text-[9px] uppercase tracking-wider text-muted font-medium">
                          {row.kind}
                        </span>
                        <span class="truncate font-mono text-[11px] text-muted">
                          {row.kind === "plugin" ? row.plugin.marketplace : row.skill.source}
                        </span>
                      </span>
                      <Show when={row.kind === "plugin" ? row : null} keyed>
                        {(pluginRow) => (
                          <span class={`font-mono text-xs font-semibold ${pluginRow.plugin.enabled ? "text-signal" : "text-muted"}`}>
                            {pluginRow.plugin.enabled ? "enabled" : "disabled"}
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
