import { readFileSync } from "node:fs";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

/// Regression coverage for the multitasking-grid CSS specificity bug (bug 3 in the
/// fix/multitasking-layout-bugs report): `.terminal-layout.is-grid.count-1` /
/// `.terminal-layout.is-grid.count-2` (3 classes, added so a 1-2 pane non-multitasking grid
/// fills the bay instead of sitting at its auto-row floor) has higher specificity than
/// `.terminal-layout.is-multitasking` (2 classes) and — before the `:not(.is-multitasking)`
/// guard — silently won for a *multitasking* session with only 1-2 panes selected too, since
/// `effectiveLayout()` reports "grid" for multitasking regardless of pane count. That collapsed
/// the measured `grid-auto-rows` floor down to `minmax(0, 1fr)` for that row,
/// letting it compress toward zero height and clip the composer at the bottom of the terminal.
///
/// This loads the actual shipped stylesheet into jsdom and asks the real cascade/specificity
/// engine to resolve it — not a hand-rolled specificity calculation — so it breaks if the
/// selector guard (or the shared measured-height variable) ever regresses.
describe("index.css multitasking grid row sizing", () => {
  let style: HTMLStyleElement;

  beforeAll(() => {
    const cssPath = path.resolve(process.cwd(), "src/index.css");
    const raw = readFileSync(cssPath, "utf-8");
    // jsdom's CSS engine doesn't fetch `@import` targets (tailwindcss, xterm's stylesheet); they
    // aren't relevant to the rules under test, so strip them rather than let a failed resolution
    // risk dropping the whole sheet.
    const css = raw
      .split("\n")
      .filter((line) => !line.trim().startsWith("@import"))
      .join("\n");
    style = document.createElement("style");
    style.textContent = css;
    document.head.appendChild(style);
  });

  afterAll(() => {
    style.remove();
  });

  function elementWithClasses(className: string): HTMLDivElement {
    const el = document.createElement("div");
    el.className = className;
    document.body.appendChild(el);
    return el;
  }

  it("does not let count-1/count-2 override the multitasking row-height floor", () => {
    for (const count of [1, 2]) {
      const el = elementWithClasses(`terminal-layout is-grid count-${count} is-multitasking`);
      const computed = getComputedStyle(el);
      // The count-1/2 shrink-to-fit rule must not apply here: no explicit grid-template-rows.
      expect(computed.gridTemplateRows).toBe("");
      // So the multitasking auto-row minimum is what actually governs row height.
      expect(computed.gridAutoRows).toBe("minmax(var(--multitask-row-min-height, 14rem), 1fr)");
      el.remove();
    }
  });

  it("still lets non-multitasking grid fill the bay at 1-2 panes (behavior preserved)", () => {
    for (const count of [1, 2]) {
      const el = elementWithClasses(`terminal-layout is-grid count-${count}`);
      expect(getComputedStyle(el).gridTemplateRows).toBe("minmax(0, 1fr)");
      el.remove();
    }
  });

  it("leaves 3+ pane grids on the multitasking auto-row minimum either way", () => {
    const el = elementWithClasses("terminal-layout is-grid count-4 is-multitasking");
    expect(getComputedStyle(el).gridTemplateRows).toBe("");
    expect(getComputedStyle(el).gridAutoRows).toBe("minmax(var(--multitask-row-min-height, 14rem), 1fr)");
    el.remove();
  });

  it("gives .multitask-pane a real min-height floor independent of grid track sizing", () => {
    const el = elementWithClasses("multitask-pane");
    const computed = getComputedStyle(el);
    expect(computed.minHeight).toBe("var(--multitask-row-min-height, 14rem)");
    // No second overflow:hidden clip layered on top of TerminalPane's own root clip.
    expect(computed.overflow).toBe("");
    // Each pane gets its own stacking context so a sibling's header/tooltip/canvas can never
    // paint over it regardless of z-index bleed.
    expect(computed.isolation).toBe("isolate");
    el.remove();
  });

  it("takes warmed multitasking panes completely out of grid flow", () => {
    const grid = elementWithClasses("terminal-layout is-grid is-multitasking");
    const pane = document.createElement("div");
    pane.className = "warm-terminal-hidden";
    grid.appendChild(pane);

    const computed = getComputedStyle(pane);
    expect(computed.position).toBe("absolute");
    expect(computed.visibility).toBe("hidden");
    expect(getComputedStyle(grid).gridAutoFlow).toBe("row");
    grid.remove();
  });
});
