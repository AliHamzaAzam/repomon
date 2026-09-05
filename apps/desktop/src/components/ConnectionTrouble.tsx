import { Show, createSignal } from "solid-js";

import type { ConnectionSnapshot } from "../ipc/connection";
import { IconCheck, IconCopy, IconTerminal } from "./icons";

/// The second line of the connection rail, drawn only while the daemon is unreachable.
///
/// A retrying pill on its own is a dead end: the daemon is spawned detached and windowless, so
/// there is no console to read and no dialog to dismiss. This row carries the one line that says
/// what to do about it, and the two controls that make the failure reportable: the daemon log,
/// and a diagnostics block with the endpoint, the resolved binary, and the log tail already in it.
export interface ConnectionTroubleProps {
  snapshot: ConnectionSnapshot;
  /// Opens the daemon log in the system's text viewer.
  onShowLog: () => Promise<void>;
  /// Returns the diagnostics block to put on the clipboard.
  collectDiagnostics: () => Promise<string>;
  /// Injected so the copy path is testable without a real clipboard.
  writeClipboard?: (text: string) => Promise<void>;
}

export default function ConnectionTrouble(props: ConnectionTroubleProps) {
  const [copied, setCopied] = createSignal(false);
  const [failure, setFailure] = createSignal<string | null>(null);

  const write = (text: string) =>
    props.writeClipboard
      ? props.writeClipboard(text)
      : navigator.clipboard.writeText(text);

  async function showLog() {
    setFailure(null);
    try {
      await props.onShowLog();
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  }

  async function copyDiagnostics() {
    setFailure(null);
    try {
      await write(await props.collectDiagnostics());
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <div class="flex min-w-0 items-center gap-2 pl-3.5" data-testid="connection-trouble">
      <Show when={props.snapshot.hint}>
        {(hint) => (
          <span class="truncate font-sans text-[11px] text-attention" title={hint()}>
            {hint()}
          </span>
        )}
      </Show>
      <span class="shrink-0 truncate text-[10px] text-muted" title={props.snapshot.endpoint}>
        {props.snapshot.endpoint}
      </span>
      <button
        type="button"
        class="focus-ring flex shrink-0 cursor-pointer items-center gap-1 rounded border border-line bg-surface px-1.5 py-0.5 font-sans text-[10px] font-medium text-foreground transition-colors hover:bg-raised"
        onClick={() => void showLog()}
        title={props.snapshot.log_path ?? "Open the daemon log"}
      >
        <IconTerminal size={10} class="text-muted" />
        <span>Show log</span>
      </button>
      <button
        type="button"
        class="focus-ring flex shrink-0 cursor-pointer items-center gap-1 rounded border border-line bg-surface px-1.5 py-0.5 font-sans text-[10px] font-medium text-foreground transition-colors hover:bg-raised"
        onClick={() => void copyDiagnostics()}
      >
        <Show when={copied()} fallback={<IconCopy size={10} class="text-muted" />}>
          <IconCheck size={10} class="text-signal" />
        </Show>
        <span>{copied() ? "Copied" : "Copy diagnostics"}</span>
      </button>
      <Show when={failure()}>
        {(message) => (
          <span class="truncate font-sans text-[10px] text-fault" title={message()}>
            {message()}
          </span>
        )}
      </Show>
    </div>
  );
}
