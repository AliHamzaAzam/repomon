import { describe, expect, it } from "vitest";

import {
  DEFAULT_TERMINAL_APPEARANCE,
  TERMINAL_CSS_VAR_TOKENS,
  TERMINAL_FONT_FALLBACK_STACK,
  terminalFontFamily,
  terminalSurfaceStyle,
  type TerminalThemeTokens,
} from "./theme";

const tokens: TerminalThemeTokens = {
  background: "rgb(10, 12, 16)",
  foreground: "rgb(230, 232, 236)",
  signal: "rgb(100, 196, 187)",
};

describe("terminalFontFamily", () => {
  it("quotes the chosen family and appends the shared fallback stack", () => {
    expect(terminalFontFamily("Berkeley Mono")).toBe(`"Berkeley Mono", ${TERMINAL_FONT_FALLBACK_STACK}`);
  });

  it("falls back the same way for every configurable family, including ones with no spaces", () => {
    expect(terminalFontFamily("monospace")).toBe(`"monospace", ${TERMINAL_FONT_FALLBACK_STACK}`);
  });
});

describe("terminalSurfaceStyle", () => {
  it("uses the plain background token untouched when tint is off", () => {
    const surface = terminalSurfaceStyle({ ...DEFAULT_TERMINAL_APPEARANCE, tintEnabled: false }, tokens);
    expect(surface.background).toBe(tokens.background);
  });

  it("blends in the signal token at the configured percentage when tint is on", () => {
    const surface = terminalSurfaceStyle(
      { ...DEFAULT_TERMINAL_APPEARANCE, tintEnabled: true, tintOpacity: 0.08 },
      tokens,
    );
    expect(surface.background).toBe(`color-mix(in srgb, ${tokens.signal} 8%, ${tokens.background})`);
  });

  it.each([
    [0.01, 1],
    [0.02, 2],
    [0.15, 15],
    [0.3, 30],
  ])("rounds a tint opacity of %s to a %s%% color-mix stop", (tintOpacity, pct) => {
    const surface = terminalSurfaceStyle({ ...DEFAULT_TERMINAL_APPEARANCE, tintEnabled: true, tintOpacity }, tokens);
    expect(surface.background).toBe(`color-mix(in srgb, ${tokens.signal} ${pct}%, ${tokens.background})`);
  });

  it("carries the foreground and signal tokens through unchanged", () => {
    const surface = terminalSurfaceStyle(DEFAULT_TERMINAL_APPEARANCE, tokens);
    expect(surface.foreground).toBe(tokens.foreground);
    expect(surface.accent).toBe(tokens.signal);
  });

  it("builds the font family with the shared fallback stack and passes font size through", () => {
    const surface = terminalSurfaceStyle({ ...DEFAULT_TERMINAL_APPEARANCE, fontFamily: "JetBrains Mono", fontSize: 14 }, tokens);
    expect(surface.fontFamily).toBe(`"JetBrains Mono", ${TERMINAL_FONT_FALLBACK_STACK}`);
    expect(surface.fontSize).toBe(14);
  });

  it("accepts the CSS-var tokens the Settings preview renders with, leaving the var() references intact", () => {
    const surface = terminalSurfaceStyle(
      { ...DEFAULT_TERMINAL_APPEARANCE, tintEnabled: true, tintOpacity: 0.02 },
      TERMINAL_CSS_VAR_TOKENS,
    );
    expect(surface.background).toBe("color-mix(in srgb, var(--signal) 2%, var(--background))");
  });
});
