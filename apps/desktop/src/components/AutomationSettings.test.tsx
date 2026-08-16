import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import AutomationSettings from "./AutomationSettings";

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string) => {
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
    return Promise.resolve({});
  },
}));

afterEach(() => {
  cleanup();
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
});
