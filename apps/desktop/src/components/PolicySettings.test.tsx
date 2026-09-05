import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import PolicySettings from "./PolicySettings";

const daemonCallMock = vi.hoisted(() => vi.fn());
const subscribeDaemonMock = vi.hoisted(() => vi.fn(async () => () => {}));

function baseSupervisionConfig() {
  return {
    enabled: true,
    nudge_text: "Repomon: checking in on this lane.",
    stall_mins: 15,
    nudge_retries: 2,
    classes: {
      command_exec: "auto_approve",
      deletion: "hold",
    },
  };
}

vi.mock("../ipc/rpc", () => ({
  daemonCall: daemonCallMock,
  subscribeDaemon: subscribeDaemonMock,
}));

daemonCallMock.mockImplementation((method: string, params?: unknown) => {
  if (method === "approval.list") {
    return Promise.resolve({
      rules: [{ repo: "repomon", pattern: "cargo test", created_at: "2026-07-27T00:00:00Z" }],
    });
  }
  if (method === "config.get") {
    return Promise.resolve({ supervision: baseSupervisionConfig() });
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

describe("Settings > Policies", () => {
  it("opens on approvals and lists the rules Repomon may run unattended", async () => {
    render(() => <PolicySettings />);
    expect(screen.getByText("Standing rules")).toBeInTheDocument();
    expect(await screen.findByText("repomon")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
  });

  it("holds only approvals and supervision, and says where the rest went", async () => {
    render(() => <PolicySettings />);
    await screen.findByText("repomon");
    expect(screen.queryByRole("button", { name: /Playbooks/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /Schedules/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /Activity Journal/i })).toBeNull();
    expect(
      screen.getByText(/Playbooks, standing duties, and the journal live in the Repomind panel/),
    ).toBeInTheDocument();
  });

  it("opens on the sub-tab a caller asked for", async () => {
    render(() => <PolicySettings initialSection="supervision" />);
    expect(await screen.findByText("Enable supervision")).toBeInTheDocument();
  });

  it("renders the supervision defaults from config.get", async () => {
    render(() => <PolicySettings />);
    fireEvent.click(screen.getByRole("button", { name: /^Supervision$/i }));

    expect(await screen.findByText("Enable supervision")).toBeInTheDocument();
    expect(screen.getByText("Default permission policies")).toBeInTheDocument();
    expect(screen.getByDisplayValue("Repomon: checking in on this lane.")).toBeInTheDocument();
    expect(screen.getByDisplayValue("15")).toBeInTheDocument();
    expect(screen.getByDisplayValue("2")).toBeInTheDocument();
  });

  it("issues one config.set with enabled flipped and every other field preserved", async () => {
    render(() => <PolicySettings />);
    fireEvent.click(screen.getByRole("button", { name: /^Supervision$/i }));

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
    expect(supervision.stall_mins).toBe(15);
    expect(supervision.nudge_retries).toBe(2);
  });

  it("changes one class without disturbing the others", async () => {
    render(() => <PolicySettings />);
    fireEvent.click(screen.getByRole("button", { name: /^Supervision$/i }));

    const policySelect = await screen.findByRole("combobox", {
      name: "Command execution default policy",
    });
    daemonCallMock.mockClear();
    fireEvent.click(policySelect);
    fireEvent.click(await screen.findByRole("option", { name: "Auto-deny" }));

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
