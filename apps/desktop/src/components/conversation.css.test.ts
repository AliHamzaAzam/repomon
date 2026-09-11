import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

// Round 7 item 6: the you/time label must sit as far from the message body as the agent/time
// label does, not just share the same grid gap. Both gutter columns stretch to fill their full
// track width, so a shared "text-align:left" put codex's label flush to the outer margin (a big
// visual gap) while you's label sat flush to the inner gap (a small one) - see
// qa/chat-ui-finish-report.md for the measured before/after pixel gaps.
describe("conversation.css gutter label alignment (round 7 item 6)", () => {
  const raw = readFileSync(path.resolve(process.cwd(), "src/components/conversation.css"), "utf-8");
  const assistantRule = raw.match(/\.conversation-gutter\s*{[^}]*}/)?.[0] ?? "";
  const userRule = raw.match(/\.conversation-user \.conversation-gutter\s*{[^}]*}/)?.[0] ?? "";

  it("leaves the assistant gutter at its initial (start/left) alignment", () => {
    expect(assistantRule).not.toMatch(/text-align/);
  });

  it("flushes the user gutter to its outer edge, mirroring the assistant side", () => {
    expect(userRule).toContain("grid-column:3");
    expect(userRule).toContain("text-align:right");
  });
});

// DESIGN.md's Layout section pins the gutter/gap/edge tokens at each container breakpoint; this
// guards the CSS from drifting out of sync with that documented table.
describe("conversation.css gutter tokens at each documented breakpoint", () => {
  const raw = readFileSync(path.resolve(process.cwd(), "src/components/conversation.css"), "utf-8");
  const base = raw.match(/\.conversation-main\s*{[^}]*}/)?.[0] ?? "";
  const at800 = raw.match(/@container \(max-width:800px\) {\s*\.conversation-main\s*{[^}]*}/)?.[0] ?? "";
  const at480 = raw.match(/@container \(max-width:480px\) {\s*\.conversation-main\s*{[^}]*}/)?.[0] ?? "";

  it("uses the base 68px gutter, 16px gap, and 24px edge above 800px", () => {
    expect(base).toContain("--conversation-gutter:68px");
    expect(base).toContain("--conversation-gap:16px");
    expect(base).toContain("--conversation-edge:24px");
  });

  it("narrows to a 44px gutter, 12px gap, and 16px edge at 800px", () => {
    expect(at800).toContain("--conversation-gutter:44px");
    expect(at800).toContain("--conversation-gap:12px");
    expect(at800).toContain("--conversation-edge:16px");
  });

  it("narrows to a 32px gutter, 8px gap, and 8px edge at 480px", () => {
    expect(at480).toContain("--conversation-gutter:32px");
    expect(at480).toContain("--conversation-gap:8px");
    expect(at480).toContain("--conversation-edge:8px");
  });
});
