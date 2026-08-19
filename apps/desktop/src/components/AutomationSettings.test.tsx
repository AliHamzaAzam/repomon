import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import AutomationSettings from "./AutomationSettings";

const daemonCallMock = vi.hoisted(() => vi.fn());
const subscribeDaemonMock = vi.hoisted(() => vi.fn(async () => () => {}));

function baseSupervisionConfig() {
  return {
    enabled: true,
    nudge_text: "Repomon: checking in on this lane.",
    mail_mode: "nudge",
    stall_mins: 15,
    nudge_retries: 2,
    classes: {
      command_exec: "auto_approve",
      deletion: "hold",
    },
  };
}

function baseConfig() {
  return { supervision: baseSupervisionConfig() };
}

vi.mock("../ipc/rpc", () => ({
  daemonCall: daemonCallMock,
  subscribeDaemon: subscribeDaemonMock,
}));

daemonCallMock.mockImplementation((method: string, params?: unknown) => {
  if (method === "playbook.list") {
    return Promise.resolve({
      playbooks: [
        {
          name: "test-playbook",
          content: "Automated regression verification\nrun cargo test",
          status: "draft",
          draft_content: null,
          created_at: "2026-07-27T00:00:00Z",
          updated_at: "2026-07-27T00:00:00Z",
          approved_at: null,
        },
      ],
    });
  }
  if (method === "schedule.list") {
    return Promise.resolve({
      schedules: [
        {
          id: 1,
          spec: "weekdays 09:00",
          prompt: "Morning fleet check",
          max_actions: 15,
          last_run_at: "2026-07-27T09:00:00Z",
        },
      ],
    });
  }
  if (method === "approval.list") {
    return Promise.resolve({
      rules: [
        {
          repo: "repomon",
          pattern: "cargo test",
          created_at: "2026-07-27T00:00:00Z",
        },
      ],
    });
  }
  if (method === "journal.query") {
    return Promise.resolve({
      entries: [
        {
          action: "merge_lane",
          outcome: "ok",
          at: "2026-07-27T10:00:00Z",
          repo: "repomon",
          lane_id: 1,
          params: "lane 1",
          detail: "merged cleanly",
        },
      ],
    });
  }
  if (method === "config.get") {
    return Promise.resolve(baseConfig());
  }
  if (method === "config.set") {
    const patch = (params as { supervision?: Record<string, unknown> } | undefined)?.supervision;
    return Promise.resolve({ supervision: patch ?? baseSupervisionConfig() });
  }
  return Promise.resolve({});
});

afterEach(() => {
  cleanup();
  daemonCallMock.mockClear();
});

describe("AutomationSettings component", () => {
  it("renders subtabs and shows playbooks on load", async () => {
    render(() => <AutomationSettings />);

    expect(screen.getByText("Orchestration & Standing Rules")).toBeInTheDocument();

    // Playbook rendered
    expect(await screen.findByText("test-playbook")).toBeInTheDocument();
    expect(screen.getByText("Automated regression verification")).toBeInTheDocument();
    expect(screen.getByText("draft")).toBeInTheDocument();
  });

  it("switches to Schedules tab and shows scheduled tasks", async () => {
    render(() => <AutomationSettings />);

    const schTab = screen.getByRole("button", { name: /Schedules/i });
    fireEvent.click(schTab);

    expect(await screen.findByText("weekdays 09:00")).toBeInTheDocument();
    expect(screen.getByText("Morning fleet check")).toBeInTheDocument();
  });

  it("switches to Approvals tab and shows approval rules", async () => {
    render(() => <AutomationSettings />);

    const appTab = screen.getByRole("button", { name: /Approvals/i });
    fireEvent.click(appTab);

    expect(await screen.findByText("repomon")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
  });

  it("switches to Activity Journal tab and shows journal entries", async () => {
    render(() => <AutomationSettings />);

    const jTab = screen.getByRole("button", { name: /Activity Journal/i });
    fireEvent.click(jTab);

    expect(await screen.findByText("merge_lane")).toBeInTheDocument();
    expect(screen.getByText("ok")).toBeInTheDocument();
  });

  it("switches to Supervision tab and renders defaults from config.get", async () => {
    render(() => <AutomationSettings />);

    const supTab = screen.getByRole("button", { name: /^Supervision$/i });
    fireEvent.click(supTab);

    expect(await screen.findByText("Enable supervision")).toBeInTheDocument();
    expect(screen.getByText("Default permission policies")).toBeInTheDocument();
    expect(screen.getByDisplayValue("Repomon: checking in on this lane.")).toBeInTheDocument();
    expect(screen.getByDisplayValue("15")).toBeInTheDocument();
    expect(screen.getByDisplayValue("2")).toBeInTheDocument();
  });

  it("toggling the master switch issues one config.set with enabled flipped and other fields preserved", async () => {
    render(() => <AutomationSettings />);

    const supTab = screen.getByRole("button", { name: /^Supervision$/i });
    fireEvent.click(supTab);

    const masterSwitch = await screen.findByRole("switch", { name: "Enable supervision" });
    daemonCallMock.mockClear();
    fireEvent.click(masterSwitch);

    await waitFor(() => {
      const setCalls = daemonCallMock.mock.calls.filter(([method]) => method === "config.set");
      expect(setCalls).toHaveLength(1);
    });

    const [, params] = daemonCallMock.mock.calls.find(([method]) => method === "config.set")!;
    const supervision = (params as { supervision: Record<string, unknown> }).supervision;
    expect(supervision.enabled).toBe(false);
    expect(supervision.nudge_text).toBe("Repomon: checking in on this lane.");
    expect(supervision.mail_mode).toBe("nudge");
    expect(supervision.stall_mins).toBe(15);
    expect(supervision.nudge_retries).toBe(2);
  });

  it("changing one class select issues config.set with only that class's action changed", async () => {
    render(() => <AutomationSettings />);

    const supTab = screen.getByRole("button", { name: /^Supervision$/i });
    fireEvent.click(supTab);

    const policySelect = await screen.findByRole("combobox", { name: "Command execution default policy" });
    daemonCallMock.mockClear();
    fireEvent.click(policySelect);
    const denyOption = await screen.findByRole("option", { name: "Auto-deny" });
    fireEvent.click(denyOption);

    await waitFor(() => {
      const setCalls = daemonCallMock.mock.calls.filter(([method]) => method === "config.set");
      expect(setCalls).toHaveLength(1);
    });

    const [, params] = daemonCallMock.mock.calls.find(([method]) => method === "config.set")!;
    const supervision = (params as { supervision: { classes: Record<string, string> } }).supervision;
    expect(supervision.classes.command_exec).toBe("auto_deny");
    expect(supervision.classes.deletion).toBe("hold");
  });
});
