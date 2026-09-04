import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import BrandLockup from "./BrandLockup";

afterEach(cleanup);

describe("brand lockup", () => {
  it("sets the mark and the wordmark as one unit", () => {
    const { container } = render(() => <BrandLockup />);
    const lockup = container.querySelector("[data-brand-lockup]")!;

    expect(lockup.querySelector("svg")).toBeInTheDocument();
    expect(lockup.textContent).toBe("Repomon");
  });

  // The capitals are a CSS treatment, not the content: assistive technology should read the
  // product's name, not an initialism.
  it("keeps the readable name in the DOM and puts the capitals in CSS", () => {
    const { container } = render(() => <BrandLockup />);
    const wordmark = container.querySelector("[data-brand-lockup] span:last-child")!;

    expect(wordmark.textContent).toBe("Repomon");
    expect(wordmark.className).toContain("uppercase");
  });

  // The mark is cropped to its own ink here rather than drawn on the app icon's padded canvas,
  // which is what keeps the bars above a pixel at title-bar sizes.
  it("crops the mark to the glyph so it stays crisp small", () => {
    const { container } = render(() => <BrandLockup />);
    const svg = container.querySelector("svg")!;

    expect(svg.getAttribute("viewBox")).toBe("283.5 280 688 688");
    expect(svg.getAttribute("width")).toBe("16");
  });

  it("draws itself from theme tokens, with no fixed colors", () => {
    const { container } = render(() => <BrandLockup />);
    const lockup = container.querySelector("[data-brand-lockup]")!;

    expect(lockup.innerHTML).toContain("var(--signal)");
    expect(lockup.innerHTML).not.toMatch(/#[0-9a-f]{3,6}/i);
    expect(lockup.innerHTML).not.toContain("rgb(");
  });

  it("is the page heading only where the caller asks for one", () => {
    const { container, unmount } = render(() => <BrandLockup heading />);
    expect(container.querySelector("h1")!.textContent).toBe("Repomon");
    unmount();

    const plain = render(() => <BrandLockup />);
    expect(plain.container.querySelector("h1")).toBeNull();
  });

  // A lockup with nowhere to go is text. Rendering it as a button anyway would give it a hover
  // state and a pointer cursor that promise something the click does not deliver.
  it("is inert text when there is nothing to open", () => {
    const { container } = render(() => <BrandLockup />);
    const lockup = container.querySelector("[data-brand-lockup]")!;

    expect(lockup.tagName).toBe("DIV");
    expect(lockup.className).not.toContain("hover:");
    expect(lockup.getAttribute("aria-label")).toBeNull();
  });

  it("becomes a labelled button with a hover state when given somewhere to go", () => {
    const onActivate = vi.fn();
    render(() => <BrandLockup onActivate={onActivate} activateLabel="About Repomon" />);

    const button = screen.getByRole("button", { name: "About Repomon" });
    expect(button.className).toContain("hover:bg-line/40");

    fireEvent.click(button);
    expect(onActivate).toHaveBeenCalledTimes(1);
  });
});
