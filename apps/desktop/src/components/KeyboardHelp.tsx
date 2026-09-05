import { For, Show, createMemo, createSignal } from "solid-js";

import {
  BINDINGS,
  CTRL_TERMINAL_CAVEAT,
  findConflicts,
  formatChord,
  isMac,
  numberedPanelBindings,
  type Binding,
  type KeymapScope,
  type KeymapSection,
} from "../keymap";
import { shortcutsHtmlDataUrl } from "../shortcutsPrint";
import { IconAlertTriangle, IconPrinter } from "./icons";

const SECTIONS: KeymapSection[] = ["Panels", "Layout", "Fleet", "Lane", "Agents", "Terminal", "Editor", "Help"];

const SCOPE_LABELS: Record<KeymapScope, string> = {
  global: "Global",
  editor: "Editor",
  terminal: "Terminal",
  finder: "Finder",
  sidebar: "Sidebar",
};

export interface KeyboardHelpProps {
  /// "full" (default) is the Settings > Keyboard tab: search, every section, the conflict check,
  /// the Windows/Linux Ctrl caveat, and Print. "compact" is the cheat sheet overlay: just search
  /// and the grouped list, kept light since it is meant to be glanced at and dismissed.
  variant?: "full" | "compact";
  /// The scope matching whatever has focus right now (editor, terminal, or none). Rows in this
  /// scope are highlighted so the reader can answer "what does this key do right now" instead of
  /// having to guess from context.
  activeScope?: KeymapScope;
  autofocus?: boolean;
}

function openPrintout() {
  void import("@tauri-apps/plugin-opener")
    .then(({ openUrl }) => openUrl(shortcutsHtmlDataUrl(BINDINGS, isMac() ? "mac" : "other")))
    .catch((error: unknown) => console.warn("Failed to open the shortcuts printout:", error));
}

export default function KeyboardHelp(props: KeyboardHelpProps) {
  const [query, setQuery] = createSignal("");
  const full = () => (props.variant ?? "full") === "full";

  const matches = createMemo(() => {
    const needle = query().trim().toLowerCase();
    if (!needle) return BINDINGS;
    return BINDINGS.filter(
      (binding) =>
        binding.label.toLowerCase().includes(needle)
        || formatChord(binding.chord).toLowerCase().includes(needle)
        || SCOPE_LABELS[binding.scope ?? "global"].toLowerCase().includes(needle),
    );
  });

  const conflicts = createMemo(() => findConflicts());

  return (
    <div class="space-y-4">
      <input
        class="settings-input mt-0"
        placeholder="Search shortcuts"
        value={query()}
        autofocus={props.autofocus}
        onInput={(event) => setQuery(event.currentTarget.value)}
      />

      <Show when={full() && !isMac()}>
        <div class="flex items-start gap-2 rounded border border-attention/30 bg-attention/10 px-3 py-2 text-xs text-attention">
          <IconAlertTriangle size={14} class="mt-0.5 shrink-0" />
          <span>{CTRL_TERMINAL_CAVEAT}</span>
        </div>
      </Show>

      <Show when={full() && conflicts().length > 0}>
        <div class="space-y-1 rounded border border-fault/30 bg-fault/10 px-3 py-2 text-xs text-fault">
          <p class="flex items-center gap-2 font-medium">
            <IconAlertTriangle size={14} class="shrink-0" />
            {conflicts().length} shortcut {conflicts().length === 1 ? "conflict" : "conflicts"} detected
          </p>
          <For each={conflicts()}>
            {(conflict) => (
              <p class="pl-5">
                {formatChord(conflict.chord)} ({SCOPE_LABELS[conflict.scope]}):{" "}
                {conflict.bindings.map((binding) => binding.label).join(" vs. ")}
              </p>
            )}
          </For>
        </div>
      </Show>

      <For each={SECTIONS}>
        {(section) => {
          // Numbered chords come first, in numeric order: the reader is looking up "what is
          // mod+4", and a list sorted by anything else makes them scan for it.
          const rows = createMemo(() => {
            const inSection = matches().filter((binding) => binding.section === section);
            const numbered = numberedPanelBindings(inSection);
            return [...numbered, ...inSection.filter((binding) => !numbered.includes(binding))];
          });
          return (
            <Show when={rows().length > 0}>
              <section class="space-y-1">
                <p class="section-label text-signal">{section}</p>
                <For each={rows()}>{(binding) => <ShortcutRow binding={binding} activeScope={props.activeScope} />}</For>
              </section>
            </Show>
          );
        }}
      </For>

      <Show when={full()}>
        <button
          type="button"
          class="focus-ring flex w-full items-center justify-center gap-1.5 rounded border border-line px-3 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised"
          onClick={openPrintout}
        >
          <IconPrinter size={13} />
          Print cheat sheet
        </button>
      </Show>
    </div>
  );
}

function ShortcutRow(props: { binding: Binding; activeScope?: KeymapScope }) {
  const scope = () => props.binding.scope ?? "global";
  const active = () => props.activeScope && props.activeScope === scope();
  return (
    <div
      class={`flex items-center justify-between gap-3 rounded border px-3 py-1.5 text-xs transition-colors ${
        active() ? "border-signal/50 bg-signal/10" : "border-line"
      }`}
      title={props.binding.platform}
    >
      <span class="flex min-w-0 flex-1 items-center gap-1.5">
        <span class="truncate">{props.binding.label}</span>
        <Show when={scope() !== "global"}>
          <span class="shrink-0 rounded bg-raised px-1 py-0.5 text-[9px] uppercase tracking-wider text-muted">
            {SCOPE_LABELS[scope()]}
          </span>
        </Show>
      </span>
      <span class="lane-badge shrink-0 font-mono">{formatChord(props.binding.chord)}</span>
    </div>
  );
}
