import { createEffect, createRoot } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import type { DaemonEvent } from "../ipc/rpc";
import { createFleetStore, laneIndicator, type FleetSource } from "./fleet";

function repo(): Repo {
  return {
    id: 2,
    path: "/code/repomon",
    name: "repomon",
    added_at: "2026-07-20T00:00:00Z",
    worktree_root_template: null,
    hidden: false,
    position: null,
    label: null,
  };
}

function agent(overrides: Partial<AgentSession> = {}): AgentSession {
  return {
    id: 0,
    agent: "claude-code",
    repo_id: 2,
    worktree_id: 3,
    started_at: "2026-07-20T00:00:00Z",
    last_activity_at: "2026-07-20T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 0,
    title: "Ship desktop",
    status: "idle",
    external: false,
    session_id: "s1",
    resume_at: null,
    inferred: false,
    tmux_window: "lane-7-1",
    last_message: null,
    pending_prompt: null,
    pending_dialog: null,
    stale: false,
    stalled_since: null,
    subagent_running: null,
    status_reason: "no output for 4m",
    gate: null,
    config_dir: null,
    custom_label: null,
    generated_label: null,
    ...overrides,
  };
}

function lane(sessions: AgentSession[]): Lane {
  return {
    id: 7,
    repo: repo(),
    worktree: {
      id: 3,
      repo_id: 2,
      path: "/code/repomon-wt/desktop",
      branch: "feat/desktop",
      head: "abc",
      is_main: false,
      name: "desktop",
    },
    state: {
      worktree_id: 3,
      head: "abc",
      branch: "feat/desktop",
      upstream: null,
      ahead: 0,
      behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 },
      last_commit_at: null,
      locked: false,
      prunable: false,
      last_change_at: null,
    },
    agent_sessions: sessions,
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
  };
}

