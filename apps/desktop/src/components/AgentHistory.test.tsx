import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import AgentHistory from "./AgentHistory";

afterEach(() => {
  cleanup();
  clearMocks();
});

describe("AgentHistory", () => {
  it("loads the newest page and prepends earlier pages with the returned cursor", async () => {
    const calls: unknown[] = [];
    mockIPC((command, args) => {
      expect(command).toBe("daemon_call");
      calls.push(args);
      const params = (args as { params: { before?: number } }).params;
      if (params.before === 120) {
        return {
          items: [{ role: "user", text: "older", at: null }],
          next_before: null,
        };
      }
      return {
        items: [{ role: "assistant", text: "newest", at: null }],
        next_before: 120,
      };
    });

    render(() => <AgentHistory laneId={7} sessionId="session-7" visible />);

    expect(await screen.findByText("newest")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Load earlier" }));
    expect(await screen.findByText("older")).toBeInTheDocument();
    expect(screen.getAllByRole("article").map((item) => item.textContent)).toEqual([
      "userolder",
      "assistantnewest",
    ]);
    expect(calls).toEqual([
      {
        method: "agent.transcript_page",
        params: { lane_id: 7, session_id: "session-7" },
      },
      {
        method: "agent.transcript_page",
        params: { lane_id: 7, session_id: "session-7", before: 120 },
      },
    ]);
  });

  it("does not request a transcript until its pane is visible", async () => {
    let called = false;
    mockIPC(() => {
      called = true;
      return { items: [], next_before: null };
    });

    render(() => <AgentHistory laneId={4} sessionId="session-4" visible={false} />);
    await waitFor(() => expect(called).toBe(false));
  });

  it("loads earlier history when native scrolling reaches the top", async () => {
    const cursors: Array<number | undefined> = [];
    mockIPC((_command, args) => {
      const before = (args as { params: { before?: number } }).params.before;
      cursors.push(before);
      return before === 80
        ? { items: [{ role: "user", text: "first", at: null }], next_before: null }
        : { items: [{ role: "assistant", text: "second", at: null }], next_before: 80 };
    });

    render(() => <AgentHistory laneId={2} sessionId="session-2" visible />);
    expect(await screen.findByText("second")).toBeInTheDocument();
    fireEvent.scroll(screen.getByLabelText("Full agent history"));
    expect(await screen.findByText("first")).toBeInTheDocument();
    expect(cursors).toEqual([undefined, 80]);
  });
});
