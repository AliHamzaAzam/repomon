import { createRoot, createSignal } from "solid-js";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Lane } from "../bindings";
import { DaemonRpcError } from "../ipc/rpc";
import {
  createEditorStore,
  DEFAULT_TREE_WIDTH_PX,
  EDITOR_STORAGE_KEY,
  MIN_TREE_WIDTH_PX,
} from "./editor";
import type { FleetStore } from "./fleet";

const daemonCallMock = vi.fn();

vi.mock("../ipc/rpc", async () => {
  const actual = await vi.importActual<typeof import("../ipc/rpc")>("../ipc/rpc");
  return {
    ...actual,
    daemonCall: (...args: unknown[]) => daemonCallMock(...args),
    subscribeDaemon: vi.fn().mockResolvedValue(() => {}),
  };
});

function lane(id: number): Lane {
  return {
    id,
    repo: {
      id,
      name: `repo-${id}`,
      path: `/repo-${id}`,
      added_at: "",
      worktree_root_template: null,
      hidden: false,
      position: null,
      label: null,
    },
    worktree: {
      id,
      repo_id: id,
      name: `lane-${id}`,
      branch: `lane-${id}`,
      path: `/repo-${id}/lane`,
      head: "abc",
      is_main: false,
    },
    state: {
      worktree_id: id,
      head: "abc",
      branch: `lane-${id}`,
      upstream: null,
      ahead: 0,
      behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 },
      last_commit_at: null,
      last_change_at: null,
      locked: false,
      prunable: false,
    },
    agent_sessions: [],
    last_activity_at: "",
    pinned: false,
    role: null,
  };
}

function fleetStub(laneId: () => number | null, lanesList: Lane[]): FleetStore {
  return {
    refresh: vi.fn().mockResolvedValue(undefined),
    selectedLaneId: laneId,
    selectedLane: () => {
      const id = laneId();
      return id == null ? null : lanesList.find((l) => l.id === id) ?? null;
    },
    lanes: () => lanesList,
    terminals: () => [],
    setFocusedWindow: vi.fn(),
  } as unknown as FleetStore;
}

beforeEach(() => {
  localStorage.removeItem(EDITOR_STORAGE_KEY);
  daemonCallMock.mockReset();
});

