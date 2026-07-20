import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";

import { daemonCall } from "./rpc";

afterEach(() => clearMocks());

describe("daemon RPC bridge", () => {
  it("passes typed method parameters through the host", async () => {
    mockIPC((command, args) => {
      expect(command).toBe("daemon_call");
      expect(args).toEqual({
        method: "agent.capture",
        params: { lane_id: 7, window: "lane-7", lines: 30 },
      });
      return { content: "ready" };
    });

    await expect(
      daemonCall("agent.capture", { lane_id: 7, window: "lane-7", lines: 30 }),
    ).resolves.toEqual({ content: "ready" });
  });

  it("exposes daemon error code and replacement data", async () => {
    mockIPC(() => {
      throw { code: -32010, message: "dialog changed", data: { dialog: null } };
    });

    const promise = daemonCall("agent.answer", {
      lane_id: 7,
      choice: 0,
      expect_summary: "old prompt",
    });

    await expect(promise).rejects.toMatchObject({
      name: "DaemonRpcError",
      code: -32010,
      data: { dialog: null },
    });
  });
});