/// A source whose snapshot the test rewrites between polls, plus a hook to push a daemon event.
function mutableSource() {
  let sessions: AgentSession[] = [agent()];
  let sink: ((event: DaemonEvent) => void) | null = null;
  let loads = 0;
  const source: FleetSource = {
    load: () => {
      loads += 1;
      return Promise.resolve({
        repos: [repo()],
        lanes: [lane(sessions.map((s) => ({ ...s })))],
        usage: [],
        terminals: [],
        sortReposByActivity: false,
        sortMode: "default",
        tabSortMode: null,
      });
    },
    refreshUsage: () => Promise.resolve(),
    subscribe: (onEvent) => {
      sink = onEvent;
      return Promise.resolve(() => {
        sink = null;
      });
    },
  };
  return {
    source,
    setSessions: (next: AgentSession[]) => {
      sessions = next;
    },
    emit: (method: `event.${string}`, params: unknown) =>
      sink?.({ jsonrpc: "2.0", method, params }),
    loads: () => loads,
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("status propagation to the sidebar", () => {
  it("flips the lane pill from idle to running and back on the next poll", async () => {
    const feed = mutableSource();
    const { fleet, teardown } = createRoot((dispose) => {
      const store = createFleetStore(feed.source);
      store.start();
      return { fleet: store, teardown: () => { store.stop(); dispose(); } };
    });
    await fleet.refresh();
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("idle");
    expect(fleet.counts().running).toBe(0);

    feed.setSessions([agent({ status: "running", status_reason: "spinner on screen: Thinking" })]);
    await fleet.refresh();
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("running");
    expect(fleet.counts().running).toBe(1);

    feed.setSessions([agent({ status: "idle", status_reason: "turn finished on screen" })]);
    await fleet.refresh();
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("idle");
    expect(fleet.counts().running).toBe(0);
    teardown();
  });

  it("re-renders subscribers when only a session's status changed", async () => {
    const feed = mutableSource();
    const seen: string[] = [];
    const { fleet, teardown } = createRoot((dispose) => {
      const store = createFleetStore(feed.source);
      store.start();
      createEffect(() => {
        const first = store.lanes()[0];
        if (first) seen.push(laneIndicator(first).label);
      });
      return { fleet: store, teardown: () => { store.stop(); dispose(); } };
    });
    await fleet.refresh();
    await settle();
    feed.setSessions([agent({ status: "running" })]);
    await fleet.refresh();
    await settle();
    expect(seen[seen.length - 1]).toBe("running");
    teardown();
  });

  it("refreshes within the event debounce when the daemon announces a status change", async () => {
    const feed = mutableSource();
    const { fleet, teardown } = createRoot((dispose) => {
      const store = createFleetStore(feed.source);
      store.start();
      return { fleet: store, teardown: () => { store.stop(); dispose(); } };
    });
    await fleet.refresh();
    feed.setSessions([agent({ status: "running" })]);
    feed.emit("event.agent.status", { lane_id: 7, status: "running" });
    await new Promise((resolve) => setTimeout(resolve, 120));
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("running");
    teardown();
  });
});

describe("status propagation latency", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  /// The whole path the sidebar depends on, timed: the daemon reclassifies a pane, the store's
  /// heartbeat picks it up, and the derived pill flips. One heartbeat is the budget; anything
  /// slower is the store holding a status the daemon has already corrected.
  it("shows a daemon status change within one heartbeat with no event at all", async () => {
    vi.useFakeTimers();
    const feed = mutableSource();
    const { fleet, teardown } = createRoot((dispose) => {
      const store = createFleetStore(feed.source);
      store.start();
      return { fleet: store, teardown: () => { store.stop(); dispose(); } };
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("idle");

    feed.setSessions([agent({ status: "running", status_reason: "spinner on screen: Thinking" })]);
    // Nothing is pushed: only the 1200ms heartbeat can carry this.
    await vi.advanceTimersByTimeAsync(1250);
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("running");
    expect(fleet.counts().running).toBe(1);

    feed.setSessions([agent({ status: "idle", status_reason: "background file activity only, pane at rest" })]);
    await vi.advanceTimersByTimeAsync(1250);
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("idle");
    expect(fleet.counts().running).toBe(0);
    teardown();
  });

  /// With the daemon pushing `event.agent.status` the same flip lands inside the 60ms event
  /// debounce, well before the heartbeat would have carried it.
  it("shows a pushed status change inside the event debounce", async () => {
    vi.useFakeTimers();
    const feed = mutableSource();
    const { fleet, teardown } = createRoot((dispose) => {
      const store = createFleetStore(feed.source);
      store.start();
      return { fleet: store, teardown: () => { store.stop(); dispose(); } };
    });
    await vi.advanceTimersByTimeAsync(0);

    feed.setSessions([agent({ status: "running" })]);
    feed.emit("event.agent.status", {
      lane_id: 7,
      session: "s1",
      window: "lane-7-1",
      status: "running",
      reason: "spinner on screen: Thinking",
      previous: "idle",
    });
    await vi.advanceTimersByTimeAsync(100);
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("running");
    teardown();
  });
});

describe("multi-agent lanes", () => {
  /// The pill shows the most urgent state among a lane's agents, so one of five starting work has
  /// to flip it on its own. Each row is matched by its own `tmux_window`, which the daemon puts on
  /// every session, so a lane's slots never trade places across a poll.
  it("flips the pill when one of five agents starts running", async () => {
    const idle = (slot: number) =>
      agent({ session_id: `s${slot}`, tmux_window: `lane-7-${slot}`, status: "idle" });
    const feed = mutableSource();
    feed.setSessions([1, 2, 3, 4, 5].map(idle));
    const { fleet, teardown } = createRoot((dispose) => {
      const store = createFleetStore(feed.source);
      store.start();
      return { fleet: store, teardown: () => { store.stop(); dispose(); } };
    });
    await fleet.refresh();
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("idle");

    feed.setSessions([
      idle(1),
      idle(2),
      { ...idle(3), status: "running", status_reason: "spinner on screen: Thinking" },
      idle(4),
      idle(5),
    ]);
    await fleet.refresh();
    expect(laneIndicator(fleet.lanes()[0]).label).toBe("running");
    expect(fleet.counts().running).toBe(1);
    expect(fleet.counts().idle).toBe(4);
    // Every row keeps its own window: the pill's agent is identifiable, not just countable.
    expect(fleet.lanes()[0].agent_sessions.map((s) => s.tmux_window)).toEqual([
      "lane-7-1",
      "lane-7-2",
      "lane-7-3",
      "lane-7-4",
      "lane-7-5",
    ]);
    expect(
      fleet.lanes()[0].agent_sessions.find((s) => s.status === "running")?.tmux_window,
    ).toBe("lane-7-3");
    teardown();
  });
});
