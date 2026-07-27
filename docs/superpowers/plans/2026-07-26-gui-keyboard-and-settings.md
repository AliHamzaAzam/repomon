# GUI Keyboard Controls and Settings Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Repomon desktop GUI fully operable from the keyboard, document every binding inside the app, and give Settings real controls.

**Architecture:** A single data table (`keymap.ts`) is the only source of truth for shortcuts; one global `keydown` listener in `App.tsx` matches against it, and the Settings help tab renders from it, so the two can never drift. Because most lane operations currently exist only as inline closures inside `ControlCenter.tsx` and `TerminalWorkspace.tsx`, the first two tasks lift them into stores so a shortcut has something to call.

**Tech Stack:** SolidJS 1.9 (not React), TypeScript, Vite, Vitest + `@solidjs/testing-library`, Tauri v2.

## Global Constraints

- **SolidJS, not React.** Never destructure props (it breaks reactivity): always `props.x`. Use `class=` not `className`, `classList=` for conditionals, and `event.currentTarget`.
- **Render tests use a thunk:** `render(() => <Component />)`. Use `fireEvent`, never `userEvent` (not a dependency).
- Do not use em-dashes in any code comment, label, or copy. Use commas, colons, or periods.
- Commit messages: single line, no AI co-author trailer. This repo uses Conventional Commit prefixes (`fix(desktop):`, `feat(ext):`, `refactor(desktop):`), so match that; each task states its exact message.
- Never commit anything under `docs/` or `.superpowers/`.
- Existing tests must stay green (50 at plan time) and `npm run check` (tsc --noEmit) must pass.
- Work from `/Users/azaleas/Developer/Claude/repomon`; the desktop app is `apps/desktop`.

---

## File Structure

| File | Responsibility |
|---|---|
| `apps/desktop/src/keymap.ts` | **New.** The binding table plus `matchChord` / `formatChord`. Pure, no imports from components. |
| `apps/desktop/src/keymap.test.ts` | **New.** Table integrity and chord matching. |
| `apps/desktop/src/stores/actions.ts` | **Modify.** Gains the lane operations that today live inline in `ControlCenter.tsx`. |
| `apps/desktop/src/stores/actions.test.ts` | **New.** Covers the new lane operations. |
| `apps/desktop/src/stores/workspace.ts` | **New.** Layout mode, active tab, and shell open, lifted out of `TerminalWorkspace.tsx`. |
| `apps/desktop/src/stores/workspace.test.ts` | **New.** Tab cycling and layout persistence. |
| `apps/desktop/src/App.tsx` | **Modify.** One keymap-driven listener replaces the two ad-hoc ones. |
| `apps/desktop/src/components/TerminalWorkspace.tsx` | **Modify.** Consumes the workspace store instead of local signals. |
| `apps/desktop/src/components/controls/Switch.tsx` | **New.** Accessible switch replacing raw checkboxes. |
| `apps/desktop/src/components/controls/Select.tsx` | **New.** Labeled `<select>`. |
| `apps/desktop/src/components/controls/ColorField.tsx` | **New.** Accent swatches plus custom hex. |
| `apps/desktop/src/components/SettingsModal.tsx` | **Modify.** Tabbed, using the new controls. |
| `apps/desktop/src/components/KeyboardHelp.tsx` | **New.** Renders the keymap, with search. |
| `apps/desktop/src/components/KeyboardHelp.test.tsx` | **New.** Every binding appears. |
| `apps/desktop/src/theme.ts` | **Modify.** Export the existing private `accents` map. |

---

## Corrections to the design spec

The spec at `docs/superpowers/specs/2026-07-26-gui-keyboard-and-settings-design.md` was written before the codebase was explored. Three of its assumptions are wrong, and this plan supersedes them:

1. **There is no `AUTO` layout mode.** `WorkspaceLayout = "focused" | "split" | "grid"` (`TerminalWorkspace.tsx:16`). The `AUTO` control in the toolbar is the terminal *renderer* (`auto | webgl | dom`), a separate setting.
2. **`LOCAL` and `SYSTEM` are not panels.** `LOCAL` is a static `<span>` badge. `SYSTEM` is the label of the theme-cycle button (`themeLabel(theme())` renders System/Dark/Light). Only Extensions and Repomind are real togglable panels; Settings and Control already have `mod+,` and `mod+k`. So `mod+1..6` as "six views" does not exist and is dropped.
3. **`mod+shift+f` collides with the terminal's find shortcut** (`TerminalPane.tsx:213`, `Cmd/Ctrl+Shift+F`). Layout chords use `mod+shift+1/2/3` instead.

---

## Final binding table

`mod` is Cmd on macOS, Ctrl elsewhere. `mod+k` (Control) stays owned by `ControlCenter`'s own listener and is not in this table; the help reference documents it as a static row. `mod+,` IS in the table: Task 4 deletes the ad-hoc listener that used to implement it, so the table must carry it or the binding dies.

