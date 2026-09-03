import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";

import { createEditorStore } from "./editor";
import type { FleetStore } from "./fleet";

const subscribers: Array<(event: unknown) => void> = [];
const rpcCalls: Array<{ method: string; params: unknown }> = [];
const rpcMock = vi.fn();

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params?: unknown) => {
    rpcCalls.push({ method, params });
    return rpcMock(method, params);
  },
  subscribeDaemon: (onEvent: (event: unknown) => void) => {
    subscribers.push(onEvent);
    return Promise.resolve(() => {
      const idx = subscribers.indexOf(onEvent);
      if (idx >= 0) subscribers.splice(idx, 1);
    });
  },
}));

function emitDaemonEvent(event: unknown) {
  for (const s of [...subscribers]) {
    s(event);
  }
}

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

beforeEach(() => {
  localStorage.clear();
  subscribers.length = 0;
  rpcCalls.length = 0;
  rpcMock.mockReset();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("EditorStore live refresh on event.file.changed", () => {
  it("silently reloads a clean buffer preserving cursor position", async () => {
    rpcMock.mockImplementation((method: string) => {
      if (method === "file.read") {
        return Promise.resolve({
          content: "initial file content",
          mtime_ms: 1000,
          size: 20,
          kind: "text",
        });
      }
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);

      await editor.openFile("src/index.ts", { cursor: 8 });

      expect(editor.openFiles()[0].content).toBe("initial file content");
      expect(editor.openFiles()[0].cursor).toBe(8);

      // Now external update changes the content on disk
      rpcMock.mockImplementation((method: string) => {
        if (method === "file.read") {
          return Promise.resolve({
            content: "updated file content from external tool",
            mtime_ms: 2000,
            size: 39,
            kind: "text",
          });
        }
        return Promise.resolve({});
      });

      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 1, path: "src/index.ts", op: "modified" },
      });

      // Let microtasks run
      await Promise.resolve();
      await Promise.resolve();

      const updated = editor.openFiles()[0];
      expect(updated.content).toBe("updated file content from external tool");
      expect(updated.cursor).toBe(8);
      expect(updated.conflict).toBeNull();

      dispose();
    });
  });

  it("marks a dirty buffer with a conflict banner", async () => {
    rpcMock.mockImplementation((method: string) => {
      if (method === "file.read") {
        return Promise.resolve({
          content: "original disk content",
          mtime_ms: 1000,
          size: 21,
          kind: "text",
        });
      }
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);

      await editor.openFile("src/dirty.ts");
      editor.updateContent("src/dirty.ts", "locally modified dirty content");

      // Disk changes externally
      rpcMock.mockImplementation((method: string) => {
        if (method === "file.read") {
          return Promise.resolve({
            content: "changed on disk concurrently",
            mtime_ms: 2000,
            size: 28,
            kind: "text",
          });
        }
        return Promise.resolve({});
      });

      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 1, path: "src/dirty.ts", op: "modified" },
      });

      await Promise.resolve();
      await Promise.resolve();

      const dirtyFile = editor.openFiles()[0];
      expect(dirtyFile.content).toBe("locally modified dirty content");
      expect(dirtyFile.conflict).toEqual({ deleted: false, actualMtimeMs: 2000 });

      dispose();
    });
  });

  it("marks a buffer as deleted-on-disk when op is removed", async () => {
    rpcMock.mockImplementation((method: string) => {
      if (method === "file.read") {
        return Promise.resolve({
          content: "hello",
          mtime_ms: 1000,
          size: 5,
          kind: "text",
        });
      }
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);

      await editor.openFile("src/deleted.ts");
      expect(editor.openFiles()[0].conflict).toBeNull();

      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 1, path: "src/deleted.ts", op: "removed" },
      });

      await Promise.resolve();
      await Promise.resolve();

      const file = editor.openFiles()[0];
      expect(file.conflict).toEqual({ deleted: true, actualMtimeMs: null });

      dispose();
    });
  });

  it("debounces directory tree reloads at most every 300ms on event bursts", async () => {
    rpcMock.mockImplementation((method: string) => {
      if (method === "file.list") {
        return Promise.resolve({ entries: [] });
      }
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);
      void editor;
      await Promise.resolve();

      // Rapidly emit 10 file changes in "src/nested"
      for (let i = 0; i < 10; i++) {
        emitDaemonEvent({
          method: "event.file.changed",
          params: { lane_id: 1, path: `src/nested/file${i}.ts`, op: "created" },
        });
      }

      // Initially no file.list has fired for src/nested because of 300ms debounce
      const nestedLists = () =>
        rpcCalls.filter(
          (c) => c.method === "file.list" && (c.params as { path?: string })?.path === "src/nested"
        );
      expect(nestedLists().length).toBe(0);

      // Fast forward by 150ms
      vi.advanceTimersByTime(150);
      expect(nestedLists().length).toBe(0);

      // Fast forward past 300ms
      vi.advanceTimersByTime(200);
      await Promise.resolve();

      // Exactly 1 directory list call for "src/nested"
      expect(nestedLists().length).toBe(1);

      dispose();
    });
  });
});
