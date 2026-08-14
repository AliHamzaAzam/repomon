import { Show, createSignal } from "solid-js";

import type { ExtensionsStore, ExtRow } from "../stores/extensions";
import ConfirmDialog from "./ConfirmDialog";
import { IconClose } from "./icons";

interface ExtensionDrawerProps {
  row: ExtRow;
  store: ExtensionsStore;
  onClose: () => void;
  onEdit: (path: string) => void;
}

export default function ExtensionDrawer(props: ExtensionDrawerProps) {
  const [confirmDelete, setConfirmDelete] = createSignal(false);

  return (
    <aside class="flex w-80 shrink-0 flex-col gap-3.5 border-l border-line bg-surface p-4 text-xs">
      <div class="flex items-center justify-between">
        <span class="section-label">{props.row.kind === "plugin" ? "Plugin Details" : "Skill Details"}</span>
        <button
          type="button"
          class="focus-ring flex size-6 items-center justify-center rounded text-muted transition-colors hover:bg-raised hover:text-foreground"
          onClick={() => props.onClose()}
          aria-label="Close details"
        >
          <IconClose size={13} />
        </button>
      </div>
      <Show when={props.row.kind === "plugin" ? props.row : null} keyed>
        {(row) => {
          const plugin = () => row.plugin;
          const details = () => props.store.detailsFor(plugin().id);
          const cliTitle = () => (props.store.cliAvailable() ? undefined : "Requires the claude CLI");

          return (
            <>
              <div>
                <h3 class="text-sm font-semibold text-foreground">{plugin().name}</h3>
                <p class="mt-0.5 font-mono text-[11px] text-muted">
                  {plugin().marketplace}{plugin().version ? ` · v${plugin().version}` : ""}
                  {plugin().installed ? "" : " · not installed"}
                </p>
              </div>
              <Show when={plugin().provides}>
                {(provides) => (
                  <p class="font-mono text-[11px] text-muted">
                    {provides().skills} skills · {provides().commands} commands · {provides().agents} agents
                  </p>
                )}
              </Show>
              <label class="flex items-center gap-2 text-xs font-medium text-foreground">
                <input
                  type="checkbox"
                  class="accent-signal"
                  checked={plugin().enabled}
                  disabled={props.store.busy()}
                  onChange={(event) => void props.store.setEnabled(plugin().id, event.currentTarget.checked)}
                />
                <span>Enabled in this scope</span>
              </label>
              <div class="flex flex-wrap gap-1.5">
                <button
                  type="button"
                  class="focus-ring rounded-lg border border-line bg-raised/70 px-2.5 py-1 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                  disabled={props.store.busy() || !props.store.cliAvailable()}
                  title={cliTitle()}
                  onClick={() => void props.store.loadDetails(plugin().id)}
                >Details</button>
                <button
                  type="button"
                  class="focus-ring rounded-lg border border-line bg-raised/70 px-2.5 py-1 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                  disabled={props.store.busy() || !props.store.cliAvailable()}
                  title={cliTitle()}
                  onClick={() => void props.store.update(plugin().id)}
                >Update</button>
                <button
                  type="button"
                  class="focus-ring rounded-lg border border-fault/30 bg-fault/10 px-2.5 py-1 text-xs font-medium text-fault transition-colors hover:bg-fault/20 disabled:opacity-40"
                  disabled={props.store.busy() || !props.store.cliAvailable()}
                  title={cliTitle()}
                  onClick={() => void props.store.remove(plugin().id)}
                >Remove</button>
              </div>
              <Show
                when={details().error}
                fallback={
                  <Show when={details().text !== null}>
                    <pre class="max-h-52 overflow-auto rounded-xl border border-line bg-background p-2.5 whitespace-pre-wrap font-mono text-[11px] text-muted">{details().text}</pre>
                  </Show>
                }
              >
                {(err) => <p class="rounded-xl border border-fault/30 bg-fault/8 p-2.5 text-xs text-fault">{err()}</p>}
              </Show>
              <p class="text-[11px] text-muted">Changes apply to new agent sessions.</p>
            </>
          );
        }}
      </Show>
      <Show when={props.row.kind === "skill" ? props.row : null} keyed>
        {(row) => {
          const skill = () => row.skill;
          return (
            <>
              <div>
                <h3 class="text-sm font-semibold text-foreground">{skill().name}</h3>
                <p class="mt-1 text-xs leading-relaxed text-muted">{skill().description ?? "No description provided."}</p>
                <p class="mt-1 font-mono text-[10px] text-muted/70">{skill().source}</p>
              </div>
              <div class="flex flex-wrap gap-1.5">
                <button
                  type="button"
                  class="focus-ring rounded-lg border border-line bg-raised/70 px-2.5 py-1 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
                  onClick={() => void navigator.clipboard.writeText(String(skill().path))}
                >Copy path</button>
                <button
                  type="button"
                  class="focus-ring rounded-lg border border-line bg-raised/70 px-2.5 py-1 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                  disabled={props.store.busy()}
                  onClick={() => props.onEdit(skill().path)}
                >Edit</button>
                <button
                  type="button"
                  class="focus-ring rounded-lg border border-fault/30 bg-fault/10 px-2.5 py-1 text-xs font-medium text-fault transition-colors hover:bg-fault/20 disabled:opacity-40"
                  disabled={props.store.busy()}
                  onClick={() => setConfirmDelete(true)}
                >Delete</button>
              </div>
              <p class="text-[11px] text-muted">Changes apply to new agent sessions.</p>
              <Show when={confirmDelete()}>
                <ConfirmDialog
                  options={{
                    title: `Delete ${skill().name}?`,
                    message: "Removes SKILL.md from disk in this scope. This action cannot be undone.",
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
