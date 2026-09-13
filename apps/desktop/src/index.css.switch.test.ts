import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

/// The Switch's knob is seated by arithmetic, not by eye: the track is the only geometry declared
/// and the knob and its travel are calc()'d from it. These tests resolve those declarations the
/// way a browser would and check that the clearance closes on all four sides in both states, so
/// the knob can never drift back into contact with the rounded end of its track.

const ROOT_FONT_SIZE = 16;

/// A calculator for the small arithmetic the sheet actually uses: `+ - * /`, parentheses, and
/// px/rem lengths. Deliberately not a general CSS parser - it is here to refuse anything it does
/// not fully understand rather than to guess at it.
function evaluate(expression: string): number {
  const tokens = expression.match(/[\d.]+(?:px|rem)?|[+\-*/()]/g) ?? [];
  let cursor = 0;

  const peek = () => tokens[cursor];

  const factor = (): number => {
    const token = tokens[cursor++];
    if (token === "(") {
      const value = expr();
      if (tokens[cursor++] !== ")") throw new Error(`unbalanced parentheses in ${expression}`);
      return value;
    }
    const length = token?.match(/^([\d.]+)(px|rem)?$/);
    if (!length) throw new Error(`unreadable token ${token} in ${expression}`);
    return Number(length[1]) * (length[2] === "rem" ? ROOT_FONT_SIZE : 1);
  };

  const term = (): number => {
    let value = factor();
    while (peek() === "*" || peek() === "/") {
      value = tokens[cursor++] === "*" ? value * factor() : value / factor();
    }
    return value;
  };

  const expr = (): number => {
    let value = term();
    while (peek() === "+" || peek() === "-") {
      value = tokens[cursor++] === "+" ? value + term() : value - term();
    }
    return value;
  };

  const value = expr();
  if (cursor !== tokens.length) throw new Error(`trailing input in ${expression}`);
  return value;
}

describe("Switch geometry in index.css", () => {
  const raw = readFileSync(path.resolve(process.cwd(), "src/index.css"), "utf-8");
  const track = raw.match(/\n\.switch-track \{([^}]*)\}/)?.[1] ?? "";
  const knobRule = raw.match(/\n\.switch-knob \{([^}]*)\}/)?.[1] ?? "";
  const checkedRule = raw.match(/\n\.switch-track\[aria-checked="true"\] \.switch-knob \{([^}]*)\}/)?.[1] ?? "";

  const declared = new Map(
    [...track.matchAll(/(--switch-[a-z-]+):\s*([^;]+);/g)].map(([, name, value]) => [name, value.trim()]),
  );

  /// Resolve a custom property the way the cascade would: substitute the var()s it names, drop
  /// the calc() wrapper, and work the arithmetic out.
  const resolve = (name: string, seen: string[] = []): number => {
    if (seen.includes(name)) throw new Error(`${name} refers to itself`);
    const value = declared.get(name);
    if (value === undefined) throw new Error(`${name} is not declared on .switch-track`);
    const substituted = value.replace(/var\((--[a-z-]+)\)/g, (_, referenced: string) =>
      String(resolve(referenced, [...seen, name])),
    );
    return evaluate(substituted.replace(/calc\(/g, "("));
  };

  const border = () => resolve("--switch-border");
  const inset = () => resolve("--switch-inset");
  const width = () => resolve("--switch-track-width");
  const height = () => resolve("--switch-track-height");
  const knob = () => resolve("--switch-knob");
  const travel = () => resolve("--switch-travel");

  it("derives the knob and the travel from the track rather than restating them", () => {
    expect(track).toContain("width: var(--switch-track-width)");
    expect(track).toContain("height: var(--switch-track-height)");
    expect(declared.get("--switch-knob")).toContain("calc(");
    expect(declared.get("--switch-travel")).toContain("calc(");
    // The knob is the track less its border and one inset per side, in both axes.
    expect(knob()).toBe(height() - 2 * border() - 2 * inset());
    expect(travel()).toBe(width() - 2 * border() - 2 * inset() - knob());
    // The incumbent 36x20 track, for the record: a 14px knob travelling 16px.
    expect([width(), height()]).toEqual([36, 20]);
    expect(knob()).toBe(14);
    expect(travel()).toBe(16);
  });

  it("seats the knob with equal clearance on all four sides in both states", () => {
    // Measured from the track's outer edge. The knob is absolutely positioned inside the border,
    // so its own offsets are the border plus the inset.
    const top = border() + inset();
    const bottom = height() - border() - inset() - knob();
    const leftWhenOff = border() + inset();
    const rightWhenOff = width() - border() - inset() - knob();
    const leftWhenOn = border() + inset() + travel();
    const rightWhenOn = width() - border() - inset() - travel() - knob();

    expect(top).toBe(bottom);
    expect(leftWhenOff).toBe(top);
    expect(rightWhenOn).toBe(top);
    // Before this fix the on state left 1px here against 3px everywhere else, and the knob read
    // as bulging out of the 999px-rounded end it was touching.
    expect(rightWhenOn).toBeGreaterThan(border());
    // The two states are mirror images of one another.
    expect(leftWhenOn).toBe(rightWhenOff);
    expect(new Set([top, bottom, leftWhenOff, rightWhenOn])).toEqual(new Set([3]));
  });

  it("keeps the knob inside a track that stays a stadium at both ends", () => {
    expect(track).toContain("border-radius: 999px");
    expect(knobRule).toContain("border-radius: 999px");
    // A knob that never reaches the curve does not need to be clipped by one.
    expect(track).not.toContain("overflow");
  });

  it("moves the knob on the tap step rather than on either spring", () => {
    expect(knobRule).toMatch(/transition: transform var\(--motion-tap\) var\(--ease-tap\);/);
    expect(knobRule).not.toMatch(/--motion-(snap|play)/);
    expect(track).toMatch(/transition: background-color var\(--motion-tap\) var\(--ease-tap\)/);
    // The gooey filter belongs to the segmented view toggle alone.
    expect(track + knobRule + checkedRule).not.toContain("filter");
  });

  it("drives the travel off the control's own aria state, not a second class", () => {
    expect(checkedRule).toContain("transform: translate3d(var(--switch-travel), -50%, 0)");
  });
});

describe("Switch component geometry", () => {
  const source = readFileSync(path.resolve(process.cwd(), "src/components/controls/Switch.tsx"), "utf-8");

  it("leaves every switch dimension and duration to the sheet", () => {
    expect(source).toContain("switch-track");
    expect(source).toContain("switch-knob");
    // No literal size, offset, travel or millisecond may return to the markup: those are exactly
    // the values that fell out of step with each other.
    expect(source).not.toMatch(/translate-x-\[/);
    expect(source).not.toMatch(/\bh-\d|\bw-\d/);
    expect(source).not.toMatch(/duration-\d+/);
    expect(source).not.toMatch(/ease-in-out/);
  });
});
