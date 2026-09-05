/// The single source of truth for keyboard shortcuts. The global handler in App.tsx, the
/// cheat sheet overlay, and the Settings keyboard reference all read this table, so a new
/// binding appears in help for free.
///
/// Chords are modifier-based on purpose: a focused terminal forwards every bare keystroke to the
/// agent, so an unmodified shortcut would steal input from a running session.
///
/// Not every entry here is dispatched from this file. Global chords are: the handler in App.tsx
/// matches them with matchChord and this file owns their behavior. Everything else (scope other
/// than "global") is a real binding that lives somewhere else - CodeMirror's own keymap inside
/// the editor, xterm's custom key handler inside a terminal, FileFinder's own list navigation, or
/// the fleet sidebar's bare-key navigation - described here only so the guide can never omit it.
/// Each of those owning modules exports the literal table or predicate it actually runs, and a
/// test in this file (or alongside the owning module) compares it against the entry below, so the
/// two cannot drift apart.

export type KeymapSection = "Panels" | "Layout" | "Fleet" | "Lane" | "Agents" | "Terminal" | "Editor" | "Help";

/// Where a binding is handled. "global" bindings go through matchChord/chordOf below and are
/// live everywhere. Everything else only fires while the named surface has focus, and can only
/// be reached through it: an editor binding needs the CodeMirror view focused, a terminal binding
/// needs a terminal pane focused, "finder" needs the file finder open, "sidebar" needs the fleet
/// list focused.
export type KeymapScope = "global" | "editor" | "terminal" | "finder" | "sidebar";

/// A guard the handler checks before dispatching. "lane" needs a selected lane; "agent" also
/// needs that lane to have a managed agent. Only meaningful for scope "global".
export type Guard = "lane" | "agent";

export interface Binding {
  id: string;
  /// Normalized chord, e.g. "mod+e", "mod+shift+m", "alt+up", "shift+escape", or a bare key like
  /// "/" for an unmodified local binding. "mod" is Cmd on macOS, Ctrl elsewhere; every other
  /// modifier token ("shift", "alt", "ctrl") means exactly that key on every platform.
  chord: string;
  label: string;
  section: KeymapSection;
  /// Defaults to "global" when omitted.
  scope?: KeymapScope;
  when?: Guard;
  /// Free text for a chord whose availability or behavior genuinely differs between macOS and
  /// Windows/Linux, beyond the ordinary mod-key substitution.
  platform?: string;
}

