import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ApprovalRule, Lane, Playbook, Repo } from "../bindings";
import { DaemonRpcError } from "../ipc/rpc";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import { createMessageStore } from "../stores/messages";
import { createNotificationStore } from "../stores/notifications";
import ControlCenter, {
  groupApprovalRules,
  journalQueryParams,
  playbookState,
  replacementDialog,
  scheduleAddParams,
} from "./ControlCenter";

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string) => {
    if (method === "journal.query") return Promise.resolve({ entries: [] });
    if (method === "playbook.list") return Promise.resolve({ playbooks: [] });
    if (method === "schedule.list") return Promise.resolve({ schedules: [] });
    if (method === "approval.list") return Promise.resolve({ rules: [] });
    if (method === "messages.list") return Promise.resolve({ messages: [] });
    if (method === "commit.recent") return Promise.resolve([]);
    if (method === "sessions") return Promise.resolve([]);
    if (method === "timeline") return Promise.resolve({ rows: [] });
    return Promise.resolve({});
  },
  subscribeDaemon: () => Promise.resolve(() => undefined),
  DaemonRpcError: class extends Error {
    code: number;
    data: unknown;
    constructor(init: { code: number; message: string; data?: unknown }) {
      super(init.message);
      this.code = init.code;
      this.data = init.data;
    }
  },
}));

afterEach(() => {
  cleanup();
});

describe("safe dialog answers", () => {
  it("extracts the replacement after DIALOG_CHANGED", () => {
    const dialog = { title: "Question", question: "Continue?", body: [], options: [], selected: null };
    const error = new DaemonRpcError({ code: -32010, message: "dialog changed", data: { dialog } });
    expect(replacementDialog(error)).toEqual(dialog);
  });

  it("does not treat unrelated failures as replacement dialogs", () => {
    expect(replacementDialog(new Error("offline"))).toBeUndefined();
  });
});

describe("journal query", () => {
  it("asks for the recent tail when the box is empty", () => {
    expect(journalQueryParams("")).toEqual({ limit: 200 });
    expect(journalQueryParams("   ")).toEqual({ limit: 200 });
  });

  it("trims a real search before sending it", () => {
    expect(journalQueryParams("  merge_lane  ")).toEqual({ query: "merge_lane", limit: 200 });
  });
});

describe("playbook approval state", () => {
  const base: Playbook = {
    name: "release-all",
    content: "steps",
    status: "draft",
    draft_content: null,
    created_at: "2026-07-27T00:00:00Z",
    updated_at: "2026-07-27T00:00:00Z",
    approved_at: null,
  };

  it("treats a fresh draft as awaiting approval", () => {
    expect(playbookState(base)).toEqual({ label: "draft", awaitingApproval: true });
  });

  it("treats an approved playbook with no pending revision as settled", () => {
    const approved = { ...base, status: "approved", approved_at: "2026-07-27T01:00:00Z" };
    expect(playbookState(approved)).toEqual({ label: "approved", awaitingApproval: false });
  });

  it("flags a re-drafted playbook as pending without calling it a draft", () => {
    const revised = { ...base, status: "approved", draft_content: "new steps" };
    expect(playbookState(revised)).toEqual({
      label: "approved · revision pending",
      awaitingApproval: true,
    });
  });
});

describe("schedule add params", () => {
  it("trims the spec and goal", () => {
    expect(scheduleAddParams("  weekdays 09:00 ", " briefing ", "")).toEqual({
      spec: "weekdays 09:00",
      prompt: "briefing",
    });
  });

  it("omits a blank or nonsense cap instead of sending zero", () => {
    for (const cap of ["", "   ", "abc", "0", "-4"]) {
      expect(scheduleAddParams("daily 09:00", "g", cap)).toEqual({ spec: "daily 09:00", prompt: "g" });
    }
  });

  it("passes a real cap through", () => {
    expect(scheduleAddParams("every 30m", "g", " 20 ")).toEqual({
      spec: "every 30m",
      prompt: "g",
      max_actions: 20,
    });
  });
});

describe("approval rules grouping", () => {
  const rule = (repo: string, pattern: string): ApprovalRule => ({
    repo,
    pattern,
    created_at: "2026-07-27T00:00:00Z",
  });

  it("groups by repo and sorts both levels", () => {
    const grouped = groupApprovalRules([
      rule("zeta", "cargo test"),
      rule("alpha", "pnpm test"),
      rule("alpha", "cargo build"),
    ]);
    expect(grouped.map((g) => g.repo)).toEqual(["alpha", "zeta"]);
    expect(grouped[0].rules.map((r) => r.pattern)).toEqual(["cargo build", "pnpm test"]);
  });

  it("keeps the same pattern in two repos as two rules", () => {
    const grouped = groupApprovalRules([rule("a", "cargo test"), rule("b", "cargo test")]);
    expect(grouped).toHaveLength(2);
    expect(grouped.every((g) => g.rules.length === 1)).toBe(true);
  });

  it("returns nothing for no rules", () => {
    expect(groupApprovalRules([])).toEqual([]);
  });
});

