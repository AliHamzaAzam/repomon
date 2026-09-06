import { cleanup, fireEvent, render, waitFor } from "@solidjs/testing-library";
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

/// On Windows the builds turn native decorations off (one bar, not a native title bar stacked on
/// the app's own), so the header has to draw the caption controls itself. They exist only there:
/// macOS has its traffic lights and Linux keeps its native decorations.
describe("window caption controls", () => {
  function fakeControls(maximized = false) {
    const calls: string[] = [];
    let onResize: (() => void) | undefined;
    const api = {
      minimize: async () => { calls.push("minimize"); },
      toggleMaximize: async () => { calls.push("toggleMaximize"); maximized = !maximized; onResize?.(); },
      close: async () => { calls.push("close"); },
      isMaximized: async () => maximized,
      onResized: async (handler: () => void) => { onResize = handler; return () => { onResize = undefined; }; },
    };
    return { api, calls };
  }

  it("draws minimize, maximize and close only on Windows", () => {
    const { api } = fakeControls();
    const windows = render(() => <WindowChromeHeader platform="windows" windowControls={api}>title</WindowChromeHeader>);
    expect(windows.getByRole("button", { name: "Minimize" })).toBeInTheDocument();
    expect(windows.getByRole("button", { name: "Maximize" })).toBeInTheDocument();
    expect(windows.getByRole("button", { name: "Close" })).toBeInTheDocument();
    cleanup();

    for (const platform of ["mac", "linux"]) {
      const other = render(() => <WindowChromeHeader platform={platform} windowControls={api}>title</WindowChromeHeader>);
      expect(other.queryByRole("button", { name: "Close" })).not.toBeInTheDocument();
      cleanup();
    }
  });

  it("keeps the drag region on the header and off every button", () => {
    const { api } = fakeControls();
    const { container } = render(() => (
      <WindowChromeHeader platform="windows" windowControls={api}>
        <button type="button">Skip setup</button>
      </WindowChromeHeader>
    ));
    expect(container.querySelector("header")!.hasAttribute("data-tauri-drag-region")).toBe(true);
    for (const button of container.querySelectorAll("button")) {
      expect(button.hasAttribute("data-tauri-drag-region")).toBe(false);
    }
  });

  it("wires each control to the window", async () => {
    const { api, calls } = fakeControls();
    const { getByRole } = render(() => <WindowChromeHeader platform="windows" windowControls={api}>title</WindowChromeHeader>);
    fireEvent.click(getByRole("button", { name: "Minimize" }));
    fireEvent.click(getByRole("button", { name: "Maximize" }));
    fireEvent.click(getByRole("button", { name: "Close" }));
    await waitFor(() => expect(calls).toEqual(["minimize", "toggleMaximize", "close"]));
  });

  it("offers Restore once the window is maximized, and Maximize again after", async () => {
    const { api } = fakeControls(true);
    const { getByRole, findByRole } = render(() => <WindowChromeHeader platform="windows" windowControls={api}>title</WindowChromeHeader>);
    const restore = await findByRole("button", { name: "Restore" });
    fireEvent.click(restore);
    await findByRole("button", { name: "Maximize" });
    expect(getByRole("button", { name: "Maximize" })).toBeInTheDocument();
  });
});
