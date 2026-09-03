import { afterEach, describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";

import { createEditorStore } from "./editor";
import type { FleetStore } from "./fleet";

function createMockFleet(laneId = 1): FleetStore {
  return {
    selectedLane: () => ({
      id: laneId,
      name: "lane-1",
      repo: "my-repo",
      worktree: { name: "my-repo", path: "/tmp/repo", branch: "main" },
      agent_sessions: [],
    }),
  } as unknown as FleetStore;
}

vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn(() => Promise.resolve({})),
  subscribeDaemon: vi.fn(() => Promise.resolve(() => {})),
}));

afterEach(() => {
  localStorage.clear();
});

describe("EditorStore tab retargeting on rename and delete", () => {
  it("retargets exact matching open tab when file is renamed", () => {
    createRoot(() => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);

      // Open two files
      editor.openFile("src/index.ts");
      editor.openFile("src/utils.ts");

      expect(editor.openFiles().map((f) => f.path)).toEqual(["src/index.ts", "src/utils.ts"]);
      expect(editor.activePath()).toBe("src/utils.ts");

      // Rename src/utils.ts to src/helpers.ts
      editor.handleFileRenamed("src/utils.ts", "src/helpers.ts", 1);

      expect(editor.openFiles().map((f) => f.path)).toEqual(["src/index.ts", "src/helpers.ts"]);
      expect(editor.activePath()).toBe("src/helpers.ts");
    });
  });

  it("retargets nested open tabs when a parent directory is renamed", () => {
    createRoot(() => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);

      editor.openFile("src/components/Header.tsx");
      editor.openFile("src/components/Button.tsx");

      expect(editor.activePath()).toBe("src/components/Button.tsx");

      // Rename directory "src/components" to "src/ui"
      editor.handleFileRenamed("src/components", "src/ui", 1);

      expect(editor.openFiles().map((f) => f.path)).toEqual([
        "src/ui/Header.tsx",
        "src/ui/Button.tsx",
      ]);
      expect(editor.activePath()).toBe("src/ui/Button.tsx");
    });
  });

  it("marks open tab deleted-on-disk when file is deleted", () => {
    createRoot(() => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);

      editor.openFile("src/old.ts");
      expect(editor.openFiles()[0].conflict).toBeNull();

      // Delete the file
      editor.handleFileDeleted("src/old.ts", 1);

      const file = editor.openFiles().find((f) => f.path === "src/old.ts");
      expect(file).toBeDefined();
      expect(file?.conflict).toEqual({ deleted: true, actualMtimeMs: null });
    });
  });

  it("marks all nested open tabs deleted when a directory is deleted", () => {
    createRoot(() => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);

      editor.openFile("crates/foo/src/lib.rs");
      editor.openFile("crates/foo/Cargo.toml");
      editor.openFile("README.md");

      editor.handleFileDeleted("crates/foo", 1);

      const files = editor.openFiles();
      const lib = files.find((f) => f.path === "crates/foo/src/lib.rs");
      const cargo = files.find((f) => f.path === "crates/foo/Cargo.toml");
      const readme = files.find((f) => f.path === "README.md");

      expect(lib?.conflict).toEqual({ deleted: true, actualMtimeMs: null });
      expect(cargo?.conflict).toEqual({ deleted: true, actualMtimeMs: null });
      expect(readme?.conflict).toBeNull();
    });
  });
});
