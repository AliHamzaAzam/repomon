import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

// PdfViewer owns its own pdf.js pipeline and is covered by PdfViewer.test.tsx; here it's replaced
// with a stand-in that reports fixed state, so the status line's kind-aware rendering can be
// tested without dragging pdf.js, its worker, and the asset-protocol IPC into this file too.
vi.mock("./PdfViewer", () => ({
  default: (props: { onStateChange?: (state: { numPages: number; zoomPercent: number } | null) => void }) => {
    props.onStateChange?.({ numPages: 3, zoomPercent: 100 });
    return <div data-testid="pdf-viewer-stub" />;
  },
}));

// Same reasoning as the PdfViewer stub above: ImageViewer owns its own asset-protocol load and
// zoom/pan state and is covered by ImageViewer.test.tsx; here it just reports fixed state so the
// status line's image branch can be tested in isolation.
vi.mock("./ImageViewer", () => ({
  default: (props: {
    onStateChange?: (
      state: {
        format: string;
        width: number | null;
        height: number | null;
        sizeBytes: number;
        zoomPercent: number;
        animated: boolean;
      } | null,
    ) => void;
  }) => {
    props.onStateChange?.({ format: "PNG", width: 432, height: 900, sizeBytes: 71168, zoomPercent: 100, animated: false });
    return <div data-testid="image-viewer-stub" />;
  },
}));

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

// jsdom has no createObjectURL/revokeObjectURL on URL at all (SvgPreview is the only component in
// this file that calls them). Stubbing them for the whole file, reset before every test, keeps
// them always callable even if a reactive effect from an unmounted component happens to fire late
// - reassigning them to `undefined` per-test previously broke unrelated later tests that way.
let capturedSvgBlobs: Blob[] = [];

