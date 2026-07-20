import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";

import { getConnectionStatus } from "./connection";

afterEach(() => clearMocks());

describe("connection IPC", () => {
  it("loads the current host snapshot", async () => {
    mockIPC((command) => {
      expect(command).toBe("connection_status");
      return {
        phase: "connected",
        endpoint: "/tmp/repomon.sock",
        message: null,
        daemon: {
          uptime_secs: 61,
          repos: 3,
          lanes: 5,
          db_size_bytes: 4096,
          version: "0.5.0",
        },
      };
    });

    const snapshot = await getConnectionStatus();

    expect(snapshot.phase).toBe("connected");
    expect(snapshot.daemon?.repos).toBe(3);
  });
});
