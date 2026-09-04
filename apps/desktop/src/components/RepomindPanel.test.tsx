import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane, RepomindStatus, Repo } from "../bindings";
import { controllerSummary, type FleetStore } from "../stores/fleet";
import type { RepomindStore } from "../stores/repomind";
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

const home: Repo = {
  id: 9,
  path: "/Users/pat/repomind",
  name: "repomind",
  added_at: "2026-09-04T00:00:00Z",
  worktree_root_template: null,
  hidden: false,
  position: null,
  label: null,
};

function session(overrides: Partial<AgentSession> = {}): AgentSession {
  return {
    id: 1,
    agent: "claude-code",
    repo_id: 9,
    worktree_id: 9,
    started_at: "2026-09-04T00:00:00Z",
    last_activity_at: "2026-09-04T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 0,
    title: "Repomind",
    status: "running",
    external: false,
    session_id: "c1",
    resume_at: null,
    inferred: false,
    tmux_window: "repomind-1",
    last_message: null,
    pending_prompt: null,
    pending_dialog: null,
    stale: false,
    stalled_since: null,
    subagent_running: null,
    gate: null,
    config_dir: null,
    custom_label: "Primary",
    generated_label: null,
    ...overrides,
  };
}

function controllerLane(sessions: AgentSession[]): Lane {
  return {
    id: 90,
    repo: home,
    worktree: { id: 90, repo_id: 9, path: "/Users/pat/repomind", branch: "main", head: "abc", is_main: true, name: "main" },
    state: {
      worktree_id: 90,
      head: "abc",
      branch: "main",
      upstream: null,
      ahead: 0,
      behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 },
      last_commit_at: null,
      locked: false,
      prunable: false,
      last_change_at: null,
    },
    agent_sessions: sessions,
    last_activity_at: "2026-09-04T00:00:00Z",
    pinned: false,
    role: "controller",
  };
}

function status(overrides: Partial<RepomindStatus> = {}): RepomindStatus {
  return {
    home: "/Users/pat/repomind",
    exists: true,
    repo_id: 9,
    lane_id: 90,
    window: "repomind-1",
    max_controllers: 2,
    export: { last_run: null, pending: false, last_error: null },
    counts: { active_plans: 1, standing: 0, playbooks: 0, drafts: 0 },
    boot: { generated_at: null, tokens_estimate: 0, trimmed: [] },
    ...overrides,
  };
}

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
    regenerateBoot: vi.fn().mockResolvedValue(undefined),
    runExport: vi.fn().mockResolvedValue(undefined),
    ...extra,
  } as unknown as RepomindStore;
}

