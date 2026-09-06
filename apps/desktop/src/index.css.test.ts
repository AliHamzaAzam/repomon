import { readFileSync } from "node:fs";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

/// Check the shipped cascade preserves measured multitasking minimums even with only one or two
/// selected panes.
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

/// The responsive rules are media queries, which jsdom parses into the CSSOM but never applies to
/// a layout (there is none). So these assert the rules as written: the four breakpoints exist
/// with the widths the brief names, and each carries the rule the components rely on.
describe("index.css breakpoints", () => {
  const cssPath = path.resolve(process.cwd(), "src/index.css");
  const raw = readFileSync(cssPath, "utf-8").split("\n").filter((line) => !line.trim().startsWith("@import")).join("\n");

  function mediaRules(): Map<string, string> {
    const style = document.createElement("style");
    style.textContent = raw;
    document.head.appendChild(style);
    const rules = new Map<string, string>();
    for (const rule of [...(style.sheet?.cssRules ?? [])]) {
      if (rule instanceof CSSMediaRule) {
        rules.set(rule.media.mediaText, [...rule.cssRules].map((inner) => inner.cssText).join("\n"));
      }
    }
    style.remove();
    return rules;
  }

  it("declares exactly the four named breakpoints plus reduced motion", () => {
    const rules = mediaRules();
    expect([...rules.keys()].sort()).toEqual(
      ["(max-height: 720px)", "(max-width: 1100px)", "(max-width: 1280px)", "(min-width: 1600px)", "(prefers-reduced-motion: reduce)"].sort(),
    );
  });

  it("collapses the header toolbar to icons at medium", () => {
    const medium = mediaRules().get("(max-width: 1280px)") ?? "";
    expect(medium).toContain(".header-toolbar .toolbar-label");
    expect(medium).toContain("display: none");
  });

  it("caps the right rail to 40vw and narrows the sidebar at narrow", () => {
    const narrow = mediaRules().get("(max-width: 1100px)") ?? "";
    expect(narrow).toContain("min(var(--right-panel-width, 20rem), 40vw)");
    expect(narrow).toContain("15rem minmax(0, 1fr)");

    expect(narrow).not.toContain("display: none");
  });

  it("lets overlays and the wizard use more of a short window", () => {
    const short = mediaRules().get("(max-height: 720px)") ?? "";
    expect(short).toContain(".modal-card");
    expect(short).toContain(".onboarding-page");
  });

  it("gives panel headers one shrinkable lead and one fixed action group", () => {
    const style = document.createElement("style");
    style.textContent = raw;
    document.head.appendChild(style);
    const header = document.createElement("div");
    header.className = "panel-header";
    const lead = document.createElement("div");
    lead.className = "panel-header-lead";
    const actions = document.createElement("div");
    actions.className = "panel-header-actions";
    header.append(lead, actions);
    document.body.appendChild(header);
    expect(getComputedStyle(header).minWidth).toBe("0");
    expect(getComputedStyle(lead).minWidth).toBe("0");
    expect(getComputedStyle(lead).flexShrink).toBe("1");
    expect(getComputedStyle(actions).flexShrink).toBe("0");
    header.remove();
    style.remove();
  });
});
