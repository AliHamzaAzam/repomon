import { describe, expect, it } from "vitest";

import {
  BINDINGS,
  detectActiveScope,
  findConflicts,
  formatChord,
  fromCodeMirrorKey,
  isMac,
  isWindows,
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

  it("never fires while a platform modifier is held, even if event.key still reads as the bare key", () => {
    // Regression: some browsers report event.key as "/" for Cmd+Shift+/ (the help.open chord)
    // even though a modifier is held. This handler sits on a DOM ancestor of the fleet filter
    // input, ahead of the window-level chord dispatcher, so without this guard it would steal
    // the keystroke and focus the filter instead of ever letting the shortcuts overlay open.
    expect(matchSidebarKey(key({ key: "/", metaKey: true }))).toBeNull();
    expect(matchSidebarKey(key({ key: "/", metaKey: true, shiftKey: true }))).toBeNull();
    expect(matchSidebarKey(key({ key: "/", ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(matchSidebarKey(key({ key: "j", metaKey: true }))).toBeNull();
    expect(matchSidebarKey(key({ key: "n", altKey: true }))).toBeNull();
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

  describe("the Slash key: mod+? vs mod+/", () => {
    // The Slash key is the one place a shifted and unshifted chord are both bound globally
    // (help.open on "?", fleet.filter on "/"). Regression coverage for the bug report: Cmd+? was
    // opening the fleet filter and typing into it instead of the shortcuts overlay.
    it("opens the shortcuts guide for Cmd+Shift+/ when the browser reports key \"?\"", () => {
      expect(
        matchChord(key({ key: "?", code: "Slash", shiftKey: true, metaKey: true }), "mac")?.id,
      ).toBe("help.open");
    });

    it("opens the shortcuts guide for Ctrl+Shift+/ on non-mac when the browser reports key \"?\"", () => {
      expect(
        matchChord(key({ key: "?", code: "Slash", shiftKey: true, ctrlKey: true }), "other")?.id,
      ).toBe("help.open");
    });

    it("also opens the shortcuts guide when the browser suppresses the shift translation and still reports key \"/\"", () => {
      // Real-world quirk: holding a platform modifier can leave event.key at the unshifted
      // character even though event.shiftKey is true and event.code confirms the physical key.
      expect(
        matchChord(key({ key: "/", code: "Slash", shiftKey: true, metaKey: true }), "mac")?.id,
      ).toBe("help.open");
    });

    it("focuses the fleet filter for a plain Cmd+/, never the shortcuts guide", () => {
      const binding = matchChord(key({ key: "/", code: "Slash", metaKey: true }), "mac");
      expect(binding?.id).toBe("fleet.filter");
    });

    it("focuses the fleet filter for a plain Ctrl+/ on non-mac, never the shortcuts guide", () => {
      const binding = matchChord(key({ key: "/", code: "Slash", ctrlKey: true }), "other");
      expect(binding?.id).toBe("fleet.filter");
    });

    it("never lets the shifted chord match the unshifted sibling's id or vice versa", () => {
      const help = matchChord(key({ key: "?", code: "Slash", shiftKey: true, metaKey: true }), "mac");
      const filter = matchChord(key({ key: "/", code: "Slash", metaKey: true }), "mac");
      expect(help?.id).not.toBe(filter?.id);
      expect(help?.id).toBe("help.open");
      expect(filter?.id).toBe("fleet.filter");
    });
  });
});

/// event.key for a shifted US-layout symbol, keyed by the unshifted character. Only the
/// punctuation keys actually bound to a global shifted chord need an entry.
const SHIFTED_SYMBOL: Record<string, string> = { "9": "(", "0": ")", "1": "!", "2": "@" };

/// Build a plausible KeyboardEvent for a registry chord string ("mod+shift+9", "mod+/", "mod+?",
/// "mod+e", ...), on the given platform, either respecting or deliberately flipping the chord's
/// own shift state. Digits always carry their `code` so the digit-vs-symbol ambiguity chordOf
/// resolves via `code` is exercised the same way a real browser would trigger it; the Slash key
/// carries `code: "Slash"` for the same reason.
function eventForChord(chord: string, platform: "mac" | "other", opts: { flipShift?: boolean } = {}): KeyboardEvent {
  const tokens = chord.split("+");
  const base = tokens[tokens.length - 1];
  const wantsShift = tokens.includes("shift") || base === "?";
  const shiftKey = opts.flipShift ? !wantsShift : wantsShift;
  const mod = platform === "mac" ? { metaKey: true } : { ctrlKey: true };

  if (/^\d$/.test(base)) {
    const keyChar = shiftKey ? SHIFTED_SYMBOL[base] ?? base : base;
    return key({ key: keyChar, code: `Digit${base}`, shiftKey, ...mod });
  }
  if (base === "/" || base === "?") {
    // A real "/" chord is never itself written with an explicit "shift+" token, and "?" always
    // implies shift - either way `wantsShift` above already reflects which symbol this is.
    const keyChar = shiftKey ? "?" : "/";
    return key({ key: keyChar, code: "Slash", shiftKey, ...mod });
  }
  if (/^[a-z]$/.test(base)) {
    return key({ key: shiftKey ? base.toUpperCase() : base, shiftKey, ...mod });
  }
  // Punctuation with no registered shifted twin (",", ".", "[", "]"): event.key is stable
  // regardless of the shift flag we're asked to simulate, since there is nothing to flip to.
  return key({ key: base, shiftKey, ...mod });
}

describe("matchChord over the whole registry: a shifted chord never matches its unshifted sibling", () => {
  const globalBindings = BINDINGS.filter((binding) => (binding.scope ?? "global") === "global");

  for (const platform of ["mac", "other"] as const) {
    for (const binding of globalBindings) {
      it(`${binding.id} (${binding.chord}) matches itself on ${platform}, and its shift-flipped twin never matches it`, () => {
        const own = eventForChord(binding.chord, platform);
        expect(matchChord(own, platform)?.id).toBe(binding.id);

        const flipped = eventForChord(binding.chord, platform, { flipShift: true });
        const flippedMatch = matchChord(flipped, platform);
        expect(flippedMatch?.id).not.toBe(binding.id);
      });
    }
  }
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

  it("renders the help chord distinctly from the filter chord on both platforms", () => {
    expect(formatChord("mod+?", "mac")).toBe("⌘?");
    expect(formatChord("mod+?", "other")).toBe("Ctrl+?");
    expect(formatChord("mod+/", "mac")).toBe("⌘/");
    expect(formatChord("mod+/", "other")).toBe("Ctrl+/");
  });
});

describe("isMac / isWindows", () => {
  it("trust an injected platform over navigator sniffing", () => {
    expect(isMac("mac")).toBe(true);
    expect(isMac("windows")).toBe(false);
    expect(isMac("other")).toBe(false);
    expect(isWindows("windows")).toBe(true);
    expect(isWindows("mac")).toBe(false);
    expect(isWindows("other")).toBe(false);
  });

  it("are mutually exclusive for every injected platform", () => {
    for (const platform of ["mac", "windows", "other"]) {
      expect(isMac(platform) && isWindows(platform)).toBe(false);
    }
  });
});
