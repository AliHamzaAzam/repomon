import { For, Show, createSignal, onCleanup } from "solid-js";

import type { PaneSpan } from "../stores/workspace";
import { paneAccent, type PaneTarget } from "./terminalTargets";
import { AgentIcon, IconArrowDown, IconArrowUp, IconCheck, IconGrid } from "./icons";

interface PanePickerProps {
  available: PaneTarget[];
  selected: PaneTarget[];
  onChange: (windows: string[]) => void;
  multitasking?: boolean;
  spans?: Record<string, PaneSpan>;
  onSpanChange?: (window: string, span: PaneSpan) => void;
}

export default function PanePicker(props: PanePickerProps) {
  let root: HTMLDivElement | undefined;
  const [open, setOpen] = createSignal(false);
  const selectedWindows = () => props.selected.map((target) => target.window);

  const closeOnOutside = (event: PointerEvent) => {
    if (root && !root.contains(event.target as Node)) setOpen(false);
  };

  const toggleOpen = () => {
    const next = !open();
    setOpen(next);
    if (next) document.addEventListener("pointerdown", closeOnOutside);
    else document.removeEventListener("pointerdown", closeOnOutside);
  };

  onCleanup(() => document.removeEventListener("pointerdown", closeOnOutside));

  const toggle = (target: PaneTarget) => {
    const current = selectedWindows();
    if (current.includes(target.window)) {
      if (current.length === 1) return;
      props.onChange(current.filter((window) => window !== target.window));
    } else {
      props.onChange([...current, target.window]);
    }
  };

  const move = (window: string, delta: number) => {
    const current = selectedWindows();
    const from = current.indexOf(window);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= current.length) return;
    const next = [...current];
    [next[from], next[to]] = [next[to], next[from]];
    props.onChange(next);
  };

  const spanOf = (window: string): PaneSpan => props.spans?.[window] ?? { columns: 1, rows: 1 };

  return (
    <div ref={root} class="relative">
      <button
        type="button"
        class={`focus-ring flex h-6 items-center gap-1.5 rounded-md border px-2 font-mono text-[10px] uppercase tracking-wider transition-colors ${
          open() ? "border-signal/50 bg-signal/10 text-signal" : "border-line bg-raised/40 text-muted hover:bg-raised hover:text-foreground"
        }`}
        aria-haspopup="dialog"
        aria-expanded={open()}
        onClick={toggleOpen}
      >
        <IconGrid size={11} />
        <span>Panes</span>
        <span class="rounded bg-background/70 px-1 text-[9px]">{props.selected.length}</span>
      </button>

      <Show when={open()}>
        <section
          role="dialog"
          aria-label={props.multitasking ? "Choose multitasking panes" : "Choose lane panes"}
          class="absolute right-0 top-8 z-[60] w-[24rem] overflow-hidden rounded-xl border border-line bg-surface shadow-[0_18px_55px_var(--shadow)]"
        >
          <div class="border-b border-line bg-raised/30 px-3.5 py-3">
            <p class="section-label">{props.multitasking ? "Fleet workspace" : "Lane workspace"}</p>
            <p class="mt-1 text-xs leading-relaxed text-muted">
              {props.multitasking
                ? "Choose agents, set their footprint, and reorder the fleet view."
                : "Choose which agents stay visible in split and grid layouts."}
            </p>
          </div>

          <div class="max-h-[25rem] overflow-y-auto p-1.5">
            <For each={props.available}>
              {(target) => {
                const selected = () => selectedWindows().includes(target.window);
                const selectedIndex = () => selectedWindows().indexOf(target.window);
                const span = () => spanOf(target.window);
                return (
                  <div
                    class={`group rounded-lg border px-2.5 py-2 transition-colors ${
                      selected() ? "border-line bg-raised/45" : "border-transparent hover:bg-raised/25"
                    }`}
                  >
                    <div class="flex min-w-0 items-center gap-2">
                      <button
                        type="button"
                        class={`focus-ring flex size-5 shrink-0 items-center justify-center rounded border transition-colors ${
                          selected() ? "border-signal bg-signal text-white" : "border-line text-transparent hover:border-signal/60"
                        }`}
                        aria-label={`${selected() ? "Hide" : "Show"} ${target.label}`}
                        aria-pressed={selected()}
                        onClick={() => toggle(target)}
                      >
                        <IconCheck size={12} />
                      </button>
                      <span class={target.shell ? "text-attention" : "text-signal"}>
                        <AgentIcon agent={target.agent} shell={target.shell} size={14} />
                      </span>
                      <div class="min-w-0 flex-1">
                        <div class="truncate text-xs font-medium text-foreground">{target.label}</div>
                        <Show when={props.multitasking}>
                          <div class="mt-0.5 flex items-center gap-1.5 truncate font-mono text-[9px] uppercase tracking-wide text-muted">
                            <span class="size-1.5 shrink-0 rounded-full" style={{ "background-color": paneAccent(target) }} />
                            <span class="truncate">{target.repoName} / {target.branch || target.laneName}</span>
                          </div>
                        </Show>
                      </div>

                      <Show when={props.multitasking && selected()}>
                        <div class="flex shrink-0 items-center gap-0.5">
                          <button
                            type="button"
                            class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-background hover:text-foreground disabled:opacity-25"
                            aria-label={`Move ${target.label} earlier`}
                            disabled={selectedIndex() <= 0}
                            onClick={() => move(target.window, -1)}
                          >
                            <IconArrowUp size={10} />
                          </button>
                          <button
                            type="button"
                            class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-background hover:text-foreground disabled:opacity-25"
                            aria-label={`Move ${target.label} later`}
                            disabled={selectedIndex() === selectedWindows().length - 1}
                            onClick={() => move(target.window, 1)}
                          >
                            <IconArrowDown size={10} />
                          </button>
                        </div>
                      </Show>
                    </div>

                    <Show when={props.multitasking && selected()}>
                      <div class="mt-2 ml-12 flex items-center gap-2 border-t border-line/50 pt-2">
                        <span class="font-mono text-[9px] uppercase tracking-wider text-muted">Footprint</span>
                        <div class="ml-auto flex items-center gap-1">
                          <button
                            type="button"
                            class={`focus-ring rounded px-1.5 py-0.5 font-mono text-[9px] ${span().columns === 1 ? "bg-foreground text-background" : "bg-background text-muted hover:text-foreground"}`}
                            aria-label={`${target.label}: one column wide`}
                            onClick={() => props.onSpanChange?.(target.window, { ...span(), columns: 1 })}
                          >1×</button>
                          <button
                            type="button"
                            class={`focus-ring rounded px-1.5 py-0.5 font-mono text-[9px] ${span().columns === 2 ? "bg-foreground text-background" : "bg-background text-muted hover:text-foreground"}`}
                            aria-label={`${target.label}: two columns wide`}
                            onClick={() => props.onSpanChange?.(target.window, { ...span(), columns: 2 })}
                          >2×</button>
                          <button
                            type="button"
                            class={`focus-ring rounded px-1.5 py-0.5 font-mono text-[9px] ${span().rows === 2 ? "bg-signal/15 text-signal" : "bg-background text-muted hover:text-foreground"}`}
                            aria-pressed={span().rows === 2}
                            aria-label={`${target.label}: toggle double height`}
                            onClick={() => props.onSpanChange?.(target.window, { ...span(), rows: span().rows === 2 ? 1 : 2 })}
                          >Tall</button>
                        </div>
                      </div>
                    </Show>
                  </div>
                );
              }}
            </For>
          </div>
        </section>
      </Show>
    </div>
  );
}
