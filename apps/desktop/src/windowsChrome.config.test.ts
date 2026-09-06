import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

/// Windows decoration overrides must retain size floors and match caption-command capabilities
/// while shared platform settings remain intact.
const tauriDir = path.resolve(process.cwd(), "src-tauri");
const read = (file: string) => JSON.parse(readFileSync(path.join(tauriDir, file), "utf-8"));

describe("Windows single title bar configuration", () => {
  const shared = read("tauri.conf.json");
  const base = shared.app.windows[0];

  for (const file of ["tauri.preview.win.conf.json", "tauri.release.win.conf.json"]) {
    it(`${file} turns native decorations off and keeps the window floor`, () => {
      const [window] = read(file).app.windows;
      expect(window.decorations).toBe(false);
      expect(window.minWidth).toBe(base.minWidth);
      expect(window.minHeight).toBe(base.minHeight);
      expect(window.width).toBe(base.width);
      expect(window.height).toBe(base.height);
      expect(window.dragDropEnabled).toBe(base.dragDropEnabled);
    });
  }

  it("leaves decorations to the system in the shared conf (macOS overlay, Linux native)", () => {
    expect(base.decorations).toBeUndefined();
    expect(base.titleBarStyle).toBe("Overlay");
    for (const file of ["tauri.preview.conf.json", "tauri.release.conf.json"]) {
      expect(read(file).app?.windows).toBeUndefined();
    }
  });

  it("grants the window commands the caption controls and the drag region call", () => {
    const permissions: string[] = read("capabilities/default.json").permissions;
    for (const permission of [
      "core:window:allow-start-dragging",
      "core:window:allow-internal-toggle-maximize",
      "core:window:allow-minimize",
      "core:window:allow-toggle-maximize",
      "core:window:allow-close",
      "core:window:allow-is-maximized",
    ]) {
      expect(permissions).toContain(permission);
    }
  });
});
