import { describe, expect, it } from "vitest";

import {
  BINDINGS,
  detectActiveScope,
  findConflicts,
  formatChord,
  fromCodeMirrorKey,
  keyCapParts,
  matchChord,
  matchSidebarKey,
  numberedPanelBindings,
} from "./keymap";

function key(init: Partial<KeyboardEvent> & { key: string }): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, ...init });
}

describe("keymap table", () => {
  it("has no chord collisions within a scope", () => {
    // A chord may legitimately repeat across *different* scopes (mod+shift+f is both the global
    // "search project files" and the terminal-only "find in terminal"; focus decides which one
    // fires), but two entries in the same scope sharing a chord is a real bug.
    expect(findConflicts()).toEqual([]);
  });

  it("every global binding has a chord unique in the dispatch map", () => {
    // matchChord's BY_CHORD index is keyed by chord alone within scope "global", so this is the
    // one place a cross-scope duplicate would actually be dangerous.
    const globalChords = BINDINGS.filter((binding) => (binding.scope ?? "global") === "global").map(
      (binding) => binding.chord,
    );
    expect(new Set(globalChords).size).toBe(globalChords.length);
  });

  it("every binding has an id, a label, and a section", () => {
    for (const binding of BINDINGS) {
      expect(binding.id.length).toBeGreaterThan(0);
      expect(binding.label.length).toBeGreaterThan(0);
      expect(binding.section.length).toBeGreaterThan(0);
    }
  });
});

describe("findConflicts", () => {
  it("flags two entries that share a chord and a scope", () => {
    const conflicts = findConflicts([
      { id: "a", chord: "mod+x", label: "A", section: "Panels" },
      { id: "b", chord: "mod+x", label: "B", section: "Panels" },
    ]);
    expect(conflicts).toHaveLength(1);
    expect(conflicts[0].bindings.map((binding) => binding.id)).toEqual(["a", "b"]);
  });

  it("does not flag the same chord in two different scopes", () => {
    const conflicts = findConflicts([
      { id: "a", chord: "mod+shift+f", label: "A", section: "Panels", scope: "global" },
      { id: "b", chord: "mod+shift+f", label: "B", section: "Terminal", scope: "terminal" },
    ]);
    expect(conflicts).toEqual([]);
  });
});

describe("fromCodeMirrorKey", () => {
  it("normalizes CodeMirror key strings into this file's chord vocabulary", () => {
    expect(fromCodeMirrorKey("Mod-s")).toBe("mod+s");
    expect(fromCodeMirrorKey("Mod-/")).toBe("mod+/");
    expect(fromCodeMirrorKey("Alt-ArrowUp")).toBe("alt+up");
    expect(fromCodeMirrorKey("Shift-Alt-ArrowDown")).toBe("shift+alt+down");
  });
});

describe("matchSidebarKey", () => {
  it("matches the bare keys App.tsx's navigateFleet dispatches", () => {
    expect(matchSidebarKey(key({ key: "/" }))).toBe("sidebar.filter");
    expect(matchSidebarKey(key({ key: "j" }))).toBe("sidebar.next");
    expect(matchSidebarKey(key({ key: "ArrowDown" }))).toBe("sidebar.next");
    expect(matchSidebarKey(key({ key: "k" }))).toBe("sidebar.prev");
    expect(matchSidebarKey(key({ key: "ArrowUp" }))).toBe("sidebar.prev");
    expect(matchSidebarKey(key({ key: "n" }))).toBe("sidebar.jumpUrgent");
  });

  it("ignores keys with no sidebar meaning", () => {
    expect(matchSidebarKey(key({ key: "x" }))).toBeNull();
  });
});

describe("detectActiveScope", () => {
  it("reports editor when focus is inside .cm-editor", () => {
    const root = document.createElement("div");
    root.className = "cm-editor";
    const input = document.createElement("div");
    root.appendChild(input);
    expect(detectActiveScope(input)).toBe("editor");
  });

  it("reports terminal when focus is inside .xterm", () => {
    const root = document.createElement("div");
    root.className = "xterm";
    const textarea = document.createElement("textarea");
    root.appendChild(textarea);
    expect(detectActiveScope(textarea)).toBe("terminal");
  });

  it("falls back to global otherwise", () => {
    expect(detectActiveScope(document.createElement("div"))).toBe("global");
    expect(detectActiveScope(null)).toBe("global");
  });
});

describe("keyCapParts", () => {
  it("splits a modified chord into individual caps", () => {
    expect(keyCapParts("mod+shift+m", "mac")).toEqual(["⌘", "⇧", "M"]);
    expect(keyCapParts("mod+shift+m", "other")).toEqual(["Ctrl", "Shift", "M"]);
  });

  it("renders a bare local chord with no modifier cap", () => {
    expect(keyCapParts("/", "mac")).toEqual(["/"]);
    expect(keyCapParts("j", "mac")).toEqual(["J"]);
  });

  it("renders alt and arrow keys", () => {
    expect(keyCapParts("alt+up", "mac")).toEqual(["⌥", "↑"]);
    expect(keyCapParts("alt+up", "other")).toEqual(["Alt", "↑"]);
  });
});

