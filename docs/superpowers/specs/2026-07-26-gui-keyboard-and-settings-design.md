# GUI keyboard controls, help reference, and settings redesign

Design spec, 2026-07-26. Repomon desktop (`apps/desktop`).

## Context

The TUI can be driven entirely from the keyboard: `keybinds.rs` maps about 35 actions onto
single keys, arrow-first with `hjkl` aliases. The desktop GUI has roughly six shortcuts
(`Cmd+,` settings, `Cmd+K` control, bare `6` extensions, and `/`, `j`/`k`, arrows, `n` only
while the fleet list holds focus). Everything else needs the mouse.

Settings is equally thin: fourteen boolean options render as raw checkboxes, and fields that
should be pickers (default agent, orchestrator agent and model, renderer, accent color) are
free-text inputs with a datalist. There is no help anywhere in the GUI, so the keys that do
exist are undiscoverable.

Goal: make the GUI fully operable without a mouse, make the bindings discoverable from inside
the app, and give Settings real controls.

## Constraint that shapes the design

A focused terminal pane forwards every keystroke to the agent. Bare single-key shortcuts of the
kind the TUI uses cannot be global in the GUI without stealing input from a running Claude
session. Shortcuts are therefore **modifier-based**: `Cmd` on macOS, `Ctrl` elsewhere, reusing
the TUI's letters so existing muscle memory transfers. This works even while a terminal is
focused and needs no mode concept.

Chords avoid combinations macOS reserves: `Cmd+M` (minimize), `Cmd+H` (hide), `Cmd+Q` (quit),
`Cmd+W` (close).

## Non-goals

- **Rebindable shortcuts.** Bindings are fixed in this pass. The keymap is defined as data so a
  later rebinding feature is a drop-in, but no config schema, conflict UI, or persistence now.
- Changing which settings exist. This redesigns the controls, not the option set.
- Touching the TUI. `keybinds.rs` is the reference for letters, not something to modify.

## 1. Keymap as data

`apps/desktop/src/keymap.ts` is the single source of truth. The global handler and the help
reference both read it, so they cannot drift.

```ts
export type KeymapSection = "Views" | "Layout" | "Fleet" | "Lane" | "Agents" | "Help";

export interface Binding {
  /// Stable identifier the handler dispatches on.
  id: string;
  /// Chord in normalized form, e.g. "mod+e", "mod+shift+m", "mod+[".
  /// "mod" renders as Cmd on macOS and Ctrl elsewhere.
  chord: string;
  /// Human label shown in the help reference.
  label: string;
  section: KeymapSection;
  /// Optional guard. When present the action is inert (and shown dimmed in help)
  /// unless the condition holds, e.g. "lane" requires a selected lane.
  when?: "lane" | "agent";
}
```

Two pure helpers, both unit-tested:

- `matchChord(event: KeyboardEvent): Binding | null` normalizes a `KeyboardEvent` to a chord
  string and looks it up. Returns `null` for anything unbound so normal typing is untouched.
- `formatChord(chord: string): string` renders a chord for display (`mod+shift+m` becomes
  `⌘⇧M` on macOS, `Ctrl+Shift+M` elsewhere).

## 2. Bindings

`Cmd+K` (control) and `Cmd+,` (settings) already exist and keep their meaning.

| Section | Chord | Action |
|---|---|---|
| Views | `mod+1` … `mod+6` | Local, Control, Settings, Extensions, Repomind, System |
| Layout | `mod+shift+f` / `mod+shift+s` / `mod+shift+g` | Focused / Split / Grid |
| Fleet | `mod+/` | Filter fleet |
| Fleet | `mod+u` | Toggle urgent only |
| Fleet | `mod+r` | Refresh |
| Fleet | `mod+shift+n` | New repo |
| Fleet | `mod+l` | Focus the fleet list |
| Lane | `mod+e` | Spawn agent |
| Lane | `mod+t` | Open terminal |
| Lane | `mod+p` | Pin / unpin |
| Lane | `mod+d` | Delete lane (confirms) |
| Lane | `mod+shift+m` | Merge lane |
| Lane | `mod+.` | Stop agent (confirms) |
| Agents | `mod+[` / `mod+]` | Previous / next agent tab |
| Agents | `mod+g` | Jump to a lane needing attention |
| Help | `mod+?` | Open the keyboard reference |

