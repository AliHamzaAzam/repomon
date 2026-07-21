import { Show, createSignal } from "solid-js";

import Modal from "./Modal";

export interface ConfirmOptions {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
  onConfirm: () => Promise<void> | void;
}

export default function ConfirmDialog(props: { options: ConfirmOptions; onClose: () => void }) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function confirm() {
    setBusy(true);
    setError(null);
    try {
      await props.options.onConfirm();
      props.onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  const footer = (
    <>
      <button type="button" class="focus-ring rounded border border-line px-3 py-2 text-xs text-muted" onClick={props.onClose}>
        Cancel
      </button>
      <button
        type="button"
        class={`focus-ring rounded px-4 py-2 font-mono text-[0.6rem] font-semibold uppercase text-background disabled:opacity-50 ${props.options.danger ? "bg-fault" : "bg-signal"}`}
        disabled={busy()}
        onClick={() => void confirm()}
      >
        {busy() ? "Working…" : props.options.confirmLabel ?? "Confirm"}
      </button>
    </>
  );

  return (
    <Modal title={props.options.title} onClose={props.onClose} footer={footer}>
      <p class="text-sm leading-relaxed">{props.options.message}</p>
      <Show when={error()}>
        <p class="mt-3 rounded-md border border-fault/40 bg-fault/8 p-2 text-xs text-fault">{error()}</p>
      </Show>
    </Modal>
  );
}
