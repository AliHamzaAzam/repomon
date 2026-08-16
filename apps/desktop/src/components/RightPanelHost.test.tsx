import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import App from "../App";
import type { ConnectionSnapshot, ConnectionSource } from "../ipc/connection";
import { RIGHT_PANEL_ACTIVE_TAB_KEY } from "../stores/uiSettings";
import { IconGitBranch, IconLayers } from "./icons";
import RightPanelHost, { type RightPanelTabDef } from "./RightPanelHost";

const REPOMIND_OPEN_KEY = "repomon.repomind_open";
const RIGHT_PANEL_WIDTH_KEY = "repomon:right-panel-width";

function sourceFor(snapshot: ConnectionSnapshot): ConnectionSource {
  return {
    current: async () => snapshot,
    subscribe: async () => () => undefined,
  };
}

afterEach(() => {
  cleanup();
  localStorage.removeItem(REPOMIND_OPEN_KEY);
  localStorage.removeItem(RIGHT_PANEL_WIDTH_KEY);
  localStorage.removeItem(RIGHT_PANEL_ACTIVE_TAB_KEY);
  document.documentElement.style.removeProperty("--right-panel-width");
});

// A fake two-tab registry exercises routing and switching without depending on RepomindPanel's
// daemon calls. Each panel counts its own onMount to prove F2's warm-keep decision: switching
// tabs hides the inactive panel (inert + visually hidden), it does not unmount it.
function makeCountingPanel(id: string, label: string, mounts: Record<string, number>): RightPanelTabDef {
  return {
    id,
    label,
    icon: id === "alpha" ? IconGitBranch : IconLayers,
    component: () => {
      mounts[id] = (mounts[id] ?? 0) + 1;
      return <div data-testid={`panel-${id}`}>{label} content</div>;
    },
  };
}

describe("RightPanelHost", () => {
  it("hides the tab strip when only one panel is registered", () => {
    const mounts: Record<string, number> = {};
    render(() => (
      <RightPanelHost
        onToggleFullscreen={() => {}}
        panels={[makeCountingPanel("alpha", "Alpha", mounts)]}
      />
    ));

    expect(screen.queryByRole("tablist", { name: "Right panel" })).not.toBeInTheDocument();
    expect(screen.getByTestId("panel-alpha")).toBeInTheDocument();
  });

  it("routes to the panel matching the active tab and switches on click", () => {
    const mounts: Record<string, number> = {};
    render(() => (
      <RightPanelHost
        onToggleFullscreen={() => {}}
        panels={[
          makeCountingPanel("alpha", "Alpha", mounts),
          makeCountingPanel("beta", "Beta", mounts),
        ]}
      />
    ));

    const tablist = screen.getByRole("tablist", { name: "Right panel" });
    expect(tablist).toBeInTheDocument();

    const alphaTab = within(tablist).getByRole("tab", { name: "Alpha" });
    const betaTab = within(tablist).getByRole("tab", { name: "Beta" });
    expect(alphaTab).toHaveAttribute("aria-selected", "true");
    expect(betaTab).toHaveAttribute("aria-selected", "false");

    // jsdom does not reflect the `inert` IDL property onto an attribute, so assert via the
    // aria-hidden attribute the host sets in lockstep with it (both driven by the same
    // `isActive()` check in RightPanelHost).
    const alphaPanel = screen.getByTestId("panel-alpha").parentElement as HTMLElement;
    const betaPanel = screen.getByTestId("panel-beta").parentElement as HTMLElement;
    expect(alphaPanel).not.toHaveAttribute("aria-hidden");
    expect(betaPanel).toHaveAttribute("aria-hidden", "true");

    fireEvent.click(betaTab);

    expect(alphaTab).toHaveAttribute("aria-selected", "false");
    expect(betaTab).toHaveAttribute("aria-selected", "true");
    expect(alphaPanel).toHaveAttribute("aria-hidden", "true");
    expect(betaPanel).not.toHaveAttribute("aria-hidden");
  });

  it("keeps inactive panels mounted (warm) instead of remounting on tab switch", () => {
    const mounts: Record<string, number> = {};
    render(() => (
      <RightPanelHost
        onToggleFullscreen={() => {}}
        panels={[
          makeCountingPanel("alpha", "Alpha", mounts),
          makeCountingPanel("beta", "Beta", mounts),
        ]}
      />
    ));

    expect(mounts.alpha).toBe(1);
    expect(mounts.beta).toBe(1);

    fireEvent.click(screen.getByRole("tab", { name: "Beta" }));
    fireEvent.click(screen.getByRole("tab", { name: "Alpha" }));
    fireEvent.click(screen.getByRole("tab", { name: "Beta" }));

    // Neither panel's component factory runs again — both stayed mounted the whole time.
    expect(mounts.alpha).toBe(1);
    expect(mounts.beta).toBe(1);
  });

  it("persists the active tab across remounts", () => {
    const mounts: Record<string, number> = {};
    const panels = () => [
      makeCountingPanel("alpha", "Alpha", mounts),
      makeCountingPanel("beta", "Beta", mounts),
    ];

    const { unmount } = render(() => <RightPanelHost onToggleFullscreen={() => {}} panels={panels()} />);
    fireEvent.click(screen.getByRole("tab", { name: "Beta" }));
    expect(localStorage.getItem(RIGHT_PANEL_ACTIVE_TAB_KEY)).toBe("beta");
    unmount();

    render(() => <RightPanelHost onToggleFullscreen={() => {}} panels={panels()} />);
    expect(screen.getByRole("tab", { name: "Beta" })).toHaveAttribute("aria-selected", "true");
  });

  it("falls back to the first registered tab when the persisted id no longer exists", () => {
    localStorage.setItem(RIGHT_PANEL_ACTIVE_TAB_KEY, "stale-panel-id");
    const mounts: Record<string, number> = {};
    render(() => (
      <RightPanelHost
        onToggleFullscreen={() => {}}
        panels={[makeCountingPanel("alpha", "Alpha", mounts), makeCountingPanel("beta", "Beta", mounts)]}
      />
    ));

    expect(screen.getByRole("tab", { name: "Alpha" })).toHaveAttribute("aria-selected", "true");
  });
});

