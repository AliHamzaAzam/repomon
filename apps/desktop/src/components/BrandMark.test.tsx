import { cleanup, render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import BrandMark from "./BrandMark";

afterEach(() => {
  cleanup();
});

describe("brand mark", () => {
  // Theme tokens keep the mark readable in both light and dark themes.
  it("draws itself from theme tokens rather than fixed colors", () => {
    const { container } = render(() => <BrandMark />);
    const svg = container.querySelector("svg")!;

    expect(svg.innerHTML).toContain("var(--brand-ink)");
    expect(svg.innerHTML).toContain("var(--signal)");
    // Keep brand color independent of attention state so the mark cannot imply a warning.
    expect(svg.innerHTML).not.toContain("var(--attention)");
    expect(svg.innerHTML).not.toMatch(/#[0-9a-f]{3,6}/i);
    expect(svg.innerHTML).not.toContain("rgb(");
  });

  it("renders on a transparent background, no backing rect", () => {
    const { container } = render(() => <BrandMark />);
    const svg = container.querySelector("svg")!;
    expect(svg.innerHTML).not.toContain("var(--background)");
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

  it("scales with the requested size", () => {
    const { container } = render(() => <BrandMark size={48} />);
    const svg = container.querySelector("svg")!;
    expect(svg.getAttribute("width")).toBe("48");
    expect(svg.getAttribute("height")).toBe("48");
  });
});
