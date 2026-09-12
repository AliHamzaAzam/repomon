import { For, Show, createEffect, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { bareAlias } from "../agentCommands";
import type { CatalogCommand } from "../../bindings";

// Reference: typing "/m" opens a filtered list above the composer. Each row is the command name,
// the typed prefix visually distinguished from the rest, plugin commands shown namespaced with
// their bare alias in parentheses. The highlighted row shows a short action label (its
// description) beside the list.
function Highlighted(props: { text: string; query: string }) {
  const prefixLen = () => (props.query && props.text.toLowerCase().startsWith(props.query.toLowerCase()) ? props.query.length : 0);
  return (
    <span class="truncate">
      <span class="text-foreground">{props.text.slice(0, prefixLen())}</span>
      <span class="text-muted">{props.text.slice(prefixLen())}</span>
    </span>
  );
}

function sourceLabel(source: CatalogCommand["source"]): string | null {
  return source === "plugin" ? "plugin" : source === "user" ? "user" : null;
}

export default function SlashPalette(props: {
  commands: CatalogCommand[];
  query: string;
  highlightedIndex: number;
  anchor: HTMLElement;
  onHighlight: (index: number) => void;
  onRun: (command: CatalogCommand) => void;
}) {
  let listRef!: HTMLDivElement;
  const style = (): JSX.CSSProperties => {
    const rect = props.anchor.getBoundingClientRect();
    return {
      position: "fixed",
      bottom: `${window.innerHeight - rect.top + 6}px`,
      left: `${rect.left}px`,
      width: `${rect.width}px`,
    };
  };

  createEffect(() => {
    props.highlightedIndex;
    listRef?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: "nearest" });
  });

  const highlighted = () => props.commands[props.highlightedIndex];

  return (
    <Portal>
      <div
        ref={listRef}
        role="listbox"
        aria-label="Slash commands"
        style={style()}
        class="z-[100] max-h-72 overflow-y-auto rounded-xl border border-line bg-surface p-1 shadow-[0_12px_36px_var(--shadow)] outline-none backdrop-blur-md"
      >
        <Show when={props.commands.length} fallback={<p class="px-2.5 py-2 text-xs text-muted">No commands known for this agent.</p>}>
          <For each={props.commands}>
            {(command, index) => {
              const alias = () => bareAlias(command.name);
              const namespaced = () => command.name !== alias();
              const isHighlighted = () => index() === props.highlightedIndex;
              return (
                <div
                  role="option"
                  aria-selected={isHighlighted()}
                  class={`flex cursor-pointer select-none items-center gap-2 rounded-lg px-2.5 py-1.5 font-mono text-xs transition-colors ${
                    isHighlighted() ? "bg-raised text-foreground" : "text-muted hover:bg-raised/60 hover:text-foreground"
                  }`}
                  onPointerMove={() => props.onHighlight(index())}
                  onClick={() => props.onRun(command)}
                >
                  <span class="min-w-0 flex-1 truncate">
                    /<Highlighted text={command.name} query={props.query} />
                    <Show when={namespaced()}> <span class="text-muted">({alias()})</span></Show>
                  </span>
                  <Show when={sourceLabel(command.source)}>{(label) => <span class="shrink-0 text-[10px] uppercase tracking-wider text-muted">{label()}</span>}</Show>
                </div>
              );
            }}
          </For>
          <Show when={highlighted()?.description}>
            <div class="mt-1 border-t border-line px-2.5 pt-1.5 text-xs text-muted">{highlighted()!.description}</div>
          </Show>
        </Show>
      </div>
    </Portal>
  );
}
