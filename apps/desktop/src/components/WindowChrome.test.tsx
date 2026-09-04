import { cleanup, render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import WindowChromeHeader, { windowChromeInsetClass } from "./WindowChrome";

const ORIGINAL_PLATFORM = navigator.platform;

afterEach(() => {
  cleanup();
  Object.defineProperty(navigator, "platform", { value: ORIGINAL_PLATFORM, configurable: true });
});

/// The window is frameless with `titleBarStyle: "Overlay"`, so on macOS the system paints the
/// traffic lights over the top-left of the page and anything drawn at the leading edge lands
/// underneath them. The main shell always had this inset; the setup wizard drew its own header
/// without one and put the brand mark under the buttons.
describe("window chrome inset", () => {
  it("leaves room for the macOS traffic lights", () => {
    expect(windowChromeInsetClass("mac")).toBe("pl-[78px]");
  });

  it("uses ordinary symmetric padding on platforms with no overlay buttons", () => {
    expect(windowChromeInsetClass("linux")).toBe("px-3.5");
    expect(windowChromeInsetClass("windows")).toBe("px-3.5");
  });

  it("resolves the platform from the browser when none is passed", () => {
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
    expect(windowChromeInsetClass()).toBe("pl-[78px]");

    Object.defineProperty(navigator, "platform", { value: "Linux x86_64", configurable: true });
    expect(windowChromeInsetClass()).toBe("px-3.5");
  });
});

describe("window chrome header", () => {
  it("applies the inset and stays draggable", () => {
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
    const { container } = render(() => <WindowChromeHeader>title</WindowChromeHeader>);
    const header = container.querySelector("header")!;

    expect(header.className).toContain("pl-[78px]");
    expect(header.className).not.toContain("px-3.5");
    // Without this the frameless window cannot be moved by its title bar at all.
    expect(header.hasAttribute("data-tauri-drag-region")).toBe(true);
  });

  it("drops the inset off macOS", () => {
    Object.defineProperty(navigator, "platform", { value: "Linux x86_64", configurable: true });
    const { container } = render(() => <WindowChromeHeader>title</WindowChromeHeader>);
    const header = container.querySelector("header")!;

    expect(header.className).toContain("px-3.5");
    expect(header.className).not.toContain("pl-[78px]");
  });

  it("appends caller classes after the shared chrome", () => {
    const { container } = render(() => <WindowChromeHeader class="z-50">title</WindowChromeHeader>);
    expect(container.querySelector("header")!.className).toContain("z-50");
  });
});
