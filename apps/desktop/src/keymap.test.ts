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
