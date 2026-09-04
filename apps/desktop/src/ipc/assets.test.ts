import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { ensureWorktreeAssetsAllowed, resetWorktreeAssetsAllowedCacheForTests } from "./assets";

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  resetWorktreeAssetsAllowedCacheForTests();
});

describe("ensureWorktreeAssetsAllowed", () => {
  it("grants a worktree root once and skips repeat calls for the same root", async () => {
    await ensureWorktreeAssetsAllowed("/repo/lane-a");
    await ensureWorktreeAssetsAllowed("/repo/lane-a");
    await ensureWorktreeAssetsAllowed("/repo/lane-a");

    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("allow_worktree_assets", { path: "/repo/lane-a" });
  });

  it("grants each distinct worktree root separately", async () => {
    await ensureWorktreeAssetsAllowed("/repo/lane-a");
    await ensureWorktreeAssetsAllowed("/repo/lane-b");

    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(invokeMock).toHaveBeenNthCalledWith(1, "allow_worktree_assets", { path: "/repo/lane-a" });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "allow_worktree_assets", { path: "/repo/lane-b" });
  });

  it("does not cache a failed grant, so a later call can retry", async () => {
    invokeMock.mockRejectedValueOnce(new Error("scope error"));
    await expect(ensureWorktreeAssetsAllowed("/repo/lane-a")).rejects.toThrow("scope error");

    invokeMock.mockResolvedValueOnce(undefined);
    await ensureWorktreeAssetsAllowed("/repo/lane-a");

    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
});