describe("editor store", () => {
  it("initializes with default settings and respects min width", () => {
    createRoot((dispose) => {
      const [selectedId] = createSignal<number | null>(7);
      const store = createEditorStore(fleetStub(selectedId, [lane(7)]));

      expect(store.treeColumnWidth()).toBe(DEFAULT_TREE_WIDTH_PX);
      expect(store.wrap()).toBe(false);
      expect(store.whitespace()).toBe(false);

      store.setTreeColumnWidth(100);
      expect(store.treeColumnWidth()).toBe(MIN_TREE_WIDTH_PX);

      store.toggleWrap();
      expect(store.wrap()).toBe(true);

      store.toggleWhitespace();
      expect(store.whitespace()).toBe(true);

      dispose();
    });
  });

  it("does not write the tree column width to localStorage until persistTreeColumnWidth is called (item 8)", () => {
    createRoot((dispose) => {
      const [selectedId] = createSignal<number | null>(7);
      const store = createEditorStore(fleetStub(selectedId, [lane(7)]));

      // setTreeColumnWidth (called on every mousemove during a drag) updates the live signal
      // only - it must not thrash localStorage dozens of times a second for the length of a drag.
      store.setTreeColumnWidth(300);
      store.setTreeColumnWidth(320);
      store.setTreeColumnWidth(340);
      expect(store.treeColumnWidth()).toBe(340);
      expect(localStorage.getItem(EDITOR_STORAGE_KEY)).toBeNull();

      // The final width is committed once the caller explicitly persists it (e.g. on mouseup).
      store.persistTreeColumnWidth();
      const stored = JSON.parse(localStorage.getItem(EDITOR_STORAGE_KEY)!);
      expect(stored.treeColumnWidth).toBe(340);

      dispose();
    });
  });

  it("opens a file, updates content, and marks dirty", async () => {
    daemonCallMock.mockImplementation(async (method: string) => {
      if (method === "file.read") {
        return { content: "hello world", mtime_ms: 1000, size: 11, truncated: false };
      }
      if (method === "file.list") {
        return { entries: [], truncated: false };
      }
      return {};
    });

    await new Promise<void>((resolve) => {
      createRoot(async (dispose) => {
        const [selectedId] = createSignal<number | null>(7);
        const store = createEditorStore(fleetStub(selectedId, [lane(7)]));

        await store.openFile("src/main.rs");
        expect(store.openFiles().length).toBe(1);
        expect(store.activePath()).toBe("src/main.rs");

        const file = store.activeFile();
        expect(file?.content).toBe("hello world");
        expect(file?.savedContent).toBe("hello world");

        store.updateContent("src/main.rs", "hello modified");
        expect(store.activeFile()?.content).toBe("hello modified");
        expect(store.activeFile()?.savedContent).toBe("hello world");

        dispose();
        resolve();
      });
    });
  });

  it("persists open paths, active path, and toggles to localStorage without saving buffer content", async () => {
    daemonCallMock.mockImplementation(async (method: string) => {
      if (method === "file.read") {
        return { content: "secret unsaved buffer", mtime_ms: 1000, size: 21, truncated: false };
      }
      if (method === "file.list") {
        return { entries: [], truncated: false };
      }
      return {};
    });

    await new Promise<void>((resolve) => {
      createRoot(async (dispose) => {
        const [selectedId] = createSignal<number | null>(7);
        const store = createEditorStore(fleetStub(selectedId, [lane(7)]));

        await store.openFile("src/secret.rs");
        store.updateContent("src/secret.rs", "unsaved modifications");
        store.updateCursor("src/secret.rs", 42, 100);

        const raw = localStorage.getItem(EDITOR_STORAGE_KEY);
        expect(raw).toBeTruthy();
        const parsed = JSON.parse(raw!);
        expect(parsed.lanes["7"]).toBeDefined();
        expect(parsed.lanes["7"].openPaths).toEqual(["src/secret.rs"]);
        expect(parsed.lanes["7"].activePath).toBe("src/secret.rs");
        expect(parsed.lanes["7"].cursors["src/secret.rs"]).toBe(42);
        expect(parsed.lanes["7"].scrollTops["src/secret.rs"]).toBe(100);

        // Never persist buffer content
        expect(raw).not.toContain("secret unsaved buffer");
        expect(raw).not.toContain("unsaved modifications");

        dispose();
        resolve();
      });
    });
  });

  it("keeps state in memory across lane switches and does not discard dirty buffers", async () => {
    daemonCallMock.mockImplementation(async (method: string, params: unknown) => {
      if (method === "file.read") {
        const p = params as { lane_id: number; path: string };
        return { content: `content from lane ${p.lane_id}`, mtime_ms: 1000, size: 20, truncated: false };
      }
      if (method === "file.list") {
        return { entries: [], truncated: false };
      }
      return {};
    });

    await new Promise<void>((resolve) => {
      createRoot(async (dispose) => {
        const [selectedId, setSelectedId] = createSignal<number | null>(7);
        const store = createEditorStore(fleetStub(selectedId, [lane(7), lane(8)]));

        await store.openFile("file7.rs");
        store.updateContent("file7.rs", "dirty lane 7 edits");

        // Switch to lane 8
        setSelectedId(8);
        await Promise.resolve();

        expect(store.currentLaneId()).toBe(8);
        expect(store.openFiles().length).toBe(0);

        await store.openFile("file8.rs");
        expect(store.openFiles().length).toBe(1);
        expect(store.activePath()).toBe("file8.rs");

        // Switch back to lane 7
        setSelectedId(7);
        await Promise.resolve();

        expect(store.currentLaneId()).toBe(7);
        expect(store.openFiles().length).toBe(1);
        expect(store.activePath()).toBe("file7.rs");
        expect(store.activeFile()?.content).toBe("dirty lane 7 edits");
        expect(store.activeFile()?.savedContent).toBe("content from lane 7");

        dispose();
        resolve();
      });
    });
  });

  it("handles saving and conflict detection (-32011)", async () => {
    let returnConflict = false;
    daemonCallMock.mockImplementation(async (method: string) => {
      if (method === "file.read") {
        return { content: "initial", mtime_ms: 1000, size: 7, truncated: false };
      }
      if (method === "file.write") {
        if (returnConflict) {
          throw new DaemonRpcError({
            code: -32011,
            message: "conflict",
            data: { expected_mtime_ms: 1000, actual_mtime_ms: 2000 },
          });
        }
        return { mtime_ms: 1500, size: 7 };
      }
      return { entries: [], truncated: false };
    });

    await new Promise<void>((resolve) => {
      createRoot(async (dispose) => {
        const [selectedId] = createSignal<number | null>(7);
        const store = createEditorStore(fleetStub(selectedId, [lane(7)]));

        await store.openFile("test.txt");
        store.updateContent("test.txt", "new content");

        returnConflict = true;
        await store.saveFile("test.txt");

        expect(store.activeFile()?.conflict).toEqual({
          deleted: false,
          actualMtimeMs: 2000,
        });

        // Keep mine re-reads mtime
        daemonCallMock.mockImplementation(async (method: string) => {
          if (method === "file.read") {
            return { content: "remote content", mtime_ms: 2000, size: 14, truncated: false };
          }
          if (method === "file.write") {
            return { mtime_ms: 2500, size: 11 };
          }
          return {};
        });

        await store.keepMine("test.txt");
        expect(store.activeFile()?.conflict).toBeNull();
        expect(store.activeFile()?.mtimeMs).toBe(2000);

        returnConflict = false;
        await store.saveFile("test.txt");
        expect(store.activeFile()?.savedContent).toBe("new content");
        expect(store.activeFile()?.mtimeMs).toBe(2500);

        dispose();
        resolve();
      });
    });
  });

  it("bumps saveVersion on a successful save but not on a conflict, so CodeEditor's diff-base refresh fires exactly once per save", async () => {
    let returnConflict = false;
    daemonCallMock.mockImplementation(async (method: string) => {
      if (method === "file.read") {
        return { content: "initial", mtime_ms: 1000, size: 7, truncated: false };
      }
      if (method === "file.write") {
        if (returnConflict) {
          throw new DaemonRpcError({
            code: -32011,
            message: "conflict",
            data: { expected_mtime_ms: 1000, actual_mtime_ms: 2000 },
          });
        }
        return { mtime_ms: 1500, size: 7 };
      }
      return { entries: [], truncated: false };
    });

    await new Promise<void>((resolve) => {
      createRoot(async (dispose) => {
        const [selectedId] = createSignal<number | null>(7);
        const store = createEditorStore(fleetStub(selectedId, [lane(7)]));

        await store.openFile("test.txt");
        expect(store.activeFile()?.saveVersion).toBe(0);

        store.updateContent("test.txt", "new content");

        returnConflict = true;
        await store.saveFile("test.txt");
        // A rejected (conflicting) save must not bump saveVersion - nothing was actually saved.
        expect(store.activeFile()?.saveVersion).toBe(0);

        returnConflict = false;
        await store.saveFile("test.txt");
        expect(store.activeFile()?.saveVersion).toBe(1);

        store.updateContent("test.txt", "newer content");
        await store.saveFile("test.txt");
        expect(store.activeFile()?.saveVersion).toBe(2);

        dispose();
        resolve();
      });
    });
  });

  it("marks large files and prevents updateContent and saveFile", async () => {
    daemonCallMock.mockImplementation(async (method: string) => {
      if (method === "file.read") {
        return {
          content: "big content",
          mtime_ms: 1000,
          size: 3 * 1024 * 1024,
          truncated: false,
          kind: "text",
          large: true,
        };
      }
      return { entries: [], truncated: false };
    });

    await new Promise<void>((resolve) => {
      createRoot(async (dispose) => {
        const [selectedId] = createSignal<number | null>(7);
        const store = createEditorStore(fleetStub(selectedId, [lane(7)]));

        await store.openFile("large.log");
        expect(store.activeFile()?.large).toBe(true);

        store.updateContent("large.log", "edited content");
        expect(store.activeFile()?.content).toBe("big content");

        await store.saveFile("large.log");
        expect(daemonCallMock).not.toHaveBeenCalledWith("file.write", expect.anything());

        dispose();
        resolve();
      });
    });
  });

  it("keeps a pdf tab read-only: never dirty, never saved, activates on openAt", async () => {
    daemonCallMock.mockImplementation(async (method: string) => {
      if (method === "file.read") {
        return { content: "", mtime_ms: 1000, size: 40000, truncated: false, kind: "pdf" };
      }
      return { entries: [], truncated: false };
    });

    await new Promise<void>((resolve) => {
      createRoot(async (dispose) => {
        const [selectedId] = createSignal<number | null>(7);
        const store = createEditorStore(fleetStub(selectedId, [lane(7)]));

        await store.openFile("report.pdf");
        expect(store.activeFile()?.kind).toBe("pdf");
        expect(store.activeFile()?.content).toBe("");

        store.updateContent("report.pdf", "should not stick");
        expect(store.activeFile()?.content).toBe("");
        expect(store.activeFile()?.content).toBe(store.activeFile()?.savedContent);

        await store.saveFile("report.pdf");
        expect(daemonCallMock).not.toHaveBeenCalledWith("file.write", expect.anything());

        // requestCloseFile only blocks on a dirty buffer - a pdf tab is never dirty, so it
        // closes immediately with no confirm.
        expect(store.requestCloseFile("report.pdf")).toBe(true);
        expect(store.openFiles().length).toBe(0);

        await store.openAt("report.pdf", 1, 0);
        expect(store.activeFile()?.path).toBe("report.pdf");
        expect(store.activeFile()?.kind).toBe("pdf");

        dispose();
        resolve();
      });
    });
  });
});



