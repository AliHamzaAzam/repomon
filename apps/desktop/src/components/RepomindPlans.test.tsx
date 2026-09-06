import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import RepomindPlans from "./RepomindPlans";

const daemonCall = vi.fn();

// Provide subscribeDaemon because importing the shared store initializes its status subscription.
vi.mock("../ipc/rpc", () => ({
  daemonCall: (...args: unknown[]) => daemonCall(...args),
  subscribeDaemon: vi.fn(async () => () => {}),
}));

afterEach(() => {
  cleanup();
  daemonCall.mockReset();
});

const SHIP_R6 = "---\ntitle: Ship R6\nowner: lane-7/1\n---\n\nNext step: land the board\n";

/// Stubs board reads against a shared plan directory and resolves unhandled writes to null.
function mockDaemon(overrides: Record<string, unknown> = {}) {
  daemonCall.mockImplementation((method: string, params?: { path?: string }) => {
    if (method in overrides) {
      const value = overrides[method];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    switch (method) {
      case "file.list":
        return Promise.resolve({
          entries: [
            { name: "ship-r6.md", path: "plans/active/ship-r6.md", is_dir: false, size: 40, ignored: false },
            { name: "README.md", path: "plans/active/README.md", is_dir: false, size: 10, ignored: false },
          ],
          truncated: false,
        });
      case "file.read":
        return params?.path === "plans/active/ship-r6.md"
          ? Promise.resolve({
              content: SHIP_R6,
              mtime_ms: 0,
              size: 40,
              truncated: false,
              kind: "text",
              large: false,
            })
          : Promise.reject(new Error("no such file"));
      default:
        return Promise.resolve(null);
    }
  });
}

describe("the plans board", () => {
  it.each(["Escape", "Cancel"])("returns focus to Add goal after %s cancellation", async (action) => {
    mockDaemon();
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: "Add goal" });
    trigger.focus();
    fireEvent.click(trigger);
    const field = screen.getByRole("textbox", { name: "Goal title" });
    expect(field).toHaveFocus();
    if (action === "Escape") fireEvent.keyDown(field, { key: "Escape" });
    else fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("form", { name: "Add goal" })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    fireEvent.click(trigger);
    expect(screen.getByRole("textbox", { name: "Goal title" })).toHaveFocus();
    expect(daemonCall).not.toHaveBeenCalledWith("file.write", expect.anything());
  });

  it("lists a goal with its next step and owner, skipping the directory README", async () => {
    mockDaemon();
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
    expect(screen.getByText("Next: land the board")).toBeInTheDocument();
    expect(screen.getByText(/lane-7\/1/)).toBeInTheDocument();
    expect(screen.queryByText("README")).toBeNull();
  });

  it("hides the Next line rather than showing 'Next: .', and shows unassigned only when the owner is genuinely unset", async () => {
    const bare =
      "---\ntitle: Bare goal\nnext step: .\ncreated: 2026-09-01T00:00:00Z\n---\n\n# Bare goal\n\nno owner line at all\n";
    daemonCall.mockImplementation((method: string, params?: { path?: string }) => {
      switch (method) {
        case "file.list":
          return Promise.resolve({
            entries: [{ name: "bare.md", path: "plans/active/bare.md", is_dir: false, size: 10, ignored: false }],
            truncated: false,
          });
        case "file.read":
          return params?.path === "plans/active/bare.md"
            ? Promise.resolve({
                content: bare,
                mtime_ms: 0,
                size: bare.length,
                truncated: false,
                kind: "text",
                large: false,
              })
            : Promise.reject(new Error("no such file"));
        default:
          return Promise.resolve(null);
      }
    });
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("Bare goal")).toBeInTheDocument());
    expect(screen.queryByText(/^Next:/)).toBeNull();
    expect(screen.getByText(/^unassigned/)).toBeInTheDocument();
  });

  it("explains the file shape when no goal is in flight", async () => {
    mockDaemon({ "file.list": { entries: [], truncated: false } });
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} />);

    await waitFor(() =>
      expect(screen.getByText(/one file in plans\/active/)).toBeInTheDocument(),
    );
  });

  it("opens a goal in the editor on the home lane", async () => {
    mockDaemon();
    const onOpen = vi.fn();
    render(() => <RepomindPlans laneId={90} onOpen={onOpen} />);

    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Ship R6"));
    expect(onOpen).toHaveBeenCalledWith("plans/active/ship-r6.md");
  });

  it("tells the primary controller first, then writes the goal file owned by repomind", async () => {
    mockDaemon();
    const onChanged = vi.fn();
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} onChanged={onChanged} />);

    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Add goal"));
    fireEvent.input(screen.getByLabelText("Goal title"), {
      target: { value: "Ship the control room" },
    });
    fireEvent.input(screen.getByLabelText("Next step"), { target: { value: "land the board" } });
    fireEvent.submit(screen.getByLabelText("Add goal", { selector: "form" }));

    await waitFor(() =>
      expect(daemonCall).toHaveBeenCalledWith("file.write", {
        lane_id: 90,
        path: "plans/active/ship-the-control-room.md",
        content: expect.stringContaining("Next step: land the board"),
      }),
    );
    expect(daemonCall).toHaveBeenCalledWith("repomind.instruct", {
      text: "New goal in plans/active/ship-the-control-room.md: Ship the control room. Pick it up.",
    });
    const [, writeParams] = daemonCall.mock.calls.find(([method]) => method === "file.write") ?? [];
    expect((writeParams as { content: string }).content).toContain("owner: repomind");
    await waitFor(() => expect(screen.getByText(/told the controller/)).toBeInTheDocument());
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("defaults the next step to the title when the operator gives no intent", async () => {
    mockDaemon();
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Add goal"));
    fireEvent.input(screen.getByLabelText("Goal title"), { target: { value: "Ship it" } });
    fireEvent.submit(screen.getByLabelText("Add goal", { selector: "form" }));

    await waitFor(() =>
      expect(daemonCall).toHaveBeenCalledWith(
        "file.write",
        expect.objectContaining({ content: expect.stringContaining("Next step: Ship it") }),
      ),
    );
  });

  it("keeps the goal, owned unassigned, and says Repomind will pick it up at boot when no controller is running", async () => {
    mockDaemon({ "repomind.instruct": new Error("no controller is running in the repomind home") });
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Add goal"));
    fireEvent.input(screen.getByLabelText("Goal title"), { target: { value: "Lone goal" } });
    fireEvent.input(screen.getByLabelText("Next step"), { target: { value: "wait for one" } });
    fireEvent.submit(screen.getByLabelText("Add goal", { selector: "form" }));

    await waitFor(() =>
      expect(
        screen.getByText(
          /No controller is running; start Repomind and it will pick this up at boot/,
        ),
      ).toBeInTheDocument(),
    );
    expect(daemonCall).toHaveBeenCalledWith(
      "file.write",
      expect.objectContaining({
        lane_id: 90,
        content: expect.stringContaining("owner: unassigned"),
      }),
    );
  });

  it("moves a finished goal into plans/done with its outcome appended", async () => {
    mockDaemon();
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("Ship R6")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Done"));
    fireEvent.input(screen.getByLabelText("Outcome"), { target: { value: "shipped on main" } });
    fireEvent.submit(screen.getByLabelText("Close Ship R6", { selector: "form" }));

    await waitFor(() =>
      expect(daemonCall).toHaveBeenCalledWith("file.rename", {
        lane_id: 90,
        from: "plans/active/ship-r6.md",
        to: "plans/done/ship-r6.md",
      }),
    );
    expect(daemonCall).toHaveBeenCalledWith("file.write", {
      lane_id: 90,
      path: "plans/done/ship-r6.md",
      content: expect.stringContaining("Outcome: shipped on main"),
    });
  });

  it("reports a directory it cannot read instead of claiming there are no goals", async () => {
    mockDaemon({ "file.list": new Error("plans/active: no such directory") });
    render(() => <RepomindPlans laneId={90} onOpen={vi.fn()} />);

    await waitFor(() =>
      expect(screen.getByText("plans/active: no such directory")).toBeInTheDocument(),
    );
    expect(screen.queryByText(/one file in plans\/active/)).toBeNull();
  });
});
