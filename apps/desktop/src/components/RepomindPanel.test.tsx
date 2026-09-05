import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane, RepomindStatus } from "../bindings";
import { controllerSummary, type FleetStore } from "../stores/fleet";
import type { RepomindStore } from "../stores/repomind";
import type { WorkspaceStore } from "../stores/workspace";
import { controllerLane, session, status } from "../test/repomindFixtures";
import RepomindPanel, { sinceLabel } from "./RepomindPanel";

const daemonCall = vi.fn();

vi.mock("../ipc/rpc", () => ({
  daemonCall: (...args: unknown[]) => daemonCall(...args),
  subscribeDaemon: vi.fn().mockResolvedValue(() => undefined),
}));

afterEach(() => {
  cleanup();
  daemonCall.mockReset();
});

function fleetStub(lane: Lane | null, setSelectedLaneId = vi.fn()) {
  const lanes = lane ? [lane] : [];
  return {
    controller: () => controllerSummary(lanes),
    setSelectedLaneId,
    refresh: vi.fn().mockResolvedValue(undefined),
  } as unknown as FleetStore;
}

function repomindStub(value: RepomindStatus | null, extra: Partial<RepomindStore> = {}) {
  return {
    status: () => value,
    busy: () => null,
    error: () => null,
    dismissError: vi.fn(),
    refresh: vi.fn().mockResolvedValue(undefined),
    regenerateBoot: vi.fn().mockResolvedValue(undefined),
    runExport: vi.fn().mockResolvedValue(undefined),
    ...extra,
  } as unknown as RepomindStore;
}

/// Whatever the sections read out of the home lane. The panel itself makes no daemon calls.
function mockDaemon(overrides: Record<string, unknown> = {}) {
  daemonCall.mockImplementation((method: string, params?: { path?: string }) => {
    if (method in overrides) return Promise.resolve(overrides[method]);
    switch (method) {
      case "file.list":
        return Promise.resolve({
          entries: [
            { name: "ship-r6.md", path: "plans/active/ship-r6.md", is_dir: false, size: 40, ignored: false },
          ],
          truncated: false,
        });
      case "file.read":
        return params?.path === "plans/active/ship-r6.md"
          ? Promise.resolve({
              content: "---\ntitle: Ship R6\n---\n\nNext step: land the board\n",
              mtime_ms: 0,
              size: 40,
              truncated: false,
              kind: "text",
              large: false,
            })
          : Promise.reject(new Error("no such file"));
      case "playbook.list":
        return Promise.resolve({ playbooks: [] });
      case "schedule.list":
        return Promise.resolve({ schedules: [] });
      default:
        return Promise.resolve(null);
    }
  });
}

describe("sinceLabel", () => {
  const now = Date.parse("2026-09-04T12:00:00Z");

  it("says never for a run that has not happened", () => {
    expect(sinceLabel(null, now)).toBe("never");
    expect(sinceLabel("not a date", now)).toBe("never");
  });

  it("reports coarse ages rather than exact times", () => {
    expect(sinceLabel("2026-09-04T11:59:40Z", now)).toBe("just now");
    expect(sinceLabel("2026-09-04T11:45:00Z", now)).toBe("15m ago");
    expect(sinceLabel("2026-09-04T09:00:00Z", now)).toBe("3h ago");
    expect(sinceLabel("2026-09-02T12:00:00Z", now)).toBe("2d ago");
  });
});

