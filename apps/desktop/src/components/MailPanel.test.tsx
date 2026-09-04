import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FleetMessage, Lane } from "../bindings";
import type { ConfirmOptions } from "./ConfirmDialog";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import type { MessageStore } from "../stores/messages";
import MailPanel, { groupMailThreads } from "./MailPanel";

function resolved(address: string, laneId: number | null) {
  return {
    address,
    lane_id: laneId,
    slot: laneId === null ? null : 1,
    window: laneId === null ? null : `lane-${laneId}`,
    session_id: laneId === null ? null : `session-${laneId}`,
    agent_kind: laneId === null ? null : "codex",
  };
}

function message(
  id: string,
  options: Partial<FleetMessage> & { laneId?: number } = {},
): FleetMessage {
  const laneId = options.laneId ?? 2;
  return {
    id,
    requested_to: `lane-${laneId}/1`,
    sender: resolved("operator", null),
    recipient: resolved(`lane-${laneId}/1`, laneId),
    body: `body ${id}`,
    thread_id: `thread-${id}`,
    reply_to: null,
    remaining_hops: 6,
    created_at: `2026-08-28T00:00:0${id}.000Z`,
    delivered_at: null,
    read_at: null,
    delivery_error: null,
    delivery_state: "queued",
    read_state: "unread",
    ...options,
  };
}

function lane(id: number, repo: string, branch: string): Lane {
  return {
    id,
    repo: { id, name: repo, label: null, path: `/tmp/${repo}`, added_at: "2026-08-28T00:00:00Z", worktree_root_template: null, hidden: false, position: null },
    worktree: { id, repo_id: id, path: `/tmp/${repo}/${branch}`, branch, head: "abc", is_main: false, name: branch },
    state: {
      worktree_id: id,
      head: "abc",
      branch,
      upstream: null,
      ahead: 0,
      behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 },
      last_commit_at: null,
      last_change_at: null,
      locked: false,
      prunable: false,
    },
    agent_sessions: [],
    last_activity_at: "2026-08-28T00:00:00Z",
    pinned: false,
    role: null,
  };
}

function fixture() {
  const failedReply = message("2", {
    sender: resolved("lane-2/1", 2),
    recipient: resolved("operator", null),
    body: "agent reply",
    thread_id: "thread-a",
    delivery_state: "failed",
    delivery_error: "composer submission was not observed",
  });
  const sentRoot = message("1", {
    body: "operator request",
    thread_id: "thread-a",
    delivery_state: "delivered",
    delivered_at: "2026-08-28T00:00:02.000Z",
    read_state: "read",
    read_at: "2026-08-28T00:00:03.000Z",
  });
  const otherLane = message("3", { laneId: 3, body: "beta lane mail" });
  const [items] = createSignal([otherLane, failedReply, sentRoot]);
  const open = vi.fn(async () => undefined);
  const markRead = vi.fn(async (id: string) => {
    const current = items().find((item) => item.id === id)!;
    return { ...current, read_state: "read" as const, read_at: "2026-08-28T00:01:00.000Z" };
  });
  const forceSend = vi.fn(async (id: string) => ({
    ...items().find((item) => item.id === id)!,
    delivery_state: "delivered" as const,
    delivered_at: "2026-08-28T00:02:00.000Z",
    delivery_error: null,
  }));
  const deleteMessage = vi.fn(async () => undefined);
  const store = {
    items,
    unread: () => items().filter((item) => item.read_state === "unread").length,
    unreadByLane: () => new Map(),
    nextBefore: () => null,
    refresh: vi.fn(async () => undefined),
    loadMore: vi.fn(async () => undefined),
    start: vi.fn(async () => undefined),
    stop: vi.fn(),
    markRead,
    forceSend,
    deleteMessage,
    open,
  } as unknown as MessageStore;
  const fleet = {
    lanes: () => [lane(2, "alpha", "feature/alpha"), lane(3, "beta", "feature/beta")],
  } as unknown as FleetStore;
  let confirmOptions: ConfirmOptions | null = null;
  const confirm = vi.fn((options: ConfirmOptions) => { confirmOptions = options; });
  const actions = { confirm } as unknown as ActionsStore;
  return {
    store,
    fleet,
    actions,
    failedReply,
    open,
    markRead,
    forceSend,
    deleteMessage,
    confirm,
    getConfirmOptions: () => confirmOptions,
  };
}

