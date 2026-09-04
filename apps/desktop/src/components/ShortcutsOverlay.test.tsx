import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import ShortcutsOverlay from "./ShortcutsOverlay";

vi.mock("../ipc/rpc", () => ({
  daemonCall: () => Promise.resolve(null),
}));

function fleetStub(): FleetStore {
  return { refresh: vi.fn().mockResolvedValue(undefined) } as unknown as FleetStore;
}

afterEach(() => {
  cleanup();
});

describe("ShortcutsOverlay", () => {
  it("renders nothing when closed, and the dialog once opened", async () => {
    const actions = createActionsStore(fleetStub());
    render(() => <ShortcutsOverlay actions={actions} />);
    expect(screen.queryByRole("dialog", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();

    actions.openShortcutsGuide();
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    });
  });

  it("opens, focuses search, and closes on Escape", async () => {
    const actions = createActionsStore(fleetStub());
    render(() => <ShortcutsOverlay actions={actions} />);

    actions.openShortcutsGuide();
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    });
    expect(screen.getByPlaceholderText("Search shortcuts")).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      expect(actions.shortcutsGuideOpen()).toBe(false);
    });
  });

  it("closes on a click outside the dialog", async () => {
    const actions = createActionsStore(fleetStub());
    render(() => <ShortcutsOverlay actions={actions} />);

    actions.openShortcutsGuide();
    const dialog = await screen.findByRole("dialog", { name: "Keyboard shortcuts" });
    const backdrop = dialog.parentElement;
    expect(backdrop).not.toBeNull();

    fireEvent.click(backdrop as Element);
    await waitFor(() => {
      expect(actions.shortcutsGuideOpen()).toBe(false);
    });
  });

  it("routes to the full Settings reference and closes itself", async () => {
    const actions = createActionsStore(fleetStub());
    render(() => <ShortcutsOverlay actions={actions} />);

    actions.openShortcutsGuide();
    await screen.findByRole("dialog", { name: "Keyboard shortcuts" });

    fireEvent.click(screen.getByText("Open full reference in Settings"));

    expect(actions.settingsTab()).toBe("keyboard");
    expect(actions.settingsOpen()).toBe(true);
    await waitFor(() => {
      expect(actions.shortcutsGuideOpen()).toBe(false);
    });
  });

  it("hides the print action and conflict/caveat banners in its compact variant", async () => {
    const actions = createActionsStore(fleetStub());
    render(() => <ShortcutsOverlay actions={actions} />);

    actions.openShortcutsGuide();
    await screen.findByRole("dialog", { name: "Keyboard shortcuts" });

    expect(screen.queryByText("Print cheat sheet")).not.toBeInTheDocument();
  });
});
