import { Show, createSignal, onMount } from "solid-js";

import { daemonBootCheck, hasTauriBridge, openDaemonLog, type DaemonBootCheck } from "../ipc/boot";
import { IconCheck, IconRefresh, IconTerminal } from "./icons";

/// Configures a bundled-daemon launch probe that works even when the daemon cannot serve system
/// checks.
export interface DaemonBootRowProps {
  /// Injected in tests; the app uses the Tauri command.
  check?: () => Promise<DaemonBootCheck>;
  /// Injected in tests; the app opens the log in the system viewer.
  showLog?: () => Promise<void>;
}

export default function DaemonBootRow(props: DaemonBootRowProps) {
  const [result, setResult] = createSignal<DaemonBootCheck | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [failed, setFailed] = createSignal(false);

  async function run() {
    setBusy(true);
    try {
      setResult(await (props.check ?? daemonBootCheck)());
      setFailed(false);
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  onMount(() => {
    // Outside the Tauri shell there is no command bridge, so there is nothing truthful to say
    // about this machine's daemon binary. Render nothing rather than a fabricated failure.
    if (!props.check && !hasTauriBridge()) return;
    void run();
  });

  const ok = () => result()?.ok === true;

  return (
    <Show when={result() ?? failed()}>
      <div class="rounded-xl border border-line bg-surface p-3.5 space-y-3" data-testid="daemon-boot-row">
        <div class="flex items-center justify-between">
          <span class="section-label">Bundled Daemon</span>
          <button
            type="button"
            class="focus-ring flex cursor-pointer items-center gap-1.5 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-line/40 disabled:opacity-60"
            disabled={busy()}
            onClick={() => void run()}
            aria-label="Re-run the daemon launch check"
          >
            <IconRefresh size={13} class={busy() ? "animate-spin text-signal" : "text-muted"} />
            <span>{busy() ? "Checking…" : "Check again"}</span>
          </button>
        </div>

        <div class="rounded-lg bg-background/50 p-3 space-y-1.5">
          <div class="flex items-start justify-between gap-3">
            <div class="flex min-w-0 items-center gap-2.5">
              <div class="flex size-6 shrink-0 items-center justify-center rounded-md border border-line bg-surface text-foreground">
                <IconTerminal size={13} />
              </div>
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <span class="text-xs font-medium text-foreground">Daemon binary launches</span>
                  <span class="truncate text-[11px] text-muted">repomond --version</span>
                </div>
                <p class="mt-0.5 truncate font-mono text-[10.5px] text-muted" title={result()?.path}>
                  {result()?.path ?? "Could not run the launch check"}
                </p>
              </div>
            </div>

            <div class="flex shrink-0 items-center gap-2">
              <Show when={result()?.version}>
                {(version) => (
                  <span class="rounded border border-line bg-surface px-2 py-0.5 font-mono text-[10.5px] text-muted">
                    {version()}
                  </span>
                )}
              </Show>
              <Show
                when={ok()}
                fallback={
                  <span class="rounded border border-fault/30 bg-fault/10 px-2 py-0.5 text-[10.5px] font-medium text-fault">
                    Does not start
                  </span>
                }
              >
                <span class="flex items-center gap-1.5 rounded border border-signal/30 bg-signal/10 px-2 py-0.5 text-[10.5px] font-medium text-signal">
                  <IconCheck size={11} strokeWidth={2.5} />
                  Starts
                </span>
              </Show>
            </div>
          </div>

          <Show when={!ok()}>
            <div class="space-y-2 rounded-lg border border-fault/20 bg-fault/5 p-2.5">
              <p class="font-mono text-[10.5px] leading-relaxed text-fault">
                {result()?.error ?? "The launch check could not be run from this window."}
              </p>
              <Show when={result()?.hint}>
                {(hint) => <p class="text-[11px] leading-relaxed text-foreground">{hint()}</p>}
              </Show>
              <Show when={result()?.log_path}>
                {(logPath) => (
                  <div class="flex items-center gap-2">
                    <code class="min-w-0 flex-1 select-all truncate rounded border border-line bg-surface px-2 py-0.5 font-mono text-[10.5px] text-foreground">
                      {logPath()}
                    </code>
                    <button
                      type="button"
                      class="focus-ring shrink-0 cursor-pointer rounded-md border border-line bg-surface px-2 py-0.5 text-xs font-medium text-foreground transition-colors hover:bg-line/40"
                      onClick={() => void (props.showLog ?? openDaemonLog)()}
                    >
                      Show log
                    </button>
                  </div>
                )}
              </Show>
            </div>
          </Show>
        </div>
      </div>
    </Show>
  );
}