beforeEach(() => {
  capturedSvgBlobs = [];
  URL.createObjectURL = vi.fn((blob: Blob) => {
    capturedSvgBlobs.push(blob);
    return `blob:mock-${capturedSvgBlobs.length}`;
  });
  URL.revokeObjectURL = vi.fn();
});

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
    role: null,
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

  it("shows PDF status and hides the code items for a pdf tab", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [entry({ name: "report.pdf", path: "report.pdf", is_dir: false })],
        truncated: false,
      }),
      "file.read": () => ({
        content: "",
        mtime_ms: 1000,
        size: 2048,
        truncated: false,
        kind: "pdf",
      }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("report.pdf")).toBeInTheDocument());
    fireEvent.click(screen.getByText("report.pdf"));
    await waitFor(() => expect(editor.activePath()).toBe("report.pdf"));

    // The code-editor status items (language, cursor position, indent, wrap, whitespace) are
    // gone for a pdf tab - PdfViewer's stub reports its own state instead.
    expect(screen.queryByTitle("Click to override syntax language")).not.toBeInTheDocument();
    expect(screen.queryByText(/^Ln \d+, Col \d+$/)).not.toBeInTheDocument();
    expect(screen.queryByTitle("Toggle line wrapping")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Toggle render whitespace")).not.toBeInTheDocument();

    expect(screen.getByText("PDF · 3 pages · 2.0 KB")).toBeInTheDocument();
    expect(screen.getByText("100%")).toBeInTheDocument();
  });

  it("shows image status (kind, dimensions, size, zoom) and hides the code items for an image tab", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [entry({ name: "logo.png", path: "logo.png", is_dir: false })],
        truncated: false,
      }),
      "file.read": () => ({
        content: "",
        mtime_ms: 1000,
        size: 71168,
        truncated: false,
        kind: "image",
      }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("logo.png")).toBeInTheDocument());
    fireEvent.click(screen.getByText("logo.png"));
    await waitFor(() => expect(editor.activePath()).toBe("logo.png"));

    // Same code-item hiding as the pdf tab above - ImageViewer's stub reports its own state.
    expect(screen.queryByTitle("Click to override syntax language")).not.toBeInTheDocument();
    expect(screen.queryByText(/^Ln \d+, Col \d+$/)).not.toBeInTheDocument();
    expect(screen.queryByTitle("Toggle line wrapping")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Toggle render whitespace")).not.toBeInTheDocument();

    expect(screen.getByText("PNG · 432 x 900 · 69.5 KB")).toBeInTheDocument();
    expect(screen.getByText("100%")).toBeInTheDocument();
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

  it("dismisses the tree context menu on Escape and returns focus to the row that opened it (brief E item 5)", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [entry({ name: "notes.txt", path: "notes.txt", is_dir: false })],
        truncated: false,
      }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("notes.txt")).toBeInTheDocument());
    const row = screen.getByTitle("notes.txt");
    fireEvent.contextMenu(row);

    await waitFor(() => expect(screen.getByText("New File")).toBeInTheDocument());

    fireEvent.keyDown(window, { key: "Escape" });

    await waitFor(() => expect(screen.queryByText("New File")).not.toBeInTheDocument());
    expect(document.activeElement).toBe(row);
  });

  it("issues exactly one file.create RPC when Enter is followed by blur during inline create (brief E item 7)", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({ entries: [], truncated: false }),
      "file.create": () => ({}),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    fireEvent.click(screen.getByTitle("New file in root"));

    const input = await screen.findByPlaceholderText("File name...");
    fireEvent.input(input, { target: { value: "notes.txt" } });

    // Enter starts the RPC; blur fires immediately after, before the RPC settles. The second
    // trigger must be a no-op, not a second file.create call.
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.blur(input);

    await waitFor(() => {
      expect(screen.queryByPlaceholderText("File name...")).not.toBeInTheDocument();
    });

    expect(calls.list.filter((c) => c.method === "file.create")).toHaveLength(1);
  });

  it("issues exactly one file.rename RPC when Enter is followed by blur during inline rename (brief E item 7)", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [entry({ name: "old.txt", path: "old.txt", is_dir: false })],
        truncated: false,
      }),
      "file.rename": () => ({}),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("old.txt")).toBeInTheDocument());
    fireEvent.contextMenu(screen.getByTitle("old.txt"));

    await waitFor(() => expect(screen.getByText("Rename")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Rename"));

    const input = await screen.findByDisplayValue("old.txt");
    fireEvent.input(input, { target: { value: "new.txt" } });

    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.blur(input);

    await waitFor(() => {
      expect(screen.queryByDisplayValue("new.txt")).not.toBeInTheDocument();
    });

    expect(calls.list.filter((c) => c.method === "file.rename")).toHaveLength(1);
  });

  it("toggles markdown preview split for markdown files (item F5)", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [
          entry({ name: "notes.md", path: "notes.md", is_dir: false }),
          entry({ name: "main.ts", path: "main.ts", is_dir: false }),
        ],
        truncated: false,
      }),
      "file.read": (p: unknown) => {
        const path = (p as { path: string }).path;
        if (path === "notes.md") {
          return { content: "# Heading\n\nPreview text", mtime_ms: 1000, size: 25, truncated: false, kind: "text" };
        }
        return { content: "const x = 1;", mtime_ms: 1000, size: 12, truncated: false, kind: "text" };
      },
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    const { container } = render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("main.ts")).toBeInTheDocument());
    fireEvent.click(screen.getByText("main.ts"));
    await waitFor(() => expect(editor.activePath()).toBe("main.ts"));

    // Preview toggle is not in status bar for non-markdown file
    expect(screen.queryByText(/Preview: (On|Off)/)).not.toBeInTheDocument();

    // Open markdown file
    fireEvent.click(screen.getByText("notes.md"));
    await waitFor(() => expect(editor.activePath()).toBe("notes.md"));

    // Preview toggle appears in status bar
    const previewBtn = await screen.findByText("Preview: Off");
    expect(previewBtn).toBeInTheDocument();
    expect(container.querySelector("[data-testid='markdown-preview']")).not.toBeInTheDocument();

    // Toggle preview on
    fireEvent.click(previewBtn);
    expect(await screen.findByText("Preview: On")).toBeInTheDocument();
    expect(container.querySelector("[data-testid='markdown-preview']")).toBeInTheDocument();

    // CodeEditor view is still mounted
    expect(container.querySelector(".cm-content")).toBeInTheDocument();

    // Toggle preview off
    fireEvent.click(screen.getByText("Preview: On"));
    expect(await screen.findByText("Preview: Off")).toBeInTheDocument();
    expect(container.querySelector("[data-testid='markdown-preview']")).not.toBeInTheDocument();
    expect(container.querySelector(".cm-content")).toBeInTheDocument();
  });

  it("shows the Preview toggle for an svg tab and renders sanitized markup in the split", async () => {
    const currentLane = lane();
    const svgWithScript =
      '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">' +
      "<script>alert(1)</script>" +
      '<rect onclick="alert(2)" width="10" height="10" fill="red" /></svg>';
    mockRpc({
      "file.list": () => ({
        entries: [entry({ name: "icon.svg", path: "icon.svg", is_dir: false })],
        truncated: false,
      }),
      "file.read": () => ({
        content: svgWithScript,
        mtime_ms: 1000,
        size: svgWithScript.length,
        truncated: false,
        kind: "text",
      }),
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    const { container } = render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("icon.svg")).toBeInTheDocument());
    fireEvent.click(screen.getByText("icon.svg"));
    await waitFor(() => expect(editor.activePath()).toBe("icon.svg"));

    const previewBtn = await screen.findByText("Preview: Off");
    expect(container.querySelector("[data-testid='svg-preview']")).not.toBeInTheDocument();

    fireEvent.click(previewBtn);
    expect(await screen.findByText("Preview: On")).toBeInTheDocument();
    expect(container.querySelector("[data-testid='svg-preview']")).toBeInTheDocument();
    // The code editor stays mounted beside the split, same as the markdown case.
    expect(container.querySelector(".cm-content")).toBeInTheDocument();

    expect(capturedSvgBlobs.length).toBeGreaterThan(0);
    // jsdom's Blob has no `.text()` method, so read it back through FileReader instead.
    const sanitized = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result));
      reader.onerror = () => reject(reader.error);
      reader.readAsText(capturedSvgBlobs[capturedSvgBlobs.length - 1]);
    });
    expect(sanitized).not.toContain("<script");
    expect(sanitized).not.toContain("onclick");
    expect(sanitized).toContain("<rect");
  });

  it("renders Large file: read-only status line note for files over 2 MiB (item F6)", async () => {
    const currentLane = lane();
    mockRpc({
      "file.list": () => ({
        entries: [
          entry({ name: "large.txt", path: "large.txt", is_dir: false }),
          entry({ name: "small.txt", path: "small.txt", is_dir: false }),
        ],
        truncated: false,
      }),
      "file.read": (p: unknown) => {
        const path = (p as { path: string }).path;
        if (path === "large.txt") {
          return {
            content: "lots of data",
            mtime_ms: 1000,
            size: 3 * 1024 * 1024,
            truncated: false,
            kind: "text",
            large: true,
          };
        }
        return {
          content: "small data",
          mtime_ms: 1000,
          size: 100,
          truncated: false,
          kind: "text",
          large: false,
        };
      },
    });

    const fleet = fleetWith(currentLane);
    const editor = createEditorStore(fleet);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    await waitFor(() => expect(screen.getByText("large.txt")).toBeInTheDocument());
    fireEvent.click(screen.getByText("large.txt"));
    await waitFor(() => expect(editor.activePath()).toBe("large.txt"));

    expect(await screen.findByText("Large file: read-only")).toBeInTheDocument();

    // Switch to small file
    fireEvent.click(screen.getByText("small.txt"));
    await waitFor(() => expect(editor.activePath()).toBe("small.txt"));
    expect(screen.queryByText("Large file: read-only")).not.toBeInTheDocument();
  });
});
