import { describe, expect, it } from "vitest";

import { translateKeyboardKey } from "./term";

function key(value: string, modifiers: Partial<KeyboardEvent> = {}): KeyboardEvent {
  return new KeyboardEvent("keydown", { key: value, ...modifiers });
}

describe("terminal key translation", () => {
  it("lets xterm collect printable runs", () => {
    expect(translateKeyboardKey(key("a"))).toBeNull();
  });

  it("matches tmux control and navigation syntax", () => {
    expect(translateKeyboardKey(key("o", { ctrlKey: true }))).toEqual({ key: "C-o", literal: false });
    expect(translateKeyboardKey(key("ArrowLeft", { altKey: true }))).toEqual({ key: "M-Left", literal: false });
    expect(translateKeyboardKey(key("Tab", { shiftKey: true }))).toEqual({ key: "BTab", literal: false });
    expect(translateKeyboardKey(key("Escape"))).toEqual({ key: "Escape", literal: false });
  });
});
