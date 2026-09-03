import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FileEntry, Lane, Repo } from "../bindings";
import type { FleetStore } from "../stores/fleet";
import { createEditorStore, EDITOR_STORAGE_KEY } from "../stores/editor";
import EditorWorkspace from "./EditorWorkspace";

function getView(container: HTMLElement): EditorView {
  const content = container.querySelector<HTMLElement>(".cm-content");
  if (!content) throw new Error("no .cm-content rendered - is a file open?");
  const view = EditorView.findFromDOM(content);
  if (!view) throw new Error("EditorView.findFromDOM found no view");
  return view;
}

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

afterEach(() => {
  cleanup();
  localStorage.clear();
  calls.list = [];
  subscribers.list = [];
  daemonCallMock.mockReset();
});

function repo(): Repo {
  return { id: 2, path: "/code/repomon", name: "repomon", added_at: "2026-07-20T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null };
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

function fleetWith(current: Lane | null): FleetStore {
  return { selectedLane: () => current, setSelectedLaneId: () => {} } as unknown as FleetStore;
}

describe("EditorWorkspace component", () => {
  it("renders empty state when no files are open", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({ entries: [], truncated: false }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    expect(screen.getByText("No file open")).toBeInTheDocument();
    expect(screen.getByText("Select a file from the tree on the left to view or edit.")).toBeInTheDocument();
  });

  it("lists directory entries and opens file on click", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [
          entry({ name: "src", path: "src", is_dir: true }),
          entry({ name: "README.md", path: "README.md", is_dir: false }),
        ],
        truncated: false,
      }),
      "file.read": () => ({
        content: "# Repomon Docs\nHello world",
        mtime_ms: 1000,
        size: 26,
        truncated: false,
        kind: "text",
      }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("README.md")).toBeInTheDocument());
    fireEvent.click(screen.getByText("README.md"));

    await waitFor(() => {
      expect(editor.activePath()).toBe("README.md");
      expect(screen.getByText("Ln 1, Col 1")).toBeInTheDocument();
    });
  });

  it("filters tree entries using the search input", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [
          entry({ name: "Cargo.toml", path: "Cargo.toml", is_dir: false }),
          entry({ name: "README.md", path: "README.md", is_dir: false }),
        ],
        truncated: false,
      }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => {
      expect(screen.getByText("Cargo.toml")).toBeInTheDocument();
      expect(screen.getByText("README.md")).toBeInTheDocument();
    });

    const filterInput = screen.getByPlaceholderText("Filter files...");
    fireEvent.input(filterInput, { target: { value: "cargo" } });

    await waitFor(() => {
      expect(screen.getByText("Cargo.toml")).toBeInTheDocument();
      expect(screen.queryByText("README.md")).not.toBeInTheDocument();
    });
  });

  it("toggles wrap and whitespace from the status line", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({ entries: [], truncated: false }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    const wrapButton = screen.getByTitle("Toggle line wrapping");
    const whitespaceButton = screen.getByTitle("Toggle render whitespace");

    expect(wrapButton).toHaveTextContent("Wrap: Off");
    expect(whitespaceButton).toHaveTextContent("Whitespace: Off");

    fireEvent.click(wrapButton);
    expect(wrapButton).toHaveTextContent("Wrap: On");
    expect(editor.wrap()).toBe(true);

    fireEvent.click(whitespaceButton);
    expect(whitespaceButton).toHaveTextContent("Whitespace: On");
    expect(editor.whitespace()).toBe(true);
  });

  it("allows overriding the syntax language from status line menu", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [entry({ name: "script", path: "script", is_dir: false })],
        truncated: false,
      }),
      "file.read": () => ({
        content: "echo hello",
        mtime_ms: 1000,
        size: 10,
        truncated: false,
        kind: "text",
      }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("script")).toBeInTheDocument());
    fireEvent.click(screen.getByText("script"));

    await waitFor(() => expect(editor.activePath()).toBe("script"));

    const langButton = screen.getByTitle("Click to override syntax language");
    fireEvent.click(langButton);

    const rustOption = screen.getByRole("button", { name: "Rust" });
    fireEvent.click(rustOption);

    expect(editor.languageOverrides()["script"]).toBe("rust");
  });

  it("restores each tab's own cursor position when switching between open tabs (item 5)", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [
          entry({ name: "a.ts", path: "a.ts", is_dir: false }),
          entry({ name: "b.ts", path: "b.ts", is_dir: false }),
        ],
        truncated: false,
      }),
      "file.read": (params) => {
        const p = (params as { path: string }).path;
        return {
          content: p === "a.ts" ? "const alpha = 1;" : "const beta = 2;",
          mtime_ms: 1000,
          size: 20,
          truncated: false,
          kind: "text",
        };
      },
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    const { container } = render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("a.ts")).toBeInTheDocument());
    fireEvent.click(screen.getByText("a.ts"));
    await waitFor(() => expect(editor.activePath()).toBe("a.ts"));

    // The center workspace keeps a single CodeEditor instance alive across tab switches (see
    // FileEditorPanel.tsx's non-keyed Show fix), so this same `view` stays valid the whole test.
    const view = getView(container);
    view.dispatch({ selection: { anchor: 5 } });
    expect(view.state.selection.main.head).toBe(5);

    fireEvent.click(screen.getByText("b.ts"));
    await waitFor(() => expect(editor.activePath()).toBe("b.ts"));
    // b.ts has never had a saved cursor - starts at the document head.
    expect(view.state.selection.main.head).toBe(0);

    const tabRowA = screen.getByLabelText("Close a.ts").closest("div");
    const tabButtonA = tabRowA?.querySelector<HTMLElement>('button[title="a.ts"]');
    expect(tabButtonA).toBeTruthy();
    fireEvent.click(tabButtonA!);
    await waitFor(() => expect(editor.activePath()).toBe("a.ts"));

    // a.ts's own cursor must be re-applied on switching back to it - not just once at mount.
    expect(view.state.selection.main.head).toBe(5);
  });

  it("shows selection count and selected character count in the status line (item 6)", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({ entries: [entry({ name: "a.ts", path: "a.ts", is_dir: false })], truncated: false }),
      "file.read": () => ({ content: "hello world", mtime_ms: 1000, size: 11, truncated: false, kind: "text" }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    const { container } = render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("a.ts")).toBeInTheDocument());
    fireEvent.click(screen.getByText("a.ts"));
    await waitFor(() => expect(editor.activePath()).toBe("a.ts"));

    // A plain cursor (no selection) shows no selection/char indicator next to Ln/Col.
    expect(screen.queryByText(/chars$/)).not.toBeInTheDocument();

    const view = getView(container);
    view.dispatch({ selection: { anchor: 0, head: 5 } });

    await waitFor(() => expect(screen.getByText("5 chars")).toBeInTheDocument());
  });

  it("persists the tree column width to localStorage once, on drag end, not on every mousemove (item 8)", async () => {
    const currentLane = lane();
    mockRpc({ "file.list": () => ({ entries: [], truncated: false }) });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    const { container } = render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    const divider = container.querySelector<HTMLElement>(".cursor-col-resize");
    expect(divider).toBeTruthy();
    fireEvent.mouseDown(divider!, { clientX: 0 });

    fireEvent.mouseMove(window, { clientX: 40 });
    fireEvent.mouseMove(window, { clientX: 80 });
    fireEvent.mouseMove(window, { clientX: 120 });

    // The live signal updates on every mousemove...
    expect(editor.treeColumnWidth()).toBeGreaterThan(240);
    // ...but nothing is written to localStorage until the drag ends.
    expect(localStorage.getItem(EDITOR_STORAGE_KEY)).toBeNull();

    fireEvent.mouseUp(window);

    const stored = JSON.parse(localStorage.getItem(EDITOR_STORAGE_KEY) ?? "null");
    expect(stored?.treeColumnWidth).toBe(editor.treeColumnWidth());
  });
});
