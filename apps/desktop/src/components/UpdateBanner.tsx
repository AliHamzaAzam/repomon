import { Show, createSignal } from "solid-js";

import type { AvailableUpdate, UpdateProgress } from "../ipc/updater";

/// Launch-time "an update is available" bar. Shown when the check on startup finds a newer
/// build; installing downloads, applies, and relaunches.
export default function UpdateBanner(props: { update: AvailableUpdate; onDismiss: () => void }) {
  const [busy, setBusy] = createSignal(false);
  const [progress, setProgress] = createSignal<UpdateProgress | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  async function install() {
    setBusy(true);
    setError(null);
    try {
      await props.update.install(setProgress);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      setBusy(false);
    }
  }

  return (
    <div class="fixed inset-x-0 top-0 z-[70] flex items-center justify-center gap-3 border-b border-signal/40 bg-signal/12 px-4 py-2 text-xs backdrop-blur">
      <span class="font-mono uppercase tracking-[0.12em] text-signal">Update</span>
      <span class="text-foreground">Repomon {props.update.version} is available.</span>
      <Show when={error()}>
        <span class="text-fault">{error()}</span>
      </Show>
      <Show when={progress()?.total}>
        <progress class="h-1 w-32 accent-signal" max={progress()!.total} value={progress()!.downloaded} />
      </Show>
      <button
        type="button"
        class="focus-ring rounded bg-signal px-3 py-1 font-mono text-[0.55rem] font-semibold uppercase text-background disabled:opacity-50"
        disabled={busy()}
        onClick={() => void install()}
      >
        {busy() ? "Installing…" : "Install & restart"}
      </button>
      <button type="button" class="focus-ring rounded border border-line px-2 py-1 text-muted" onClick={props.onDismiss} disabled={busy()}>
        Later
      </button>
    </div>
  );
}
