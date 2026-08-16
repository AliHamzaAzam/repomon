import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { EditorView } from "@codemirror/view";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FileEntry, Lane, Repo } from "../bindings";
import { DaemonRpcError } from "../ipc/rpc";
import type { FleetStore } from "../stores/fleet";
import FileEditorPanel from "./FileEditorPanel";

const calls = vi.hoisted(() => ({ list: [] as Array<{ method: string; params: unknown }> }));
const subscribers = vi.hoisted(() => ({ list: [] as Array<(event: unknown) => void> }));
const daemonCallMock = vi.hoisted(() => vi.fn());

vi.mock("../ipc/rpc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/rpc")>();
  return {
    ...actual,
    daemonCall: (method: string, params?: unknown) => {
      calls.list.push({ method, params });
      return daemonCallMock(method, params);
    },
    subscribeDaemon: (onEvent: (event: unknown) => void) => {
      subscribers.list.push(onEvent);
      return Promise.resolve(() => {
        subscribers.list = subscribers.list.filter((l) => l !== onEvent);
      });
    },
  };
});

/// Wires the mocked `daemonCall` to a per-method handler table for one test. Handlers may return
/// a plain value, a promise, or throw/reject to simulate an RPC error - mirrors how `daemonCall`
/// itself either resolves or throws.
function mockRpc(handlers: Record<string, (params: unknown) => unknown>) {
  daemonCallMock.mockImplementation((method: string, params?: unknown) => {
    const handler = handlers[method];
    if (!handler) return Promise.resolve({});
    try {
      return Promise.resolve(handler(params));
    } catch (error) {
      return Promise.reject(error);
    }
  });
}

function emitFileChanged(laneId: number, path: string) {
  for (const listener of subscribers.list) {
    listener({ method: "event.file.changed", params: { lane_id: laneId, path } });
  }
}

afterEach(() => {
  cleanup();
  calls.list = [];
  subscribers.list = [];
  daemonCallMock.mockReset();
});

function repo(): Repo {
  return { id: 2, path: "/code/repomon", name: "repomon", added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false };
}

function lane(overrides: Partial<Lane> = {}): Lane {
  return {
    id: 7,
    repo: repo(),
    worktree: { id: 3, repo_id: 2, path: "/code/repomon-wt/desktop", branch: "feat/desktop", head: "abc1234", is_main: false, name: "desktop" },
    state: {
      worktree_id: 3,
      head: "abc1234",
      branch: "feat/desktop",
      upstream: null,
      ahead: 0,
      behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 },
      last_commit_at: null,
      locked: false,
      prunable: false,
      last_change_at: null,
    },
    agent_sessions: [],
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
    ...overrides,
  };
}

function entry(overrides: Partial<FileEntry> & Pick<FileEntry, "name" | "path" | "is_dir">): FileEntry {
  return { size: null, ignored: false, ...overrides };
}

function fleetWith(current: Lane | null, setSelectedLaneId: (id: number | null) => void = () => {}): FleetStore {
  return { selectedLane: () => current, setSelectedLaneId } as unknown as FleetStore;
}

/// A reactive fake fleet backing the lane-switch tests: `setSelectedLaneId` actually moves
/// `selectedLane()` between the two given lanes, the same round trip the real store performs when
/// FileEditorPanel calls it to revert (or re-dispatch) a switch.
function reactiveFleet(initial: Lane, lanesById: Record<number, Lane>) {
  const [id, setId] = createSignal(initial.id);
  const selectedLane = () => lanesById[id()] ?? null;
  const setSelectedLaneId = (next: number | null) => setId(next ?? initial.id);
  return { store: { selectedLane, setSelectedLaneId } as unknown as FleetStore, setId };
}

function getView(container: HTMLElement): EditorView {
  const content = container.querySelector<HTMLElement>(".cm-content");
  if (!content) throw new Error("no .cm-content rendered - is a file open?");
  const view = EditorView.findFromDOM(content);
  if (!view) throw new Error("EditorView.findFromDOM found no view");
  return view;
}

