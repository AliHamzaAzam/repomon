import { afterEach, describe, expect, it, vi } from "vitest";

const daemonCall = vi.fn();

vi.mock("../ipc/rpc", () => ({
  daemonCall: (...args: unknown[]) => daemonCall(...args),
  subscribeDaemon: vi.fn(async () => () => {}),
}));

const { addGoal } = await import("./repomind");

afterEach(() => {
  daemonCall.mockReset();
});

describe("addGoal", () => {
  it("tells the controller before writing, and the write carries owner: repomind when it landed", async () => {
    daemonCall.mockImplementation((method: string) => {
      if (method === "repomind.instruct") return Promise.resolve({ outcome: "sent" });
      if (method === "file.write") return Promise.resolve(null);
      throw new Error(`unexpected call: ${method}`);
    });

    const result = await addGoal(90, "Ship the control room", "land the board");

    expect(result).toEqual({ path: "plans/active/ship-the-control-room.md", told: true });
    const methods = daemonCall.mock.calls.map(([method]) => method);
    expect(methods).toEqual(["repomind.instruct", "file.write"]);
    expect(daemonCall).toHaveBeenCalledWith("repomind.instruct", {
      text: "New goal in plans/active/ship-the-control-room.md: Ship the control room. Pick it up.",
    });
    expect(daemonCall).toHaveBeenCalledWith("file.write", {
      lane_id: 90,
      path: "plans/active/ship-the-control-room.md",
      content: expect.stringContaining("owner: repomind"),
    });
    const [, writeParams] = daemonCall.mock.calls[1] as [string, { content: string }];
    expect(writeParams.content).toContain("Next step: land the board");
  });

  it("still writes the goal file, owned unassigned, when no controller is running", async () => {
    daemonCall.mockImplementation((method: string) => {
      if (method === "repomind.instruct") {
        return Promise.reject(new Error("no controller is running in the repomind home"));
      }
      if (method === "file.write") return Promise.resolve(null);
      throw new Error(`unexpected call: ${method}`);
    });

    const result = await addGoal(90, "Lone goal", "wait for one");

    expect(result).toEqual({ path: "plans/active/lone-goal.md", told: false });
    expect(daemonCall).toHaveBeenCalledWith("file.write", {
      lane_id: 90,
      path: "plans/active/lone-goal.md",
      content: expect.stringContaining("owner: unassigned"),
    });
  });

  it("uses the title as the next step when the operator gave no intent", async () => {
    daemonCall.mockImplementation((method: string) => {
      if (method === "repomind.instruct") return Promise.resolve({ outcome: "sent" });
      if (method === "file.write") return Promise.resolve(null);
      throw new Error(`unexpected call: ${method}`);
    });

    await addGoal(90, "Ship it", "");

    const [, writeParams] = daemonCall.mock.calls.find(([method]) => method === "file.write") as [
      string,
      { content: string },
    ];
    expect(writeParams.content).toContain("Next step: Ship it");
  });
});
