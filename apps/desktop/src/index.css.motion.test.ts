import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

/// The motion vocabulary is the contract: every transition in the sheet names a duration/easing
/// pair, so a future edit cannot quietly reintroduce a one-off millisecond literal.
describe("index.css motion vocabulary", () => {
  const raw = readFileSync(path.resolve(process.cwd(), "src/index.css"), "utf-8");

  it("defines a duration and an easing for each named motion family", () => {
    for (const family of ["tap", "snap", "play"]) {
      expect(raw).toMatch(new RegExp(`--motion-${family}:\\s*\\d+ms;`));
      expect(raw).toMatch(new RegExp(`--ease-${family}:\\s*(cubic-bezier|linear)\\(`));
    }
  });

  it("falls back to a real curve on a webview without linear()", () => {
    const guarded = raw.match(/@supports \(transition-timing-function: linear\(0, 1\)\) \{[\s\S]*?\n\}/)?.[0] ?? "";
    expect(guarded).toContain("--ease-snap: linear(");
    expect(guarded).toContain("--ease-play: linear(");
    // The unguarded definitions have to stand alone, or an old webview drops the declaration and
    // the transition with it.
    const base = raw.slice(0, raw.indexOf("@supports"));
    expect(base).toContain("--ease-snap: cubic-bezier(");
    expect(base).toContain("--ease-play: cubic-bezier(");
  });

  it("animates named properties only, never the layout-inviting all", () => {
    const transitions = raw.match(/^\s*transition:[^;]+;/gm) ?? [];
    expect(transitions.length).toBeGreaterThan(0);
    for (const declaration of transitions) {
      expect(declaration).not.toMatch(/transition:\s*all\b/);
      expect(declaration).toMatch(/var\(--motion-(tap|snap|play)\)/);
      expect(declaration).toMatch(/var\(--ease-(tap|snap|play)\)/);
      expect(declaration).not.toMatch(/\d+ms/);
    }
  });

  it("drops the gooey filter under reduced motion instead of rastering a shape that cannot move", () => {
    const block = raw.match(/@media \(prefers-reduced-motion: reduce\) \{[\s\S]*?\n\}\n/)?.[0] ?? "";
    expect(block).toContain("transition-duration: 0.01ms !important");
    expect(block).toMatch(/\.view-toggle__liquid \{\s*filter: none;/);
  });

  it("moves the switch with transforms rather than with its box", () => {
    const blob = raw.match(/\.view-toggle__blob \{[^}]*\}/)?.[0] ?? "";
    expect(blob).toContain("transform: translate3d(var(--view-toggle-travel");
    expect(blob).toMatch(/transition: transform var\(--motion-snap\) var\(--ease-snap\);/);
    const follower = raw.match(/\.view-toggle__blob\.is-follower \{[^}]*\}/)?.[0] ?? "";
    expect(follower).toMatch(/transition: transform var\(--motion-play\) var\(--ease-play\);/);
  });
});

/// The filter primitives live in the document, not the component: a filter id has to be unique.
describe("index.html gooey filter", () => {
  const html = readFileSync(path.resolve(process.cwd(), "index.html"), "utf-8");

  it("ships the blur and alpha ramp the liquid switch resolves by id", () => {
    expect(html).toContain('id="repomon-liquid"');
    expect(html).toContain('color-interpolation-filters="sRGB"');
    expect(html).toMatch(/<feGaussianBlur in="SourceGraphic" stdDeviation="5" result="smear" \/>/);
    expect(html).toContain('values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 20 -10"');
  });
});
