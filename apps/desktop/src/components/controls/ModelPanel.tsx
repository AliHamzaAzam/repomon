import { For, Show, createEffect, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { IconCheck, IconChevronRight } from "../icons";
import type { CatalogModel } from "../../ipc/rpc";

// Reference: a small panel anchored to the composer's model chip. Rows are model names, the
// active one carries a check, the rest carry a number shortcut, then (past a handful) a "More
// models" disclosure. No redesign beyond that: no toggle section here, since the catalog contract
// carries nothing to back one - inventing a control with no data behind it is the same mistake as
// inventing a command.
const VISIBLE_BEFORE_MORE = 5;

export default function ModelPanel(props: {
  models: CatalogModel[];
  anchor: HTMLElement;
  onSelect: (id: string) => void;
  onClose: () => void;
}) {
  const [expanded, setExpanded] = createSignal(props.models.length <= VISIBLE_BEFORE_MORE);
  const [style, setStyle] = createSignal<JSX.CSSProperties>({});
  let panelRef!: HTMLDivElement;

  const visible = () => (expanded() ? props.models : props.models.slice(0, VISIBLE_BEFORE_MORE));
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
    if (/^[1-9]$/.test(event.key)) {
      const n = Number(event.key);
      const match = visible().find((model) => numberOf(model) === n);
      if (match) { event.preventDefault(); props.onSelect(match.id); }
    }
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
          <For each={visible()}>
            {(model) => (
              <button
                type="button"
                role="menuitemradio"
                aria-checked={model.current}
                class={`flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium transition-colors ${
                  model.current ? "text-signal" : "text-foreground hover:bg-raised"
                }`}
                onClick={() => props.onSelect(model.id)}
              >
                <span class="truncate">{model.label}</span>
                <Show when={model.current} fallback={<Show when={numberOf(model)}>{(n) => <span class="shrink-0 font-mono text-[10px] text-muted">{n()}</span>}</Show>}>
                  <span class="shrink-0 text-signal"><IconCheck size={13} strokeWidth={2.5} /></span>
                </Show>
              </button>
            )}
          </For>
          <Show when={!expanded() && props.models.length > VISIBLE_BEFORE_MORE}>
            <div class="my-1 border-t border-line" />
            <button
              type="button"
              class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-muted hover:bg-raised hover:text-foreground"
              onClick={() => setExpanded(true)}
            >
              <span>More models</span>
              <IconChevronRight size={12} />
            </button>
          </Show>
        </Show>
      </div>
    </Portal>
  );
}
