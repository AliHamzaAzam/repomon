import { For, Show, createMemo, createSignal } from "solid-js";

import { BINDINGS, formatChord, type KeymapSection } from "../keymap";

const SECTIONS: KeymapSection[] = ["Panels", "Layout", "Fleet", "Lane", "Agents", "Help"];

/// Two real shortcuts live outside BINDINGS and must still appear here, or the reference lies by
/// omission. Neither belongs in the shared table:
/// - Cmd+K / Ctrl+K opens the control center via ControlCenter.tsx's own listener. Putting it in
///   BINDINGS would make the global handler in App.tsx call preventDefault and swallow the key
///   before ControlCenter ever saw it.
/// - Shift+Escape leaves a focused terminal. It has no Cmd/Ctrl modifier, unlike every BINDINGS
///   entry, so it gets a literal chord label instead of formatChord, which always assumes a mod
///   key is present.
const STATIC_ROWS: Array<{ id: string; label: string; chord: string }> = [
  { id: "static.controlCenter", label: "Open the control center", chord: formatChord("mod+k") },
  { id: "static.leaveTerminal", label: "Leave the terminal", chord: "Shift+Esc" },
];

export default function KeyboardHelp() {
  const [query, setQuery] = createSignal("");

  const matches = createMemo(() => {
    const needle = query().trim().toLowerCase();
    if (!needle) return BINDINGS;
    return BINDINGS.filter((binding) =>
      binding.label.toLowerCase().includes(needle) || formatChord(binding.chord).toLowerCase().includes(needle));
  });

  const staticMatches = createMemo(() => {
    const needle = query().trim().toLowerCase();
    if (!needle) return STATIC_ROWS;
    return STATIC_ROWS.filter((row) => row.label.toLowerCase().includes(needle) || row.chord.toLowerCase().includes(needle));
  });

  return (
    <div class="space-y-4">
      <input
        class="settings-input mt-0"
        placeholder="Search shortcuts"
        value={query()}
        onInput={(event) => setQuery(event.currentTarget.value)}
      />
      <For each={SECTIONS}>
        {(section) => {
          const rows = createMemo(() => matches().filter((binding) => binding.section === section));
          // The Help section also carries the two rows above; every other section only shows
          // table-driven rows.
          const extraRows = createMemo(() => (section === "Help" ? staticMatches() : []));
          return (
            <Show when={rows().length > 0 || extraRows().length > 0}>
              <section class="space-y-1">
                <p class="section-label text-signal">{section}</p>
                <For each={rows()}>
                  {(binding) => (
                    <div class="flex items-center justify-between rounded border border-line px-3 py-1.5 text-xs">
                      <span>{binding.label}</span>
                      <span class="lane-badge font-mono">{formatChord(binding.chord)}</span>
                    </div>
                  )}
                </For>
                <For each={extraRows()}>
                  {(row) => (
                    <div class="flex items-center justify-between rounded border border-line px-3 py-1.5 text-xs">
                      <span>{row.label}</span>
                      <span class="lane-badge font-mono">{row.chord}</span>
                    </div>
                  )}
                </For>
              </section>
            </Show>
          );
        }}
      </For>
    </div>
  );
}
