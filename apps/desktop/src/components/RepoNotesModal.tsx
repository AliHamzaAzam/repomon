import { Show, createResource, createSignal } from "solid-js";

import type { Repo } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import Modal from "./Modal";

/// Mirrors `repomon_core::notes::MAX_NOTES_BYTES`. The daemon rejects anything larger, so the
/// editor counts down to the same number rather than letting you write a save that will bounce.
const MAX_NOTES_BYTES = 8192;

interface RepoNotesModalProps {
  repo: Repo;
  onClose: () => void;
}

function message(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export default function RepoNotesModal(props: RepoNotesModalProps) {
  const [loaded] = createResource(
    () => props.repo.id,
    (repoId) => daemonCall("repo.notes.get", { repo_id: repoId }),
  );
  const [draft, setDraft] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const content = () => draft() ?? loaded()?.content ?? "";
  const bytes = () => new TextEncoder().encode(content()).length;
  const overCap = () => bytes() > MAX_NOTES_BYTES;
  const dirty = () => draft() !== null && draft() !== (loaded()?.content ?? "");

  async function save() {
    if (overCap()) return;
    setSaving(true);
    setError(null);
    try {
      await daemonCall("repo.notes.set", { repo_id: props.repo.id, content: content() });
      props.onClose();
    } catch (cause) {
      setError(message(cause));
    } finally {
      setSaving(false);
    }
  }

  const footer = (
    <>
      <span
        class={`mr-auto font-mono text-[11px] ${overCap() ? "text-fault font-semibold" : "text-muted"}`}
        aria-live="polite"
      >{`${bytes()} / ${MAX_NOTES_BYTES} bytes`}</span>
      <button
        type="button"
        class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
        onClick={props.onClose}
      >
        Cancel
      </button>
      <button
        type="button"
        class="focus-ring rounded-lg bg-signal px-4 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-40"
        disabled={saving() || overCap() || !dirty()}
        onClick={() => void save()}
      >
        {saving() ? "Saving…" : "Save"}
      </button>
    </>
  );

  return (
    <Modal
      title={`Notes for ${props.repo.name}`}
      subtitle="repomind reads these when planning and passes them to workers it spawns here."
      onClose={props.onClose}
      footer={footer}
      width="44rem"
    >
      <Show when={!loaded.loading} fallback={<p class="text-xs text-muted">Loading notes…</p>}>
        <textarea
          class="focus-ring h-72 w-full resize-none rounded-xl border border-line bg-background p-3 font-mono text-xs leading-relaxed text-foreground outline-none placeholder:text-muted/50"
          value={content()}
          onInput={(event) => setDraft(event.currentTarget.value)}
          placeholder={"# Conventions\n\n- Use `pnpm test`, never `npm test`.\n- Squash-merge only."}
          spellcheck={false}
          aria-label={`Notes markdown for ${props.repo.name}`}
        />
        <p class="mt-2 truncate font-mono text-[10px] text-muted" title={loaded()?.path}>
          {loaded()?.exists ? loaded()?.path : `${loaded()?.path} (not created yet)`}
        </p>
      </Show>
      <Show when={error()}>
        {(text) => <p class="mt-2 rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault">{text()}</p>}
      </Show>
    </Modal>
  );
}