async function openFileTab(container: HTMLElement, fileName: string) {
  const row = await screen.findByText(fileName);
  fireEvent.click(row);
  await screen.findByTitle(fileName);
  await waitFor(() => expect(container.querySelector(".cm-content")).toBeInTheDocument());
}

describe("FileEditorPanel empty states", () => {
  it("shows a designed empty state and fetches nothing when no lane is selected", () => {
    mockRpc({});
    render(() => <FileEditorPanel fleet={fleetWith(null)} />);

    expect(screen.getByText("No lane selected")).toBeInTheDocument();
    expect(calls.list).toHaveLength(0);
  });
});

describe("FileEditorPanel file tree", () => {
  it("auto-loads the root level and lazily loads a directory only when it's expanded", async () => {
    mockRpc({
      "file.list": (params) => {
        const { path } = params as { lane_id: number; path: string };
        if (path === "") {
          return {
            entries: [entry({ name: "src", path: "src", is_dir: true }), entry({ name: "README.md", path: "README.md", is_dir: false, size: 120 })],
            truncated: false,
          };
        }
        if (path === "src") {
          return { entries: [entry({ name: "app.ts", path: "src/app.ts", is_dir: false, size: 40 })], truncated: false };
        }
        throw new Error(`unexpected file.list path: ${path}`);
      },
    });

    render(() => <FileEditorPanel fleet={fleetWith(lane())} />);

    expect(await screen.findByText("README.md")).toBeInTheDocument();
    expect(screen.getByText("src")).toBeInTheDocument();
    // Root loaded automatically (mirrors GitExplorerPanel loading on mount) - the nested "src"
    // level has not been requested yet, since it has never been expanded.
    expect(calls.list.some((c) => c.method === "file.list" && (c.params as { path: string }).path === "")).toBe(true);
    expect(calls.list.some((c) => c.method === "file.list" && (c.params as { path: string }).path === "src")).toBe(false);

    const dirRow = screen.getByText("src");
    fireEvent.click(dirRow);

    expect(await screen.findByText("app.ts")).toBeInTheDocument();
    expect(calls.list.some((c) => c.method === "file.list" && (c.params as { path: string }).path === "src")).toBe(true);
  });

  it("dims ignored entries and shows a quiet note for a truncated listing", async () => {
    mockRpc({
      "file.list": () => ({
        entries: [
          entry({ name: "node_modules", path: "node_modules", is_dir: true, ignored: true }),
          entry({ name: "index.ts", path: "index.ts", is_dir: false, size: 5 }),
        ],
        truncated: true,
      }),
    });

    render(() => <FileEditorPanel fleet={fleetWith(lane())} />);

    const ignoredRow = await screen.findByText("node_modules");
    expect(ignoredRow.closest("button")).toHaveClass("opacity-50");
    expect(screen.getByText(/partial listing/)).toBeInTheDocument();
  });
});

