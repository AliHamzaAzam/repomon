/// The single source of truth for keyboard shortcuts. The global handler in App.tsx and the
/// Settings keyboard reference both read this table, so a new binding appears in help for free.
///
/// Chords are modifier-based on purpose: a focused terminal forwards every bare keystroke to the
/// agent, so an unmodified shortcut would steal input from a running session.

export type KeymapSection = "Panels" | "Layout" | "Fleet" | "Lane" | "Agents" | "Help";

/// A guard the handler checks before dispatching. "lane" needs a selected lane; "agent" also
/// needs that lane to have a managed agent.
export type Guard = "lane" | "agent";

export interface Binding {
  id: string;
  /// Normalized chord, e.g. "mod+e", "mod+shift+m". "mod" is Cmd on macOS, Ctrl elsewhere.
  chord: string;
  label: string;
  section: KeymapSection;
  when?: Guard;
}

export const BINDINGS: Binding[] = [
  { id: "panel.extensions", chord: "mod+4", label: "Toggle extensions", section: "Panels" },
  { id: "panel.repomind", chord: "mod+5", label: "Toggle repomind", section: "Panels" },
  { id: "panel.theme", chord: "mod+6", label: "Cycle theme", section: "Panels" },

  { id: "layout.focused", chord: "mod+shift+1", label: "Focused layout", section: "Layout" },
  { id: "layout.split", chord: "mod+shift+2", label: "Split layout", section: "Layout" },
  { id: "layout.grid", chord: "mod+shift+3", label: "Grid layout", section: "Layout" },

  { id: "fleet.filter", chord: "mod+/", label: "Filter the fleet", section: "Fleet" },
  { id: "fleet.urgent", chord: "mod+u", label: "Show only lanes needing attention", section: "Fleet" },
  { id: "fleet.refresh", chord: "mod+r", label: "Refresh", section: "Fleet" },
  { id: "fleet.newLane", chord: "mod+n", label: "New lane", section: "Fleet" },
  { id: "fleet.addRepo", chord: "mod+shift+n", label: "Add repository", section: "Fleet" },
  { id: "fleet.jumpUrgent", chord: "mod+g", label: "Jump to a lane needing attention", section: "Fleet" },

  { id: "lane.spawn", chord: "mod+e", label: "Spawn agent", section: "Lane", when: "lane" },
  { id: "lane.terminal", chord: "mod+t", label: "Open terminal", section: "Lane", when: "lane" },
  { id: "lane.pin", chord: "mod+p", label: "Pin or unpin lane", section: "Lane", when: "lane" },
  { id: "lane.delete", chord: "mod+d", label: "Delete lane", section: "Lane", when: "lane" },
  { id: "lane.merge", chord: "mod+shift+m", label: "Merge lane", section: "Lane", when: "lane" },
  { id: "lane.stop", chord: "mod+.", label: "Stop agent", section: "Lane", when: "agent" },

  { id: "agents.prev", chord: "mod+[", label: "Previous agent tab", section: "Agents", when: "lane" },
  { id: "agents.next", chord: "mod+]", label: "Next agent tab", section: "Agents", when: "lane" },

  { id: "help.open", chord: "mod+?", label: "Keyboard shortcuts", section: "Help" },
];

const BY_CHORD = new Map(BINDINGS.map((binding) => [binding.chord, binding]));

function isMac(platform?: string): boolean {
  if (platform) return platform === "mac";
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
}

/// Normalize an event to a chord string, or null when no platform modifier is held. Returning
/// null for unmodified keys is what keeps ordinary typing (and a focused terminal) untouched.
export function chordOf(event: KeyboardEvent): string | null {
  if (!event.metaKey && !event.ctrlKey) return null;
  // "?" already implies shift on most layouts, so do not double-encode it.
  const key = event.key === "?" ? "?" : event.key.toLowerCase();
  const shift = event.shiftKey && key !== "?" ? "shift+" : "";
  return `mod+${shift}${key}`;
}

export function matchChord(event: KeyboardEvent): Binding | null {
  const chord = chordOf(event);
  return chord ? BY_CHORD.get(chord) ?? null : null;
}

const SYMBOLS: Record<string, string> = { "[": "[", "]": "]", "/": "/", ".": ".", "?": "?" };

/// Render a chord for display: "mod+shift+m" becomes "⌘⇧M" on macOS, "Ctrl+Shift+M" elsewhere.
export function formatChord(chord: string, platform?: string): string {
  const parts = chord.split("+");
  const key = parts[parts.length - 1];
  const shift = parts.includes("shift");
  const label = SYMBOLS[key] ?? key.toUpperCase();
  return isMac(platform)
    ? `⌘${shift ? "⇧" : ""}${label}`
    : `Ctrl+${shift ? "Shift+" : ""}${label}`;
}
