import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  DialogClass,
  Lane,
  PolicyAction,
  SupervisionConfig,
  SupervisionEntry,
  SupervisionPolicy,
} from "../bindings";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import SupervisionPanel from "./SupervisionPanel";

const calls = vi.hoisted(() => ({ list: [] as Array<{ method: string; params: unknown }> }));
const subscribers = vi.hoisted(() => ({ list: [] as Array<(event: { jsonrpc: "2.0"; method: `event.${string}`; params: unknown }) => void> }));
const daemonCallMock = vi.hoisted(() => vi.fn());

vi.mock("../ipc/rpc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/rpc")>();
  return {
    ...actual,
    daemonCall: (method: string, params?: unknown) => {
      calls.list.push({ method, params });
      return daemonCallMock(method, params);
    },
    subscribeDaemon: (onEvent: (event: { jsonrpc: "2.0"; method: `event.${string}`; params: unknown }) => void) => {
      subscribers.list.push(onEvent);
      return Promise.resolve(() => {
        subscribers.list = subscribers.list.filter((l) => l !== onEvent);
      });
    },
  };
});

function defaultDefaults(): SupervisionConfig {
  return {
    enabled: true,
    nudge_text: "Repomon: checking in on this lane.",
    mail_mode: "nudge",
    stall_mins: 15,
    nudge_retries: 2,
    classes: {
      command_exec: "hold",
      file_write: "hold",
      deletion: "auto_deny",
      network_access: "hold",
      credential_access: "hold",
      push_remote: "auto_deny",
      install: "hold",
      device_access: "auto_deny",
      unknown: "hold",
    },
  };
}

function defaultEffective(overrides?: Partial<SupervisionPolicy>): SupervisionPolicy {
  return {
    enabled: true,
    nudge_text: "Repomon: checking in on this lane.",
    mail_mode: "nudge",
    stall_mins: 15,
    nudge_retries: 2,
    expect_work: false,
    classes: {
      command_exec: "hold",
      file_write: "hold",
      deletion: "auto_deny",
      network_access: "hold",
      credential_access: "hold",
      push_remote: "auto_deny",
      install: "hold",
      device_access: "auto_deny",
      unknown: "hold",
    },
    ...overrides,
  };
}