describe("FileEditorPanel open/edit/save", () => {
  it("opens a file into a tab with its content, on the click that lazily loads it via file.read", async () => {
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 })], truncated: false }),
      "file.read": (params) => {
        expect(params).toEqual({ lane_id: 7, path: "a.ts" });
        return { content: "console.log(1)", mtime_ms: 1000, size: 15, truncated: false };
      },
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);

    await openFileTab(container, "a.ts");

    expect(screen.getByTitle("a.ts")).toBeInTheDocument();
    expect(container.querySelector(".cm-content")?.textContent).toContain("console.log(1)");
  });

  it("marks the tab dirty (a titled dot) when the buffer is edited, and clears it again on save", async () => {
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 })], truncated: false }),
      "file.read": () => ({ content: "abc", mtime_ms: 1000, size: 3, truncated: false }),
      "file.write": (params) => {
        expect(params).toEqual({ lane_id: 7, path: "a.ts", content: "abcd", expected_mtime_ms: 1000 });
        return { mtime_ms: 2000, size: 4 };
      },
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);
    await openFileTab(container, "a.ts");

    expect(screen.queryByTitle("Unsaved changes")).not.toBeInTheDocument();

    const view = getView(container);
    view.dispatch({ changes: { from: 3, insert: "d" } });

    expect(await screen.findByTitle("Unsaved changes")).toBeInTheDocument();

    const saveButton = screen.getByRole("button", { name: "Save" });
    expect(saveButton).not.toBeDisabled();
    fireEvent.click(saveButton);

    await waitFor(() => expect(screen.queryByTitle("Unsaved changes")).not.toBeInTheDocument());
    const writeCall = calls.list.find((c) => c.method === "file.write");
    expect(writeCall?.params).toEqual({ lane_id: 7, path: "a.ts", content: "abcd", expected_mtime_ms: 1000 });
  });

  it("item 5b: gives the open-tab strip TerminalWorkspace's horizontal-scroll chain (scroll, not wrap)", async () => {
    mockRpc({
      "file.list": () => ({
        entries: [
          entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 }),
          entry({ name: "b.ts", path: "b.ts", is_dir: false, size: 5 }),
        ],
        truncated: false,
      }),
      "file.read": (params) => ({ content: `// ${(params as { path: string }).path}`, mtime_ms: 1000, size: 10, truncated: false }),
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);
    await openFileTab(container, "a.ts");
    // Opening a file auto-collapses the tree (effectiveTreeExpanded); re-expand it to reach b.ts.
    fireEvent.click(screen.getByRole("button", { name: "Toggle file tree" }));
    await openFileTab(container, "b.ts");

    const tabA = screen.getByTitle("a.ts");
    const tabB = screen.getByTitle("b.ts");
    const strip = tabA.closest(".overflow-x-auto");
    expect(strip).not.toBeNull();
    expect(strip).toContainElement(tabB);
    // Same scrolling ergonomics as TerminalWorkspace's tab strip: overflow-x-auto with a hidden
    // scrollbar and smooth scrolling, not flex-wrap.
    expect(strip).toHaveClass("no-scrollbar");
    expect(strip).toHaveClass("scroll-smooth");
    expect(strip).not.toHaveClass("flex-wrap");
    // Each tab keeps shrink-0 so the strip overflows (and scrolls) instead of squeezing tabs.
    expect(tabA.closest(".group")).toHaveClass("shrink-0");
    expect(tabB.closest(".group")).toHaveClass("shrink-0");
  });
});

