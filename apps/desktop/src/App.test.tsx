import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import App from "./App";
import type { ConnectionSnapshot, ConnectionSource } from "./ipc/connection";
import type { FleetSource } from "./stores/fleet";
import { formatChord } from "./keymap";
import { SHORTCUTS_HINT_LAUNCH_COUNT_KEY } from "./stores/uiSettings";

function sourceFor(snapshot: ConnectionSnapshot): ConnectionSource {
  return {
    current: async () => snapshot,
    subscribe: async () => () => undefined,
  };
}

describe("Repomon desktop shell", () => {
  beforeAll(() => {
    // App.tsx's shortcut handler resolves "mod" from navigator.platform when no explicit
    // platform is passed in (the real, unmocked path the app uses at runtime). jsdom reports an
    // empty platform string, so pin it to macOS here: the fixtures below fire metaKey to mean
    // "mod", matching how the app actually runs on macOS.
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
  });

  // Without this, each test's <App> stays mounted (and its window keydown listener stays live)
  // for the rest of the file. That was merely untidy until App.tsx's global shortcut handler
  // started honoring `event.defaultPrevented`: an earlier, still-mounted instance's listener
  // runs first, calls preventDefault() on its own matched binding, and the current test's
  // instance then sees the same event as already handled and skips it.
  afterEach(() => {
    cleanup();
  });


  it("renders the mission control frame and connection rail", () => {
    render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    expect(screen.getByRole("heading", { name: "Repomon" })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Fleet" })).toBeInTheDocument();
    expect(screen.getByRole("main", { name: "Terminal bay" })).toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "Repomind" })).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "Daemon connection" })).toBeInTheDocument();
  });

  it("shows live daemon metrics when the host connects", async () => {
    render(() => <App connectionSource={sourceFor({
      phase: "connected",
      endpoint: "/tmp/repomon.sock",
      message: null,
      daemon: {
        uptime_secs: 3661,
        repos: 3,
        lanes: 5,
        db_size_bytes: 4096,
        version: "0.5.0",
      },
    })} />);

    await waitFor(() => {
      expect(screen.getByText("Connected")).toBeInTheDocument();
      expect(screen.getByText(/daemon 0\.5\.0/)).toBeInTheDocument();
      expect(screen.getByText("3 repos / 5 lanes")).toBeInTheDocument();
      expect(screen.getByText("Uptime 1h 01m")).toBeInTheDocument();
    });
  });

  it("makes a lost connection actionable", async () => {
    render(() => <App connectionSource={sourceFor({
      phase: "retrying",
      endpoint: "/tmp/repomon.sock",
      message: "daemon connection closed",
      daemon: null,
    })} />);

    await waitFor(() => {
      expect(screen.getByText("Retrying")).toBeInTheDocument();
      expect(screen.getByText("daemon connection closed")).toBeInTheDocument();
    });
  });

  it("surfaces fleet loading errors instead of failing silently", async () => {
    const fleetSource: FleetSource = {
      load: async () => { throw new Error("fleet sync failed"); },
      refreshUsage: async () => undefined,
      subscribe: async () => () => undefined,
    };
    render(() => <App connectionSource={sourceFor({
      phase: "connected",
      endpoint: "/tmp/repomon.sock",
      message: null,
      daemon: null,
    })} fleetSource={fleetSource} />);

    expect(await screen.findByText("fleet sync failed")).toBeInTheDocument();
  });

  it("drives the Extensions panel from the keymap table, not a bare digit", async () => {
    // Scoped to this render's container: earlier tests in this file leave their DOM mounted
    // (no cleanup wired up), so an unscoped query would see every prior "Extensions" button too.
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const extensions = within(container).getByRole("button", { name: "Extensions" });
    expect(extensions).toHaveAttribute("aria-pressed", "false");

    // The old ad-hoc listener toggled Extensions on a bare "6"; that would steal a keystroke
    // meant for a focused agent terminal. The keymap-driven handler ignores unmodified keys.
    fireEvent.keyDown(window, { key: "6", code: "Digit6" });
    expect(extensions).toHaveAttribute("aria-pressed", "false");

    fireEvent.keyDown(window, { key: "4", code: "Digit4", metaKey: true });
    await waitFor(() => expect(extensions).toHaveAttribute("aria-pressed", "true"));

    fireEvent.keyDown(window, { key: "4", code: "Digit4", metaKey: true });
    await waitFor(() => expect(extensions).toHaveAttribute("aria-pressed", "false"));
  });

  it("shows the real keymap.ts chord in header toolbar button titles, not hand-written text", () => {
    // Regression check mirroring ControlCenter.test.tsx's "Keyboard Shortcuts" chord test: these
    // titles used to hard-code strings like "Extensions (⌘4)" that could drift from the actual
    // binding in keymap.ts. They're now derived from BINDINGS via chordFor/formatChord.
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const extensions = within(container).getByRole("button", { name: "Extensions" });
    expect(extensions).toHaveAttribute("title", `Extensions (${formatChord("mod+4")})`);

    const settings = within(container).getByRole("button", { name: "Settings" });
    expect(settings).toHaveAttribute("title", `Settings (${formatChord("mod+,")})`);
  });

  it("toggles the fleet-wide Multitasking workspace and hides the lane sidebar", async () => {
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const button = within(container).getByRole("button", { name: "Multitasking" });
    expect(button).toHaveAttribute("aria-pressed", "false");
    expect(within(container).getByRole("navigation", { name: "Fleet" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "9", code: "Digit9", metaKey: true });
    await waitFor(() => expect(button).toHaveAttribute("aria-pressed", "true"));
    expect(within(container).queryByRole("navigation", { name: "Fleet" })).not.toBeInTheDocument();
    expect(within(container).getByText("Multitasking", { selector: ".section-label" })).toBeInTheDocument();

    fireEvent.click(button);
    await waitFor(() => expect(button).toHaveAttribute("aria-pressed", "false"));
    expect(within(container).getByRole("navigation", { name: "Fleet" })).toBeInTheDocument();
  });

  it("exits Multitasking before opening every right-rail panel", async () => {
    localStorage.setItem("repomon.repomind_open", "false");
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const multitasking = within(container).getByRole("button", { name: "Multitasking" });
    for (const panel of ["Repomail", "Git", "Editor", "Supervision", "Repomind"]) {
      if (multitasking.getAttribute("aria-pressed") !== "true") fireEvent.click(multitasking);
      await waitFor(() => expect(multitasking).toHaveAttribute("aria-pressed", "true"));
      const panelButton = within(container).getByRole("button", { name: panel });
      fireEvent.click(panelButton);
      await waitFor(() => {
        expect(multitasking).toHaveAttribute("aria-pressed", "false");
        expect(panelButton).toHaveAttribute("aria-pressed", "true");
      });
    }
    localStorage.setItem("repomon.repomind_open", "false");
  });

  it("opens and closes the Repomail panel with mod+2", async () => {
    localStorage.setItem("repomon.repomind_open", "false");
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const button = within(container).getByRole("button", { name: "Repomail" });
    expect(button).toHaveAttribute("aria-pressed", "false");

    fireEvent.keyDown(window, { key: "2", code: "Digit2", metaKey: true });
    await waitFor(() => expect(button).toHaveAttribute("aria-pressed", "true"));
    const panel = within(container).getByRole("complementary", { name: "Repomind" });
    expect(within(panel).getByText("Repomail", { selector: "span.text-xs.font-semibold" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "2", code: "Digit2", metaKey: true });
    await waitFor(() => expect(button).toHaveAttribute("aria-pressed", "false"));
  });

  it("ignores a shortcut keydown whose default was already prevented by another handler (item 6)", async () => {
    // Regression guard: mod+shift+f collided with the terminal's own find-bar chord because
    // TerminalPane called preventDefault without stopPropagation, so App.tsx's global shortcut
    // handler still fired the panel toggle underneath it. The fix makes the global handler
    // return early once event.defaultPrevented is true, so any earlier, more specific handler
    // wins. Simulate that earlier handler with a capture-phase listener that preventDefaults
    // before App's own bubble-phase listener runs.
    localStorage.setItem("repomon.repomind_open", "false");
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const button = within(container).getByRole("button", { name: "Repomail" });
    expect(button).toHaveAttribute("aria-pressed", "false");

    const preventer = (e: KeyboardEvent) => e.preventDefault();
    window.addEventListener("keydown", preventer, true);
    try {
      fireEvent.keyDown(window, { key: "2", code: "Digit2", metaKey: true });
    } finally {
      window.removeEventListener("keydown", preventer, true);
    }

    // Give any (incorrect) handling a turn before asserting nothing changed.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(button).toHaveAttribute("aria-pressed", "false");
  });

  it("opens settings on the system tab when the footer connection pill is clicked", async () => {
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "connected",
      endpoint: "/tmp/repomon.sock",
      message: null,
      daemon: null,
    })} />);

    const connectionButton = within(container).getByRole("button", { name: "View system health and daemon connection" });
    expect(connectionButton).toBeInTheDocument();

    fireEvent.click(connectionButton);

    const systemTab = await screen.findByRole("tab", { name: "System" });
    expect(systemTab).toBeInTheDocument();
    expect(systemTab).toHaveAttribute("aria-selected", "true");
  });

  it("mounts first-run onboarding wizard when repos=0 and onboarding not completed", async () => {
    localStorage.removeItem("repomon:onboarding-completed");
    localStorage.removeItem("repomon:onboarding-step");
    const fleetSource: FleetSource = {
      load: async () => ({
        repos: [],
        lanes: [],
        usage: [],
        terminals: [],
        sortReposByActivity: null, sortMode: null, tabSortMode: null,
      }),
      refreshUsage: async () => undefined,
      subscribe: async () => () => undefined,
    };

    const { container } = render(() => (
      <App
        connectionSource={sourceFor({
          phase: "connected",
          endpoint: "/tmp/repomon.sock",
          message: null,
          daemon: null,
        })}
        fleetSource={fleetSource}
      />
    ));

    await waitFor(() => {
      expect(within(container).getByTestId("onboarding-wizard")).toBeInTheDocument();
      expect(within(container).getByText("Set up Repomon")).toBeInTheDocument();
    });

    // Skip setup sets the completed flag and closes the overlay
    const skipBtn = within(container).getByRole("button", { name: "Skip setup" });
    fireEvent.click(skipBtn);

    await waitFor(() => {
      expect(within(container).queryByTestId("onboarding-wizard")).not.toBeInTheDocument();
      expect(localStorage.getItem("repomon:onboarding-completed")).toBe("true");
    });
  });

  // The two full-window headers (the shell's and the wizard's) are one component, so the macOS
  // traffic-light inset and the brand treatment cannot drift apart between them. Before this,
  // the wizard drew its own header and its mark sat under the traffic lights.
  it("draws the same brand lockup in the shell header and the wizard header", async () => {
    localStorage.removeItem("repomon:onboarding-completed");
    localStorage.removeItem("repomon:onboarding-step");
    const fleetSource: FleetSource = {
      load: async () => ({
        repos: [],
        lanes: [],
        usage: [],
        terminals: [],
        sortReposByActivity: null, sortMode: null, tabSortMode: null,
      }),
      refreshUsage: async () => undefined,
      subscribe: async () => () => undefined,
    };

    const { container } = render(() => (
      <App
        connectionSource={sourceFor({
          phase: "connected",
          endpoint: "/tmp/repomon.sock",
          message: null,
          daemon: null,
        })}
        fleetSource={fleetSource}
      />
    ));

    await waitFor(() => {
      expect(within(container).getByTestId("onboarding-wizard")).toBeInTheDocument();
    });

    const headers = Array.from(container.querySelectorAll("[data-window-chrome]"));
    expect(headers).toHaveLength(2);

    const lockups = headers.map((header) => header.querySelector("[data-brand-lockup]"));
    expect(lockups.every(Boolean)).toBe(true);
    // navigator.platform is pinned to macOS for this file, so both must carry the inset.
    for (const header of headers) expect(header.className).toContain("pl-[78px]");
    // Same component, same mark: compare the drawn glyph rather than the wrapper, which differs
    // by design (the shell's lockup opens Settings, the wizard's is inert).
    const marks = lockups.map((lockup) => lockup!.querySelector("svg")!.outerHTML);
    expect(marks[0]).toBe(marks[1]);
    expect(lockups.map((lockup) => lockup!.textContent)).toEqual(["Repomon", "Repomon"]);
  });

  it("does not mount onboarding wizard if user already has repositories", async () => {
    localStorage.removeItem("repomon:onboarding-completed");
    localStorage.removeItem("repomon:onboarding-step");
    const fleetSource: FleetSource = {
      load: async () => ({
        repos: [{ id: 1, name: "repo-1", path: "/path/to/1", added_at: "2026-08-01T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null }],
        lanes: [],
        usage: [],
        terminals: [],
        sortReposByActivity: null, sortMode: null, tabSortMode: null,
      }),
      refreshUsage: async () => undefined,
      subscribe: async () => () => undefined,
    };

    const { container } = render(() => (
      <App
        connectionSource={sourceFor({
          phase: "connected",
          endpoint: "/tmp/repomon.sock",
          message: null,
          daemon: null,
        })}
        fleetSource={fleetSource}
      />
    ));

    await waitFor(() => {
      expect(within(container).getByText("Connected")).toBeInTheDocument();
    });
    expect(within(container).queryByTestId("onboarding-wizard")).not.toBeInTheDocument();
  });

  it("item 5a: keeps min-w-0 on the right-rail pane so long content scrolls instead of blowing out the rail", async () => {
    // Regression guard for a flexbox "automatic minimum size" bug: this row-flex pane
    // (ResizableSplit handle + this div) previously had no min-w-0, so a deeply nested
    // no-wrap element's min-content width (e.g. an unwrapped long code line in CodeMirror)
    // won this div's width instead of the resizable rail's actual pixel width, and the
    // `aside` ancestor's overflow:hidden silently clipped the excess instead of letting the
    // editor's own `.cm-scroller` handle horizontal scrolling. Without min-w-0 here, the fix
    // has no effect regardless of what CodeEditor/CM6 itself does.
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const repomindToggle = within(container).getByRole("button", { name: "Repomind" });
    fireEvent.click(repomindToggle);
    await waitFor(() => expect(repomindToggle).toHaveAttribute("aria-pressed", "true"));

    const pane = container.querySelector(".border-l.border-line");
    expect(pane).not.toBeNull();
    expect(pane).toHaveClass("min-w-0");
    expect(pane).toHaveClass("flex-1");
  });

  it("opens the shortcuts overlay on mod+? (help.open)", async () => {
    render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    expect(screen.queryByRole("dialog", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "?", metaKey: true });
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    });

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();
    });
  });

  it("opens the shortcuts overlay on a bare \"?\" outside a text input", async () => {
    render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    fireEvent.keyDown(window, { key: "?" });
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    });
  });

  it("does not steal a bare \"?\" typed into a text input", async () => {
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const filterInput = within(container).getByPlaceholderText(/Filter/i);
    fireEvent.keyDown(filterInput, { key: "?" });
    expect(screen.queryByRole("dialog", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();
  });

  it("shows the footer shortcuts hint for a fresh install and opens the overlay from it", async () => {
    localStorage.removeItem(SHORTCUTS_HINT_LAUNCH_COUNT_KEY);
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const hint = within(container).getByRole("button", { name: /for shortcuts/i });
    fireEvent.click(hint);
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    });
  });

  it("stops showing the footer shortcuts hint after the max launch count", () => {
    localStorage.setItem(SHORTCUTS_HINT_LAUNCH_COUNT_KEY, "3");
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    expect(within(container).queryByRole("button", { name: /for shortcuts/i })).not.toBeInTheDocument();
  });
});
