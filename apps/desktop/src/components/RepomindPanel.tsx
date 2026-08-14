import { For, Show, createSignal, onCleanup, onMount } from "solid-js";

import type { TranscriptItem } from "../bindings";
import { stripAnsi, trimBlankEdges } from "./ansi";
import { daemonCall, subscribeDaemon, type OrchestratorStatus } from "../ipc/rpc";
import { IconClose, IconPlay, IconRefresh, IconSparkles, IconStop } from "./icons";

type RepomindView = "live" | "transcript";

interface RepomindPanelProps {
  fullscreen?: boolean;
  onToggleFullscreen?: () => void;
}

const AWAITING_ANSWER = ["permission", "decision"];

export default function RepomindPanel(props: RepomindPanelProps) {
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

  const pane = () => trimBlankEdges(stripAnsi(liveOutput()));
  const awaitingAnswer = () => AWAITING_ANSWER.includes(status().attention ?? "");

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

  async function sendKey(key: string, label = key) {
    if (!status().running) return;
    setBusy(`key:${label}`);
    setError(null);
    try {
      await daemonCall("orchestrator.key", { key });
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
      await daemonCall("orchestrator.send_input", { text });
      setMessage("");
      setView("live");
      await refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div class="flex h-full flex-col bg-surface">
      <div class="flex items-center justify-between border-b border-line px-3.5 py-2.5">
        <div class="flex items-center gap-2">
          <span class={`lane-pulse ${status().running ? "is-signal" : ""}`} />
          <div>
            <div class="flex items-center gap-1.5">
              <span class="text-xs font-semibold text-foreground">Repomind</span>
              <Show when={status().backend}>
                <span class="rounded border border-line bg-raised px-1 font-mono text-[9px] uppercase text-muted">
                  {status().backend}
                </span>
              </Show>
            </div>
          </div>
        </div>
        <div class="flex items-center gap-1.5">
          <Show when={props.onToggleFullscreen}>
            <button
              type="button"
              class="focus-ring flex h-6 items-center rounded border border-line bg-raised/50 px-2 text-[10px] font-medium text-muted hover:text-foreground"
              onClick={props.onToggleFullscreen}
            >
              {props.fullscreen ? "Collapse" : "Expand"}
            </button>
          </Show>
          <Show when={status().running}>
            <button
              type="button"
              class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
              onClick={() => void lifecycle("restart")}
              disabled={Boolean(busy())}
              title="Restart repomind"
              aria-label="Restart repomind"
            >
              <IconRefresh size={12} />
            </button>
          </Show>
          <button
            type="button"
            class={`focus-ring flex h-6 items-center gap-1 rounded-md border px-2 text-[11px] font-medium transition-colors ${
              status().running
                ? "border-fault/30 bg-fault/10 text-fault hover:bg-fault/20"
                : "border-signal/40 bg-signal/10 text-signal hover:bg-signal/20"
            }`}
            onClick={() => void lifecycle(status().running ? "stop" : "start")}
            disabled={Boolean(busy())}
          >
            {status().running ? <IconStop size={11} /> : <IconPlay size={11} />}
            <span>{busy() === "start" ? "Starting…" : busy() === "stop" ? "Stopping…" : status().running ? "Stop" : "Start"}</span>
          </button>
        </div>
      </div>

      <Show when={error()}>
        {(err) => (
          <div role="alert" class="m-3 mb-0 flex items-start justify-between gap-2 rounded-xl border border-fault/30 bg-fault/8 p-2.5 text-xs text-fault">
            <span>{err()}</span>
            <button type="button" class="focus-ring text-muted hover:text-foreground" aria-label="Dismiss repomind error" onClick={() => setError(null)}>
              <IconClose size={12} />
            </button>
          </div>
        )}
      </Show>

      <div class="flex items-center justify-between border-b border-line px-3 py-2" role="tablist" aria-label="Repomind views">
        <div class="flex items-center rounded-lg border border-line bg-raised/50 p-0.5">
          <button
            type="button"
            role="tab"
            aria-selected={view() === "live"}
            class={`focus-ring rounded-md px-2.5 py-0.5 text-[11px] font-medium transition-colors ${
              view() === "live" ? "bg-surface text-foreground shadow-xs font-semibold" : "text-muted hover:text-foreground"
            }`}
            onClick={() => setView("live")}
          >
            Live Feed
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={view() === "transcript"}
            class={`focus-ring rounded-md px-2.5 py-0.5 text-[11px] font-medium transition-colors ${
              view() === "transcript" ? "bg-surface text-foreground shadow-xs font-semibold" : "text-muted hover:text-foreground"
            }`}
            onClick={() => setView("transcript")}
          >
            Transcript
          </button>
        </div>
        <Show when={status().attention && status().attention !== "none"}>
          <span class="rounded-full border border-attention/40 bg-attention/10 px-2 py-0.5 font-mono text-[9px] font-semibold uppercase text-attention">
            {status().attention}
          </span>
        </Show>
      </div>

      <Show when={status().running && awaitingAnswer()}>
        <div class="flex items-center gap-1.5 border-b border-line bg-attention/5 px-3 py-2">
          <span class="mr-1 font-mono text-[10px] font-semibold uppercase text-attention">Answer:</span>
          <For each={["1", "2", "3"]}>
            {(digit) => (
              <button
                type="button"
                class="focus-ring flex size-6 items-center justify-center rounded-md border border-line bg-surface font-mono text-xs font-medium text-foreground hover:bg-raised"
                disabled={Boolean(busy())}
                onClick={() => void sendKey(digit)}
                title={`Pick option ${digit}`}
              >{digit}</button>
            )}
          </For>
          <button
            type="button"
            class="focus-ring ml-1 rounded-md border border-signal/40 bg-signal/10 px-2.5 py-1 font-mono text-[11px] font-medium text-signal hover:bg-signal/20"
            disabled={Boolean(busy())}
            onClick={() => void sendKey("Enter")}
            title="Confirm selected option"
          >{busy() === "key:Enter" ? "…" : "Enter"}</button>
          <button
            type="button"
            class="focus-ring rounded-md border border-line bg-surface px-2 py-1 font-mono text-[11px] font-medium text-muted hover:text-fault"
            disabled={Boolean(busy())}
            onClick={() => void sendKey("Escape")}
            title="Cancel prompt"
          >Esc</button>
        </div>
      </Show>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <Show when={view() === "live"} fallback={
          <div class="space-y-2.5">
            <For each={items()}>
              {(item) => (
                <article class={`repomind-message is-${item.role}`}>
                  <p class="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wider text-muted">{item.role}</p>
                  <p class="whitespace-pre-wrap text-xs leading-relaxed text-foreground">{item.text}</p>
                </article>
              )}
            </For>
            <Show when={!items().length}>
              <div class="rounded-xl border border-line bg-surface/50 p-4 text-center">
                <p class="text-xs text-muted">
                  {status().running && status().backend === "codex"
                    ? "This backend streams directly to the live feed."
                    : status().running
                    ? "Waiting for orchestrator activity…"
                    : "Start Repomind to coordinate work across the fleet."}
                </p>
              </div>
            </Show>
          </div>
        }>
          <pre
            aria-label="Repomind live pane"
            class={`font-mono text-xs leading-relaxed text-muted/90 ${props.fullscreen ? "overflow-x-auto whitespace-pre" : "whitespace-pre-wrap break-words"}`}
          >{pane() || (status().running ? "Attaching to the live repomind pane…" : "Start Repomind to view live agent orchestrations.")}</pre>
        </Show>
      </div>

      <form class="border-t border-line bg-surface/50 p-3" onSubmit={(event) => { event.preventDefault(); void send(); }}>
        <textarea
          aria-label="Message repomind"
          class="focus-ring min-h-16 w-full resize-none rounded-xl border border-line bg-background p-2.5 text-xs text-foreground outline-none placeholder:text-muted/60"
          placeholder="Coordinate the fleet or issue instructions…"
          value={message()}
          onInput={(event) => setMessage(event.currentTarget.value)}
          disabled={!status().running || busy() === "send"}
        />
        <button
          type="submit"
          class="focus-ring mt-2 flex w-full items-center justify-center gap-1.5 rounded-lg bg-signal px-3 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-40"
          disabled={!status().running || !message().trim() || busy() === "send"}
        >
          <IconSparkles size={13} />
          <span>{busy() === "send" ? "Sending…" : "Send Instruction"}</span>
        </button>
      </form>
    </div>
  );
}
