import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

/// `.truncate-tail` is the fleet row's "keep the end of the branch name" rule. It only works
/// while the box keeps its rtl base direction; `unicode-bidi: plaintext` re-derives the direction
/// from the Latin text and silently turns it back into an ordinary end-truncation.
describe("index.css fleet row truncation", () => {
  const raw = readFileSync(path.resolve(process.cwd(), "src/index.css"), "utf-8");
  const rule = raw.match(/\.truncate-tail\s*{[^}]*}/)?.[0] ?? "";

  it("truncates from the start by keeping an rtl base direction", () => {
    expect(rule).toContain("direction: rtl");
    expect(rule).toContain("text-overflow: ellipsis");
  });

  it("isolates the run instead of letting plaintext flip the base direction back", () => {
    expect(rule).toContain("unicode-bidi: isolate");
    expect(rule).not.toMatch(/unicode-bidi:\s*plaintext/);
  });
});