describe("Right rail integration (App)", () => {
  beforeEach(() => {
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
  });

  function renderApp() {
    return render(() => (
      <App
        connectionSource={sourceFor({
          phase: "starting",
          endpoint: "Resolving local daemon endpoint",
          message: null,
          daemon: null,
        })}
      />
    ));
  }

  it("preserves open/close toggling, its keymap binding, and localStorage persistence", async () => {
    const { container } = renderApp();

    const toggle = within(container).getByRole("button", { name: "Repomind" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toHaveAttribute("aria-pressed", "true"));
    expect(localStorage.getItem(REPOMIND_OPEN_KEY)).toBe("true");

    // The Repomind content (still real RepomindPanel, unchanged) is now live in the right rail.
    // Scoped to RepomindPanel's own header span (not a bare `getByText`): C1 registers a second
    // tab, so the tab strip's "Repomind" pill now sits alongside it in the same `aside`, and both
    // are plain-text matches for "Repomind".
    const aside = within(container).getByRole("complementary", { name: "Repomind" });
    expect(within(aside).getByText("Repomind", { selector: "span.text-xs.font-semibold" })).toBeInTheDocument();

    // mod+5 is the keymap binding for panel.repomind (src/keymap.ts) — toggling via keyboard
    // must reach the same state as the header button.
    fireEvent.keyDown(window, { key: "5", code: "Digit5", metaKey: true });
    await waitFor(() => expect(toggle).toHaveAttribute("aria-pressed", "false"));
    expect(localStorage.getItem(REPOMIND_OPEN_KEY)).toBe("false");
  });

  it("preserves the fullscreen takeover with Escape-to-close", async () => {
    const { container } = renderApp();

    fireEvent.keyDown(window, { key: "5", code: "Digit5", metaKey: true, shiftKey: true });
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: "Repomind, full screen" })).toBeInTheDocument();
    });

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Repomind, full screen" })).not.toBeInTheDocument();
    });
    void container;
  });

  it("wires ResizableSplit's width var onto the mission grid when the panel opens", async () => {
    const { container } = renderApp();

    const toggle = within(container).getByRole("button", { name: "Repomind" });
    fireEvent.click(toggle);

    await waitFor(() => {
      expect(document.documentElement.style.getPropertyValue("--right-panel-width")).toBe("320px");
    });
    expect(within(container).getByRole("separator", { name: "Resize right panel" })).toBeInTheDocument();
  });
});
