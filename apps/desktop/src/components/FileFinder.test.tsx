import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import FileFinder from "./FileFinder";

const calls = vi.hoisted(() => ({ list: [] as Array<{ method: string; params: unknown }> }));
const daemonCallMock = vi.hoisted(() => vi.fn());

vi.mock("../ipc/rpc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/rpc")>();
  return {
    ...actual,
    daemonCall: (method: string, params?: unknown) => {
      calls.list.push({ method, params });
      return daemonCallMock(method, params);
    },
  };
});

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

function makeMockEditorStore(laneId = 1) {
  return {
    selectedLane: () => ({ id: laneId, worktree: { name: "test-repo", path: "/tmp/test" } }),
    openFile: vi.fn(),
  } as unknown as import("../stores/editor").EditorStore;
}

afterEach(() => {
  cleanup();
  calls.list = [];
  daemonCallMock.mockReset();
});

describe("FileFinder component", () => {
  it("fetches index and renders files on open", async () => {
    mockRpc({
      "file.index": () => ({
        paths: ["src/main.rs", "crates/lib.rs", "README.md"],
        truncated: false,
        generation: 1,
      }),
    });

    const editor = makeMockEditorStore(1);
    const onClose = vi.fn();
    const onOpenPath = vi.fn();

    render(() => (
      <FileFinder
        editor={editor}
        isOpen={true}
        onClose={onClose}
        onOpenPath={onOpenPath}
      />
    ));

    await waitFor(() => {
      expect(screen.getByText("main.rs")).toBeDefined();
      expect(screen.getByText("lib.rs")).toBeDefined();
      expect(screen.getByText("README.md")).toBeDefined();
    });

    expect(calls.list.some((c) => c.method === "file.index")).toBe(true);
  });

  it("filters files as query is typed", async () => {
    mockRpc({
      "file.index": () => ({
        paths: ["src/main.rs", "src/parser.rs", "README.md"],
        truncated: false,
        generation: 1,
      }),
    });

    const editor = makeMockEditorStore(1);
    render(() => (
      <FileFinder
        editor={editor}
        isOpen={true}
        onClose={vi.fn()}
      />
    ));

    await waitFor(() => {
      expect(screen.getByText("main.rs")).toBeDefined();
    });

    const input = screen.getByPlaceholderText("Search files by name...");
    fireEvent.input(input, { target: { value: "parser" } });

    await waitFor(() => {
      expect(screen.queryByTitle("src/main.rs")).toBeNull();
      expect(screen.getByTitle("src/parser.rs")).toBeDefined();
    });
  });

  it("navigates selection with arrow keys and opens on enter", async () => {
    mockRpc({
      "file.index": () => ({
        paths: ["file_a.txt", "file_b.txt", "file_c.txt"],
        truncated: false,
        generation: 1,
      }),
    });

    const editor = makeMockEditorStore(1);
    const onClose = vi.fn();
    const onOpenPath = vi.fn();

    render(() => (
      <FileFinder
        editor={editor}
        isOpen={true}
        onClose={onClose}
        onOpenPath={onOpenPath}
      />
    ));

    await waitFor(() => {
      expect(screen.getByText("file_a.txt")).toBeDefined();
    });

    const input = screen.getByPlaceholderText("Search files by name...");
    // Press ArrowDown to select file_b.txt
    fireEvent.keyDown(input, { key: "ArrowDown" });
    // Press Enter to open file_b.txt
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onClose).toHaveBeenCalled();
    expect(onOpenPath).toHaveBeenCalledWith("file_b.txt");
  });

  it("closes when escape is pressed", async () => {
    mockRpc({
      "file.index": () => ({
        paths: ["alpha.txt"],
        truncated: false,
        generation: 1,
      }),
    });

    const editor = makeMockEditorStore(1);
    const onClose = vi.fn();

    render(() => (
      <FileFinder
        editor={editor}
        isOpen={true}
        onClose={onClose}
      />
    ));

    await waitFor(() => {
      expect(screen.getByText("alpha.txt")).toBeDefined();
    });

    const dialog = screen.getByRole("dialog").querySelector(".shadow-2xl")!;
    fireEvent.keyDown(dialog, { key: "Escape" });

    expect(onClose).toHaveBeenCalled();
  });

  it("ignores responses for superseded lane when lane switches while finder is open", async () => {
    let resolveLane1: (val: any) => void;
    const p1 = new Promise((res) => {
      resolveLane1 = res;
    });

    daemonCallMock.mockImplementation((method: string, params?: any) => {
      if (method === "file.index") {
        if (params?.lane_id === 1) return p1;
        if (params?.lane_id === 2) {
          return Promise.resolve({
            paths: ["lane2_file.txt"],
            truncated: false,
            generation: 1,
          });
        }
      }
      return Promise.resolve({});
    });

    const [currentLaneId, setCurrentLaneId] = createSignal(1);
    const editor = {
      selectedLane: () => ({ id: currentLaneId(), worktree: { name: "test-repo", path: "/tmp/test" } }),
      openFile: vi.fn(),
    } as unknown as import("../stores/editor").EditorStore;

    render(() => (
      <FileFinder
        editor={editor}
        isOpen={true}
        onClose={vi.fn()}
      />
    ));

    // Switch lane to 2 before lane 1's slow index resolves
    setCurrentLaneId(2);

    await waitFor(() => {
      expect(screen.getByText("lane2_file.txt")).toBeDefined();
    });

    // Now resolve lane 1's slow response
    resolveLane1!({
      paths: ["lane1_stale.txt"],
      truncated: false,
      generation: 1,
    });

    await new Promise((r) => setTimeout(r, 50));

    // Stale lane 1 response must NOT overwrite lane 2's paths
    expect(screen.queryByText("lane1_stale.txt")).toBeNull();
    expect(screen.getByText("lane2_file.txt")).toBeDefined();
  });
});
