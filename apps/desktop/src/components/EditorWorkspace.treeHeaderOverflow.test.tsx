import { cleanup, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FleetStore } from "../stores/fleet";
import { createEditorStore, MIN_TREE_WIDTH_PX } from "../stores/editor";
import EditorWorkspace from "./EditorWorkspace";

// jsdom cannot measure overflow, so verify wrapping, shrinkable labels, fixed action targets, and
// accessible names structurally.

vi.mock("../ipc/rpc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/rpc")>();
  return {
    ...actual,
    daemonCall: () => Promise.resolve({}),
    subscribeDaemon: () => Promise.resolve(() => {}),
  };
});

afterEach(() => {
  cleanup();
  localStorage.clear();
});

function fleetWith(current: null) {
  return { selectedLane: () => current, setSelectedLaneId: () => {} } as unknown as FleetStore;
}

const TREE_HEADER_ACTIONS = [
  "New file in root",
  "New folder in root",
  "Find file",
  "Reveal active file in tree",
  "Refresh file tree",
];

describe("EditorWorkspace tree column header overflow", () => {
  it("keeps the icon action group shrink-0 and the mode switcher shrinkable, with a wrap fallback", async () => {
    const fleet = fleetWith(null);
    const editor = createEditorStore(fleet);
    const { container } = render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    const modeSwitcher = container.querySelector<HTMLElement>(
      'button[aria-label="Files explorer"]'
    )?.parentElement;
    expect(modeSwitcher).toBeTruthy();
    expect(modeSwitcher?.className).toContain("min-w-0");

    const actionGroup = container.querySelector<HTMLElement>(
      'button[aria-label="Refresh file tree"]'
    )?.parentElement;
    expect(actionGroup).toBeTruthy();
    expect(actionGroup?.className).toContain("shrink-0");

    const header = modeSwitcher?.parentElement;
    expect(header).toBeTruthy();
    expect(header?.className).toContain("flex-wrap");
  });

  it("collapses the mode switcher to icon-only at the minimum column width, keeping every action reachable", async () => {
    const fleet = fleetWith(null);
    const editor = createEditorStore(fleet);
    editor.setTreeColumnWidth(MIN_TREE_WIDTH_PX);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    // Labels collapse away at the floor width...
    expect(screen.queryByText("Files")).not.toBeInTheDocument();
    expect(screen.queryByText("Search")).not.toBeInTheDocument();

    // ...but the mode switcher buttons are still present and keyboard-reachable
    // by their accessible name, and no icon action was dropped to make room.
    expect(screen.getByRole("button", { name: "Files explorer" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Search in project" })).toBeInTheDocument();
    for (const label of TREE_HEADER_ACTIONS) {
      expect(screen.getByRole("button", { name: label })).toBeInTheDocument();
    }
  });

  it("restores the mode switcher's text labels once the column is wide enough", async () => {
    const fleet = fleetWith(null);
    const editor = createEditorStore(fleet);
    editor.setTreeColumnWidth(480);
    render(() => <EditorWorkspace fleet={fleet} editor={editor} />);

    expect(screen.getByText("Files")).toBeInTheDocument();
    expect(screen.getByText("Search")).toBeInTheDocument();
  });
});