Destructive actions (`mod+d`, `mod+.`) route through the existing confirm flow rather than
firing immediately, matching the TUI's two-press delete.

Lane and Agents bindings carry `when: "lane"` / `when: "agent"`: with nothing selected they do
nothing and render dimmed in help, instead of erroring.

## 3. Global handler

One `keydown` listener registered in `App.tsx`, replacing the two ad-hoc listeners there today
(`onSettingsShortcut`, `onExtensionsShortcut`). It matches through `matchChord`, checks the
binding's guard, calls `preventDefault()` only on a match, and dispatches to the existing action
functions. Unmatched keys fall through untouched, so typing in a terminal, an input, or the
repomind composer behaves exactly as before.

The bare `6` shortcut for extensions is removed: it is exactly the kind of binding that fires
while typing. `mod+4` replaces it.

## 4. Settings redesign

`SettingsModal` becomes tabbed: **General · Notifications · Appearance · Keyboard**. Tabs are a
proper tablist: left/right arrows move between them, and the panel is reachable by Tab.

New reusable controls, each in its own file under `components/controls/`:

- `Switch`: an accessible switch (`role="switch"`, `aria-checked`, Space/Enter to toggle)
  replacing the raw checkbox used by all fourteen booleans.
- `Select`: a real `<select>` for default agent, orchestrator agent, orchestrator model, and
  renderer, which are free-text-with-datalist today. Options come from the existing
  `agent.detect` call already made on mount.
- `ColorField`: accent presets as swatches plus a custom hex input, replacing free text.
  Keeps calling `applyAccent` so the live preview behavior is unchanged.

The **Keyboard** tab renders the keymap grouped by section, with a search box filtering on label
and chord. It is generated from `keymap.ts`, so a new binding appears in help automatically.

## 5. Keyboard navigation

- `Escape` in a focused terminal releases focus to the app shell, so list navigation works
  without reaching for the mouse. The terminal's existing key handling is otherwise untouched.
- `mod+l` focuses the fleet list, which already supports `j`/`k`, arrows, and Enter.
- Modals already trap focus and restore the opener (see `Modal.test.tsx`); the new tabs
  participate in that same order.

## 6. Testing

- `keymap.test.ts`: chord normalization across platforms; no duplicate chord in the table; every
  binding has a non-empty label and a known section; guarded bindings declare a valid `when`.
- `matchChord`: returns `null` for unmodified keys (so terminal typing is never intercepted) and
  matches modifier chords regardless of focus.
- A render test that the Keyboard tab lists every binding in the table, so help cannot silently
  omit one.
- The existing 50 tests stay green; `tsc --noEmit` clean.

## Verification

1. With a terminal focused and an agent running, every chord in the table still fires and normal
   typing still reaches the agent.
2. Each view opens from its chord; layout chords switch Focused/Split/Grid.
3. With no lane selected, lane chords do nothing and show dimmed in help.
4. Settings: every boolean toggles via Switch and persists; agent/model/renderer are dropdowns;
   accent changes from a swatch and from a custom hex, applying live.
5. The Keyboard tab lists every binding and filters on search.
6. A full pass through the app using only the keyboard: switch views, filter and select a lane,
   spawn an agent, open a terminal, type into it, escape out, and change a setting.

## Sequencing

Three steps, each independently verifiable: keymap plus global handler, then the Keyboard help
tab, then the Settings controls. This order means the help tab has real data to render as soon
as it exists.
