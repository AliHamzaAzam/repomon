import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  invoke: vi.fn(),
  relaunch: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: mocks.relaunch }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }));

import { checkForUpdate, installAvailableUpdate } from "./updater";

beforeEach(() => {
  vi.clearAllMocks();
  mocks.invoke.mockResolvedValue(undefined);
  mocks.relaunch.mockResolvedValue(undefined);
});

describe("desktop updater", () => {
  it("marks the daemon update before installation and then relaunches", async () => {
    const order: string[] = [];
    mocks.invoke.mockImplementation(async (command: string) => {
      order.push(command);
    });
    mocks.relaunch.mockImplementation(async () => {
      order.push("relaunch");
    });
    mocks.check.mockResolvedValue({
      version: "0.6.1",
      downloadAndInstall: async (onEvent: (event: unknown) => void) => {
        order.push("install");
        onEvent({ event: "Started", data: { contentLength: 100 } });
        onEvent({ event: "Progress", data: { chunkLength: 40 } });
      },
    });
    const progress: unknown[] = [];

    const update = await checkForUpdate();
    await update?.install((value) => progress.push(value));

    expect(order).toEqual(["mark_daemon_update", "install", "relaunch"]);
    expect(progress[progress.length - 1]).toEqual({ version: "0.6.1", downloaded: 40, total: 100 });
  });

  it("clears the pending marker when installation fails", async () => {
    mocks.check.mockResolvedValue({
      version: "0.6.1",
      downloadAndInstall: async () => {
        throw new Error("download failed");
      },
    });

    const update = await checkForUpdate();
    await expect(update?.install(() => undefined)).rejects.toThrow("download failed");

    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "mark_daemon_update");
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "clear_daemon_update");
    expect(mocks.relaunch).not.toHaveBeenCalled();
  });

  it("does not mark or relaunch when the app is current", async () => {
    mocks.check.mockResolvedValue(null);

    await expect(installAvailableUpdate(() => undefined)).resolves.toBe("current");
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(mocks.relaunch).not.toHaveBeenCalled();
  });
});