export const BINDINGS: Binding[] = [
  { id: "panel.control", chord: "mod+k", label: "Open the control center", section: "Panels" },
  { id: "panel.settings", chord: "mod+,", label: "Open settings", section: "Panels" },
  { id: "panel.multitasking", chord: "mod+9", label: "Toggle multitasking", section: "Panels" },
  { id: "panel.mail", chord: "mod+2", label: "Toggle the repomail panel", section: "Panels" },
  { id: "panel.git", chord: "mod+3", label: "Toggle the git explorer panel", section: "Panels" },
  { id: "panel.extensions", chord: "mod+4", label: "Toggle extensions", section: "Panels" },
  { id: "panel.repomind", chord: "mod+5", label: "Toggle repomind", section: "Panels" },
  { id: "panel.repomindFull", chord: "mod+shift+5", label: "Repomind full screen", section: "Panels" },
  { id: "panel.theme", chord: "mod+6", label: "Cycle theme (system, dark, light)", section: "Panels" },
  {
    id: "panel.editor",
    chord: "mod+7",
    label: "Toggle the compact in-app editor in the right rail",
    section: "Panels",
  },
  { id: "panel.supervision", chord: "mod+8", label: "Toggle the supervision panel", section: "Panels" },
  { id: "panel.usage", chord: "mod+1", label: "Toggle the usage view", section: "Panels" },
  { id: "finder.open", chord: "mod+p", label: "Find file in workspace", section: "Panels", when: "lane" },
  {
    id: "search.project",
    chord: "mod+shift+f",
    label: "Search project files in workspace (inside a focused terminal, this searches the terminal)",
    section: "Panels",
    when: "lane",
  },
  {
    id: "editor.markdownPreview",
    chord: "mod+shift+v",
    label: "Toggle preview (Markdown or SVG tab, whichever is active)",
    section: "Panels",
    when: "lane",
  },

  { id: "layout.focused", chord: "mod+shift+1", label: "Focused layout, one pane", section: "Layout" },
  { id: "layout.split", chord: "mod+shift+2", label: "Split layout, active pane plus its peer", section: "Layout" },
  {
    id: "layout.grid",
    chord: "mod+shift+0",
    label: "Grid layout, up to six panes",
    section: "Layout",
    // Not mod+shift+3: that chord is the macOS system screenshot shortcut and never reaches the app.
    platform: "Not mod+shift+3 on any platform: that chord is reserved for the macOS screenshot tool, so mod+shift+0 is used everywhere instead.",
  },

  { id: "fleet.filter", chord: "mod+/", label: "Filter the fleet", section: "Fleet" },
  { id: "fleet.urgent", chord: "mod+u", label: "Show only lanes needing attention", section: "Fleet" },
  { id: "fleet.refresh", chord: "mod+r", label: "Refresh", section: "Fleet" },
  { id: "fleet.newLane", chord: "mod+n", label: "New lane", section: "Fleet" },
  { id: "fleet.addRepo", chord: "mod+shift+n", label: "Add repository", section: "Fleet" },
  { id: "fleet.jumpUrgent", chord: "mod+g", label: "Jump to a lane needing attention", section: "Fleet" },
  {
    id: "fleet.hideRepo",
    chord: "mod+shift+h",
    label: "Hide the selected lane's project",
    section: "Fleet",
    when: "lane",
    // Not mod+h: Cmd+H hides the application on macOS and never reaches the app.
    platform: "Not mod+h on any platform: Cmd+H hides the whole app on macOS, so mod+shift+h is used everywhere instead.",
  },
  { id: "fleet.repoNotes", chord: "mod+shift+b", label: "Edit the selected lane's project notes", section: "Fleet", when: "lane" },

  { id: "lane.spawn", chord: "mod+e", label: "Spawn agent", section: "Lane", when: "lane" },
  { id: "lane.terminal", chord: "mod+t", label: "Open terminal", section: "Lane", when: "lane" },
  { id: "lane.pin", chord: "mod+shift+p", label: "Pin or unpin lane", section: "Lane", when: "lane" },
  { id: "lane.delete", chord: "mod+d", label: "Delete lane (asks first)", section: "Lane", when: "lane" },
  { id: "lane.merge", chord: "mod+shift+m", label: "Merge lane (asks first)", section: "Lane", when: "lane" },
  {
    id: "lane.stop",
    chord: "mod+.",
    label: "Stop the agent in the visible pane (asks first)",
    section: "Lane",
    when: "agent",
  },

  { id: "agents.prev", chord: "mod+[", label: "Previous agent tab", section: "Agents", when: "lane" },
  { id: "agents.next", chord: "mod+]", label: "Next agent tab", section: "Agents", when: "lane" },

  { id: "help.open", chord: "mod+?", label: "Keyboard shortcuts", section: "Help" },

  // --- Terminal-local bindings. These live in TerminalPane.tsx's attachCustomKeyEventHandler,
  // not in the global dispatcher, so they only fire while a terminal pane has focus. See
  // src/ipc/term.ts's isTerminalReleaseChord and isTerminalFindChord, which term.test.ts checks
  // against the two chords below.
  {
    id: "terminal.find",
    chord: "mod+shift+f",
    label: "Find in the terminal",
    section: "Terminal",
    scope: "terminal",
    platform: "Responds to Cmd+Shift+F or Ctrl+Shift+F on every platform - unlike other chords, both are accepted regardless of which one is normally \"mod\".",
  },
  {
    id: "terminal.releaseFocus",
    chord: "shift+escape",
    label: "Leave the terminal, back to the fleet list",
    section: "Terminal",
    scope: "terminal",
    platform: "Plain Escape is deliberately left alone on every platform: Claude Code and other agent CLIs use it to interrupt their own work, so only Shift+Escape releases focus.",
  },

  // --- Editor-local bindings. These live in CodeEditor.tsx's own CodeMirror keymap (see the
  // exported EDITOR_LOCAL_KEYMAP table there), not the global dispatcher, so they only fire while
  // the CodeMirror view has focus.
  { id: "editor.save", chord: "mod+s", label: "Save file", section: "Editor", scope: "editor" },
  { id: "editor.toggleComment", chord: "mod+/", label: "Toggle line comment", section: "Editor", scope: "editor" },
  { id: "editor.selectNextOccurrence", chord: "mod+d", label: "Select next occurrence", section: "Editor", scope: "editor" },
  { id: "editor.gotoLine", chord: "mod+g", label: "Go to line", section: "Editor", scope: "editor" },
  { id: "editor.moveLineUp", chord: "alt+up", label: "Move line up", section: "Editor", scope: "editor" },
  { id: "editor.moveLineDown", chord: "alt+down", label: "Move line down", section: "Editor", scope: "editor" },
  { id: "editor.copyLineDown", chord: "shift+alt+down", label: "Copy line down", section: "Editor", scope: "editor" },

  // --- File finder-local bindings. These live in FileFinder.tsx's own key handler, so they only
  // fire while the finder (mod+p) is open.
  { id: "finder.next", chord: "ctrl+n", label: "Next result (also Down arrow)", section: "Panels", scope: "finder" },
  { id: "finder.prev", chord: "ctrl+p", label: "Previous result (also Up arrow)", section: "Panels", scope: "finder" },
  { id: "finder.close", chord: "escape", label: "Close the finder", section: "Panels", scope: "finder" },

  // --- Sidebar-local bindings. These live in App.tsx's navigateFleet (via matchSidebarKey
  // below), so they only fire while the fleet list has focus and no text input is focused.
  { id: "sidebar.filter", chord: "/", label: "Jump to the filter box", section: "Fleet", scope: "sidebar" },
  { id: "sidebar.next", chord: "j", label: "Next lane (also Down arrow)", section: "Fleet", scope: "sidebar" },
  { id: "sidebar.prev", chord: "k", label: "Previous lane (also Up arrow)", section: "Fleet", scope: "sidebar" },
  { id: "sidebar.jumpUrgent", chord: "n", label: "Jump to the next lane needing attention", section: "Fleet", scope: "sidebar" },
];

