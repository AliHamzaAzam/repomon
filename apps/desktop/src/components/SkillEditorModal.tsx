import { Show, createResource, createSignal } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import Modal from "./Modal";

interface SkillEditorModalProps {
  path: string;
  onClose: () => void;
}

/// Loads SKILL.md through skill.read, edits it in a plain textarea, and saves through
/// skill.write. Content is fetched once per mount since a given modal instance always
/// edits a single fixed path.
export default function SkillEditorModal(props: SkillEditorModalProps) {
  const [content] = createResource(() => daemonCall("skill.read", { path: props.path }));
  const [draft, setDraft] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const text = () => draft() ?? content()?.content ?? "";

  async function save() {
    setSaving(true);
    setError(null);
    try {
      await daemonCall("skill.write", { path: props.path, content: text() });
      props.onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  const footer = (
    <>
      <button type="button" class="focus-ring rounded border border-line px-3 py-2 text-xs text-muted" onClick={props.onClose}>Cancel</button>
      <button
        type="button"
        class="focus-ring rounded bg-signal px-4 py-2 font-mono text-[0.6rem] font-semibold uppercase text-background disabled:opacity-50"
        disabled={saving() || content.loading}
        onClick={() => void save()}
      >
        {saving() ? "Saving…" : "Save"}
      </button>
    </>
  );

  return (
    <Modal title="Edit skill" subtitle={props.path} onClose={props.onClose} footer={footer} width="min(42rem, 94vw)">
      <div class="flex flex-col gap-3">
        <Show
          when={!content.error}
          fallback={<p class="rounded-md border border-fault/40 bg-fault/8 p-2 font-mono text-[0.64rem] text-fault">{String(content.error)}</p>}
        >
          <Show when={!content.loading} fallback={<p class="font-mono text-[0.64rem] text-muted">Loading…</p>}>
            <textarea
              class="focus-ring h-72 w-full resize-y rounded-md border border-line bg-raised p-3 font-mono text-[0.7rem] leading-relaxed"
              value={text()}
              onInput={(event) => setDraft(event.currentTarget.value)}
              spellcheck={false}
            />
          </Show>
        </Show>
        <Show when={error()}>
          {(message) => <p class="rounded-md border border-fault/40 bg-fault/8 p-2 font-mono text-[0.64rem] text-fault">{message()}</p>}
        </Show>
        <p class="text-[0.62rem] text-muted">Saved changes apply to new agent sessions.</p>
      </div>
    </Modal>
  );
}
