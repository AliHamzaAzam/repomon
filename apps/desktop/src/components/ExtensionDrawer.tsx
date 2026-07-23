import { Show } from "solid-js";

import type { ExtensionsStore, ExtRow } from "../stores/extensions";

interface ExtensionDrawerProps {
  row: ExtRow;
  store: ExtensionsStore;
  onClose: () => void;
}

export default function ExtensionDrawer(props: ExtensionDrawerProps) {
  return (
    <aside class="flex w-72 shrink-0 flex-col gap-3 border-l border-line bg-surface p-4 text-sm">
      <div class="flex items-center justify-between">
        <span class="section-label">{props.row.kind === "plugin" ? "Plugin" : "Skill"}</span>
        <button type="button" class="focus-ring rounded px-1 font-mono text-muted hover:text-foreground" onClick={() => props.onClose()} aria-label="Close details">×</button>
      </div>
      <Show when={props.row.kind === "plugin" ? props.row : null} keyed>
        {(row) => {
          const plugin = () => row.plugin;
          return (
            <>
              <h3 class="font-mono text-[0.8rem] text-foreground">{plugin().name}</h3>
              <p class="font-mono text-[0.64rem] text-muted">
                {plugin().marketplace}{plugin().version ? ` · v${plugin().version}` : ""}
                {plugin().installed ? "" : " · not installed"}
              </p>
              <Show when={plugin().provides}>
                {(provides) => (
                  <p class="font-mono text-[0.64rem] text-muted">
                    {provides().skills} skills · {provides().commands} commands · {provides().agents} agents
                  </p>
                )}
              </Show>
              <label class="flex items-center gap-2 font-mono text-[0.7rem]">
                <input
                  type="checkbox"
                  checked={plugin().enabled}
                  disabled={props.store.busy()}
                  onChange={(event) => void props.store.setEnabled(plugin().id, event.currentTarget.checked)}
                />
                Enabled in this scope
              </label>
              <p class="text-[0.62rem] text-muted">Changes apply to new agent sessions.</p>
            </>
          );
        }}
      </Show>
      <Show when={props.row.kind === "skill" ? props.row : null} keyed>
        {(row) => {
          const skill = () => row.skill;
          return (
            <>
              <h3 class="font-mono text-[0.8rem] text-foreground">{skill().name}</h3>
              <p class="text-[0.68rem] text-muted">{skill().description ?? "No description"}</p>
              <p class="font-mono text-[0.6rem] text-muted">{skill().source}</p>
              <button
                type="button"
                class="focus-ring self-start rounded-md border border-line bg-raised px-2 py-1 font-mono text-[0.6rem] text-muted hover:text-foreground"
                onClick={() => void navigator.clipboard.writeText(String(skill().path))}
              >Copy path</button>
              <p class="text-[0.62rem] text-muted">Changes apply to new agent sessions.</p>
            </>
          );
        }}
      </Show>
    </aside>
  );
}
