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
      <button type="button" class="focus-ring rounded border border-line px-3 py-2 text-xs text-muted" onClick={props.onClose}>Cancel</button>
      <button type="button" class="focus-ring rounded bg-signal px-4 py-2 font-mono text-[0.6rem] font-semibold uppercase text-background disabled:opacity-50" disabled={busy()} onClick={() => void rename()}>
        {busy() ? "Saving…" : "Save"}
      </button>
    </>
  );

  return (
    <Modal title="Rename session" onClose={props.onClose} footer={footer}>
      <label class="block">
        <span class="section-label">Label</span>
        <input class="settings-input" value={label()} placeholder="Leave blank to clear" onInput={(event) => setLabel(event.currentTarget.value)} autofocus />
      </label>
      <Show when={error()}>
        <p class="mt-3 rounded-md border border-fault/40 bg-fault/8 p-2 text-xs text-fault">{error()}</p>
      </Show>
    </Modal>
  );
}