/// The panel's own polling calls, plus whatever the Home view reads out of the lane.
function mockDaemon(overrides: Record<string, unknown> = {}) {
  daemonCall.mockImplementation((method: string, params?: { path?: string }) => {
    if (method in overrides) return Promise.resolve(overrides[method]);
    switch (method) {
      case "orchestrator.status":
        return Promise.resolve({ running: true, backend: "claude" });
      case "orchestrator.transcript":
        return Promise.resolve([]);
      case "orchestrator.watch":
        return Promise.resolve(null);
      case "file.list":
        return Promise.resolve({
          entries: [
            { name: "ship-r4.md", path: "plans/active/ship-r4.md", is_dir: false, size: 40, ignored: false },
            { name: "README.md", path: "plans/active/README.md", is_dir: false, size: 10, ignored: false },
          ],
          truncated: false,
        });
      case "file.read":
        if (params?.path === "plans/active/ship-r4.md") {
          return Promise.resolve({
            content: "---\ntitle: Ship R4\n---\n\nNext step: land the panel\n",
            mtime_ms: 0,
            size: 40,
            truncated: false,
            kind: "text",
            large: false,
          });
        }
        if (params?.path?.startsWith("journal/")) {
          return Promise.resolve({
            content: "# Journal\n\n## 09:12:00 spawn_agent (ok)\n\n- Repo: repomon\n",
            mtime_ms: 0,
            size: 20,
            truncated: false,
            kind: "text",
            large: false,
          });
        }
        return Promise.reject(new Error("no such file"));
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

describe("the Repomind panel's Home view", () => {
  it("lists the home's active plans with their next step, skipping the directory README", async () => {
    mockDaemon();
    render(() => (
      <RepomindPanel fleet={fleetStub(controllerLane([session()]))} repomind={repomindStub(status())} />
    ));

    await waitFor(() => expect(screen.getByText("Ship R4")).toBeInTheDocument());
    expect(screen.getByText("Next: land the panel")).toBeInTheDocument();
    // A directory's own README is a guide, never a goal.
    expect(screen.queryByText("README")).toBeNull();
  });

  it("opens a plan in the editor on the home lane", async () => {
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

    await waitFor(() => expect(screen.getByText("Ship R4")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Ship R4"));
    expect(setSelectedLaneId).toHaveBeenCalledWith(90);
    expect(onEnsureEditorOpen).toHaveBeenCalled();
    expect(openFile).toHaveBeenCalledWith("plans/active/ship-r4.md");
  });

  it("shows today's journal tail", async () => {
    mockDaemon();
    render(() => (
      <RepomindPanel fleet={fleetStub(controllerLane([session()]))} repomind={repomindStub(status())} />
    ));

    await waitFor(() => expect(screen.getByText(/spawn_agent \(ok\)/)).toBeInTheDocument());
    // The digest's own title heading is not an entry.
    expect(screen.queryByText(/^# Journal$/)).toBeNull();
  });

  it("reports the boot context and regenerates it on demand", async () => {
    mockDaemon();
    const regenerateBoot = vi.fn().mockResolvedValue(undefined);
    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]))}
        repomind={repomindStub(
          status({ boot: { generated_at: "2026-09-04T00:00:00Z", tokens_estimate: 8400, trimmed: ["journal/2026-09-01.md"] } }),
          { regenerateBoot },
        )}
      />
    ));

    await waitFor(() => expect(screen.getByText("8400 tokens")).toBeInTheDocument());
    expect(screen.getByText(/The token budget left out: journal\/2026-09-01\.md/)).toBeInTheDocument();
    fireEvent.click(screen.getByText("Regenerate"));
    expect(regenerateBoot).toHaveBeenCalled();
  });

  it("opens the boot document in the editor", async () => {
    mockDaemon();
    const openFile = vi.fn().mockResolvedValue(undefined);
    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]))}
        repomind={repomindStub(status())}
        editor={{ openFile } as never}
      />
    ));

    await waitFor(() => expect(screen.getByText("Open boot.md")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Open boot.md"));
    expect(openFile).toHaveBeenCalledWith(".repomind/boot.md");
  });

  it("shows the export state, its error, and runs one on demand", async () => {
    mockDaemon();
    const runExport = vi.fn().mockResolvedValue(undefined);
    render(() => (
      <RepomindPanel
        fleet={fleetStub(controllerLane([session()]))}
        repomind={repomindStub(
          status({ export: { last_run: null, pending: true, last_error: "home is not a git repo" } }),
          { runExport },
        )}
      />
    ));

    await waitFor(() => expect(screen.getByText("Last run never")).toBeInTheDocument());
    expect(screen.getByText("pending")).toBeInTheDocument();
    expect(screen.getByText("home is not a git repo")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Export now"));
    expect(runExport).toHaveBeenCalled();
  });

  it("lists the controllers with their state and the daemon's reason for it", async () => {
    mockDaemon();
    const lane = controllerLane([
      session(),
      session({ id: 2, session_id: "c2", custom_label: "Second", status: "waiting", status_reason: "asked a question" }),
    ]);
    render(() => <RepomindPanel fleet={fleetStub(lane)} repomind={repomindStub(status())} />);

    await waitFor(() => expect(screen.getByText("Primary")).toBeInTheDocument());
    expect(screen.getByText("Second")).toBeInTheDocument();
    expect(screen.getByText("running")).toBeInTheDocument();
    // Twice on purpose: the header states the most urgent controller, the row states its own.
    expect(screen.getAllByText("needs you")).toHaveLength(2);
    expect(screen.getByText("Second: asked a question")).toBeInTheDocument();
  });

  it("spawns another controller into the home lane, up to the configured cap", async () => {
    mockDaemon();
    const spawn = vi.fn();
    const lane = controllerLane([session(), session({ id: 2, session_id: "c2" })]);
    const { unmount } = render(() => (
      <RepomindPanel fleet={fleetStub(lane)} repomind={repomindStub(status())} actions={{ spawn } as never} />
    ));

    // Two controllers with max_controllers 2: the control is present but refuses.
    const capped = screen.getByLabelText("Spawn controller");
    expect(capped).toBeDisabled();
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
    expect(screen.queryByLabelText("Active plans")).toBeNull();
  });

  it("keeps the live and transcript views for the primary controller", async () => {
    mockDaemon({ "orchestrator.transcript": [{ role: "assistant", text: "spawned a worker" }] });
    render(() => (
      <RepomindPanel fleet={fleetStub(controllerLane([session()]))} repomind={repomindStub(status())} />
    ));

    fireEvent.click(screen.getByRole("tab", { name: "Transcript" }));
    await waitFor(() => expect(screen.getByText("spawned a worker")).toBeInTheDocument());

    fireEvent.click(screen.getByRole("tab", { name: "Live Feed" }));
    expect(screen.getByLabelText("Repomind live pane")).toBeInTheDocument();
  });
});
