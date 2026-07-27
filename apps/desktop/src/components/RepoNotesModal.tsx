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

/// Per-repo notes: conventions, build and test commands, gotchas, "always tell workers X".
///
/// repomind reads these when it plans work and folds them into worker prompts, so this is the one
/// place a human writes something the orchestrator will still know next week. The file is plain
/// markdown on disk and editable outside the app, which is why the editor loads fresh on open
/// rather than caching.
export default function RepoNotesModal(props: RepoNotesModalProps) {
  const [loaded] = createResource(
    () => props.repo.id,
    (repoId) => daemonCall("repo.notes.get", { repo_id: repoId }),
  );
  // `null` until the user types: it lets the textarea show the loaded content without a effect
  // copying server state into a signal (which would clobber edits on any refetch).
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
        class={`mr-auto font-mono text-[0.58rem] ${overCap() ? "text-fault" : "text-muted"}`}
        aria-live="polite"
      >
        {bytes()} / {MAX_NOTES_BYTES} bytes
      </span>
      <button
        type="button"
        class="focus-ring rounded-md border border-line bg-raised px-3 py-1.5 font-mono text-[0.6rem] uppercase tracking-[0.1em] text-muted hover:text-foreground"
        onClick={props.onClose}
      >Cancel</button>
      <button
        type="button"
        class="focus-ring rounded-md border border-signal/40 bg-signal/10 px-3 py-1.5 font-mono text-[0.6rem] uppercase tracking-[0.1em] text-signal disabled:opacity-40"
        disabled={saving() || overCap() || !dirty()}
        onClick={() => void save()}
      >{saving() ? "Saving…" : "Save"}</button>
    </>
  );

  return (
    <Modal
      title={`Notes for ${props.repo.name}`}
      subtitle="repomind reads these when planning and passes them to workers it spawns here."
      onClose={props.onClose}
      footer={footer}
      width="42rem"
    >
      <Show when={!loaded.loading} fallback={<p class="text-xs text-muted">Loading notes…</p>}>
        <textarea
          class="focus-ring h-72 w-full resize-none rounded-md border border-line bg-background p-2 font-mono text-[0.68rem] leading-relaxed outline-none"
          value={content()}
          onInput={(event) => setDraft(event.currentTarget.value)}
          placeholder={"# Conventions\n\n- Use `pnpm test`, never `npm test`.\n- Squash-merge only."}
          spellcheck={false}
          aria-label={`Notes markdown for ${props.repo.name}`}
        />
        <p class="mt-2 truncate font-mono text-[0.55rem] text-muted/70" title={loaded()?.path}>
          {loaded()?.exists ? loaded()?.path : `${loaded()?.path} (not created yet)`}
        </p>
      </Show>
      <Show when={error()}>
        {(text) => <p class="mt-2 font-mono text-[0.6rem] text-fault">{text()}</p>}
      </Show>
    </Modal>
  );
}
