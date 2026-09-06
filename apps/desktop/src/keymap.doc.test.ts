import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { renderDocSections } from "../scripts/print-shortcuts-doc";

// Vitest runs with apps/desktop as its working directory (where vite.config.ts lives), so the
// repo's docs/ directory is two levels up from here.
const DOCS_PATH = resolve(process.cwd(), "../../docs/desktop.md");

/// Extract the first table under a heading, rejecting missing headings or tables instead of
/// comparing empty output.
function tableAfterHeading(doc: string, heading: string): string {
  const lines = doc.split("\n");
  const headingIndex = lines.findIndex((line) => line.trim() === heading);
  if (headingIndex === -1) {
    throw new Error(`docs/desktop.md has no "${heading}" heading`);
  }
  const tableLines: string[] = [];
  let inTable = false;
  for (let i = headingIndex + 1; i < lines.length; i++) {
    const line = lines[i];
    if (line.startsWith("|")) {
      inTable = true;
      tableLines.push(line);
    } else if (inTable) {
      break;
    } else if (line.startsWith("#")) {

      break;
    }
    // Otherwise: blank line or lead-in prose before the table - keep scanning.
  }
  if (tableLines.length === 0) {
    throw new Error(`docs/desktop.md's "${heading}" heading has no table under it before the next heading`);
  }
  return tableLines.join("\n");
}

describe("docs/desktop.md keyboard tables", () => {
  const doc = readFileSync(DOCS_PATH, "utf8");

  for (const { heading, table } of renderDocSections()) {
    it(`matches the generated "${heading}" table`, () => {
      expect(tableAfterHeading(doc, heading)).toBe(table);
    });
  }
});