describe("the Repomind control room", () => {
  it("names the lane, the home path, and the controller count in the header", () => {
    mockDaemon();
    render(() => (
      <RepomindPanel fleet={fleetStub(controllerLane([session()]))} repomind={repomindStub(status())} />
    ));

    expect(screen.getByText("main")).toBeInTheDocument();
    expect(screen.getByText("/Users/pat/repomind")).toBeInTheDocument();
    expect(screen.getByTitle("1 controller in the home lane")).toBeInTheDocument();
  });

  it("shows every section of the control room in one column", async () => {
    mockDaemon();
    render(() => (
      <RepomindPanel fleet={fleetStub(controllerLane([session()]))} repomind={repomindStub(status())} />
    ));

    for (const name of ["Plans", "Playbooks", "Standing duties", "Memory", "Controllers"]) {
      expect(screen.getByLabelText(name)).toBeInTheDocument();
    }
    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
  });

  it("is a control room and not a second chat", async () => {
    mockDaemon();
    render(() => (
      <RepomindPanel fleet={fleetStub(controllerLane([session()]))} repomind={repomindStub(status())} />
    ));

    // The pane in the terminal bay is the conversation: no composer, no live feed, no transcript.
    expect(screen.queryByLabelText("Message repomind")).toBeNull();
    expect(screen.queryByLabelText("Repomind live pane")).toBeNull();
    expect(screen.queryByRole("tab")).toBeNull();
    // And none of the hidden-window era's polling behind them.
    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
    const methods = daemonCall.mock.calls.map((call) => call[0] as string);
    expect(methods.some((method) => method.startsWith("orchestrator."))).toBe(false);
  });

  it("focuses a controller's pane in the terminal bay", () => {
    mockDaemon();
    const setSelectedLaneId = vi.fn();
    const setActiveWindow = vi.fn();
    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]), setSelectedLaneId)}
        repomind={repomindStub(status())}
        workspace={{ setActiveWindow } as unknown as WorkspaceStore}
      />
    ));

    fireEvent.click(screen.getByText("Focus pane"));
    expect(setSelectedLaneId).toHaveBeenCalledWith(90);
    expect(setActiveWindow).toHaveBeenCalledWith("repomind-1");
  });

  it("opens a home file in the editor on the home lane", async () => {
    mockDaemon();
    const setSelectedLaneId = vi.fn();
    const openFile = vi.fn().mockResolvedValue(undefined);
    const onEnsureEditorOpen = vi.fn();
    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]), setSelectedLaneId)}
        repomind={repomindStub(status())}
        editor={{ openFile } as never}
        onEnsureEditorOpen={onEnsureEditorOpen}
      />
    ));

    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Ship R6"));
    expect(setSelectedLaneId).toHaveBeenCalledWith(90);
    expect(onEnsureEditorOpen).toHaveBeenCalled();
    expect(openFile).toHaveBeenCalledWith("plans/active/ship-r6.md");
  });

  it("adds a duty in the panel rather than sending the operator to settings", async () => {
    mockDaemon();
    const openSettingsTab = vi.fn();
    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]))}
        repomind={repomindStub(status())}
        actions={{ openSettingsTab } as never}
      />
    ));

    await waitFor(() => expect(screen.getByText("Add")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Add"));
    expect(screen.getByRole("form", { name: "Add a standing duty" })).toBeInTheDocument();
    expect(openSettingsTab).not.toHaveBeenCalled();
  });

  it("drives the lifecycle through the one action every surface shares", async () => {
    mockDaemon();
    const startRepomind = vi.fn().mockResolvedValue(undefined);
    const stopRepomind = vi.fn().mockResolvedValue(undefined);
    const { unmount } = render(() => (
      <RepomindPanel
        fleet={fleetStub(null)}
        repomind={repomindStub(status())}
        actions={{ startRepomind, stopRepomind } as never}
      />
    ));

    fireEvent.click(screen.getByText("Start"));
    await waitFor(() => expect(startRepomind).toHaveBeenCalled());
    unmount();

    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]))}
        repomind={repomindStub(status())}
        actions={{ startRepomind, stopRepomind } as never}
      />
    ));
    fireEvent.click(screen.getByText("Stop"));
    await waitFor(() => expect(stopRepomind).toHaveBeenCalled());
  });

  it("spawns another controller into the home lane, up to the configured cap", () => {
    mockDaemon();
    const spawn = vi.fn();
    const capped: AgentSession[] = [session(), session({ id: 2, session_id: "c2" })];
    const { unmount } = render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane(capped))}
        repomind={repomindStub(status())}
        actions={{ spawn } as never}
      />
    ));

    expect(screen.getByLabelText("Spawn controller")).toBeDisabled();
    unmount();

    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]))}
        repomind={repomindStub(status())}
        actions={{ spawn } as never}
      />
    ));
    fireEvent.click(screen.getByLabelText("Spawn controller"));
    expect(spawn).toHaveBeenCalled();
  });

  it("says the home has no lane yet rather than showing empty sections", async () => {
    mockDaemon();
    render(() => <RepomindPanel fleet={fleetStub(null)} repomind={repomindStub(null)} />);

    await waitFor(() => expect(screen.getByText(/no lane yet/)).toBeInTheDocument());
    expect(screen.queryByLabelText("Plans")).toBeNull();
  });

  it("surfaces the home store's own error with a way to dismiss it", () => {
    mockDaemon();
    const dismissError = vi.fn();
    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]))}
        repomind={repomindStub(status(), { error: () => "home is unreachable", dismissError })}
      />
    ));

    expect(screen.getByRole("alert")).toHaveTextContent("home is unreachable");
    fireEvent.click(screen.getByLabelText("Dismiss repomind error"));
    expect(dismissError).toHaveBeenCalled();
  });
});

describe("Repomind panel header at a narrow rail", () => {
  // The screenshot that started this: at a narrow rail the Expand button was painted over the
  // status pill. jsdom cannot measure, so this asserts the structure that makes overlap
  // impossible: the leading group is the one shrinkable child (and the pill inside it may
  // truncate), the actions never shrink, and Expand is an icon with its name in aria-label.
  it("keeps the status pill fluid, the actions fixed, and Expand icon-only with a name", () => {
    mockDaemon();
    const onToggleFullscreen = vi.fn();
    const { container } = render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]))}
        repomind={repomindStub(status())}
        onToggleFullscreen={onToggleFullscreen}
      />
    ));

    const expand = screen.getByRole("button", { name: "Expand Repomind to full screen" });
    expect(expand.textContent).toBe("");
    expect(expand.getAttribute("title")).toBeTruthy();
    expect(expand.querySelector("svg")).toBeInTheDocument();
    expect(expand.parentElement?.className).toContain("panel-header-actions");

    const pill = container.querySelector<HTMLElement>(".lane-status")!;
    expect(pill.className).toContain("is-fluid");
    // The full word survives in the tooltip when the pill has to truncate it.
    expect(pill.getAttribute("title")).toBe(pill.textContent);
    expect(pill.parentElement?.className).toContain("panel-header-lead");

    fireEvent.click(expand);
    expect(onToggleFullscreen).toHaveBeenCalledTimes(1);
  });

  it("names the same button Collapse when the panel is full screen", () => {
    mockDaemon();
    render(() => <RepomindPanel fullscreen onToggleFullscreen={() => undefined} fleet={fleetStub(null)} repomind={repomindStub(status())} />);
    expect(screen.getByRole("button", { name: "Collapse Repomind to the side rail" })).toBeInTheDocument();
  });
});
