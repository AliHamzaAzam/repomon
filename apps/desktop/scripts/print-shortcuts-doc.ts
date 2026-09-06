/// Prints keyboard tables from BINDINGS for docs/desktop.md; keymap.doc.test.ts checks the
/// published tables for drift.

import { BINDINGS, formatChord, type Binding } from "../src/keymap";

interface DocSection {
  /// Markdown heading text, without the leading "#"s - printed at `level`.
  heading: string;
  level: 3 | 4;
  filter: (binding: Binding) => boolean;
}

const GLOBAL: (binding: Binding) => boolean = (binding) => (binding.scope ?? "global") === "global";

/// One entry per table this script owns. Order here is the order the tables appear in the doc.
export const DOC_SECTIONS: DocSection[] = [
  { heading: "Panels", level: 3, filter: (b) => GLOBAL(b) && b.section === "Panels" },
  { heading: "File finder", level: 4, filter: (b) => b.scope === "finder" },
  { heading: "Layout", level: 3, filter: (b) => GLOBAL(b) && b.section === "Layout" },
  { heading: "Fleet", level: 3, filter: (b) => GLOBAL(b) && b.section === "Fleet" },
  { heading: "Lane", level: 3, filter: (b) => b.section === "Lane" },
  { heading: "Agents", level: 3, filter: (b) => b.section === "Agents" },
  { heading: "Terminals", level: 3, filter: (b) => b.scope === "terminal" },
  { heading: "Editor", level: 3, filter: (b) => b.scope === "editor" },
];

/// Render one table (no heading) for the given bindings, in BINDINGS' own order.
export function renderTable(bindings: Binding[]): string {
  const rows = bindings.map((binding) => `| \`${formatChordForDoc(binding)}\` | ${binding.label} |`);
  return ["| Chord | Action |", "|---|---|", ...rows].join("\n");
}

/// Docs show the platform-neutral chord token (e.g. "mod+shift+f"), not a rendered key cap - the
/// in-app guide is what shows ⌘ vs Ctrl. formatChord is only reused here to confirm every chord
/// is renderable; the literal chord string is what actually gets printed.
function formatChordForDoc(binding: Binding): string {
  formatChord(binding.chord);
  return binding.chord;
}

/// One markdown block per DOC_SECTIONS entry: `{heading: "### Panels", table: "| Chord | ... "}`.
export function renderDocSections(bindings: Binding[] = BINDINGS): Array<{ heading: string; table: string }> {
  return DOC_SECTIONS.map((section) => ({
    heading: `${"#".repeat(section.level)} ${section.heading}`,
    table: renderTable(bindings.filter(section.filter)),
  }));
}

if (import.meta.main) {
  for (const { heading, table } of renderDocSections()) {
    console.log(heading);
    console.log("");
    console.log(table);
    console.log("");
  }
}
