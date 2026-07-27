import { cleanup, render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import BrandMark from "./BrandMark";

afterEach(() => {
  cleanup();
});

describe("brand mark", () => {
  // The point of drawing the mark from tokens is that it re-shades with the theme and the accent.
  // A hardcoded hex would look right in dark mode and wrong in light, which is easy to miss.
  it("draws itself from theme tokens rather than fixed colors", () => {
    const { container } = render(() => <BrandMark />);
    const svg = container.querySelector("svg")!;

    expect(svg.innerHTML).toContain("var(--background)");
    expect(svg.innerHTML).toContain("var(--signal)");
    expect(svg.innerHTML).toContain("var(--attention)");
    expect(svg.innerHTML).not.toMatch(/#[0-9a-f]{3,6}/i);
    expect(svg.innerHTML).not.toContain("rgb(");
  });

  it("is decorative unless given a title", () => {
    const { container, unmount } = render(() => <BrandMark />);
    expect(container.querySelector("svg")!.getAttribute("aria-hidden")).toBe("true");
    unmount();

    const titled = render(() => <BrandMark title="Repomon" />);
    const svg = titled.container.querySelector("svg")!;
    expect(svg.getAttribute("role")).toBe("img");
    expect(svg.getAttribute("aria-label")).toBe("Repomon");
    expect(svg.getAttribute("aria-hidden")).toBeNull();
  });

  it("scales its corner radius with the requested size", () => {
    const { container } = render(() => <BrandMark size={48} />);
    const svg = container.querySelector("svg")!;
    expect(svg.getAttribute("width")).toBe("48");
    expect(svg.style.borderRadius).toBe("11px");
  });
});
