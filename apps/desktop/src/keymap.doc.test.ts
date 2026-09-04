import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { renderDocSections } from "../scripts/print-shortcuts-doc";

// Vitest runs with apps/desktop as its working directory (where vite.config.ts lives), so the
// repo's docs/ directory is two levels up from here.
const DOCS_PATH = resolve(process.cwd(), "../../docs/desktop.md");

/// Pull the first markdown table under `heading` in `doc` (the first contiguous run of lines
/// starting with "|" before the next heading), tolerating a short lead-in paragraph between the
/// heading and the table itself. Throws with a useful message if the heading is missing or the
/// next heading arrives before any table does, so a renamed/deleted/reordered section fails
/// loudly instead of comparing against an empty string.
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
      // Reached the next heading without ever finding a table row.
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
