import { afterEach, describe, expect, it, vi } from "vitest";

const daemonCall = vi.fn();

vi.mock("../ipc/rpc", () => ({
  daemonCall: (...args: unknown[]) => daemonCall(...args),
  subscribeDaemon: vi.fn(async () => () => {}),
}));

const { addGoal, createRepomindStore } = await import("./repomind");

afterEach(() => {
  daemonCall.mockReset();
  vi.restoreAllMocks();
  vi.useRealTimers();
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


describe("Repomind event refreshes", () => {
  it("coalesces bursts, gates background events, resumes on focus and cancels on stop", async () => {
    vi.useFakeTimers();
    let focused = true;
    let hidden = false;
    vi.spyOn(document, "hasFocus").mockImplementation(() => focused);
    vi.spyOn(document, "hidden", "get").mockImplementation(() => hidden);
    let emit!: Parameters<import("./repomind").RepomindSource["subscribe"]>[0];
    const status = vi.fn(async () => ({}) as import("../bindings").RepomindStatus);
    const store = createRepomindStore({
      status,
      boot: async () => {},
      export: async () => {},
      subscribe: async (sink) => { emit = sink; return () => {}; },
    });
    const event = (method: `event.${string}`, params = {}) => emit({ jsonrpc: "2.0", method, params });
    store.setControllerWindows(["controller"]);
    store.start();
    try {
      await vi.advanceTimersByTimeAsync(0);
      expect(status).toHaveBeenCalledTimes(1);
      for (let i = 0; i < 100; i += 1) event("event.repo.changed");
      event("event.repomind.changed");
      event("event.agent.status", { window: "controller" });
      await vi.advanceTimersByTimeAsync(59);
      expect(status).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(status).toHaveBeenCalledTimes(2);
      event("event.agent.status", { window: "other" });
      await vi.advanceTimersByTimeAsync(60);
      expect(status).toHaveBeenCalledTimes(2);

      // A queued foreground event must not escape the policy if focus leaves before it fires.
      event("event.repo.changed");
      focused = false;
      window.dispatchEvent(new Event("blur"));
      for (let i = 0; i < 100; i += 1) event("event.repo.changed");
      event("event.agent.status", { window: "controller" });
      await vi.advanceTimersByTimeAsync(29_999);
      expect(status).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(1);
      expect(status).toHaveBeenCalledTimes(3);
      focused = true;
      window.dispatchEvent(new Event("focus"));
      await vi.advanceTimersByTimeAsync(0);
      expect(status).toHaveBeenCalledTimes(4);

      hidden = true;
      document.dispatchEvent(new Event("visibilitychange"));
      event("event.repo.changed");
      await vi.advanceTimersByTimeAsync(60);
      expect(status).toHaveBeenCalledTimes(4);
      hidden = false;
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(0);
      expect(status).toHaveBeenCalledTimes(5);
      event("event.repo.changed");
      store.stop();
      await vi.advanceTimersByTimeAsync(60_000);
      event("event.repo.changed");
      await vi.advanceTimersByTimeAsync(60);
      expect(status).toHaveBeenCalledTimes(5);
    } finally { store.stop(); }
  });
});
