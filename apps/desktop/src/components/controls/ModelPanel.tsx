import { For, Show, createEffect, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { IconCheck, IconChevronRight } from "../icons";
import type { CatalogModel } from "../../bindings";

// Reference: a small panel anchored to the composer's model chip. Rows are model names, the
// active one carries a check, the rest carry a number shortcut, then (past a handful) a "More
// models" disclosure. No toggle section here: the catalog contract carries nothing to back one -
// inventing a control with no data behind it is the same mistake as inventing a command.
const VISIBLE_BEFORE_MORE = 5;

export default function ModelPanel(props: {
  models: CatalogModel[];
  // Null means this kind has models to show but nothing on this machine confirmed a one-shot
  // switch form for them - the panel must say so, not offer selection it cannot actually drive.
  modelCommand: string | null;
  kind: string;
  anchor: HTMLElement;
  onSelect: (id: string) => void;
  onClose: () => void;
}) {
  const selectable = () => !!props.modelCommand;
  const [expanded, setExpanded] = createSignal(props.models.length <= VISIBLE_BEFORE_MORE);
  const [style, setStyle] = createSignal<JSX.CSSProperties>({});
  const [highlighted, setHighlighted] = createSignal(0);
  let panelRef!: HTMLDivElement;

  const visible = () => (expanded() ? props.models : props.models.slice(0, VISIBLE_BEFORE_MORE));
  const hasMore = () => !expanded() && props.models.length > VISIBLE_BEFORE_MORE;
  const rowCount = () => visible().length + (hasMore() ? 1 : 0);
  createEffect(() => { rowCount(); setHighlighted(0); });

  // Numbers go to non-current rows only, in list order, stable across expansion.
  const numberOf = (model: CatalogModel): number | null => {
    if (model.current) return null;
    let n = 0;
    for (const entry of props.models) {
      if (entry.current) continue;
      n += 1;
      if (entry.id === model.id) return n <= 9 ? n : null;
    }
    return null;
  };

  function activateHighlighted() {
    const models = visible();
    const i = highlighted();
    if (i < models.length) { props.onSelect(models[i].id); return; }
    if (hasMore()) setExpanded(true);
  }

  function position() {
    const rect = props.anchor.getBoundingClientRect();
    setStyle({
      position: "fixed",
      bottom: `${window.innerHeight - rect.top + 6}px`,
      right: `${window.innerWidth - rect.right}px`,
      "min-width": `${Math.max(200, rect.width)}px`,
    });
  }

  function onPointerDown(event: PointerEvent) {
    const target = event.target as Node;
    if (panelRef && !panelRef.contains(target) && !props.anchor.contains(target)) props.onClose();
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key === "Escape") { event.preventDefault(); props.onClose(); return; }
    if (!selectable()) return;
    if (/^[1-9]$/.test(event.key)) {
      const n = Number(event.key);
      const match = visible().find((model) => numberOf(model) === n);
      if (match) { event.preventDefault(); props.onSelect(match.id); }
      return;
    }
    const count = rowCount();
    if (event.key === "ArrowDown") { event.preventDefault(); setHighlighted((i) => (count ? (i + 1) % count : 0)); return; }
    if (event.key === "ArrowUp") { event.preventDefault(); setHighlighted((i) => (count ? (i - 1 + count) % count : 0)); return; }
    if (event.key === "Enter") { event.preventDefault(); activateHighlighted(); }
  }

  onMount(() => {
    position();
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", position);
  });
  onCleanup(() => {
    window.removeEventListener("pointerdown", onPointerDown);
    window.removeEventListener("keydown", onKeyDown);
    window.removeEventListener("resize", position);
  });
  createEffect(() => { props.models; position(); });

  return (
    <Portal>
      <div
        ref={panelRef}
        role="menu"
        aria-label="Choose model"
        style={style()}
        class="z-[100] max-h-72 overflow-y-auto rounded-xl border border-line bg-surface p-1 shadow-[0_12px_36px_var(--shadow)] outline-none backdrop-blur-md"
      >
        <Show when={props.models.length} fallback={<p class="px-2.5 py-2 text-xs text-muted">No models known for this agent.</p>}>
          <Show
            when={selectable()}
            fallback={
              <>
                <p class="px-2.5 py-2 text-xs text-muted">
                  Repomon can't switch {props.kind}'s model with a single command yet - use {props.kind}'s own model command instead.
                </p>
                <div class="my-1 border-t border-line" />
                <For each={props.models}>
                  {(model) => (
                    <div class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-xs font-medium text-muted">
                      <span class="truncate">{model.label}</span>
                      <Show when={model.current}><span class="shrink-0 text-signal"><IconCheck size={13} strokeWidth={2.5} /></span></Show>
                    </div>
                  )}
                </For>
              </>
            }
          >
            <For each={visible()}>
              {(model, index) => (
                <button
                  type="button"
                  role="menuitemradio"
                  aria-checked={model.current}
                  class={`flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium transition-colors ${
                    model.current ? "text-signal" : "text-foreground"
                  } ${index() === highlighted() ? "bg-raised" : "hover:bg-raised"}`}
                  onPointerMove={() => setHighlighted(index())}
                  onClick={() => props.onSelect(model.id)}
                >
                  <span class="truncate">{model.label}</span>
                  <Show when={model.current} fallback={<Show when={numberOf(model)}>{(n) => <span class="shrink-0 font-mono text-[10px] text-muted">{n()}</span>}</Show>}>
                    <span class="shrink-0 text-signal"><IconCheck size={13} strokeWidth={2.5} /></span>
                  </Show>
                </button>
              )}
            </For>
            <Show when={hasMore()}>
              <div class="my-1 border-t border-line" />
              <button
                type="button"
                class={`flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-muted ${
                  visible().length === highlighted() ? "bg-raised text-foreground" : "hover:bg-raised hover:text-foreground"
                }`}
                onPointerMove={() => setHighlighted(visible().length)}
                onClick={() => setExpanded(true)}
              >
                <span>More models</span>
                <IconChevronRight size={12} />
              </button>
            </Show>
          </Show>
        </Show>
      </div>
    </Portal>
  );
}
