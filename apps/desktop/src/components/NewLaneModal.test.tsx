import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Repo } from "../bindings";
import NewLaneModal from "./NewLaneModal";

const state = vi.hoisted(() => ({
  createError: null as string | null,
  createCalls: [] as Array<{ repo_id: number; branch: string; source_branch?: string }>,
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => {
    if (method === "lane.create") {
      state.createCalls.push(params as { repo_id: number; branch: string; source_branch?: string });
      if (state.createError) return Promise.reject(new Error(state.createError));
      return Promise.resolve({ id: 10, repo_id: 1, path: "/tmp/wt" });
    }
    return Promise.resolve(null);
  },
}));

afterEach(() => {
  cleanup();
  state.createError = null;
  state.createCalls = [];
});

describe("NewLaneModal error rendering", () => {
  const dummyRepos: Repo[] = [
    { id: 1, name: "repomon", path: "/tmp/repo", hidden: false, added_at: "2026-08-01T00:00:00Z", worktree_root_template: null },
  ];

  it("renders friendly error and details when lane creation fails due to missing git", async () => {
    state.createError = "failed to run git worktree add: No such file or directory (os error 2)";

    render(() => (
      <NewLaneModal repos={dummyRepos} onClose={vi.fn()} onDone={vi.fn()} />
    ));

    const branchInput = screen.getByPlaceholderText("feature/my-change");
    fireEvent.input(branchInput, { target: { value: "feature/auth" } });

    const createButton = screen.getByRole("button", { name: "Create Lane" });
    fireEvent.click(createButton);

    const friendlyMsg = await screen.findByText(
      "git isn't installed or couldn't be found — Repomon needs git to manage repositories and worktrees",
    );
    expect(friendlyMsg).toBeInTheDocument();
    expect(screen.getByText("Technical details")).toBeInTheDocument();
  });
});
