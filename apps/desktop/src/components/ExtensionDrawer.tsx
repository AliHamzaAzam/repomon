import { Show, createSignal } from "solid-js";

import type { ExtensionsStore, ExtRow } from "../stores/extensions";
import ConfirmDialog from "./ConfirmDialog";

interface ExtensionDrawerProps {
  row: ExtRow;
  store: ExtensionsStore;
  onClose: () => void;
  onEdit: (path: string) => void;
}

export default function ExtensionDrawer(props: ExtensionDrawerProps) {
  // Lives at this level (not inside the keyed per-kind Show below) so a background store
  // refresh, which rebuilds row objects on every snapshot, cannot silently dismiss the
  // confirm dialog mid-flow.
  const [confirmDelete, setConfirmDelete] = createSignal(false);

  return (
    <aside class="flex w-72 shrink-0 flex-col gap-3 border-l border-line bg-surface p-4 text-sm">
      <div class="flex items-center justify-between">
        <span class="section-label">{props.row.kind === "plugin" ? "Plugin" : "Skill"}</span>
        <button type="button" class="focus-ring rounded px-1 font-mono text-muted hover:text-foreground" onClick={() => props.onClose()} aria-label="Close details">×</button>
      </div>
      <Show when={props.row.kind === "plugin" ? props.row : null} keyed>
        {(row) => {
          const plugin = () => row.plugin;
          const details = () => props.store.detailsFor(plugin().id);
          const cliTitle = () => (props.store.cliAvailable() ? undefined : "Requires the claude CLI");

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
              <div class="flex flex-wrap gap-2">
                <button
                  type="button"
                  class="focus-ring rounded-md border border-line bg-raised px-2 py-1 font-mono text-[0.6rem] uppercase text-muted hover:text-foreground disabled:opacity-40"
                  disabled={props.store.busy() || !props.store.cliAvailable()}
                  title={cliTitle()}
                  onClick={() => void props.store.loadDetails(plugin().id)}
                >Details</button>
                <button
                  type="button"
                  class="focus-ring rounded-md border border-line bg-raised px-2 py-1 font-mono text-[0.6rem] uppercase text-muted hover:text-foreground disabled:opacity-40"
                  disabled={props.store.busy() || !props.store.cliAvailable()}
                  title={cliTitle()}
                  onClick={() => void props.store.update(plugin().id)}
                >Update</button>
                <button
                  type="button"
                  class="focus-ring rounded-md border border-fault/40 bg-fault/10 px-2 py-1 font-mono text-[0.6rem] uppercase text-fault disabled:opacity-40"
                  disabled={props.store.busy() || !props.store.cliAvailable()}
                  title={cliTitle()}
                  onClick={() => void props.store.remove(plugin().id)}
                >Remove</button>
              </div>
              <Show
                when={details().error}
                fallback={
                  <Show when={details().text !== null}>
                    <pre class="max-h-48 overflow-auto whitespace-pre-wrap font-mono text-[0.58rem]">{details().text}</pre>
                  </Show>
                }
              >
                {(err) => <p class="rounded-md border border-fault/40 bg-fault/10 px-2 py-1 font-mono text-[0.6rem] text-fault">{err()}</p>}
              </Show>
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
              <div class="flex flex-wrap gap-2">
                <button
                  type="button"
                  class="focus-ring rounded-md border border-line bg-raised px-2 py-1 font-mono text-[0.6rem] uppercase text-muted hover:text-foreground"
                  onClick={() => void navigator.clipboard.writeText(String(skill().path))}
                >Copy path</button>
                <button
                  type="button"
                  class="focus-ring rounded-md border border-line bg-raised px-2 py-1 font-mono text-[0.6rem] uppercase text-muted hover:text-foreground disabled:opacity-40"
                  disabled={props.store.busy()}
                  onClick={() => props.onEdit(skill().path)}
                >Edit</button>
                <button
                  type="button"
                  class="focus-ring rounded-md border border-fault/40 bg-fault/10 px-2 py-1 font-mono text-[0.6rem] uppercase text-fault disabled:opacity-40"
                  disabled={props.store.busy()}
                  onClick={() => setConfirmDelete(true)}
                >Delete</button>
              </div>
              <p class="text-[0.62rem] text-muted">Changes apply to new agent sessions.</p>
              <Show when={confirmDelete()}>
                <ConfirmDialog
                  options={{
                    title: `Delete ${skill().name}?`,
                    message: "Removes SKILL.md from disk in this scope. This can't be undone.",
                    confirmLabel: "Delete",
                    danger: true,
                    onConfirm: async () => {
                      const ok = await props.store.deleteSkill(skill().name);
                      if (!ok) throw new Error(props.store.error() ?? "delete failed");
                    },
                  }}
                  onClose={() => setConfirmDelete(false)}
                />
              </Show>
            </>
          );
        }}
      </Show>
    </aside>
  );
}