describe("restored file metadata", () => {
  it.each(["pdf", "image", "binary", "text", undefined])("restores %s tabs with the same read state as open and reload", async (kind) => {
    localStorage.setItem(EDITOR_STORAGE_KEY, JSON.stringify({ lanes: { "7": {
      openPaths: ["fixture"], activePath: "fixture", expandedDirs: [], cursors: {}, scrollTops: {},
    } } }));
    daemonCallMock.mockImplementation(async (method: string) => method === "file.read"
      ? { content: kind === "text" ? "hello" : "", kind, size: 321, mtime_ms: 1000, large: false }
      : { entries: [], truncated: false });
    const h = createRoot((dispose) => {
      const [id] = createSignal<number | null>(7);
      return { store: createEditorStore(fleetStub(id, [lane(7)])), dispose };
    });
    try {
      await vi.waitFor(() => expect(h.store.activeFile()?.loading).toBe(false));
      const restored = h.store.activeFile();
      expect(restored).toMatchObject({ kind: kind ?? "text", size: 321, mtimeMs: 1000, large: false });
      await h.store.openFile("fixture");
      expect(h.store.activeFile()).toEqual(restored);
      await h.store.reloadFile("fixture");
      expect(h.store.activeFile()).toEqual(restored);
      h.store.closeFile("fixture");
      await h.store.openFile("fixture");
      expect(h.store.activeFile()).toEqual(restored);
    } finally { h.dispose(); }
  });
});
