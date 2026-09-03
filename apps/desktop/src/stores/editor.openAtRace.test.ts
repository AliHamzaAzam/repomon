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

let deferredResolve1: (val: any) => void;
const p1 = new Promise((res) => {
  deferredResolve1 = res;
});

vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn((method: string) => {
    if (method === "file.read") {
      return p1;
    }
    return Promise.resolve({});
  }),
  subscribeDaemon: vi.fn(() => Promise.resolve(() => {})),
}));

afterEach(() => {
  localStorage.clear();
});

describe("openAt token race prevention", () => {
  it("two overlapping openAt calls on one file where first load resolves last end on second target", async () => {
    await createRoot(async () => {
      const fleet = createMockFleet(1);
      const editor = createEditorStore(fleet);

      // First call targets line 10, col 1, but its load is slow (deferred p1)
      const call1 = editor.openAt("file.ts", 10, 1);

      // Second call targets line 20, col 5
      const call2 = editor.openAt("file.ts", 20, 5);

      // Second call immediately sets target to line 20, col 5
      expect(editor.openAtTarget()?.line).toBe(20);
      expect(editor.openAtTarget()?.column).toBe(5);

      // Now resolve the first call
      deferredResolve1!({
        content: "line 1\nline 2",
        mtime_ms: 1000,
        size: 14,
        kind: "text",
      });

      await Promise.all([call1, call2]);

      // Target must STILL be the second call (line 20, column 5), not overridden by the first call
      expect(editor.openAtTarget()?.line).toBe(20);
      expect(editor.openAtTarget()?.column).toBe(5);
    });
  });
});
