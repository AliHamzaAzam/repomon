import { For, Show, createSignal, onMount } from "solid-js";

import { hasTauriBridge } from "../ipc/boot";
import { cliInstall, cliStatus, cliUninstall, type CliStatus } from "../ipc/cli";
import { IconCheck, IconCopy, IconTerminal } from "./icons";

/// Configures installation of bundled command-line tools and reports whether a terminal can find
/// them.
export interface CommandLineToolsCardProps {
  /// Injected in tests; the app uses the Tauri commands.
  read?: () => Promise<CliStatus>;
  install?: () => Promise<CliStatus>;
  uninstall?: () => Promise<CliStatus>;
  writeClipboard?: (text: string) => Promise<void>;
  /// Compact framing for the setup wizard, which draws its own heading around it.
  compact?: boolean;
}

export default function CommandLineToolsCard(props: CommandLineToolsCardProps) {
  const [status, setStatus] = createSignal<CliStatus | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [copied, setCopied] = createSignal(false);

  const injected = () => Boolean(props.read ?? props.install ?? props.uninstall);

  async function run(action: () => Promise<CliStatus>) {
    setBusy(true);
    setError(null);
    try {
      setStatus(await action());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  onMount(() => {
    // No command bridge outside the Tauri shell, so there is nothing truthful to report.
    if (!injected() && !hasTauriBridge()) return;
    void run(props.read ?? cliStatus);
  });

  async function copyHint() {
    const hint = status()?.path_hint;
    if (!hint) return;
    try {
      await (props.writeClipboard ?? ((text: string) => navigator.clipboard.writeText(text)))(hint);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <Show when={status() ?? error()}>
      <div
        class={
          props.compact
            ? "space-y-3"
            : "rounded-xl border border-line bg-surface p-3.5 space-y-3"
        }
        data-testid="command-line-tools"
      >
        <Show when={!props.compact}>
          <div class="flex items-center justify-between">
            <div>
              <span class="section-label">Command-line tools</span>
              <p class="mt-0.5 text-[11px] text-muted">
                The `repomon` CLI and daemon ship inside this app. Install them where your shell can
                find them.
              </p>
            </div>
          </div>
        </Show>

        <div class="rounded-lg bg-background/50 p-3 space-y-2">
          <div class="flex items-start justify-between gap-3">
            <div class="flex min-w-0 items-center gap-2.5">
              <div class="flex size-6 shrink-0 items-center justify-center rounded-md border border-line bg-surface text-foreground">
                <IconTerminal size={13} />
              </div>
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <span class="text-xs font-medium text-foreground">repomon</span>
                  <Show when={status()?.version}>
                    {(version) => (
                      <span class="truncate font-mono text-[10.5px] text-muted">{version()}</span>
                    )}
                  </Show>
                </div>
                <p class="mt-0.5 truncate font-mono text-[10.5px] text-muted" title={status()?.dir}>
                  {status()?.dir ?? "Could not resolve an install directory"}
                </p>
              </div>
            </div>

            <div class="flex shrink-0 items-center gap-2">
              <Show
                when={status()?.installed}
                fallback={
                  <span class="rounded border border-line bg-surface px-2 py-0.5 text-[10.5px] font-medium text-muted">
                    Not installed
                  </span>
                }
              >
                <Show
                  when={status()?.on_path}
                  fallback={
                    <span class="rounded border border-attention/30 bg-attention/10 px-2 py-0.5 text-[10.5px] font-medium text-attention">
                      {status()?.on_path === null ? "Installed, PATH unknown" : "Installed, not on PATH"}
                    </span>
                  }
                >
                  <span class="flex items-center gap-1.5 rounded border border-signal/30 bg-signal/10 px-2 py-0.5 text-[10.5px] font-medium text-signal">
                    <IconCheck size={11} strokeWidth={2.5} />
                    On your PATH
                  </span>
                </Show>
              </Show>

              <Show
                when={status()?.installed}
                fallback={
                  <button
                    type="button"
                    class="focus-ring cursor-pointer rounded-lg border border-signal/40 bg-signal/10 px-3 py-1 text-xs font-medium text-signal transition-colors hover:bg-signal/20 disabled:opacity-50"
                    disabled={busy()}
                    onClick={() => void run(props.install ?? cliInstall)}
                  >
                    {busy() ? "Working…" : "Install"}
                  </button>
                }
              >
                <button
                  type="button"
                  class="focus-ring cursor-pointer rounded-lg border border-line bg-surface px-3 py-1 text-xs font-medium text-foreground transition-colors hover:bg-raised disabled:opacity-50"
                  disabled={busy()}
                  onClick={() => void run(props.uninstall ?? cliUninstall)}
                >
                  {busy() ? "Working…" : "Remove"}
                </button>
              </Show>
            </div>
          </div>

          <Show when={status()?.installed && status()?.path_hint}>
            {(hint) => (
              <div class="space-y-2 rounded-lg border border-attention/20 bg-attention/5 p-2.5">
                <p class="text-[11px] leading-relaxed text-foreground">
                  {status()?.on_path === null ? "Check your shell PATH before using the command line." : "Installed, but your shell cannot see it yet."}
                </p>
                <div class="flex items-center gap-2">
                  <code class="min-w-0 flex-1 select-all truncate rounded border border-line bg-surface px-2 py-0.5 font-mono text-[10.5px] text-foreground">
                    {hint()}
                  </code>
                  <button
                    type="button"
                    class="focus-ring flex shrink-0 cursor-pointer items-center gap-1 rounded-md border border-line bg-surface px-2 py-0.5 text-xs font-medium text-foreground transition-colors hover:bg-line/40"
                    onClick={() => void copyHint()}
                    aria-label="Copy the PATH line"
                  >
                    <Show when={copied()} fallback={<IconCopy size={11} class="text-muted" />}>
                      <IconCheck size={11} class="text-signal" />
                    </Show>
                    <span>{copied() ? "Copied" : "Copy"}</span>
                  </button>
                </div>
              </div>
            )}
          </Show>

          <For each={status()?.notes}>
            {(note) => <p class="text-[11px] leading-relaxed text-muted">{note}</p>}
          </For>

          <Show when={error()}>
            {(message) => (
              <p class="rounded-lg border border-fault/20 bg-fault/5 p-2.5 text-[11px] leading-relaxed text-fault">
                {message()}
              </p>
            )}
          </Show>
        </div>
      </div>
    </Show>
  );
}
