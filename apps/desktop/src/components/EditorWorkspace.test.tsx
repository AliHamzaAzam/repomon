import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FileEntry, Lane, Repo } from "../bindings";
import type { FleetStore } from "../stores/fleet";
import { createEditorStore } from "../stores/editor";
import EditorWorkspace from "./EditorWorkspace";

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
});
