import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Repo } from "../bindings";
import RepoNotesModal from "./RepoNotesModal";

const calls = vi.hoisted(() => ({ list: [] as Array<{ method: string; params: unknown }> }));
const responses = vi.hoisted(() => ({ get: {} as Record<string, unknown>, setFails: null as string | null }));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => {
    calls.list.push({ method, params });
    if (method === "repo.notes.get") return Promise.resolve(responses.get);
    if (responses.setFails) return Promise.reject(new Error(responses.setFails));
    return Promise.resolve({ repo_id: 2, bytes: 0, path: "/n/r.md" });
  },
}));

afterEach(() => {
  cleanup();
  calls.list = [];
  responses.setFails = null;
});

const repo: Repo = {
  id: 2, path: "/code/r", name: "r", added_at: "2026-07-27T00:00:00Z",
  worktree_root_template: null, hidden: false,
};

function open(content = "", exists = true) {
  responses.get = { repo_id: 2, name: "r", exists, content, path: "/notes/r.md" };
  const onClose = vi.fn();
  const view = render(() => <RepoNotesModal repo={repo} onClose={onClose} />);
  return { onClose, ...view };
}

async function textarea() {
  return (await screen.findByLabelText("Notes markdown for r")) as HTMLTextAreaElement;
}

describe("repo notes editor", () => {
  it("loads the repo's notes and saves an edit", async () => {
    const { onClose } = open("old notes\n");
    const box = await textarea();
    expect(box.value).toBe("old notes\n");

    fireEvent.input(box, { target: { value: "use pnpm test\n" } });
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(calls.list[calls.list.length - 1]).toEqual({
      method: "repo.notes.set",
      params: { repo_id: 2, content: "use pnpm test\n" },
    });
  });

  it("will not save until something changed", async () => {
    open("unchanged\n");
    await textarea();
    expect(screen.getByText("Save")).toBeDisabled();
  });

  it("counts bytes, not characters, and blocks a save over the cap", async () => {
    open("");
    const box = await textarea();

    // Multi-byte content: a character count would under-read this and let a doomed save through,
    // since the daemon caps on bytes.
    fireEvent.input(box, { target: { value: "é".repeat(10) } });
    expect(screen.getByText("20 / 8192 bytes")).toBeInTheDocument();

    fireEvent.input(box, { target: { value: "x".repeat(8193) } });
    expect(screen.getByText("8193 / 8192 bytes")).toBeInTheDocument();
    expect(screen.getByText("Save")).toBeDisabled();
  });

  it("surfaces a rejected save and stays open", async () => {
    const { onClose } = open("a\n");
    const box = await textarea();
    responses.setFails = "notes are 9000 bytes; the cap is 8192 bytes";

    fireEvent.input(box, { target: { value: "b\n" } });
    fireEvent.click(screen.getByText("Save"));

    expect(await screen.findByText(/the cap is 8192 bytes/)).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("says so when the repo has no notes file yet", async () => {
    open("", false);
    expect(await screen.findByText(/not created yet/)).toBeInTheDocument();
  });
});