| Section | Chord | Action | Guard |
|---|---|---|---|
| Panels | `mod+,` | Open settings | |
| Panels | `mod+4` | Toggle Extensions | |
| Panels | `mod+5` | Toggle Repomind | |
| Panels | `mod+6` | Cycle theme | |
| Layout | `mod+shift+1` / `mod+shift+2` / `mod+shift+0` | Focused / Split / Grid | |
| Fleet | `mod+/` | Focus the fleet filter | |
| Fleet | `mod+u` | Toggle urgent only | |
| Fleet | `mod+r` | Refresh | |
| Fleet | `mod+n` | New lane | |
| Fleet | `mod+shift+n` | Add repo | |
| Fleet | `mod+g` | Jump to a lane needing attention | |
| Lane | `mod+e` | Spawn agent | lane |
| Lane | `mod+t` | Open terminal | lane |
| Lane | `mod+p` | Pin or unpin | lane |
| Lane | `mod+d` | Delete lane (confirms) | lane |
| Lane | `mod+shift+m` | Merge lane (confirms) | lane |
| Lane | `mod+.` | Stop agent (confirms) | agent |
| Agents | `mod+[` / `mod+]` | Previous / next agent tab | lane |
| Help | `mod+?` | Keyboard reference | |

---

## Task 1: Lane operations in the actions store

Today `pin`, `merge`, `delete lane`, `stop agent`, and `adopt` exist only as inline closures inside `ControlCenter.tsx` JSX (lines 311 to 340). A shortcut cannot call them. Lift them into `stores/actions.ts`, which already imports `daemonCall`, already holds `fleet`, and already owns the confirm flow.

**Files:**
- Modify: `apps/desktop/src/stores/actions.ts`
- Test: `apps/desktop/src/stores/actions.test.ts` (new)

**Interfaces:**
- Consumes: `FleetStore` (`stores/fleet.ts`), `ConfirmOptions` (`components/ConfirmDialog.tsx`), `daemonCall` (`ipc/rpc.ts`).
- Produces, added to the object `createActionsStore` returns:
  - `pinLane(lane: Lane): Promise<void>`
  - `mergeLane(lane: Lane): void`
  - `deleteLane(lane: Lane): void`
  - `stopAgent(lane: Lane, agent: AgentSession | null): void`
  - `adoptAgent(lane: Lane, agent: AgentSession | null): Promise<void>`

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src/stores/actions.test.ts`:

```ts
import { createRoot } from "solid-js";
import { describe, expect, it, vi, beforeEach } from "vitest";

import type { AgentSession, Lane } from "../bindings";
import { createActionsStore } from "./actions";
import type { FleetStore } from "./fleet";

const calls = vi.hoisted(() => ({ list: [] as Array<{ method: string; params: unknown }> }));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => {
    calls.list.push({ method, params });
    return Promise.resolve(null);
  },
}));

