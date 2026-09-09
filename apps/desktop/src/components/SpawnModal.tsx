import { For, Show, createSignal, onMount } from "solid-js";

import type { AgentChoice, Lane } from "../bindings";
import { translateError, type TranslatedError } from "../ipc/errors";
import { daemonCall } from "../ipc/rpc";
import { AgentIcon } from "./icons";
import Modal from "./Modal";

export default function SpawnModal(props: {
  lane: Lane;
  onClose: () => void;
  onDone: () => Promise<void>;
  onOpenSettingsTab?: (tab: import("./SettingsModal").SettingsTab) => void;
}) {
  const [choices, setChoices] = createSignal<AgentChoice[]>([]);
  const [agent, setAgent] = createSignal("");
  const [task, setTask] = createSignal("");
  const [warnings, setWarnings] = createSignal<string[]>([]);
  const [spawned, setSpawned] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<TranslatedError | null>(null);

  onMount(() => {
    void daemonCall("agent.detect")
      .then((detected) => {
        setChoices(detected);
        const preferred = detected.find((choice) => choice.default && choice.detected)
          ?? detected.find((choice) => choice.detected)
          ?? detected[0];
        setAgent(preferred?.name ?? "claude-code");
      })
      .catch((cause: unknown) => setError(translateError(cause)));
  });

  async function spawn() {
    if (!agent() || busy() || spawned()) return;
    setBusy(true);
    setError(null);
    try {
      const result = await daemonCall("agent.spawn", { lane_id: props.lane.id, agent: agent(), task: task().trim() || undefined });
      setSpawned(true);
      setWarnings(result.spawn_warnings ?? []);
      await props.onDone();
      if (warnings().length === 0) props.onClose();
    } catch (cause) {
      setError(translateError(cause, { binary: "tmux" }));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title="Spawn agent"
      subtitle={`${props.lane.repo.name} / ${props.lane.worktree.branch ?? props.lane.worktree.name}`}
      onClose={props.onClose}
      footer={
        <>
          <button
            type="button"
            class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
            onClick={props.onClose}
          >
            {spawned() ? "Close" : "Cancel"}
          </button>
          <button
            type="button"
            class="focus-ring rounded-lg bg-signal px-4 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-50"
            disabled={busy() || !agent() || spawned()}
            onClick={() => void spawn()}
          >
            {spawned() ? "Agent started" : busy() ? "Spawning…" : "Spawn Agent"}
          </button>
        </>
      }
    >
      <div class="space-y-4">
        <div>
          <span class="section-label mb-2 block">Select Runtime</span>
          <div class="grid gap-2 sm:grid-cols-2">
            <For each={choices()}>
              {(choice) => (
                <div
                  class={`flex min-w-0 items-stretch rounded-xl border transition-colors ${
                    agent() === choice.name
                      ? "border-signal bg-signal/5 ring-1 ring-signal/20 text-foreground"
                      : "border-line bg-surface text-muted hover:border-muted/50 hover:bg-raised/40"
                  }`}
                >
                  <button
                    type="button"
                    class="focus-ring flex min-w-0 flex-1 items-center justify-between gap-2 rounded-xl p-3 text-left"
                    aria-label={`Select ${choice.name}`}
                    aria-pressed={agent() === choice.name}
                    title={choice.name}
                    onClick={() => setAgent(choice.name)}
                  >
                    <span class="flex min-w-0 items-center gap-2">
                      <span class={`shrink-0 ${agent() === choice.name ? "text-signal" : "text-muted"}`}>
                        <AgentIcon agent={choice.name} size={15} />
                      </span>
                      <span class="min-w-0 truncate text-xs font-medium">{choice.name}</span>
                    </span>
                    <Show when={choice.default}>
                      <span class="shrink-0 rounded bg-signal/10 px-1.5 py-0.5 font-mono text-[9px] uppercase font-semibold text-signal">default</span>
                    </Show>
                  </button>
                  <Show when={!choice.detected}>
                    <Show
                      when={props.onOpenSettingsTab}
                      fallback={
                        <span class="mr-3 self-center rounded bg-fault/10 px-1.5 py-0.5 font-mono text-[9px] uppercase font-semibold text-fault">
                          missing
                        </span>
                      }
                    >
                      <button
                        type="button"
                        class="focus-ring mr-3 shrink-0 self-center rounded bg-fault/10 hover:bg-fault/20 border border-fault/30 px-1.5 py-0.5 font-mono text-[9px] uppercase font-semibold text-fault transition-colors cursor-pointer"
                        title="View installation instructions in Settings > System"
                        aria-label={`View install instructions for ${choice.name} in System Health`}
                        onClick={() => {
                          props.onClose();
                          props.onOpenSettingsTab?.("system");
                        }}
                      >
                        missing ↗
                      </button>
                    </Show>
                  </Show>
                </div>
              )}
            </For>
          </div>
        </div>
        <label class="block">
          <span class="section-label">Initial Task Description (Optional)</span>
          <textarea
            class="focus-ring mt-1.5 min-h-[5rem] w-full resize-y rounded-xl border border-line bg-background p-3 text-xs text-foreground outline-none placeholder:text-muted/60"
            value={task()}
            placeholder="Describe what this agent should start on…"
            onInput={(event) => setTask(event.currentTarget.value)}
          />
        </label>
        <Show when={warnings().length > 0}>
          <div role="alert" class="break-words rounded-xl border border-line bg-surface p-3 text-xs text-foreground space-y-2">
            <p class="font-medium">Agent started with a warning</p>
            <For each={warnings()}>{(warning) => <p>{warning}</p>}</For>
          </div>
        </Show>
        <Show when={error()}>
          {(err) => (
            <div role="alert" class="break-words rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault space-y-1.5">
              <div class="flex items-start gap-2">
                <span class="mt-0.5 inline-block h-1.5 w-1.5 shrink-0 rounded-full bg-fault" />
                <p class="flex-1 font-medium leading-snug">{err().friendly}</p>
              </div>
              {/* Missing-binary failures link to System settings for installation guidance. */}
              <Show when={err().isMissingBinary && props.onOpenSettingsTab}>
                <div class="pt-0.5">
                  <button
                    type="button"
                    class="focus-ring rounded bg-fault/10 hover:bg-fault/20 border border-fault/30 px-2 py-0.5 font-mono text-[9px] uppercase font-semibold text-fault transition-colors cursor-pointer"
                    onClick={() => {
                      props.onClose();
                      props.onOpenSettingsTab?.("system");
                    }}
                  >
                    View install instructions in Settings › System ↗
                  </button>
                </div>
              </Show>
              <Show when={err().raw && err().raw !== err().friendly}>
                <details class="group mt-1 pt-1 border-t border-fault/20">
                  <summary class="cursor-pointer select-none text-[11px] font-normal text-fault/75 hover:text-fault transition-colors outline-none">
                    Technical details
                  </summary>
                  <pre class="mt-1 max-h-24 overflow-x-auto whitespace-pre-wrap rounded bg-background/50 p-2 font-mono text-[10px] text-muted">
                    {err().raw}
                  </pre>
                </details>
              </Show>
            </div>
          )}
        </Show>
      </div>
    </Modal>
  );
}
