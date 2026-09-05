import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

/// The shell interiors and settings surfaces draw every color from the theme tokens, so a theme
/// switch recolors all of them at once. Raw Tailwind palette steps (`emerald-500`), utilities the
/// theme never defines (`text-accent`, `bg-surface-raised`, which render as nothing), and literal
/// black or white rings all break that, and none of them survive a review. This pins the rule at
/// the source so a future edit cannot quietly bring one back.
const OWNED = [
  "FleetSidebar.tsx",
  "RepomindRow.tsx",
  "TerminalWorkspace.tsx",
  "TerminalPane.tsx",
  "Onboarding.tsx",
  "SettingsModal.tsx",
  "UsageSettingsView.tsx",
  "PolicySettings.tsx",
  "SystemHealthView.tsx",
  "CommandLineToolsCard.tsx",
  "DaemonBootRow.tsx",
  "KeyboardHelp.tsx",
  "ShortcutsOverlay.tsx",
  "ControlCenter.tsx",
  "PanePicker.tsx",
];

const BANNED = [
  /\b(?:text|bg|border|ring)-(?:emerald|amber|red|green|blue|slate|gray|zinc|neutral|stone|sky|indigo|violet|rose|orange|yellow|teal|cyan)-\d{2,3}\b/,
  /\b(?:text|bg|border|ring)-accent\b/,
  /\bsurface-raised\b/,
  /\b(?:ring|border|bg|text)-(?:black|white)\/\d+\b/,
  /#[0-9a-fA-F]{3,8}\b/,
];

describe("shell and settings components use theme tokens only", () => {
  for (const file of OWNED) {
    it(`${file} carries no raw palette, undefined utility, or hex color`, () => {
      const source = readFileSync(path.resolve(process.cwd(), "src/components", file), "utf-8")
        .split("\n")
        .filter((line) => !line.trimStart().startsWith("//") && !line.trimStart().startsWith("*") && !line.trimStart().startsWith("///"))
        .join("\n");
      for (const pattern of BANNED) {
        expect(source).not.toMatch(pattern);
      }
    });
  }
});