/// The one cross-cutting platform caveat that applies to many chords at once rather than a
/// single binding: see docs/desktop.md's "Known gaps". Surfaced by the Settings keyboard tab.
export const CTRL_TERMINAL_CAVEAT =
  "On Windows and Linux, mod is Ctrl - the terminal's own control modifier. A bound Ctrl chord " +
  "pressed while a terminal is focused currently fires the GUI action and still reaches the " +
  "agent. macOS is unaffected, since Cmd is not a terminal control key.";

function bindingScope(binding: Binding): KeymapScope {
  return binding.scope ?? "global";
}

/// Only globally-dispatched bindings participate in chord matching. Local bindings reuse the
/// same "mod+x" chord vocabulary to describe what they do (an editor's Mod-/ next to the fleet's
/// own Mod-/), so indexing every entry here would let a local description silently shadow a real
/// global binding.
const BY_CHORD = new Map(
  BINDINGS.filter((binding) => bindingScope(binding) === "global").map((binding) => [binding.chord, binding]),
);

export function isMac(platform?: string): boolean {
  if (platform) return platform === "mac";
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
}

/// Normalize an event to a chord string, or null when the platform modifier is not held. Mod is
/// Cmd on macOS and Ctrl elsewhere; the two are never interchangeable. A focused terminal
/// forwards Ctrl chords straight to the agent (EOF, text navigation, and so on), so on macOS a
/// held Ctrl must never also satisfy "mod", or the same keystroke would fire a GUI action in
/// addition to reaching the agent. Returning null for unmodified keys is what keeps ordinary
/// typing untouched.
export function chordOf(event: KeyboardEvent, platform?: string): string | null {
  if (isMac(platform)) {
    if (!event.metaKey || event.ctrlKey) return null;
  } else if (!event.ctrlKey) {
    return null;
  }
  // Digits come from `code`, not `key`: shift turns "1" into "!" (and non-US layouts move the
  // number row entirely), so keying off `event.key` would make every shifted digit chord
  // unreachable while still rendering as available in the help reference.
  const digit = event.code.startsWith("Digit") ? event.code.slice(5) : null;
  // "?" already implies shift on most layouts, so do not double-encode it.
  const key = digit ?? (event.key === "?" ? "?" : event.key.toLowerCase());
  const shift = event.shiftKey && key !== "?" ? "shift+" : "";
  return `mod+${shift}${key}`;
}

export function matchChord(event: KeyboardEvent, platform?: string): Binding | null {
  const chord = chordOf(event, platform);
  return chord ? BY_CHORD.get(chord) ?? null : null;
}

/// Bare-key fleet navigation: j/k or the arrow keys move the selection, "/" jumps to the filter
/// box, and "n" jumps to the next lane needing attention. Unlike every chord above these carry no
/// modifier at all, so App.tsx's navigateFleet only calls this once it has already confirmed the
/// event did not land in a text input. Exported so keymap.test.ts can check it against the
/// "sidebar" scope entries in BINDINGS directly, instead of trusting App.tsx's copy of the same
/// logic to stay in sync by hand.
export function matchSidebarKey(event: KeyboardEvent): string | null {
  if (event.key === "/") return "sidebar.filter";
  if (event.key === "j" || event.key === "ArrowDown") return "sidebar.next";
  if (event.key === "k" || event.key === "ArrowUp") return "sidebar.prev";
  if (event.key === "n") return "sidebar.jumpUrgent";
  return null;
}

