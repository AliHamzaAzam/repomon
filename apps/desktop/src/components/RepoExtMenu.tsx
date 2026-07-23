import { For, Show, createResource } from "solid-js";

import type { PluginInfo } from "../bindings";
import { daemonCall } from "../ipc/rpc";

interface RepoExtMenuProps {
  repoId: number;
  x: number;
  y: number;
  onOpenExtensions: () => void;
  onClose: () => void;
}

export default function RepoExtMenu(props: RepoExtMenuProps) {
  const [snapshot, { refetch }] = createResource(() =>
    daemonCall("ext.list", { scope: "repo", repo_id: props.repoId }),
  );

  async function toggle(plugin: PluginInfo) {
    await daemonCall(plugin.enabled ? "plugin.disable" : "plugin.enable", {
      id: plugin.id,
      scope: "repo",
      repo_id: props.repoId,
    }).catch(() => undefined);
    void refetch();
  }

  return (
    <>
      <div class="fixed inset-0 z-40" onClick={() => props.onClose()} />
      <div
        class="fixed z-50 w-56 rounded-md border border-line bg-surface p-1 shadow-lg"
        style={{ left: `${props.x}px`, top: `${props.y}px` }}
        role="menu"
      >
        <button
          type="button"
          class="focus-ring block w-full rounded px-2 py-1.5 text-left font-mono text-[0.66rem] text-foreground hover:bg-raised"
          onClick={() => { props.onOpenExtensions(); props.onClose(); }}
          role="menuitem"
        >Extensions…</button>
        <div class="my-1 border-t border-line" />
        <Show when={snapshot()} fallback={<p class="px-2 py-1 font-mono text-[0.6rem] text-muted">Loading…</p>}>
          {(snap) => (
            <For each={snap().plugins.filter((plugin) => plugin.installed)}>
              {(plugin) => (
                <button
                  type="button"
                  class="focus-ring flex w-full items-center justify-between rounded px-2 py-1 text-left font-mono text-[0.62rem] text-muted hover:bg-raised hover:text-foreground"
                  onClick={() => void toggle(plugin)}
                  role="menuitemcheckbox"
                  aria-checked={plugin.enabled}
                >
                  <span class="truncate">{plugin.name}</span>
                  <span class={plugin.enabled ? "text-signal" : "text-muted"}>{plugin.enabled ? "on" : "off"}</span>
                </button>
              )}
            </For>
          )}
        </Show>
      </div>
    </>
  );
}