afterEach(cleanup);

describe("MailPanel", () => {
  it("groups a thread and exposes delivery, failure, read, and source-lane actions", async () => {
    const { store, fleet, failedReply, open, markRead } = fixture();
    render(() => <MailPanel messages={store} fleet={fleet} />);

    const thread = screen.getByRole("region", { name: "Mail thread thread-a" });
    expect(within(thread).getByText(/2 messages/)).toBeInTheDocument();
    expect(within(thread).getByText("sent")).toBeInTheDocument();
    expect(within(thread).getByText("failed")).toBeInTheDocument();
    expect(within(thread).getByText("composer submission was not observed")).toBeInTheDocument();

    const reply = within(thread).getByText("agent reply").closest("article")!;
    const markButton = within(reply).getByRole("button", { name: "Mark read" });
    fireEvent.click(markButton);
    expect(markRead).toHaveBeenCalledWith("2");
    await waitFor(() => expect(markButton).not.toBeDisabled());

    fireEvent.click(within(reply).getByRole("button", { name: "Open source lane-2/1" }));
    await waitFor(() => expect(open).toHaveBeenCalledWith(failedReply));
  });

  it("filters the management list by either side's lane", () => {
    const { store, fleet } = fixture();
    render(() => <MailPanel messages={store} fleet={fleet} />);

    fireEvent.click(screen.getByRole("combobox", { name: "Filter repomail by lane" }));
    fireEvent.click(screen.getByRole("option", { name: "beta · feature/beta" }));

    expect(screen.getByText("beta lane mail")).toBeInTheDocument();
    expect(screen.queryByText("operator request")).not.toBeInTheDocument();
    expect(screen.queryByText("agent reply")).not.toBeInTheDocument();
  });

  it("labels threads and exposes force-send plus confirmed deletion", async () => {
    const fixtureData = fixture();
    const { container } = render(() => (
      <MailPanel messages={fixtureData.store} fleet={fixtureData.fleet} actions={fixtureData.actions} />
    ));
    const thread = screen.getByRole("region", { name: "Mail thread thread-a" });

    expect(within(thread).getByText("alpha · feature/alpha")).toBeInTheDocument();
    expect(container.textContent).not.toContain("↔");
    const reply = within(thread).getByText("agent reply").closest("article")!;
    expect(within(reply).queryByRole("button", { name: /Force send/ })).toBeNull();

    const queued = screen.getByText("beta lane mail").closest("article")!;
    expect(within(queued).queryByRole("button", { name: "Mark read" })).toBeNull();
    fireEvent.click(within(queued).getByRole("button", { name: /Force send/ }));
    await waitFor(() => expect(fixtureData.forceSend).toHaveBeenCalledWith("3"));

    fireEvent.click(within(reply).getByRole("button", { name: /Delete/ }));
    expect(fixtureData.confirm).toHaveBeenCalledTimes(1);
    expect(fixtureData.getConfirmOptions()?.danger).toBe(true);
    await fixtureData.getConfirmOptions()?.onConfirm();
    expect(fixtureData.deleteMessage).toHaveBeenCalledWith("2");
  });
});

describe("groupMailThreads", () => {
  it("orders threads and their messages newest first", () => {
    const grouped = groupMailThreads([
      message("1", { thread_id: "a" }),
      message("3", { thread_id: "b" }),
      message("2", { thread_id: "a" }),
    ]);
    expect(grouped.map((thread) => thread.id)).toEqual(["b", "a"]);
    expect(grouped[1].messages.map((item) => item.id)).toEqual(["2", "1"]);
  });
});
