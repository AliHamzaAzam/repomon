import { For, Show, createSignal, onCleanup, onMount } from "solid-js";

import type { TranscriptItem } from "../bindings";
import { daemonCall, subscribeDaemon, type OrchestratorStatus } from "../ipc/rpc";

type RepomindView = "live" | "transcript";

export default function RepomindPanel() {
  const [status, setStatus] = createSignal<OrchestratorStatus>({ running: false });
  const [items, setItems] = createSignal<TranscriptItem[]>([]);
  const [liveOutput, setLiveOutput] = createSignal("");
  const [message, setMessage] = createSignal("");
  const [view, setView] = createSignal<RepomindView>("live");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  let active = true;
  let timer: ReturnType<typeof setInterval> | undefined;
  let unsubscribe: (() => void) | undefined;

  function errorMessage(cause: unknown) {
    return cause instanceof Error ? cause.message : String(cause);
  }

  async function refresh() {
    try {
      const next = await daemonCall("orchestrator.status");
      if (!active) return;
      setStatus(next);
      if (next.running) setItems(await daemonCall("orchestrator.transcript", { limit: 60 }));
      else setItems([]);
    } catch (cause) {
      if (active) setError(errorMessage(cause));
    }
  }

  onMount(() => {
    void daemonCall("orchestrator.watch", { on: true }).catch((cause: unknown) => setError(errorMessage(cause)));
    void subscribeDaemon((event) => {
      if (event.method === "event.orchestrator.output") {
        const content = (event.params as { content?: unknown }).content;
        if (typeof content === "string") setLiveOutput(content);
      } else if (event.method === "event.orchestrator.status") {
        setStatus(event.params as OrchestratorStatus);
      }
    }).then((stop) => {
      if (active) unsubscribe = stop;
      else stop();
    }).catch((cause: unknown) => setError(errorMessage(cause)));
    void refresh();
    timer = setInterval(() => void refresh(), 1500);
  });

  onCleanup(() => {
    active = false;
    if (timer) clearInterval(timer);
    unsubscribe?.();
    void daemonCall("orchestrator.watch", { on: false }).catch(() => undefined);
  });

  async function lifecycle(action: "start" | "restart" | "stop") {
    setBusy(action);
    setError(null);
    try {
      if (action === "stop" || action === "restart") await daemonCall("orchestrator.stop");
      if (action === "start" || action === "restart") await daemonCall("orchestrator.start", {});
      if (action !== "stop") setView("live");
      if (action === "stop") setLiveOutput("");
      await refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  async function send() {
    const text = message().trim();
    if (!text || !status().running) return;
    setBusy("send");
    setError(null);
    try {
      await daemonCall("orchestrator.send_input", { text, enter: true });
      setMessage("");
      setView("live");
      setTimeout(() => void refresh(), 250);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
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
        <div class="flex items-center gap-1">
          <Show when={status().running}>
            <button type="button" class="focus-ring rounded border border-line px-2 py-1 font-mono text-[0.52rem] uppercase text-muted hover:text-foreground" onClick={() => void lifecycle("restart")} disabled={Boolean(busy())}>
              {busy() === "restart" ? "Restarting…" : "Restart"}
            </button>
          </Show>
          <button
            type="button"
            class={`focus-ring rounded border px-2 py-1 font-mono text-[0.55rem] uppercase ${status().running ? "border-fault/40 text-fault" : "border-signal/40 text-signal"}`}
            onClick={() => void lifecycle(status().running ? "stop" : "start")}
            disabled={Boolean(busy())}
          >{busy() === "start" ? "Starting…" : busy() === "stop" ? "Stopping…" : status().running ? "Stop" : "Start"}</button>
        </div>
      </div>

      <Show when={error()}>
        {(message) => (
          <div role="alert" class="m-3 mb-0 flex items-start justify-between gap-2 rounded-md border border-fault/40 bg-fault/8 p-2 text-xs text-fault">
            <span>{message()}</span>
            <button type="button" class="focus-ring rounded px-1 text-muted hover:text-foreground" aria-label="Dismiss repomind error" onClick={() => setError(null)}>×</button>
          </div>
        )}
      </Show>

      <div class="flex items-center gap-1 border-b border-line px-3 py-2" role="tablist" aria-label="Repomind views">
        <button type="button" role="tab" aria-selected={view() === "live"} class={`focus-ring rounded px-2 py-1 font-mono text-[0.52rem] uppercase ${view() === "live" ? "bg-signal/10 text-signal" : "text-muted"}`} onClick={() => setView("live")}>Attach</button>
        <button type="button" role="tab" aria-selected={view() === "transcript"} class={`focus-ring rounded px-2 py-1 font-mono text-[0.52rem] uppercase ${view() === "transcript" ? "bg-signal/10 text-signal" : "text-muted"}`} onClick={() => setView("transcript")}>Transcript</button>
        <Show when={status().attention && status().attention !== "none"}>
          <span class="ml-auto rounded border border-attention/40 bg-attention/10 px-1.5 py-0.5 font-mono text-[0.48rem] uppercase text-attention">{status().attention}</span>
        </Show>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <Show when={view() === "live"} fallback={
          <div class="space-y-3">
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
                {status().running && status().backend === "codex" ? "This backend exposes its live pane instead of a structured transcript." : status().running ? "Waiting for the first orchestrator turn." : "Start repomind to coordinate work across the fleet."}
              </p>
            </Show>
          </div>
        }>
          <pre aria-label="Repomind live pane" class="min-h-full whitespace-pre-wrap break-words font-mono text-[0.62rem] leading-relaxed text-muted">{liveOutput() || (status().running ? "Attaching to the live repomind pane…" : "Start repomind to attach to its live pane.")}</pre>
        </Show>
      </div>

      <form class="border-t border-line p-3" onSubmit={(event) => { event.preventDefault(); void send(); }}>
        <textarea
          aria-label="Message repomind"
          class="focus-ring min-h-16 w-full resize-none rounded-md border border-line bg-background p-2 text-xs outline-none placeholder:text-muted/70"
          placeholder="Coordinate the fleet…"
          value={message()}
          onInput={(event) => setMessage(event.currentTarget.value)}
          disabled={!status().running || busy() === "send"}
        />
        <button
          type="submit"
          class="focus-ring mt-2 w-full rounded-md bg-signal px-2 py-1.5 font-mono text-[0.58rem] font-semibold uppercase tracking-[0.08em] text-background disabled:opacity-40"
          disabled={!status().running || !message().trim() || busy() === "send"}
        >{busy() === "send" ? "Sending…" : "Send"}</button>
      </form>
    </div>
  );
}