describe("matchChord", () => {
  it("ignores unmodified keys so typing is never intercepted", () => {
    expect(matchChord(key({ key: "e" }))).toBeNull();
    expect(matchChord(key({ key: "6" }))).toBeNull();
    expect(matchChord(key({ key: "/" }))).toBeNull();
    expect(matchChord(key({ key: "?", shiftKey: true }))).toBeNull();
  });

  it("requires Cmd on macOS and rejects a Ctrl chord there", () => {
    expect(matchChord(key({ key: "e", metaKey: true }), "mac")?.id).toBe("lane.spawn");
    expect(matchChord(key({ key: "e", ctrlKey: true }), "mac")).toBeNull();
  });

  it("requires Ctrl on non-macOS platforms", () => {
    expect(matchChord(key({ key: "e", ctrlKey: true }), "other")?.id).toBe("lane.spawn");
    expect(matchChord(key({ key: "e", metaKey: true }), "other")).toBeNull();
  });

  it("never lets a Ctrl chord reach a destructive binding on macOS", () => {
    // Ctrl+D sends EOF to a focused terminal. On mac it must not also match lane.delete, or
    // every agent session that reads EOF would also pop a destructive confirm dialog.
    expect(matchChord(key({ key: "d", ctrlKey: true }), "mac")).toBeNull();
  });

  it("distinguishes shifted chords from their unshifted twin", () => {
    expect(matchChord(key({ key: "n", metaKey: true }), "mac")?.id).toBe("fleet.newLane");
    expect(matchChord(key({ key: "N", metaKey: true, shiftKey: true }), "mac")?.id).toBe("fleet.addRepo");
  });

  it("returns null for an unbound chord", () => {
    expect(matchChord(key({ key: "z", metaKey: true }), "mac")).toBeNull();
  });

  it("matches the settings chord, whose ad-hoc listener was removed", () => {
    expect(matchChord(key({ key: ",", metaKey: true }), "mac")?.id).toBe("panel.settings");
  });

  it("matches shifted digit chords, which report a symbol in event.key", () => {
    // Cmd+Shift+1 on a US layout delivers key "!" — only event.code still says Digit1.
    expect(matchChord(key({ key: "!", code: "Digit1", metaKey: true, shiftKey: true }), "mac")?.id).toBe("layout.focused");
    expect(matchChord(key({ key: "@", code: "Digit2", metaKey: true, shiftKey: true }), "mac")?.id).toBe("layout.split");
    // Grid uses mod+shift+0, not mod+shift+3: that chord is the macOS screenshot shortcut.
    expect(matchChord(key({ key: ")", code: "Digit0", metaKey: true, shiftKey: true }), "mac")?.id).toBe("layout.grid");
  });

  it("still matches unshifted digit chords", () => {
    expect(matchChord(key({ key: "1", code: "Digit1", metaKey: true }), "mac")?.id).toBe("panel.git");
    expect(matchChord(key({ key: "3", code: "Digit3", metaKey: true }), "mac")?.id).toBe("panel.usage");
    expect(matchChord(key({ key: "8", code: "Digit8", metaKey: true }), "mac")?.id).toBe("panel.mail");
  });
});

describe("numbered panel chords", () => {
  it("runs left to right along the header toolbar", () => {
    expect(numberedPanelBindings().map((binding) => binding.id)).toEqual([
      "panel.git",
      "panel.editor",
      "panel.usage",
      "panel.control",
      "panel.multitasking",
      "panel.extensions",
      "panel.supervision",
      "panel.mail",
      "panel.repomind",
    ]);
  });

  it("keeps mod+k as a second way into the control center", () => {
    expect(matchChord(key({ key: "k", metaKey: true }), "mac")?.id).toBe("panel.control");
    expect(matchChord(key({ key: "4", code: "Digit4", metaKey: true }), "mac")?.id).toBe(
      "panel.control",
    );
  });

  it("moves theme cycling off the number row", () => {
    expect(matchChord(key({ key: "t", metaKey: true, shiftKey: true }), "mac")?.id).toBe(
      "panel.theme",
    );
    expect(matchChord(key({ key: "6", code: "Digit6", metaKey: true }), "mac")?.id).toBe(
      "panel.extensions",
    );
  });

  it("has no two panels on one number", () => {
    const chords = numberedPanelBindings().map((binding) => binding.chord);
    expect(new Set(chords).size).toBe(chords.length);
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
