import { For, Show, batch, createEffect, createSignal, createUniqueId, onMount } from "solid-js";

import type { AgentChoice, Lane } from "../bindings";
import { pickDefaultAgent } from "../ipc/agentChoices";
import { translateError, type TranslatedError } from "../ipc/errors";
import { daemonCall } from "../ipc/rpc";
import { cachedAgentChoices, loadAgentChoices, refreshAgentChoices } from "../stores/agentChoices";
import { AgentIcon, IconExternalLink } from "./icons";
import Modal from "./Modal";

/// The runtime grid is two columns at every width, so Left and Right always cross columns and Up
/// and Down always cross rows. A grid that reflowed to one column would make half the arrow keys
/// lie about where focus goes.
const RUNTIME_COLUMNS = 2;

/// Direct selection only reaches as far as there are digits to press.
const MAX_DIGIT_SHORTCUTS = 9;

/// Where the caret starts, and where it retreats to when the runtime it was on stops existing.
function defaultIndex(list: AgentChoice[]): number {
  const preferred = pickDefaultAgent(list);
  return Math.max(0, list.findIndex((choice) => choice.name === preferred));
}

export default function SpawnModal(props: {
  lane: Lane;
  onClose: () => void;
  onDone: () => Promise<void>;
  onOpenSettingsTab?: (tab: import("./SettingsModal").SettingsTab) => void;
}) {
  const initialChoices = cachedAgentChoices();
  const [choices, setChoices] = createSignal<AgentChoice[]>(initialChoices ?? []);
  const [choicesLoading, setChoicesLoading] = createSignal(!initialChoices);
  const [choicesError, setChoicesError] = createSignal<TranslatedError | null>(null);
  const [task, setTask] = createSignal("");
  const [warnings, setWarnings] = createSignal<string[]>([]);
  const [spawned, setSpawned] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<TranslatedError | null>(null);
  const [focusIndex, setFocusIndex] = createSignal(initialChoices ? defaultIndex(initialChoices) : 0);

  const groupLabelId = createUniqueId();
  const hintId = createUniqueId();
  const tiles: Array<HTMLButtonElement | undefined> = [];
  let contentRoot!: HTMLDivElement;

  const focusedChoice = () => choices()[focusIndex()];
  /// The selection is not a second state that could drift from the caret; it is the caret, read
  /// through the one question that matters. So the marked runtime is always the runtime Enter
  /// spawns, and when Enter would not spawn - an uninstalled runtime - nothing is marked at all.
  const agent = () => {
    const choice = focusedChoice();
    return choice?.detected ? choice.name : "";
  };
  /// Without a Settings tab to open there is nothing to say about a missing runtime beyond the
  /// badge, so the tile must not advertise an explanation it cannot produce.
  const canExplainMissing = () => Boolean(props.onOpenSettingsTab);

  // A cache hit paints instantly with no loading flash; only a genuinely uncached (or forced)
  // load shows the skeleton. A background refresh from a warm cache never surfaces its own
  // failure - the operator keeps looking at the choices that already worked.
  function loadChoices(force = false) {
    setChoicesError(null);
    if (force || !cachedAgentChoices()) setChoicesLoading(true);
    const request = force ? refreshAgentChoices() : loadAgentChoices();
    void request
      .then((detected) => {
        const held = focusedChoice()?.name;
        // One batch, because the caret is the selection: a render between the two writes would
        // paint the mark on whatever runtime happened to inherit the old index.
        batch(() => {
          setChoices(detected);
          const kept = held ? detected.findIndex((choice) => choice.name === held) : -1;
          setFocusIndex(kept >= 0 ? kept : defaultIndex(detected));
        });
        setChoicesLoading(false);
      })
      .catch((cause: unknown) => {
        setChoicesLoading(false);
        setChoicesError(translateError(cause));
      });
  }

  onMount(() => loadChoices());

  // Detection can finish after the shell has already placed focus. Put the caret on the
  // preselected runtime when it lands, unless the operator has meanwhile chosen somewhere to be.
  let adoptedFocus = false;
  createEffect(() => {
    const detected = choices();
    if (adoptedFocus || !detected.length) return;
    adoptedFocus = true;
    const active = document.activeElement;
    const engaged = active instanceof HTMLElement && (contentRoot.contains(active) || Boolean(active.closest("footer")));
    if (!engaged) queueMicrotask(() => tiles[focusIndex()]?.focus());
  });

  function focusTile(index: number) {
    const count = choices().length;
    if (!count) return;
    const wrapped = ((index % count) + count) % count;
    setFocusIndex(wrapped);
    tiles[wrapped]?.focus();
  }

  function openInstallHelp(choice: AgentChoice) {
    if (!choice.detected && canExplainMissing()) {
      props.onClose();
      props.onOpenSettingsTab?.("system");
    }
  }

  /// Reaching a tile is what selects it, so activation has nothing left to choose: on a detected
  /// runtime it only commits, and on a missing one it can only explain.
  function activateTile(index: number, alsoSpawn: boolean) {
    const choice = choices()[index];
    if (!choice) return;
    setFocusIndex(index);
    if (!choice.detected) {
      openInstallHelp(choice);
      return;
    }
    if (alsoSpawn) void spawn(choice.name);
  }

  function onGridKeyDown(event: KeyboardEvent) {
    const count = choices().length;
    if (!count || event.altKey || event.metaKey || event.ctrlKey) return;
    // The key belongs to the tile it was pressed on, not to whatever the roving index last
    // recorded, so a stray focus never lands a keystroke on a different runtime.
    const pressed = tiles.findIndex((tile) => tile === event.target);
    const current = pressed >= 0 ? pressed : focusIndex();
    const move = (delta: number) => {
      event.preventDefault();
      focusTile(current + delta);
    };
    switch (event.key) {
      case "ArrowRight":
        return move(1);
      case "ArrowLeft":
        return move(-1);
      case "ArrowDown":
        return move(RUNTIME_COLUMNS);
      case "ArrowUp":
        return move(-RUNTIME_COLUMNS);
      case "Home":
        event.preventDefault();
        return focusTile(0);
      case "End":
        event.preventDefault();
        return focusTile(count - 1);
      case " ":
        event.preventDefault();
        return activateTile(current, false);
      case "Enter":
        event.preventDefault();
        return activateTile(current, true);
      default:
        break;
    }
    // Eight runtimes is too many to hunt through with arrows, so each tile answers to its own
    // digit. A missing runtime still takes the focus, because reading why is the only thing
    // left to do with it.
    if (!/^[1-9]$/.test(event.key)) return;
    const index = Number(event.key) - 1;
    if (index >= Math.min(count, MAX_DIGIT_SHORTCUTS)) return;
    event.preventDefault();
    focusTile(index);
  }

  /// Enter commits the dialog from anywhere it cannot mean something else. It means a newline in
  /// the task description, so there the platform chord commits instead; and it means "press me"
  /// on any other button, so those keep it.
  function onContentKeyDown(event: KeyboardEvent) {
    if (event.defaultPrevented || event.key !== "Enter") return;
    const target = event.target as HTMLElement | null;
    const chord = event.metaKey || event.ctrlKey;
    if (!chord && (target?.tagName === "TEXTAREA" || target?.tagName === "BUTTON")) return;
    event.preventDefault();
    void spawn();
  }

  async function spawn(agentName = agent()) {
    if (!agentName || busy() || spawned()) return;
    const choice = choices().find((entry) => entry.name === agentName);
    if (choice && !choice.detected) return;
    setBusy(true);
    setError(null);
    try {
      const result = await daemonCall("agent.spawn", { lane_id: props.lane.id, agent: agentName, task: task().trim() || undefined });
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

  function tileName(choice: AgentChoice): string {
    if (!choice.detected) return `${choice.name}, not installed`;
    return choice.default ? `${choice.name}, default runtime` : choice.name;
  }

  function gridHint(): string {
    const choice = focusedChoice();
    if (choice && !choice.detected) {
      return canExplainMissing()
        ? `${choice.name} is not installed. Enter opens its install instructions.`
        : `${choice.name} is not installed, so it cannot be spawned.`;
    }
    const digits = Math.min(choices().length, MAX_DIGIT_SHORTCUTS);
    const direct = digits > 1 ? `1 to ${digits} picks a runtime, ` : "";
    // The mark answers which runtime Enter takes, so the hint is left with its real job: saying
    // that these keys exist at all.
    return `Arrows move, ${direct}Enter spawns.`;
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
      <div class="space-y-4" ref={contentRoot} onKeyDown={onContentKeyDown}>
        <div>
          <span class="section-label mb-2 block" id={groupLabelId}>Select Runtime</span>
          <Show when={choicesLoading()}>
            <div class="grid gap-2 grid-cols-2" role="status" aria-label="Detecting agent runtimes">
              <For each={[0, 1, 2, 3]}>{() => <div class="h-[52px] animate-pulse rounded-xl border border-line bg-surface" />}</For>
            </div>
          </Show>
          <Show when={choicesError()}>
            {(err) => (
              <div role="alert" class="break-words rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault space-y-2">
                <p class="font-medium leading-snug">{err().friendly}</p>
                <button
                  type="button"
                  class="focus-ring rounded bg-fault/10 hover:bg-fault/20 border border-fault/30 px-2 py-1 font-mono text-[10px] uppercase font-semibold text-fault transition-colors cursor-pointer"
                  onClick={() => loadChoices(true)}
                >
                  Retry
                </button>
              </div>
            )}
          </Show>
          <Show when={!choicesLoading() && !choicesError()}>
            <Show
              when={choices().length}
              fallback={<p class="text-xs text-muted">No agent runtimes detected on PATH.</p>}
            >
              <div
                role="radiogroup"
                aria-labelledby={groupLabelId}
                aria-describedby={hintId}
                class="grid gap-2 grid-cols-2"
                onKeyDown={onGridKeyDown}
              >
                <For each={choices()}>
                  {(choice, index) => (
                    // The one mark is the raised ground. It is tonal rather than a ring because
                    // index.css hands the signal ring to every focused button, a focused MISSING
                    // tile included, and that tile must never read as selected; the ground also
                    // survives focus leaving the grid, which a ring would not.
                    <button
                      ref={(element) => (tiles[index()] = element)}
                      type="button"
                      role="radio"
                      aria-checked={agent() === choice.name}
                      aria-disabled={choice.detected ? undefined : true}
                      aria-label={tileName(choice)}
                      tabindex={index() === focusIndex() ? 0 : -1}
                      autofocus={index() === focusIndex() || undefined}
                      title={choice.name}
                      data-selected={agent() === choice.name ? "" : undefined}
                      class={`focus-ring flex min-w-0 items-center justify-between gap-2 rounded-xl border p-3 text-left transition-colors ${
                        agent() === choice.name
                          ? "border-muted bg-raised text-foreground"
                          : choice.detected
                            ? "border-line bg-surface text-muted hover:bg-raised/40"
                            : "border-dashed border-line bg-surface text-muted hover:border-fault/40 hover:bg-raised/40"
                      }`}
                      onFocus={() => setFocusIndex(index())}
                      onClick={() => activateTile(index(), false)}
                    >
                      <span class="flex min-w-0 items-center gap-2">
                        <Show when={index() < MAX_DIGIT_SHORTCUTS}>
                          <span aria-hidden="true" class="w-2 shrink-0 text-right font-mono text-[9px] tabular-nums text-muted/60">
                            {index() + 1}
                          </span>
                        </Show>
                        <span class={`shrink-0 ${agent() === choice.name ? "text-foreground" : "text-muted"}`}>
                          <AgentIcon agent={choice.name} size={15} />
                        </span>
                        <span class={`min-w-0 truncate text-xs ${agent() === choice.name ? "font-semibold" : "font-medium"}`}>
                          {choice.name}
                        </span>
                      </span>
                      <span class="flex shrink-0 items-center gap-1.5">
                        <Show when={choice.default}>
                          {/* Which runtime is configured as the default is a fact about settings,
                              not a state of this dialog, so it stays out of the semantic hues. */}
                          <span class="rounded bg-line/70 px-1.5 py-0.5 font-mono text-[9px] uppercase font-semibold text-muted">default</span>
                        </Show>
                        <Show when={!choice.detected}>
                          <span class="flex items-center gap-1 rounded bg-fault/10 px-1.5 py-0.5 font-mono text-[9px] uppercase font-semibold text-fault">
                            missing
                            <Show when={canExplainMissing()}>
                              <IconExternalLink size={9} />
                            </Show>
                          </span>
                        </Show>
                      </span>
                    </button>
                  )}
                </For>
              </div>
              <p id={hintId} class="mt-2 font-mono text-[10px] leading-relaxed text-muted/80">{gridHint()}</p>
            </Show>
          </Show>
        </div>
        <label class="block">
          <span class="section-label">Initial Task Description (Optional)</span>
          <textarea
            class="focus-ring mt-1.5 min-h-[5rem] w-full resize-y rounded-xl border border-line bg-background p-3 text-xs text-foreground outline-none placeholder:text-muted/60"
            value={task()}
            placeholder="Describe what this agent should start on…"
            onInput={(event) => setTask(event.currentTarget.value)}
          />
          <span class="mt-1 block font-mono text-[10px] text-muted/80">Cmd or Ctrl and Enter spawns from here.</span>
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
