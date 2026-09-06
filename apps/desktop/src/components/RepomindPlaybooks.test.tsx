import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Playbook } from "../bindings";
import RepomindPlaybooks from "./RepomindPlaybooks";

const daemonCall = vi.fn();

vi.mock("../ipc/rpc", () => ({
  daemonCall: (...args: unknown[]) => daemonCall(...args),
}));

afterEach(() => {
  cleanup();
  daemonCall.mockReset();
});

function playbook(overrides: Partial<Playbook> = {}): Playbook {
  return {
    name: "fleet-sweep",
    content: "sweep the fleet\n",
    status: "draft",
    draft_content: null,
    created_at: "2026-09-04T00:00:00Z",
    updated_at: "2026-09-04T00:00:00Z",
    approved_at: null,
    ...overrides,
  };
}

function mockDaemon(playbooks: Playbook[], overrides: Record<string, unknown> = {}) {
  daemonCall.mockImplementation((method: string) => {
    if (method in overrides) {
      const value = overrides[method];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    if (method === "playbook.list") return Promise.resolve({ playbooks });
    return Promise.resolve(null);
  });
}

describe("the playbooks section", () => {
  it.each(["draft", "approved"])("opens the pending %s text for review without approving it", async (status) => {
    mockDaemon([playbook({ status, draft_content: status === "approved" ? "pending revision" : null })]);
    const onOpen = vi.fn();
    render(() => <RepomindPlaybooks onOpen={onOpen} />);
    fireEvent.click(await screen.findByRole("button", { name: "Review draft fleet-sweep" }));
    expect(onOpen).toHaveBeenCalledWith("playbooks/drafts/fleet-sweep.md");
    expect(daemonCall).not.toHaveBeenCalledWith("playbook.approve", expect.anything());
    expect(daemonCall).not.toHaveBeenCalledWith("playbook.reject", expect.anything());
  });

  it("splits drafts waiting on a human from the approved ones agents get", async () => {
    mockDaemon([
      playbook(),
      playbook({ name: "nightly", status: "approved", approved_at: "2026-09-04T00:00:00Z" }),
    ]);
    render(() => <RepomindPlaybooks onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("fleet-sweep")).toBeInTheDocument());
    expect(screen.getByText("nightly")).toBeInTheDocument();
    expect(screen.getByText("1 waiting on you")).toBeInTheDocument();

    expect(screen.getAllByText("Approve")).toHaveLength(1);
    expect(screen.getAllByText("Reject")).toHaveLength(1);
    expect(screen.getAllByText("Open")).toHaveLength(1);
  });

  it("lists an approved playbook with a pending revision on both sides", async () => {
    mockDaemon([
      playbook({
        name: "nightly",
        status: "approved",
        draft_content: "v2\n",
        approved_at: "2026-09-04T00:00:00Z",
      }),
    ]);
    render(() => <RepomindPlaybooks onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("revision")).toBeInTheDocument());
    expect(screen.getAllByText("nightly")).toHaveLength(2);
  });

  it("approves a draft and re-reads the list", async () => {
    mockDaemon([playbook()]);
    const onChanged = vi.fn();
    render(() => <RepomindPlaybooks onOpen={vi.fn()} onChanged={onChanged} />);

    await waitFor(() => expect(screen.getByText("Approve")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Approve"));

    await waitFor(() =>
      expect(daemonCall).toHaveBeenCalledWith("playbook.approve", { name: "fleet-sweep" }),
    );
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("rejects a draft rather than deleting it", async () => {
    mockDaemon([playbook()]);
    render(() => <RepomindPlaybooks onOpen={vi.fn()} />);

    await waitFor(() => expect(screen.getByText("Reject")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Reject"));

    await waitFor(() =>
      expect(daemonCall).toHaveBeenCalledWith("playbook.reject", { name: "fleet-sweep" }),
    );
    expect(daemonCall).not.toHaveBeenCalledWith("playbook.delete", expect.anything());
  });

  it("opens an approved playbook's file in the editor", async () => {
    mockDaemon([playbook({ status: "approved", approved_at: "2026-09-04T00:00:00Z" })]);
    const onOpen = vi.fn();
    render(() => <RepomindPlaybooks onOpen={onOpen} />);

    await waitFor(() => expect(screen.getByText("Open")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Open"));
    expect(onOpen).toHaveBeenCalledWith("playbooks/fleet-sweep.md");
  });

  it("says where playbooks come from when the home has none", async () => {
    mockDaemon([]);
    render(() => <RepomindPlaybooks onOpen={vi.fn()} />);

    await waitFor(() =>
      expect(screen.getByText(/drafts one into playbooks\/drafts/)).toBeInTheDocument(),
    );
  });
});