describe("FileEditorPanel conflict handling", () => {
  it("shows the conflict banner on a -32011 save rejection, and Reload replaces the buffer with the on-disk content", async () => {
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 })], truncated: false }),
      "file.read": vi.fn()
        .mockResolvedValueOnce({ content: "mine", mtime_ms: 1000, size: 4, truncated: false })
        .mockResolvedValueOnce({ content: "theirs", mtime_ms: 2000, size: 6, truncated: false }),
      "file.write": () => {
        throw new DaemonRpcError({ code: -32011, message: "conflict: file changed on disk", data: { expected_mtime_ms: 1000, actual_mtime_ms: 2000 } });
      },
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);
    await openFileTab(container, "a.ts");

    const view = getView(container);
    view.dispatch({ changes: { from: 4, insert: "!" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByText("File changed on disk.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reload" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Keep mine" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Reload" }));

    await waitFor(() => expect(container.querySelector(".cm-content")?.textContent).toContain("theirs"));
    expect(screen.queryByText("File changed on disk.")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Unsaved changes")).not.toBeInTheDocument();
  });

  it("Keep mine re-reads the current mtime, preserves the buffer, and lets the next save succeed", async () => {
    let write: { path: string; expected_mtime_ms?: number } | null = null;
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 })], truncated: false }),
      "file.read": vi.fn()
        .mockResolvedValueOnce({ content: "mine", mtime_ms: 1000, size: 4, truncated: false })
        .mockResolvedValueOnce({ content: "theirs-on-disk", mtime_ms: 2000, size: 14, truncated: false }),
      "file.write": (params) => {
        const p = params as { lane_id: number; path: string; content: string; expected_mtime_ms?: number };
        if (write === null) {
          write = p;
          throw new DaemonRpcError({ code: -32011, message: "conflict: file changed on disk", data: { expected_mtime_ms: 1000, actual_mtime_ms: 2000 } });
        }
        // Second save, after "Keep mine" refreshed the mtime - succeeds with the buffer's own
        // (still-edited) content, not the on-disk content that was never applied to the buffer.
        expect(p.expected_mtime_ms).toBe(2000);
        expect(p.content).toBe("mine!");
        return { mtime_ms: 3000, size: p.content.length };
      },
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);
    await openFileTab(container, "a.ts");
    const view = getView(container);
    view.dispatch({ changes: { from: 4, insert: "!" } });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("File changed on disk.");

    fireEvent.click(screen.getByRole("button", { name: "Keep mine" }));
    await waitFor(() => expect(screen.queryByText("File changed on disk.")).not.toBeInTheDocument());
    // Buffer untouched by "Keep mine" - still shows the user's edit, not the on-disk content.
    expect(container.querySelector(".cm-content")?.textContent).toContain("mine!");
    expect(screen.getByTitle("Unsaved changes")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByTitle("Unsaved changes")).not.toBeInTheDocument());
  });

  it("distinguishes a deleted-on-disk conflict (actual_mtime_ms null) with Save as new content / Close", async () => {
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 })], truncated: false }),
      "file.read": () => ({ content: "mine", mtime_ms: 1000, size: 4, truncated: false }),
      "file.write": vi.fn()
        .mockRejectedValueOnce(new DaemonRpcError({ code: -32011, message: "conflict: file changed on disk", data: { expected_mtime_ms: 1000, actual_mtime_ms: null } }))
        .mockResolvedValueOnce({ mtime_ms: 5000, size: 5 }),
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);
    await openFileTab(container, "a.ts");
    const view = getView(container);
    view.dispatch({ changes: { from: 4, insert: "!" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByText("This file was deleted on disk.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reload" })).not.toBeInTheDocument();
    const saveAsNew = screen.getByRole("button", { name: "Save as new content" });

    fireEvent.click(saveAsNew);

    await waitFor(() => expect(screen.queryByText("This file was deleted on disk.")).not.toBeInTheDocument());
    const secondWrite = calls.list.filter((c) => c.method === "file.write")[1];
    // No expected_mtime_ms on the recovery write - there is nothing on disk left to conflict with.
    expect(secondWrite?.params).toEqual({ lane_id: 7, path: "a.ts", content: "mine!" });
    expect(screen.queryByTitle("Unsaved changes")).not.toBeInTheDocument();
  });
});

describe("FileEditorPanel external changes", () => {
  it("silently reloads a clean buffer on event.file.changed for the open lane", async () => {
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 })], truncated: false }),
      "file.read": vi.fn()
        .mockResolvedValueOnce({ content: "v1", mtime_ms: 1000, size: 2, truncated: false })
        .mockResolvedValueOnce({ content: "v2-from-elsewhere", mtime_ms: 2000, size: 18, truncated: false }),
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);
    await openFileTab(container, "a.ts");
    expect(container.querySelector(".cm-content")?.textContent).toContain("v1");

    emitFileChanged(7, "a.ts");

    await waitFor(() => expect(container.querySelector(".cm-content")?.textContent).toContain("v2-from-elsewhere"));
    expect(screen.queryByText("File changed on disk.")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Unsaved changes")).not.toBeInTheDocument();
  });

  it("shows the non-blocking conflict banner (buffer untouched) on event.file.changed for a dirty buffer", async () => {
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 })], truncated: false }),
      "file.read": vi.fn()
        .mockResolvedValueOnce({ content: "v1", mtime_ms: 1000, size: 2, truncated: false })
        .mockResolvedValueOnce({ content: "v2-from-elsewhere", mtime_ms: 2000, size: 18, truncated: false }),
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);
    await openFileTab(container, "a.ts");
    const view = getView(container);
    view.dispatch({ changes: { from: 2, insert: "!" } });
    await screen.findByTitle("Unsaved changes");

    emitFileChanged(7, "a.ts");

    expect(await screen.findByText("File changed on disk.")).toBeInTheDocument();
    // Buffer keeps the user's edit - a dirty buffer is never silently overwritten.
    expect(container.querySelector(".cm-content")?.textContent).toContain("v1!");
  });

  it("ignores event.file.changed for a different lane or an unrelated/unopened path", async () => {
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false, size: 5 })], truncated: false }),
      "file.read": () => ({ content: "v1", mtime_ms: 1000, size: 2, truncated: false }),
    });

    const { container } = render(() => <FileEditorPanel fleet={fleetWith(lane())} />);
    await openFileTab(container, "a.ts");
    const callsBefore = calls.list.filter((c) => c.method === "file.read").length;

    emitFileChanged(99, "a.ts"); // different lane
    emitFileChanged(7, "unopened.ts"); // not an open tab

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(calls.list.filter((c) => c.method === "file.read").length).toBe(callsBefore);
  });
});