function lane(overrides: Partial<Lane> = {}): Lane {
  return {
    id: 7,
    repo: { id: 2, path: "/code/r", name: "r", added_at: "2026-07-26T00:00:00Z", worktree_root_template: null },
    worktree: { id: 3, repo_id: 2, path: "/code/r-wt", branch: "feat/x", head: "abc", is_main: false, name: "x" },
    state: { worktree_id: 3, head: "abc", branch: "feat/x", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: [],
    last_activity_at: "2026-07-26T00:00:00Z",
    pinned: false,
    ...overrides,
  };
}

function fleetStub(): FleetStore {
  return { refresh: vi.fn().mockResolvedValue(undefined) } as unknown as FleetStore;
}

beforeEach(() => {
  calls.list = [];
});

describe("lane operations", () => {
  it("pin toggles the current pinned state and refreshes", async () => {
    await createRoot(async (dispose) => {
      const fleet = fleetStub();
      const actions = createActionsStore(fleet);
      await actions.pinLane(lane({ pinned: false }));
      expect(calls.list[0]).toEqual({ method: "agent.pin", params: { lane_id: 7, pinned: true } });
      expect(fleet.refresh).toHaveBeenCalled();
      dispose();
    });
  });

  it("delete and merge go through confirm rather than firing immediately", async () => {
    await createRoot(async (dispose) => {
      const actions = createActionsStore(fleetStub());

      actions.deleteLane(lane());
      const del = actions.confirmOptions();
      expect(del?.danger).toBe(true);
      expect(calls.list).toHaveLength(0); // nothing sent until confirmed
      await del?.onConfirm();
      expect(calls.list[0].method).toBe("lane.delete");

      calls.list = [];
      actions.mergeLane(lane());
      const merge = actions.confirmOptions();
      expect(calls.list).toHaveLength(0);
      await merge?.onConfirm();
      expect(calls.list[0].method).toBe("lane.merge");
      dispose();
    });
  });

  it("stop targets the agent's tmux window", async () => {
    await createRoot(async (dispose) => {
      const actions = createActionsStore(fleetStub());
      const agent = { tmux_window: "lane-7" } as AgentSession;
      actions.stopAgent(lane(), agent);
      await actions.confirmOptions()?.onConfirm();
      expect(calls.list[0]).toEqual({ method: "agent.stop", params: { lane_id: 7, window: "lane-7" } });
      dispose();
    });
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd apps/desktop && npx vitest run src/stores/actions.test.ts`
Expected: FAIL, `actions.pinLane is not a function`.

- [ ] **Step 3: Add the operations**

In `apps/desktop/src/stores/actions.ts`, add `AgentSession` to the type import from `../bindings`, then add these functions above the `return`:

```ts
  /// Pin or unpin the lane. Pinning is not destructive, so it applies immediately.
  async function pinLane(lane: Lane) {
    setError(null);
    try {
      await daemonCall("agent.pin", { lane_id: lane.id, pinned: !lane.pinned });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function mergeLane(lane: Lane) {
    setConfirmOptions({
      title: "Merge lane?",
      message: `Merge ${lane.worktree.branch ?? lane.worktree.name} into the repository base branch.`,
      confirmLabel: "Merge",
      onConfirm: async () => {
        await daemonCall("lane.merge", { lane_id: lane.id });
        await fleet.refresh();
      },
    });
  }

  function deleteLane(lane: Lane) {
    setConfirmOptions({
      title: "Delete lane?",
      message: `Remove the ${lane.worktree.name} worktree. The branch is kept.`,
      confirmLabel: "Delete",
      danger: true,
      onConfirm: async () => {
        await daemonCall("lane.delete", { lane_id: lane.id, also_delete_branch: false });
        await fleet.refresh();
      },
    });
  }

  function stopAgent(lane: Lane, agent: AgentSession | null) {
    setConfirmOptions({
      title: "Stop agent?",
      message: "Stop this managed agent. Its terminal session ends.",
      confirmLabel: "Stop",
      danger: true,
      onConfirm: async () => {
        await daemonCall("agent.stop", { lane_id: lane.id, window: agent?.tmux_window ?? undefined });
        await fleet.refresh();
      },
    });
  }

  async function adoptAgent(lane: Lane, agent: AgentSession | null) {
    setError(null);
    try {
      await daemonCall("agent.adopt", { lane_id: lane.id, session_id: agent?.session_id ?? undefined });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }
```

Add `pinLane, mergeLane, deleteLane, stopAgent, adoptAgent` to the returned object.

- [ ] **Step 4: Run the test**

Run: `cd apps/desktop && npx vitest run src/stores/actions.test.ts`
Expected: PASS, 3 tests.

- [ ] **Step 5: Point ControlCenter at the store**

In `apps/desktop/src/components/ControlCenter.tsx`, replace the inline bodies at lines 311 to 340 with calls to the new functions, so there is exactly one implementation. For example the pin entry becomes `run("pin", () => props.actions.pinLane(lane))`, and the delete entry becomes `() => { props.actions.deleteLane(lane); closeControl(false); }`. Keep every existing guard (`!lane.worktree.is_main` for merge and delete, `agent?.tmux_window` for stop, `agent?.external` for adopt) exactly as it is.

- [ ] **Step 6: Verify nothing regressed**

Run: `cd apps/desktop && npm run check && npm test`
Expected: tsc clean, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/stores/actions.ts apps/desktop/src/stores/actions.test.ts apps/desktop/src/components/ControlCenter.tsx
git commit -m "refactor(desktop): move lane operations into the actions store"
```

---

## Task 2: Workspace store for layout and tabs

`layout`, `activeWindow`, and `openShell` are local signals inside `TerminalWorkspace.tsx`, so the layout and tab chords have nothing to call. Lift them into a store created once in `App.tsx`.

**Files:**
- Create: `apps/desktop/src/stores/workspace.ts`
- Create: `apps/desktop/src/stores/workspace.test.ts`
- Modify: `apps/desktop/src/components/TerminalWorkspace.tsx`
- Modify: `apps/desktop/src/App.tsx` (create the store, pass it down)

**Interfaces:**
- Consumes: `FleetStore`, `PaneTarget` (`components/terminalTargets.ts`), `daemonCall`.
- Produces `createWorkspaceStore(fleet: FleetStore)` returning:
  - `layout: Accessor<WorkspaceLayout>` and `chooseLayout(next: WorkspaceLayout): void`
  - `renderer: Accessor<TerminalRenderer>` and `chooseRenderer(next: TerminalRenderer): void`
  - `activeWindow: Accessor<string | null>` and `setActiveWindow(next: string | null): void`
  - `cycleTab(delta: number, targets: PaneTarget[]): void`
  - `openShell(): Promise<void>`

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src/stores/workspace.test.ts`:

```ts
import { createRoot } from "solid-js";
import { describe, expect, it, vi } from "vitest";

import type { PaneTarget } from "../components/terminalTargets";
import { createWorkspaceStore } from "./workspace";
import type { FleetStore } from "./fleet";

vi.mock("../ipc/rpc", () => ({ daemonCall: vi.fn().mockResolvedValue({ id: "term-1" }) }));

function target(window: string): PaneTarget {
  return { laneId: 7, window, label: window, shell: false, sessionId: null };
}

function fleetStub(): FleetStore {
  return { refresh: vi.fn().mockResolvedValue(undefined), selectedLaneId: () => 7 } as unknown as FleetStore;
}

describe("workspace store", () => {
  it("cycles tabs with wraparound in both directions", () => {
    createRoot((dispose) => {
      const ws = createWorkspaceStore(fleetStub());
      const tabs = [target("a"), target("b"), target("c")];

      ws.setActiveWindow("a");
      ws.cycleTab(1, tabs);
      expect(ws.activeWindow()).toBe("b");

      ws.cycleTab(-1, tabs);
      expect(ws.activeWindow()).toBe("a");

      // Wraps backwards off the front, and forwards off the end.
      ws.cycleTab(-1, tabs);
      expect(ws.activeWindow()).toBe("c");
      ws.cycleTab(1, tabs);
      expect(ws.activeWindow()).toBe("a");
      dispose();
    });
  });

  it("cycling is inert with no tabs and starts at the first when nothing is active", () => {
    createRoot((dispose) => {
      const ws = createWorkspaceStore(fleetStub());
      ws.cycleTab(1, []);
      expect(ws.activeWindow()).toBeNull();

      ws.cycleTab(1, [target("a"), target("b")]);
      expect(ws.activeWindow()).toBe("a");
      dispose();
    });
  });

  it("layout persists to localStorage", () => {
    createRoot((dispose) => {
      const ws = createWorkspaceStore(fleetStub());
      ws.chooseLayout("grid");
      expect(ws.layout()).toBe("grid");
      expect(localStorage.getItem("repomon.workspace.layout")).toBe("grid");
      dispose();
    });
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd apps/desktop && npx vitest run src/stores/workspace.test.ts`
Expected: FAIL, cannot resolve `./workspace`.

- [ ] **Step 3: Create the store**

Create `apps/desktop/src/stores/workspace.ts`. Move `WorkspaceLayout`, `readLayout`, `chooseLayout`, `renderer`, `chooseRenderer`, `activeWindow`, and `openShell` out of `TerminalWorkspace.tsx` verbatim, then add `cycleTab`:

```ts
  /// Move to the next or previous tab, wrapping at both ends. `targets` is the lane's tab strip
  /// in render order, passed in by the component so the store does not duplicate that memo.
  function cycleTab(delta: number, targets: PaneTarget[]) {
    if (targets.length === 0) return;
    const index = targets.findIndex((target) => target.window === activeWindow());
    // Nothing active yet: step in from the start rather than jumping to the end.
    const next = index < 0 ? 0 : (index + delta + targets.length) % targets.length;
    setActiveWindow(targets[next].window);
  }
```

- [ ] **Step 4: Run the test**

Run: `cd apps/desktop && npx vitest run src/stores/workspace.test.ts`
Expected: PASS, 3 tests.

- [ ] **Step 5: Consume the store in the component**

In `App.tsx` add `const workspace = createWorkspaceStore(fleet);` next to the other stores (near line 58) and pass `workspace={workspace}` to `<TerminalWorkspace>`. In `TerminalWorkspace.tsx` delete the moved local signals and read `props.workspace.layout()`, `props.workspace.activeWindow()`, and so on.

Move the `targets` and `laneTargets` memos into the store as well: both derive only from `fleet.lanes()`, `fleet.terminals()`, and `fleet.selectedLaneId()`, so they need nothing component-local, and the shortcut handler in Task 4 needs `laneTargets` to cycle tabs. `cycleTab` still takes the array as an argument so it stays a pure, directly testable function.

- [ ] **Step 6: Verify**

Run: `cd apps/desktop && npm run check && npm test && npm run build`
Expected: tsc clean, tests pass, build succeeds.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/stores/workspace.ts apps/desktop/src/stores/workspace.test.ts apps/desktop/src/components/TerminalWorkspace.tsx apps/desktop/src/App.tsx
git commit -m "refactor(desktop): lift workspace layout and tab state into a store"
```

---

## Task 3: The keymap

**Files:**
- Create: `apps/desktop/src/keymap.ts`
- Create: `apps/desktop/src/keymap.test.ts`

**Interfaces:**
- Consumes: nothing (pure).
- Produces: `KeymapSection`, `Binding`, `BINDINGS: Binding[]`, `matchChord(event: KeyboardEvent): Binding | null`, `formatChord(chord: string, platform?: string): string`.

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src/keymap.test.ts`:

```ts
import { describe, expect, it } from "vitest";

import { BINDINGS, formatChord, matchChord } from "./keymap";

function key(init: Partial<KeyboardEvent> & { key: string }): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, ...init });
}

describe("keymap table", () => {
  it("has no duplicate chords", () => {
    const chords = BINDINGS.map((binding) => binding.chord);
    expect(new Set(chords).size).toBe(chords.length);
  });

  it("every binding has an id, a label, and a section", () => {
    for (const binding of BINDINGS) {
      expect(binding.id.length).toBeGreaterThan(0);
      expect(binding.label.length).toBeGreaterThan(0);
      expect(binding.section.length).toBeGreaterThan(0);
    }
  });
});

describe("matchChord", () => {
  it("ignores unmodified keys so typing is never intercepted", () => {
    expect(matchChord(key({ key: "e" }))).toBeNull();
    expect(matchChord(key({ key: "6" }))).toBeNull();
    expect(matchChord(key({ key: "/" }))).toBeNull();
  });

  it("matches a modifier chord on either platform modifier", () => {
    expect(matchChord(key({ key: "e", metaKey: true }))?.id).toBe("lane.spawn");
    expect(matchChord(key({ key: "e", ctrlKey: true }))?.id).toBe("lane.spawn");
  });

  it("distinguishes shifted chords from their unshifted twin", () => {
    expect(matchChord(key({ key: "n", metaKey: true }))?.id).toBe("fleet.newLane");
    expect(matchChord(key({ key: "N", metaKey: true, shiftKey: true }))?.id).toBe("fleet.addRepo");
  });

  it("returns null for an unbound chord", () => {
    expect(matchChord(key({ key: "z", metaKey: true }))).toBeNull();
  });
});

describe("formatChord", () => {
  it("renders platform symbols", () => {
    expect(formatChord("mod+e", "mac")).toBe("⌘E");
    expect(formatChord("mod+shift+m", "mac")).toBe("⌘⇧M");
    expect(formatChord("mod+e", "other")).toBe("Ctrl+E");
    expect(formatChord("mod+shift+m", "other")).toBe("Ctrl+Shift+M");
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd apps/desktop && npx vitest run src/keymap.test.ts`
Expected: FAIL, cannot resolve `./keymap`.

- [ ] **Step 3: Write the keymap**

Create `apps/desktop/src/keymap.ts`:

```ts
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
  { id: "panel.settings", chord: "mod+,", label: "Open settings", section: "Panels" },
  { id: "panel.extensions", chord: "mod+4", label: "Toggle extensions", section: "Panels" },
  { id: "panel.repomind", chord: "mod+5", label: "Toggle repomind", section: "Panels" },
  { id: "panel.theme", chord: "mod+6", label: "Cycle theme", section: "Panels" },

  { id: "layout.focused", chord: "mod+shift+1", label: "Focused layout", section: "Layout" },
  { id: "layout.split", chord: "mod+shift+2", label: "Split layout", section: "Layout" },
  { id: "layout.grid", chord: "mod+shift+0", label: "Grid layout", section: "Layout" },

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
  // Digits come from `code`, not `key`: shift turns "1" into "!" (and non-US layouts move the
  // number row entirely), so keying off `event.key` would make every shifted digit chord
  // unreachable while still rendering as available in the help reference.
  const digit = event.code.startsWith("Digit") ? event.code.slice(5) : null;
  // "?" already implies shift on most layouts, so do not double-encode it.
  const key = digit ?? (event.key === "?" ? "?" : event.key.toLowerCase());
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
```

- [ ] **Step 4: Run the test**

Run: `cd apps/desktop && npx vitest run src/keymap.test.ts`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/keymap.ts apps/desktop/src/keymap.test.ts
git commit -m "feat(desktop): add the keyboard shortcut table"
```

---

## Task 4: The global handler

**Files:**
- Modify: `apps/desktop/src/App.tsx` (replace lines 89 to 105 and 139 to 146)

**Interfaces:**
- Consumes: `matchChord` (Task 3), `actions.pinLane`/`mergeLane`/`deleteLane`/`stopAgent` (Task 1), `workspace.chooseLayout`/`cycleTab`/`openShell` (Task 2).
- Produces: nothing importable; wires the table to behavior.

- [ ] **Step 1: Replace both ad-hoc listeners**

Delete `onSettingsShortcut` and `onExtensionsShortcut` and their `addEventListener`/`removeEventListener` lines. Add:

```tsx
  /// One listener for every shortcut. Registered in bubble phase so an open Modal (which listens
  /// in capture phase and stops propagation on Escape) still closes first.
  const onShortcut = (event: KeyboardEvent) => {
    const binding = matchChord(event);
    if (!binding) return;

    // A modal owns the keyboard while it is open. ControlCenter keeps its own Cmd+K listener.
    if (actions.settingsOpen() || actions.spawnLane() || actions.newLaneOpen()
      || actions.renameTarget() || actions.confirmOptions()) return;

    const lane = fleet.selectedLane();
    if (binding.when && !lane) return;
    const agent = lane?.agent_sessions.find((session) => session.tmux_window) ?? null;
    if (binding.when === "agent" && !agent) return;

    event.preventDefault();
    switch (binding.id) {
      case "panel.settings": actions.openSettings(); break;
      case "panel.extensions": setExtensionsOpen((open) => !open); break;
      case "panel.repomind": setRepomindOpen((open) => !open); break;
      case "panel.theme": cycleTheme(); break;
      case "layout.focused": workspace.chooseLayout("focused"); break;
      case "layout.split": workspace.chooseLayout("split"); break;
      case "layout.grid": workspace.chooseLayout("grid"); break;
      case "fleet.filter": searchInput?.focus(); break;
      case "fleet.urgent": fleet.setUrgentOnly(!fleet.urgentOnly()); break;
      case "fleet.refresh": void fleet.refresh(); break;
      case "fleet.newLane": actions.newLane(); break;
      case "fleet.addRepo": void actions.addRepo(); break;
      case "fleet.jumpUrgent": fleet.moveSelection(1, true); break;
      case "lane.spawn": if (lane) actions.spawn(lane); break;
      case "lane.terminal": void workspace.openShell(); break;
      case "lane.pin": if (lane) void actions.pinLane(lane); break;
      case "lane.delete": if (lane) actions.deleteLane(lane); break;
      case "lane.merge": if (lane) actions.mergeLane(lane); break;
      case "lane.stop": if (lane) actions.stopAgent(lane, agent); break;
      case "agents.prev": workspace.cycleTab(-1, workspace.laneTargets()); break;
      case "agents.next": workspace.cycleTab(1, workspace.laneTargets()); break;
      case "help.open": actions.openSettings(); setSettingsTab("keyboard"); break;
    }
  };
```

Register it in `onMount` with `window.addEventListener("keydown", onShortcut)` and remove it in `onCleanup`.

Two notes on dependencies between tasks:

- `workspace.laneTargets()` comes from Task 2, which moves that memo into the store. No change is needed here.
- `setSettingsTab` does not exist until Task 5. Implement `help.open` as `actions.openSettings()` alone for now, and add the tab selection in Task 5 Step 3 once the tab signal exists. To make that concrete: Task 5 adds `openSettingsTab(tab: SettingsTab)` to the actions store (setting a `settingsTab` signal that `SettingsModal` reads on mount), and this case becomes `actions.openSettingsTab("keyboard")`.

- [ ] **Step 2: Verify by hand against a running app**

Run: `cd apps/desktop && npm run tauri dev`
Check: with an agent's terminal focused, `Cmd+4` toggles Extensions and typing still reaches the agent. With no lane selected, `Cmd+E` does nothing. `Cmd+Shift+1/2/3` switch layout.

- [ ] **Step 3: Verify the suite**

Run: `cd apps/desktop && npm run check && npm test`

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/App.tsx apps/desktop/src/stores/workspace.ts
git commit -m "feat(desktop): drive shortcuts from the keymap table"
```

---

## Task 5: Settings controls and tabs

**Files:**
- Create: `apps/desktop/src/components/controls/Switch.tsx`
- Create: `apps/desktop/src/components/controls/Select.tsx`
- Create: `apps/desktop/src/components/controls/ColorField.tsx`
- Modify: `apps/desktop/src/theme.ts` (export `accents`)
- Modify: `apps/desktop/src/components/SettingsModal.tsx`

**Interfaces:**
- Produces: `Switch`, `Select`, `ColorField` components, and `ACCENTS` exported from `theme.ts`.

- [ ] **Step 1: Export the accent presets**

In `apps/desktop/src/theme.ts` change `const accents: Record<string, string> = {` to `export const ACCENTS: Record<string, string> = {` and update its internal references. The seven presets (cyan, green, magenta, amber, blue, red, violet) stay exactly as they are.

- [ ] **Step 2: Write the controls**

`Switch.tsx`. A real switch, not a bare checkbox, so it is reachable and operable by keyboard and announces state:

```tsx
export default function Switch(props: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div class="flex items-center justify-between rounded border border-line px-3 py-2 text-xs" classList={{ "opacity-50": props.disabled }}>
      <span>{props.label}</span>
      <button
        type="button"
        role="switch"
        aria-checked={props.checked}
        aria-label={props.label}
        disabled={props.disabled}
        class="focus-ring relative h-4 w-8 shrink-0 rounded-full border border-line transition-colors"
        classList={{ "bg-signal/70": props.checked, "bg-raised": !props.checked }}
        onClick={() => props.onChange(!props.checked)}
      >
        <span
          class="absolute top-0.5 h-2.5 w-2.5 rounded-full bg-foreground transition-all"
          classList={{ "left-4": props.checked, "left-0.5": !props.checked }}
        />
      </button>
    </div>
  );
}
```

`Select.tsx`, matching the established `.settings-input` dropdown style already used in `NewLaneModal.tsx:52`:

```tsx
import { For } from "solid-js";

export default function Select(props: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (value: string) => void;
}) {
  return (
    <label class="block">
      <span class="section-label">{props.label}</span>
      <select class="settings-input" value={props.value} onChange={(event) => props.onChange(event.currentTarget.value)}>
        <For each={props.options}>{(option) => <option value={option.value}>{option.label}</option>}</For>
      </select>
    </label>
  );
}
```

`ColorField.tsx`. `applyAccent` accepts a named preset, a 3- or 6-digit hex, or "mono":

```tsx
import { For } from "solid-js";

import { ACCENTS } from "../../theme";

export default function ColorField(props: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div class="block">
      <span class="section-label">{props.label}</span>
      <div class="mt-1.5 flex flex-wrap items-center gap-1.5">
        <For each={Object.entries(ACCENTS)}>
          {([name, color]) => (
            <button
              type="button"
              aria-label={name}
              aria-pressed={props.value === name}
              title={name}
              class="focus-ring h-5 w-5 rounded-full border"
              classList={{ "border-foreground": props.value === name, "border-line": props.value !== name }}
              style={{ background: color }}
              onClick={() => props.onChange(name)}
            />
          )}
        </For>
        <button
          type="button"
          aria-label="mono"
          aria-pressed={props.value === "mono"}
          title="mono"
          class="focus-ring h-5 w-5 rounded-full border bg-raised"
          classList={{ "border-foreground": props.value === "mono", "border-line": props.value !== "mono" }}
          onClick={() => props.onChange("mono")}
        />
        <input
          class="settings-input mt-0 ml-1 w-28"
          value={props.value}
          placeholder="#rrggbb"
          aria-label="Custom accent"
          onInput={(event) => props.onChange(event.currentTarget.value)}
        />
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Rework SettingsModal**

Add a tab signal and strip, keeping the existing `config`/`patch`/`save` flow untouched (save stays an explicit button; do not switch to save-on-change):

```tsx
export type SettingsTab = "general" | "notifications" | "appearance" | "keyboard";
const [tab, setTab] = createSignal<SettingsTab>(props.initialTab ?? "general");
```

Add `initialTab?: SettingsTab` to `SettingsModalProps`. In `stores/actions.ts` add a `settingsTab` signal plus `openSettingsTab(tab: SettingsTab)` that sets it and opens the modal, and have `ActionModals.tsx` pass `initialTab={actions.settingsTab()}` through. Then change the `help.open` case from Task 4 to `actions.openSettingsTab("keyboard")`, which is what makes `mod+?` land directly on the reference.

Render the strip above the scrolling body, matching the house pattern in `RepomindPanel.tsx:126`: a `role="tablist"` wrapper with `role="tab"` buttons carrying `aria-selected` and `focus-ring`. Move the existing fields under their tabs: General keeps default agent, worktree template, auto-continue message and the five general toggles; Notifications keeps the master switch and its nine dependents; Appearance takes accent (now `ColorField`) plus the renderer `Select`; Keyboard renders `<KeyboardHelp />` from Task 6. Replace every `Toggle` usage with `Switch` and both datalist `TextField`s (default agent, repomind agent) with `Select` populated from the `agents()` signal already loaded on mount.

- [ ] **Step 4: Verify**

Run: `cd apps/desktop && npm run check && npm test && npm run build`

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/controls apps/desktop/src/components/SettingsModal.tsx apps/desktop/src/theme.ts
git commit -m "feat(desktop): give settings switches, selects, and accent swatches"
```

---

## Task 6: The keyboard reference

**Files:**
- Create: `apps/desktop/src/components/KeyboardHelp.tsx`
- Create: `apps/desktop/src/components/KeyboardHelp.test.tsx`

**Interfaces:**
- Consumes: `BINDINGS`, `formatChord` (Task 3).
- Produces: a default-exported `KeyboardHelp` component taking no props.

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src/components/KeyboardHelp.test.tsx`:

```tsx
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";

import { BINDINGS } from "../keymap";
import KeyboardHelp from "./KeyboardHelp";

describe("keyboard reference", () => {
  it("lists every binding so help can never omit one", () => {
    render(() => <KeyboardHelp />);
    for (const binding of BINDINGS) {
      expect(screen.getByText(binding.label)).toBeInTheDocument();
    }
  });

  it("filters on the search box", () => {
    render(() => <KeyboardHelp />);
    fireEvent.input(screen.getByPlaceholderText("Search shortcuts"), { target: { value: "merge" } });
    expect(screen.getByText("Merge lane")).toBeInTheDocument();
    expect(screen.queryByText("Refresh")).not.toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd apps/desktop && npx vitest run src/components/KeyboardHelp.test.tsx`
Expected: FAIL, cannot resolve `./KeyboardHelp`.

- [ ] **Step 3: Write the component**

```tsx
import { For, createMemo, createSignal } from "solid-js";

import { BINDINGS, formatChord, type KeymapSection } from "../keymap";

const SECTIONS: KeymapSection[] = ["Panels", "Layout", "Fleet", "Lane", "Agents", "Help"];

export default function KeyboardHelp() {
  const [query, setQuery] = createSignal("");

  const matches = createMemo(() => {
    const needle = query().trim().toLowerCase();
    if (!needle) return BINDINGS;
    return BINDINGS.filter((binding) =>
      binding.label.toLowerCase().includes(needle) || formatChord(binding.chord).toLowerCase().includes(needle));
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
          return (
            <Show when={rows().length > 0}>
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
              </section>
            </Show>
          );
        }}
      </For>
    </div>
  );
}
```

Import `Show` from `solid-js` alongside the others.

- [ ] **Step 4: Run the test**

Run: `cd apps/desktop && npx vitest run src/components/KeyboardHelp.test.tsx`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/KeyboardHelp.tsx apps/desktop/src/components/KeyboardHelp.test.tsx
git commit -m "feat(desktop): add the in-app keyboard reference"
```

---

## Task 7: Escape releases the terminal

Without this there is no keyboard-only way out of a focused terminal back to the fleet list.

**Files:**
- Modify: `apps/desktop/src/components/TerminalPane.tsx` (the `attachCustomKeyEventHandler` at lines 203 to 215)

- [ ] **Step 1: Add the release**

Inside the existing handler, before the `translateKeyboardKey` forwarding, add:

```tsx
        // Shift+Escape hands focus back to the app shell so the fleet list can be driven by
        // keyboard. Plain Escape still goes to the agent: Claude Code uses it to interrupt.
        if (event.key === "Escape" && event.shiftKey) {
          event.preventDefault();
          terminal?.blur();
          document.querySelector<HTMLElement>('nav[aria-label="Fleet"]')?.focus();
          return false;
        }
```

- [ ] **Step 2: Verify by hand**

Run: `cd apps/desktop && npm run tauri dev`
Check: with a terminal focused, plain Escape still reaches the agent (it interrupts), while Shift+Escape moves focus to the fleet where `j`/`k` and arrows work.

- [ ] **Step 3: Add the binding to the table**

In `keymap.ts` add `{ id: "terminal.release", chord: "shift+escape", label: "Leave the terminal", section: "Help" }` only if you also extend `chordOf` to encode a shift-without-mod chord. If that complicates matching, instead document it in `KeyboardHelp` as a static row. Prefer the static row: it keeps `chordOf`'s "no modifier means never intercept" rule intact.

- [ ] **Step 4: Verify and commit**

Run: `cd apps/desktop && npm run check && npm test`

```bash
git add apps/desktop/src/components/TerminalPane.tsx apps/desktop/src/components/KeyboardHelp.tsx
git commit -m "feat(desktop): shift+escape releases focus from a terminal"
```

---

## Verification

End to end, on a running app (`npm run tauri dev`):

1. With an agent's terminal focused and the agent running, every chord in the table fires, and ordinary typing still reaches the agent.
2. `Cmd+4` / `Cmd+5` toggle Extensions and Repomind; `Cmd+6` cycles the theme label between System, Dark, and Light.
3. `Cmd+Shift+1/2/3` switch the workspace between focused, split, and grid, and the choice survives a reload.
4. With no lane selected, `Cmd+E`, `Cmd+T`, `Cmd+P`, `Cmd+D`, `Cmd+Shift+M` do nothing.
5. `Cmd+D`, `Cmd+Shift+M`, and `Cmd+.` each open a confirm dialog rather than acting immediately.
6. `Cmd+[` and `Cmd+]` cycle agent tabs within the selected lane and wrap at both ends.
7. Settings: every boolean toggles through a switch and persists after Save; default agent and repomind agent are dropdowns; the accent changes from a swatch and from a custom hex.
8. The Keyboard tab lists every binding and filters as you type.
9. Shift+Escape from a terminal focuses the fleet, where `j`/`k` and arrows move the selection.
10. A full mouse-free pass: switch panels, filter, select a lane, spawn an agent, open a terminal, type in it, leave it, and change a setting.

Automated: `cd apps/desktop && npm run check && npm test && npm run build` all clean.
