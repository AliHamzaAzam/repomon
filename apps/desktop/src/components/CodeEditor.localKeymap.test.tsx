import { describe, expect, it } from "vitest";

import { BINDINGS, fromCodeMirrorKey } from "../keymap";
import { EDITOR_LOCAL_KEYMAP } from "./CodeEditor";

describe("EDITOR_LOCAL_KEYMAP", () => {
  const editorBindings = BINDINGS.filter((binding) => binding.scope === "editor");

  it("has exactly one keymap.ts entry per local CodeMirror binding", () => {
    const guideChords = editorBindings.map((binding) => binding.chord).sort();
    const actualChords = EDITOR_LOCAL_KEYMAP.map((entry) => fromCodeMirrorKey(entry.key)).sort();
    expect(guideChords).toEqual(actualChords);
  });

  it("describes each binding with the same label the editor actually uses", () => {
    for (const entry of EDITOR_LOCAL_KEYMAP) {
      const chord = fromCodeMirrorKey(entry.key);
      const guideEntry = editorBindings.find((binding) => binding.chord === chord);
      expect(guideEntry, `keymap.ts is missing an "editor" scope entry for ${entry.key} (${chord})`).toBeDefined();
      expect(guideEntry?.label).toBe(entry.label);
    }
  });
});
