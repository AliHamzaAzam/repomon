import { describe, expect, it } from "vitest";

import { stripAnsi, trimBlankEdges } from "./ansi";

describe("stripAnsi", () => {
  it("drops colour and style sequences", () => {
    expect(stripAnsi("\x1b[93mAccessing\x1b[0m \x1b[1mworkspace\x1b[0m")).toBe("Accessing workspace");
  });

  // The exact shape from a Claude Code trust prompt, which is what made the panel unreadable.
  it("keeps the option text of a menu intact", () => {
    const pane = "  \x1b[94m❯\x1b[39m \x1b[37m1.\x1b[39m \x1b[94mYes,\x1b[39m \x1b[94mI\x1b[39m \x1b[94mtrust\x1b[39m";
    expect(stripAnsi(pane)).toBe("  ❯ 1. Yes, I trust");
  });

  // OSC 8 wraps a URL in two escapes; the visible label sits between them and must survive.
  it("unwraps an OSC 8 hyperlink but keeps its label", () => {
    const link = "\x1b]8;id=zaxmda;https://code.claude.com/docs/en/security\x1b\\Security guide\x1b]8;;\x1b\\";
    expect(stripAnsi(link)).toBe("Security guide");
  });

  it("handles a BEL-terminated OSC as well as an ST-terminated one", () => {
    expect(stripAnsi("\x1b]0;window title\x07done")).toBe("done");
  });

  it("leaves ordinary text, newlines, and box drawing alone", () => {
    const plain = "line one\nline two\n────────";
    expect(stripAnsi(plain)).toBe(plain);
  });

  it("does not hang on a truncated escape at the end of a capture", () => {
    // Captures are snapshots and can cut mid-sequence.
    expect(stripAnsi("text\x1b[")).toBe("text");
    expect(stripAnsi("text\x1b")).toBe("text");
    expect(stripAnsi("text\x1b]8;id=x")).toBe("text");
  });
});

describe("trimBlankEdges", () => {
  it("drops the empty rows a full-pane capture pads with", () => {
    expect(trimBlankEdges("\n\n  hello\n\n\n")).toBe("  hello");
  });

  // Interior spacing is the agent's own formatting; reflowing it would change what it drew.
  it("keeps blank lines inside the output", () => {
    expect(trimBlankEdges("\na\n\n\nb\n")).toBe("a\n\n\nb");
  });

  it("handles an all-blank capture without inventing content", () => {
    expect(trimBlankEdges("\n   \n\n")).toBe("");
  });
});