describe("ControlCenter component UI", () => {
  const mockRepo: Repo = {
    id: 1,
    name: "my-project",
    path: "/path/to/my-project",
    added_at: "2026-07-20T00:00:00Z",
    worktree_root_template: null,
    hidden: false,
  };
  const mockLane: Lane = {
    id: 10,
    repo: mockRepo,
    worktree: {
      id: 10,
      repo_id: 1,
      path: "/path/to/my-project/wt",
      branch: "feature-branch",
      head: "abc",
      is_main: false,
      name: "feature-branch",
    },
    state: {
      worktree_id: 10,
      head: "abc",
      branch: "feature-branch",
      upstream: null,
      ahead: 0,
      behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 },
      last_commit_at: null,
      locked: false,
      prunable: false,
      last_change_at: null,
    },
    agent_sessions: [],
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
  };

  function setup() {
    const [controlOpen, setControlOpen] = createSignal(false);
    const fleet = {
      repos: () => [mockRepo],
      lanes: () => [mockLane],
      selectedLane: () => mockLane,
      selectedLaneId: () => mockLane.id,
      setSelectedLaneId: vi.fn(),
      setFocusedWindow: vi.fn(),
      refresh: vi.fn().mockResolvedValue(undefined),
    } as unknown as FleetStore;
    const actions = {
      controlOpen,
      openControl: () => setControlOpen(true),
      closeControl: () => setControlOpen(false),
      toggleControl: () => setControlOpen((o) => !o),
      spawn: vi.fn(),
      newLane: vi.fn(),
      addRepo: vi.fn(),
      openSettings: vi.fn(),
      openSettingsTab: vi.fn(),
    } as unknown as ActionsStore;
    const notifications = createNotificationStore(() => undefined);
    const messages = createMessageStore(() => undefined, {
      list: async () => ({ messages: [], next_before: null }),
      markRead: async () => ({} as any),
      subscribe: async () => () => undefined,
    });

    return { fleet, actions, notifications, messages, setControlOpen };
  }

  it("opens palette via trigger button and renders search input", async () => {
    const { fleet, actions, notifications, messages } = setup();
    render(() => (
      <ControlCenter
        fleet={fleet}
        actions={actions}
        notifications={notifications}
        messages={messages}
      />
    ));

    const trigger = screen.getByRole("button", { name: /Command Palette/i });
    expect(screen.queryByRole("dialog", { name: "Command Palette" })).not.toBeInTheDocument();

    // Click trigger to open
    fireEvent.click(trigger);
    expect(actions.controlOpen()).toBe(true);

    const dialog = await screen.findByRole("dialog", { name: "Command Palette" });
    expect(dialog).toBeInTheDocument();

    // Search input rendered
    const searchInput = screen.getByPlaceholderText(/Type a command or search repos, lanes…/i);
    expect(searchInput).toBeInTheDocument();

    // Commands rendered
    expect(screen.getByText("Spawn New Agent Session")).toBeInTheDocument();
    expect(screen.getByText("Add Repository")).toBeInTheDocument();
    expect(screen.getByText("Open Settings")).toBeInTheDocument();

    // Lanes rendered
    expect(screen.getByText("feature-branch")).toBeInTheDocument();
  });

  it("filters items by search input", async () => {
    const { fleet, actions, notifications, messages } = setup();
    render(() => (
      <ControlCenter
        fleet={fleet}
        actions={actions}
        notifications={notifications}
        messages={messages}
      />
    ));

    actions.openControl();
    await screen.findByRole("dialog", { name: "Command Palette" });

    const searchInput = screen.getByPlaceholderText(/Type a command or search repos, lanes…/i);
    fireEvent.input(searchInput, { target: { value: "feature" } });

    expect(screen.getByText("feature-branch")).toBeInTheDocument();
    expect(screen.queryByText("Add Repository")).not.toBeInTheDocument();
  });

  it("closes palette on Escape key press", async () => {
    const { fleet, actions, notifications, messages } = setup();
    render(() => (
      <ControlCenter
        fleet={fleet}
        actions={actions}
        notifications={notifications}
        messages={messages}
      />
    ));

    actions.openControl();
    await screen.findByRole("dialog", { name: "Command Palette" });

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Command Palette" })).not.toBeInTheDocument();
      expect(actions.controlOpen()).toBe(false);
    });
  });
});