function sampleLane(id = 7): Lane {
  return {
    id,
    repo: { id: 1, name: "repo-1", path: "/code/repo-1", added_at: "2026-08-01T00:00:00Z", worktree_root_template: null, hidden: false },
    worktree: { id: 1, repo_id: 1, path: "/code/repo-1-wt/feat", branch: "feat/supervision", head: "abc1234", is_main: false, name: "feat" },
    state: {
      worktree_id: 1,
      head: "abc1234",
      branch: "feat/supervision",
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
    last_activity_at: "2026-08-01T00:00:00Z",
    pinned: false,
  };
}

function sampleAuditEntry(id = 1, laneId = 7, overrides?: Partial<SupervisionEntry>): SupervisionEntry {
  return {
    id,
    at: "2026-08-19T12:00:00Z",
    lane_id: laneId,
    window: "win-7",
    session_id: "sess-1",
    agent_kind: "claude-code",
    trigger: "dialog",
    dialog_class: "command_exec",
    repo_scoped: true,
    decision: "approve",
    policy_source: "lane_class",
    keys: ["Enter"],
    outcome: "sent",
    reason: "Approved command execution",
    subject: "cargo build",
    pane_excerpt: "● Running cargo build…",
    ...overrides,
  };
}

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

function emitDaemonEvent(method: `event.${string}`, params: unknown) {
  for (const subscriber of subscribers.list) {
    subscriber({ jsonrpc: "2.0", method, params });
  }
}

afterEach(() => {
  cleanup();
  calls.list = [];
  subscribers.list = [];
  daemonCallMock.mockReset();
});

describe("SupervisionPanel", () => {
  it("renders empty state when no lane is selected", () => {
    const fleet = { selectedLane: () => null } as unknown as FleetStore;
    render(() => <SupervisionPanel fleet={fleet} />);

    expect(screen.getByText("No lane selected")).toBeInTheDocument();
    expect(screen.getByText("Select a lane in the fleet to view and edit its supervision policies.")).toBeInTheDocument();
  });

  it("renders the effective policy from a stubbed supervision.get", async () => {
    const lane = sampleLane(7);
    const fleet = { selectedLane: () => lane } as unknown as FleetStore;

    mockRpc({
      "supervision.get": () => ({
        defaults: defaultDefaults(),
        lane: null,
        effective: defaultEffective(),
      }),
      "supervision.audit": () => ({
        entries: [sampleAuditEntry(1, 7)],
      }),
    });

    render(() => <SupervisionPanel fleet={fleet} />);

    await waitFor(() => {
      expect(screen.getByText("Supervision")).toBeInTheDocument();
      expect(screen.getByText("feat/supervision")).toBeInTheDocument();
      expect(screen.getByText("Permission policies")).toBeInTheDocument();
      expect(screen.getByText("Command execution")).toBeInTheDocument();
      expect(screen.getByText("File modification")).toBeInTheDocument();
      expect(screen.getByText("File deletion")).toBeInTheDocument();
      expect(screen.getByText("Delivery and thresholds")).toBeInTheDocument();
      expect(screen.getByText("Activity log")).toBeInTheDocument();
      expect(screen.getByText(/Approved command execution/)).toBeInTheDocument();
    });
  });

  it("changing one class Select issues exactly one supervision.set whose classes contains only changed override semantics", async () => {
    const lane = sampleLane(7);
    const fleet = { selectedLane: () => lane } as unknown as FleetStore;

    mockRpc({
      "supervision.get": () => ({
        defaults: defaultDefaults(),
        lane: null,
        effective: defaultEffective(),
      }),
      "supervision.audit": () => ({ entries: [] }),
      "supervision.set": (params) => {
        const p = params as { lane_id: number; classes?: Partial<Record<DialogClass, PolicyAction>> };
        return {
          effective: defaultEffective({
            classes: {
              ...defaultDefaults().classes,
              ...(p.classes ?? {}),
            },
          }),
        };
      },
    });

    render(() => <SupervisionPanel fleet={fleet} />);

    const cmdExecSelect = await screen.findByRole("combobox", { name: "Command execution policy" });
    await waitFor(() => {
      expect(cmdExecSelect).not.toBeDisabled();
    });

    fireEvent.click(cmdExecSelect);

    const approveOptions = await screen.findAllByRole("option", { name: /auto-approve/i });
    expect(approveOptions.length).toBeGreaterThan(0);
    fireEvent.click(approveOptions[0]);

    await waitFor(() => {
      const setCall = calls.list.find((c) => c.method === "supervision.set");
      expect(setCall).toBeDefined();
      expect(setCall?.params).toEqual({
        lane_id: 7,
        classes: { command_exec: "auto_approve" },
      });
    });
  });

  it("master-off renders the notice, the lane switch disabled, and issues no writes", async () => {
    const lane = sampleLane(7);
    const fleet = { selectedLane: () => lane } as unknown as FleetStore;
    const openSettingsTab = vi.fn();
    const actions = { openSettingsTab } as unknown as ActionsStore;

    const disabledDefaults = { ...defaultDefaults(), enabled: false };
    mockRpc({
      "supervision.get": () => ({
        defaults: disabledDefaults,
        lane: null,
        effective: defaultEffective({ enabled: false }),
      }),
      "supervision.audit": () => ({ entries: [] }),
    });

    render(() => <SupervisionPanel fleet={fleet} actions={actions} />);

    await waitFor(() => {
      expect(screen.getByText("Supervision is off globally.")).toBeInTheDocument();
      expect(
        screen.getByText("Lane supervision policies will not execute while the master switch is disabled."),
      ).toBeInTheDocument();
    });

    // Lane switch should be disabled
    const switchButton = screen.getByRole("switch", { name: /supervise this lane/i });
    expect(switchButton).toBeDisabled();

    // Open settings button should trigger actions.openSettingsTab("automation")
    const openSettingsBtn = screen.getByRole("button", { name: /open settings/i });
    fireEvent.click(openSettingsBtn);
    expect(openSettingsTab).toHaveBeenCalledWith("automation");

    // No supervision.set should have been called
    const setCalls = calls.list.filter((c) => c.method === "supervision.set");
    expect(setCalls).toHaveLength(0);
  });

  it("an injected event.supervision.acted for the selected lane prepends an audit row and activates the indicator; one for a different lane does not", async () => {
    const lane = sampleLane(7);
    const fleet = { selectedLane: () => lane } as unknown as FleetStore;

    mockRpc({
      "supervision.get": () => ({
        defaults: defaultDefaults(),
        lane: null,
        effective: defaultEffective(),
      }),
      "supervision.audit": () => ({ entries: [] }),
    });

    render(() => <SupervisionPanel fleet={fleet} />);

    await waitFor(() => {
      expect(screen.getByText("No supervision activity yet.")).toBeInTheDocument();
    });

    // Injected event for a DIFFERENT lane (lane_id = 99)
    emitDaemonEvent("event.supervision.acted", sampleAuditEntry(99, 99, { reason: "Ignored other lane action" }));

    // Should NOT appear
    expect(screen.queryByText(/Ignored other lane action/)).not.toBeInTheDocument();

    // Injected event for the SELECTED lane (lane_id = 7)
    emitDaemonEvent(
      "event.supervision.acted",
      sampleAuditEntry(101, 7, {
        reason: "Live acted event for selected lane",
        outcome: "sent",
        decision: "approve",
      }),
    );

    // Should appear in audit log
    await waitFor(() => {
      expect(screen.getByText(/Live acted event for selected lane/)).toBeInTheDocument();
    });

    // Indicator should be active
    const indicator = screen.getByLabelText("Supervision status indicator");
    expect(indicator.getAttribute("title")).toContain("Last action: approve (sent)");
  });
});