describe("FileEditorPanel lane switching", () => {
  it("resets the tree and closed tabs immediately when the newly selected lane has no dirty files", async () => {
    const laneA = lane({ id: 7 });
    const laneB = lane({ id: 8, worktree: { id: 4, repo_id: 2, path: "/code/repomon-wt/other", branch: "chore/other", head: "def", is_main: false, name: "other" } });
    mockRpc({
      "file.list": (params) => {
        const { lane_id } = params as { lane_id: number };
        return {
          entries: [entry({ name: lane_id === 7 ? "a.ts" : "b.ts", path: lane_id === 7 ? "a.ts" : "b.ts", is_dir: false, size: 1 })],
          truncated: false,
        };
      },
    });

    const { store, setId } = reactiveFleet(laneA, { 7: laneA, 8: laneB });
    render(() => <FileEditorPanel fleet={store} />);

    expect(await screen.findByText("a.ts")).toBeInTheDocument();
    setId(8);
    expect(await screen.findByText("b.ts")).toBeInTheDocument();
    expect(screen.queryByText("a.ts")).not.toBeInTheDocument();
  });

  it("reverts the lane switch and asks for confirmation when the previous lane has a dirty open file", async () => {
    const laneA = lane({ id: 7 });
    const laneB = lane({ id: 8, worktree: { id: 4, repo_id: 2, path: "/code/repomon-wt/other", branch: "chore/other", head: "def", is_main: false, name: "other" } });
    mockRpc({
      "file.list": (params) => {
        const { lane_id } = params as { lane_id: number };
        return {
          entries: [entry({ name: lane_id === 7 ? "a.ts" : "b.ts", path: lane_id === 7 ? "a.ts" : "b.ts", is_dir: false, size: 1 })],
          truncated: false,
        };
      },
      "file.read": () => ({ content: "v1", mtime_ms: 1000, size: 2, truncated: false }),
    });

    const { store, setId } = reactiveFleet(laneA, { 7: laneA, 8: laneB });
    const { container } = render(() => <FileEditorPanel fleet={store} />);

    await openFileTab(container, "a.ts");
    const view = getView(container);
    view.dispatch({ changes: { from: 2, insert: "!" } });
    await screen.findByTitle("Unsaved changes");

    setId(8);

    // The confirm dialog appears, and the lane immediately reverts (fleet-visible state, not just
    // this panel) rather than leaving the fleet sidebar pointed at lane 8 while this panel still
    // shows lane 7's tree underneath the dialog.
    const dialog = await screen.findByRole("dialog", { name: "Unsaved changes" });
    expect(within(dialog).getByText(/a\.ts/)).toBeInTheDocument();
    expect(screen.getByTitle("a.ts")).toBeInTheDocument(); // lane 7's tab is still open underneath

    fireEvent.click(screen.getByRole("button", { name: "Discard and switch" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(await screen.findByText("b.ts")).toBeInTheDocument();
    expect(screen.queryByTitle("a.ts")).not.toBeInTheDocument();
  });
});
