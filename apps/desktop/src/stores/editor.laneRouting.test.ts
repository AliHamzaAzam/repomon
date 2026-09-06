import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, createSignal } from "solid-js";

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

function fleetStub(laneId: () => number | null): FleetStore {
  return {
    selectedLaneId: laneId,
    selectedLane: () => {
      const id = laneId();
      if (id == null) return null;
      return {
        id,
        name: `lane-${id}`,
        repo: "my-repo",
        worktree: { name: `lane-${id}`, path: `/tmp/repo-${id}`, branch: "main" },
        agent_sessions: [],
      };
    },
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

describe("EditorStore lane-routed rename/delete events (item 8)", () => {
  it("a removed event for a background lane updates that lane's state without touching the active lane's tabs", async () => {
    rpcMock.mockImplementation((method: string) => {
      if (method === "file.read") {
        return Promise.resolve({ content: "content", mtime_ms: 1000, size: 7, kind: "text" });
      }
      if (method === "file.list") return Promise.resolve({ entries: [], truncated: false });
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const [selectedId, setSelectedId] = createSignal<number | null>(1);
      const editor = createEditorStore(fleetStub(selectedId));

      await editor.openFile("lane1/keep.ts");

      // Visit lane 2, open a file there, then come back to lane 1 (active) - this leaves
      // lane 2 tracked in laneStates as a background lane.
      setSelectedId(2);
      await Promise.resolve();
      await Promise.resolve();
      await editor.openFile("lane2/doomed.ts");
      setSelectedId(1);
      await Promise.resolve();
      await Promise.resolve();

      expect(editor.openFiles().map((f) => f.path)).toEqual(["lane1/keep.ts"]);

      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 2, path: "lane2/doomed.ts", op: "removed" },
      });
      await Promise.resolve();

      expect(editor.openFiles().map((f) => f.path)).toEqual(["lane1/keep.ts"]);
      expect(editor.openFiles()[0].conflict).toBeNull();

      const lane2State = editor.getLaneState(2);
      expect(
        lane2State?.openFiles.find((f) => f.path === "lane2/doomed.ts")?.conflict
      ).toEqual({ deleted: true, actualMtimeMs: null });

      dispose();
    });
  });

  it("a renamed event for a background lane retargets only that lane's tabs", async () => {
    rpcMock.mockImplementation((method: string) => {
      if (method === "file.read") {
        return Promise.resolve({ content: "content", mtime_ms: 1000, size: 7, kind: "text" });
      }
      if (method === "file.list") return Promise.resolve({ entries: [], truncated: false });
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const [selectedId, setSelectedId] = createSignal<number | null>(1);
      const editor = createEditorStore(fleetStub(selectedId));

      await editor.openFile("lane1/keep.ts");

      setSelectedId(2);
      await Promise.resolve();
      await Promise.resolve();
      await editor.openFile("lane2/old.ts");
      setSelectedId(1);
      await Promise.resolve();
      await Promise.resolve();

      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 2, path: "lane2/new.ts", op: "renamed", from: "lane2/old.ts" },
      });
      await Promise.resolve();

      expect(editor.openFiles().map((f) => f.path)).toEqual(["lane1/keep.ts"]);
      expect(editor.activePath()).toBe("lane1/keep.ts");

      const lane2State = editor.getLaneState(2);
      expect(lane2State?.openFiles.map((f) => f.path)).toEqual(["lane2/new.ts"]);
      expect(lane2State?.activePath).toBe("lane2/new.ts");

      dispose();
    });
  });
});

describe("EditorStore lane-scoped debounced tree reload (item 10)", () => {
  it("reloads each lane's directories under its own lane id and never leaves the active tree on a stuck loading row for a background lane's directory", async () => {
    const fileListCalls: Array<{ lane_id: number; path: string }> = [];
    rpcMock.mockImplementation((method: string, params?: unknown) => {
      if (method === "file.list") {
        const p = params as { lane_id: number; path: string };
        fileListCalls.push({ lane_id: p.lane_id, path: p.path });
        return Promise.resolve({ entries: [], truncated: false });
      }
      if (method === "file.read") {
        return Promise.resolve({ content: "x", mtime_ms: 1000, size: 1, kind: "text" });
      }
      return Promise.resolve({});
    });

    await createRoot(async (dispose) => {
      const [selectedId, setSelectedId] = createSignal<number | null>(1);
      const editor = createEditorStore(fleetStub(selectedId));
      await Promise.resolve();
      await Promise.resolve();

      // Visit lane 2 and come back, so lane 2 is tracked as a background lane.
      setSelectedId(2);
      await Promise.resolve();
      await Promise.resolve();
      setSelectedId(1);
      await Promise.resolve();
      await Promise.resolve();

      fileListCalls.length = 0; // ignore the eager root loads from switching lanes

      // Interleaved events for both lanes within one 300ms debounce window.
      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 1, path: "src/a.ts", op: "created" },
      });
      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 2, path: "docs/b.md", op: "created" },
      });
      emitDaemonEvent({
        method: "event.file.changed",
        params: { lane_id: 1, path: "src/c.ts", op: "created" },
      });

      vi.advanceTimersByTime(300);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();

      expect(fileListCalls).toContainEqual({ lane_id: 1, path: "src" });
      expect(fileListCalls).toContainEqual({ lane_id: 2, path: "docs" });
      // Lane 1 was never asked to reload lane 2's directory (bare-string dir key collision).
      expect(fileListCalls.filter((c) => c.lane_id === 1).some((c) => c.path === "docs")).toBe(false);

      // The active lane's live dirCache never got a stray "loading" entry for a background
      // lane's directory - which would otherwise freeze that row on the active tree forever,
      // since nothing routed for lane 2 would ever resolve it.
      expect(editor.dirCache().has("docs")).toBe(false);

      expect(editor.dirCache().get("src")?.status).toBe("loaded");

      // Lane 2's own stored dirCache entry for "docs" resolved to loaded, not stuck loading.
      expect(editor.getLaneState(2)?.dirCache.get("docs")?.status).toBe("loaded");

      dispose();
    });
  });
});
