import { For, Show, createSignal, onMount } from "solid-js";

import type { AgentChoice, Lane } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import Modal from "./Modal";

export default function SpawnModal(props: { lane: Lane; onClose: () => void; onDone: () => Promise<void> }) {
  const [choices, setChoices] = createSignal<AgentChoice[]>([]);
  const [agent, setAgent] = createSignal("");
  const [task, setTask] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  onMount(() => {
    void daemonCall("agent.detect")
      .then((detected) => {
        setChoices(detected);
        const preferred = detected.find((choice) => choice.default && choice.detected)
          ?? detected.find((choice) => choice.detected)
          ?? detected[0];
        setAgent(preferred?.name ?? "claude-code");
      })
      .catch((cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause)));
  });

  async function spawn() {
    if (!agent()) return;
    setBusy(true);
    setError(null);
    try {
      await daemonCall("agent.spawn", { lane_id: props.lane.id, agent: agent(), task: task().trim() || undefined });
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
      <button type="button" class="focus-ring rounded bg-signal px-4 py-2 font-mono text-[0.6rem] font-semibold uppercase text-background disabled:opacity-50" disabled={busy() || !agent()} onClick={() => void spawn()}>
        {busy() ? "Spawning…" : "Spawn"}
      </button>
    </>
  );

  return (
    <Modal title="Spawn agent" subtitle={`${props.lane.repo.name} / ${props.lane.worktree.branch ?? props.lane.worktree.name}`} onClose={props.onClose} footer={footer}>
      <div class="space-y-4">
        <div>
          <p class="section-label mb-2">Agent</p>
          <div class="grid gap-2 sm:grid-cols-2">
            <For each={choices()}>
              {(choice) => (
                <button
                  type="button"
                  class={`focus-ring flex items-center justify-between rounded-md border px-3 py-2 text-left text-xs ${agent() === choice.name ? "border-signal/50 bg-signal/10 text-foreground" : "border-line text-muted"}`}
                  onClick={() => setAgent(choice.name)}
                >
                  <span class="truncate">{choice.name}</span>
                  <Show when={!choice.detected}><span class="font-mono text-[0.5rem] uppercase text-fault">missing</span></Show>
                  <Show when={choice.default}><span class="font-mono text-[0.5rem] uppercase text-signal">default</span></Show>
                </button>
              )}
            </For>
          </div>
        </div>
        <label class="block">
          <span class="section-label">Initial task (optional)</span>
          <textarea class="settings-input min-h-[4.5rem] resize-y" value={task()} placeholder="What should this agent work on?" onInput={(event) => setTask(event.currentTarget.value)} />
        </label>
        <Show when={error()}>
          <p class="rounded-md border border-fault/40 bg-fault/8 p-2 text-xs text-fault">{error()}</p>
        </Show>
      </div>
    </Modal>
  );
}
