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
      <button
        type="button"
        class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
        onClick={props.onClose}
      >
        Cancel
      </button>
      <button
        type="button"
        class={`focus-ring rounded-lg px-4 py-1.5 text-xs font-semibold text-background transition-colors disabled:opacity-50 ${
          props.options.danger
            ? "bg-fault hover:bg-fault/90 text-white"
            : "bg-signal hover:bg-signal/90 text-background"
        }`}
        disabled={busy()}
        onClick={() => void confirm()}
      >
        {busy() ? "Working…" : props.options.confirmLabel ?? "Confirm"}
      </button>
    </>
  );

  return (
    <Modal title={props.options.title} onClose={props.onClose} footer={footer}>
      <p class="text-xs leading-relaxed text-foreground/80">{props.options.message}</p>
      <Show when={error()}>
        <p class="mt-3 rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault">{error()}</p>
      </Show>
    </Modal>
  );
}
