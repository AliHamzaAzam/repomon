import { describe, expect, it } from "vitest";

import { BINDINGS } from "./keymap";
import { buildPrintableShortcutsHtml, shortcutsHtmlDataUrl } from "./shortcutsPrint";

describe("buildPrintableShortcutsHtml", () => {
  it("lists every binding's label at least once", () => {
    const html = buildPrintableShortcutsHtml();
    for (const binding of BINDINGS) {
      expect(html).toContain(binding.label.replace(/&/g, "&amp;"));
    }
  });

  it("renders mac and other-platform chords differently", () => {
    const mac = buildPrintableShortcutsHtml(BINDINGS, "mac");
    const other = buildPrintableShortcutsHtml(BINDINGS, "other");
    expect(mac).toContain("⌘");
    expect(mac).not.toContain("Ctrl+");
    expect(other).toContain("Ctrl+");
    expect(other).not.toContain("⌘");
  });

  it("is a self-contained document with no external resources", () => {
    const html = buildPrintableShortcutsHtml();
    expect(html).not.toMatch(/https?:\/\//);
    expect(html.startsWith("<!doctype html>")).toBe(true);
  });
});

describe("shortcutsHtmlDataUrl", () => {
  it("produces a base64 text/html data URL", () => {
    const url = shortcutsHtmlDataUrl();
    expect(url.startsWith("data:text/html;base64,")).toBe(true);
  });

  it("round-trips back to the same HTML", () => {
    const html = buildPrintableShortcutsHtml();
    const url = shortcutsHtmlDataUrl();
    const encoded = url.slice("data:text/html;base64,".length);
    const decoded = Buffer.from(encoded, "base64").toString("utf8");
    expect(decoded).toBe(html);
  });
});
