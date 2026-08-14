import { Show, createSignal } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import Modal from "./Modal";

export default function RenameModal(props: {
  sessionId: string;
  current: string;
  onClose: () => void;
  onDone: () => Promise<void>;
}) {
  const [label, setLabel] = createSignal(props.current);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function rename() {
    setBusy(true);
    setError(null);
    try {
      await daemonCall("session.rename", { session_id: props.sessionId, label: label().trim() || undefined });
      await props.onDone();
      props.onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  const footer = (
    <>
      <button
        type="button"
        class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
        onClick={props.onClose}
      >
        Cancel
      </button>
      <button
        type="button"
        class="focus-ring rounded-lg bg-signal px-4 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-50"
        disabled={busy()}
        onClick={() => void rename()}
      >
        {busy() ? "Saving…" : "Save"}
      </button>
    </>
  );

  return (
    <Modal
      title="Rename session"
      subtitle="Set a custom label to easily identify this agent session."
      onClose={props.onClose}
      footer={footer}
    >
      <label class="block">
        <span class="section-label">Custom Label</span>
        <input
          class="focus-ring mt-1.5 h-9 w-full rounded-lg border border-line bg-surface px-3 text-xs text-foreground outline-none placeholder:text-muted/60"
          value={label()}
          placeholder="Leave blank to reset to default"
          onInput={(event) => setLabel(event.currentTarget.value)}
          autofocus
        />
      </label>
      <Show when={error()}>
        <p class="mt-3 rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault">{error()}</p>
      </Show>
    </Modal>
  );
}