/// Convert a CodeMirror-style key string ("Mod-s", "Alt-ArrowUp", "Shift-Alt-ArrowDown") into
/// this file's chord vocabulary ("mod+s", "alt+up", "shift+alt+down"). Exported so a test can
/// check CodeEditor.tsx's EDITOR_LOCAL_KEYMAP against the "editor" scope entries in BINDINGS
/// without hand-transcribing either table.
export function fromCodeMirrorKey(key: string): string {
  return key
    .split("-")
    .map((part) => (part.startsWith("Arrow") ? part.slice("Arrow".length).toLowerCase() : part.toLowerCase()))
    .join("+");
}

/// Two entries collide when they share both a chord and a scope: the same keystroke would try to
/// do two different things in the same context. Sharing a chord across *different* scopes is
/// deliberate (e.g. mod+shift+f is "search project files" globally and "find in terminal" only
/// while a terminal pane has focus - focus decides which one fires), so this only ever compares
/// within one scope.
export interface KeymapConflict {
  scope: KeymapScope;
  chord: string;
  bindings: Binding[];
}

export function findConflicts(bindings: Binding[] = BINDINGS): KeymapConflict[] {
  const groups = new Map<string, Binding[]>();
  for (const binding of bindings) {
    const key = `${bindingScope(binding)} ${binding.chord}`;
    const list = groups.get(key);
    if (list) list.push(binding);
    else groups.set(key, [binding]);
  }
  const conflicts: KeymapConflict[] = [];
  for (const [key, group] of groups) {
    if (group.length < 2) continue;
    const [scope, chord] = key.split(" ") as [KeymapScope, string];
    conflicts.push({ scope, chord, bindings: group });
  }
  return conflicts;
}

/// True while `active` (or an ancestor) is inside the editor's CodeMirror view or a terminal
/// pane's xterm root. Used to highlight the relevant scope when the shortcuts guide opens, so it
/// answers "what does this key do right now" rather than making the reader guess from focus.
export function detectActiveScope(active: Element | null): KeymapScope {
  if (!active) return "global";
  if (active.closest(".cm-editor")) return "editor";
  if (active.closest(".xterm")) return "terminal";
  return "global";
}

const MAC_MODIFIER_SYMBOLS: Record<string, string> = { mod: "⌘", ctrl: "⌃", alt: "⌥", shift: "⇧" };
const OTHER_MODIFIER_LABELS: Record<string, string> = { mod: "Ctrl", ctrl: "Ctrl", alt: "Alt", shift: "Shift" };

/// Non-letter/digit keys that need a friendlier cap than their raw token, on every platform.
const KEY_SYMBOLS: Record<string, string> = {
  "[": "[",
  "]": "]",
  "/": "/",
  ".": ".",
  ",": ",",
  "?": "?",
  up: "↑",
  down: "↓",
  left: "←",
  right: "→",
  escape: "Esc",
  enter: "Enter",
  tab: "Tab",
};

function keyLabel(key: string): string {
  const symbol = KEY_SYMBOLS[key];
  if (symbol) return symbol;
  return key.length === 1 ? key.toUpperCase() : key[0].toUpperCase() + key.slice(1);
}

/// Split a chord into the individual key caps a cheat sheet should render, in order: "mod+shift+m"
/// becomes ["⌘", "⇧", "M"] on macOS or ["Ctrl", "Shift", "M"] elsewhere. A bare local chord like
/// "/" or "j" carries no modifier token at all, so it renders as a single cap with no platform
/// substitution.
export function keyCapParts(chord: string, platform?: string): string[] {
  const mac = isMac(platform);
  const tokens = chord.split("+");
  const key = tokens[tokens.length - 1];
  const modifiers = tokens.slice(0, -1);
  const parts = modifiers.map((token) => (mac ? MAC_MODIFIER_SYMBOLS[token] ?? token : OTHER_MODIFIER_LABELS[token] ?? token));
  parts.push(keyLabel(key));
  return parts;
}

/// Render a chord for display: "mod+shift+m" becomes "⌘⇧M" on macOS, "Ctrl+Shift+M" elsewhere.
export function formatChord(chord: string, platform?: string): string {
  const parts = keyCapParts(chord, platform);
  return parts.join(isMac(platform) ? "" : "+");
}
