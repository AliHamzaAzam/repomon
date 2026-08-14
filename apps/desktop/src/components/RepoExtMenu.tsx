import { For, Show, createResource, createSignal, onCleanup, onMount } from "solid-js";

import type { PluginInfo } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import { IconCheck, IconExtensions } from "./icons";

interface RepoExtMenuProps {
  repoId: number;
  x: number;
  y: number;
  onOpenExtensions: () => void;
  onOpenNotes: () => void;
  onClose: () => void;
}

function message(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export default function RepoExtMenu(props: RepoExtMenuProps) {
  const [snapshot, { refetch }] = createResource(() =>
    daemonCall("ext.list", { scope: "repo", repo_id: props.repoId }),
  );
  const [error, setError] = createSignal<string | null>(null);

  async function toggle(plugin: PluginInfo) {
    try {
      await daemonCall(plugin.enabled ? "plugin.disable" : "plugin.enable", {
        id: plugin.id,
        scope: "repo",
        repo_id: props.repoId,
      });
      setError(null);
    } catch (cause) {
      setError(message(cause));
    }
    void refetch();
  }

  function onKey(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    props.onClose();
  }
  onMount(() => window.addEventListener("keydown", onKey, true));
  onCleanup(() => window.removeEventListener("keydown", onKey, true));

  const top = () => Math.max(8, Math.min(props.y, window.innerHeight - window.innerHeight * 0.6 - 8));

  return (
    <>
      <div class="fixed inset-0 z-40" onClick={() => props.onClose()} />
      <div
        class="fixed z-50 max-h-[60vh] w-60 overflow-y-auto rounded-xl border border-line bg-surface p-1.5 shadow-[0_12px_40px_var(--shadow)]"
        style={{ left: `${props.x}px`, top: `${top()}px` }}
        role="menu"
      >
        <button
          type="button"
          class="focus-ring flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-foreground transition-colors hover:bg-raised"
          onClick={() => { props.onOpenExtensions(); props.onClose(); }}
          role="menuitem"
        >
          <IconExtensions size={14} />
          <span>Extensions…</span>
        </button>
        <button
          type="button"
          class="focus-ring flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-foreground transition-colors hover:bg-raised"
          onClick={() => { props.onOpenNotes(); props.onClose(); }}
          role="menuitem"
        >
          <span>Repo notes…</span>
        </button>
        <div class="my-1 border-t border-line/70" />
        <Show when={error()}>{(message) => <p class="px-2.5 py-1 text-xs text-fault">{message()}</p>}</Show>
        <Show when={snapshot()} fallback={<p class="px-2.5 py-1 text-xs text-muted">Loading…</p>}>
          {(snap) => (
            <For each={snap().plugins.filter((plugin) => plugin.installed)}>
              {(plugin) => (
                <button
                  type="button"
                  class="focus-ring flex w-full items-center justify-between rounded-lg px-2.5 py-1.5 text-left text-xs text-muted transition-colors hover:bg-raised hover:text-foreground"
                  onClick={() => void toggle(plugin)}
                  role="menuitemcheckbox"
                  aria-checked={plugin.enabled}
                >
                  <span class="truncate">{plugin.name}</span>
                  <Show when={plugin.enabled}>
                    <span class="text-signal"><IconCheck size={13} /></span>
                  </Show>
                </button>
              )}
            </For>
          )}
        </Show>
      </div>
    </>
  );
}
