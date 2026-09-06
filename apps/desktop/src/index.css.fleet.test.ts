import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

/// Keep an RTL base direction for tail-preserving truncation; plaintext bidi would derive LTR from
/// branch names.
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
