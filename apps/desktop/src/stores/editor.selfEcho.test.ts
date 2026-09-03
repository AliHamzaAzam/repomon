import { beforeEach, describe, expect, it, vi } from "vitest";
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
});

describe("EditorStore self-echo save handling (item 9)", () => {
  it("does not flag a false conflict when the daemon's own-save echo event arrives while the write is in flight", async () => {
    let resolveWrite: ((value: { mtime_ms: number; size: number }) => void) | null = null;

    rpcMock.mockImplementation((method: string) => {
      if (method === "file.read") {
        return Promise.resolve({ content: "original", mtime_ms: 1000, size: 8, kind: "text" });
      }
      if (method === "file.list") return Promise.resolve({ entries: [], truncated: false });
      if (method === "file.write") {
        return new Promise((resolve) => {
          resolveWrite = resolve;
        });
      }
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const editor = createEditorStore(createMockFleet(1));

      await editor.openFile("src/index.ts", { cursor: 6 });
      editor.updateContent("src/index.ts", "modified content");

      const savePromise = editor.saveFile("src/index.ts");

      // Let the save register its in-flight marker before the daemon's echo of our own write
      // arrives - the daemon broadcasts event.file.changed before file.write resolves.
      await Promise.resolve();
      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 1, path: "src/index.ts", op: "modified" },
      });

      // From here on, file.read reflects what the in-flight write is about to produce - this
      // is what the post-save re-check (triggered because an echo arrived mid-save) will see.
      rpcMock.mockImplementation((method: string) => {
        if (method === "file.read") {
          return Promise.resolve({ content: "modified content", mtime_ms: 2000, size: 17, kind: "text" });
        }
        if (method === "file.list") return Promise.resolve({ entries: [], truncated: false });
        return Promise.resolve({});
      });

      resolveWrite?.({ mtime_ms: 2000, size: 17 });
      await savePromise;
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();

      const file = editor.openFiles()[0];
      expect(file.conflict).toBeNull();
      expect(file.saveError).toBeNull();
      expect(file.content).toBe("modified content");
      expect(file.savedContent).toBe("modified content");
      expect(file.mtimeMs).toBe(2000);
      expect(file.cursor).toBe(6);

      dispose();
    });
  });

  it("still picks up a genuine external change that lands right after the save resolves, distinguishing it from our own echo", async () => {
    let resolveWrite: ((value: { mtime_ms: number; size: number }) => void) | null = null;

    rpcMock.mockImplementation((method: string) => {
      if (method === "file.read") {
        return Promise.resolve({ content: "original", mtime_ms: 1000, size: 8, kind: "text" });
      }
      if (method === "file.list") return Promise.resolve({ entries: [], truncated: false });
      if (method === "file.write") {
        return new Promise((resolve) => {
          resolveWrite = resolve;
        });
      }
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const editor = createEditorStore(createMockFleet(1));

      await editor.openFile("src/index.ts");
      editor.updateContent("src/index.ts", "my local edit");

      const savePromise = editor.saveFile("src/index.ts");
      await Promise.resolve();

      // Our save resolves at mtime 2000...
      resolveWrite?.({ mtime_ms: 2000, size: 13 });

      // ...but a second, independent modification (not our own echo) also lands and broadcasts
      // while our save is still in flight, and the disk now sits at a different mtime.
      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 1, path: "src/index.ts", op: "modified" },
      });

      rpcMock.mockImplementation((method: string) => {
        if (method === "file.read") {
          return Promise.resolve({ content: "someone else's edit", mtime_ms: 3000, size: 20, kind: "text" });
        }
        if (method === "file.list") return Promise.resolve({ entries: [], truncated: false });
        return Promise.resolve({});
      });

      await savePromise;
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();

      const file = editor.openFiles()[0];
      // Buffer is clean relative to our own successful save, so the newer disk content wins.
      expect(file.content).toBe("someone else's edit");
      expect(file.mtimeMs).toBe(3000);
      expect(file.conflict).toBeNull();

      dispose();
    });
  });
});
