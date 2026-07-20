import { For, Show, createSignal, onCleanup, onMount } from "solid-js";

import type { TranscriptItem } from "../bindings";
import { daemonCall, type OrchestratorStatus } from "../ipc/rpc";

export default function RepomindPanel() {
  const [status, setStatus] = createSignal<OrchestratorStatus>({ running: false });
  const [items, setItems] = createSignal<TranscriptItem[]>([]);
  const [message, setMessage] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  let active = true;
  let timer: ReturnType<typeof setInterval> | undefined;

  async function refresh() {
    try {
      const next = await daemonCall("orchestrator.status");
      if (!active) return;
      setStatus(next);
      if (next.running) setItems(await daemonCall("orchestrator.transcript", { limit: 60 }));
      else setItems([]);
    } catch {
      // Connection state is already visible in the footer.
    }
  }

  onMount(() => {
    void refresh();
    timer = setInterval(() => void refresh(), 1500);
  });

  onCleanup(() => {
    active = false;
    if (timer) clearInterval(timer);
  });

  async function toggle() {
    setBusy(true);
    try {
      if (status().running) await daemonCall("orchestrator.stop");
      else await daemonCall("orchestrator.start", {});
      await refresh();
    } finally {
      setBusy(false);
    }
  }

  async function send() {
    const text = message().trim();
    if (!text || !status().running) return;
    setMessage("");
    await daemonCall("orchestrator.send_input", { text, enter: true });
    setTimeout(() => void refresh(), 250);
  }

  return (
    <div class="flex min-h-0 flex-1 flex-col">
      <div class="flex items-center justify-between border-b border-line px-3 py-2">
        <div class="min-w-0">
          <p class="truncate text-xs font-medium">{status().running ? status().headline ?? "Orchestrating fleet" : "Orchestrator offline"}</p>
          <p class="mt-0.5 truncate font-mono text-[0.52rem] uppercase tracking-[0.08em] text-muted">
            {status().running ? `${status().agent ?? "agent"} ${status().model ?? ""}` : "local daemon session"}
          </p>
        </div>
        <button
          type="button"
          class={`focus-ring rounded border px-2 py-1 font-mono text-[0.55rem] uppercase ${status().running ? "border-fault/40 text-fault" : "border-signal/40 text-signal"}`}
          onClick={() => void toggle()}
          disabled={busy()}
        >{busy() ? "…" : status().running ? "Stop" : "Start"}</button>
      </div>

      <div class="min-h-0 flex-1 space-y-3 overflow-y-auto p-3">
        <For each={items()}>
          {(item) => (
            <article class={`repomind-message is-${item.role}`}>
              <p class="mb-1 font-mono text-[0.5rem] uppercase tracking-[0.1em] text-muted">{item.role}</p>
              <p class="whitespace-pre-wrap text-xs leading-relaxed">{item.text}</p>
            </article>
          )}
        </For>
        <Show when={!items().length}>
          <p class="border-l border-line pl-3 text-xs leading-relaxed text-muted">
            {status().running ? "Waiting for the first orchestrator turn." : "Start repomind to coordinate work across the fleet."}
          </p>
        </Show>
      </div>

      <form class="border-t border-line p-3" onSubmit={(event) => { event.preventDefault(); void send(); }}>
        <textarea
          aria-label="Message repomind"
          class="focus-ring min-h-16 w-full resize-none rounded-md border border-line bg-background p-2 text-xs outline-none placeholder:text-muted/70"
          placeholder="Coordinate the fleet…"
          value={message()}
          onInput={(event) => setMessage(event.currentTarget.value)}
          disabled={!status().running}
        />
        <button
          type="submit"
          class="focus-ring mt-2 w-full rounded-md bg-signal px-2 py-1.5 font-mono text-[0.58rem] font-semibold uppercase tracking-[0.08em] text-background disabled:opacity-40"
          disabled={!status().running || !message().trim()}
        >Send</button>
      </form>
    </div>
  );
}
