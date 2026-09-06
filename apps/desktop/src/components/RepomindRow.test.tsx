import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import { controllerSummary } from "../stores/fleet";
import RepomindRow, { RepomindRowMenu, RepomindStateDot } from "./RepomindRow";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const home: Repo = {
  id: 9,
  path: "/Users/pat/repomind",
  name: "repomind",
  added_at: "2026-09-04T00:00:00Z",
  worktree_root_template: null,
  hidden: false,
  position: null,
  label: null,
};

function session(overrides: Partial<AgentSession> = {}): AgentSession {
  return {
    id: 1,
    agent: "claude-code",
    repo_id: 9,
    worktree_id: 9,
    started_at: "2026-09-04T00:00:00Z",
    last_activity_at: "2026-09-04T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 0,
    title: null,
    status: "running",
    external: false,
    session_id: "c1",
    resume_at: null,
    inferred: false,
    tmux_window: "repomind-1",
    last_message: null,
    pending_prompt: null,
    pending_dialog: null,
    stale: false,
    stalled_since: null,
    subagent_running: null,
    gate: null,
    config_dir: null,
    custom_label: null,
    generated_label: null,
    ...overrides,
  };
}

function controllerLane(sessions: AgentSession[]): Lane {
  return {
    id: 90,
    repo: home,
    worktree: { id: 90, repo_id: 9, path: "/Users/pat/repomind", branch: "main", head: "abc", is_main: true, name: "main" },
    state: {
      worktree_id: 90,
      head: "abc",
      branch: "main",
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
    last_activity_at: "2026-09-04T00:00:00Z",
    pinned: false,
    role: "controller",
  };
}

describe("the Repomind toolbar state dot", () => {
  it("shows nothing at all when the home is off", () => {
    render(() => <RepomindStateDot controller={controllerSummary([controllerLane([])])} />);
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("shows a signal dot while a controller runs", () => {
    render(() => <RepomindStateDot controller={controllerSummary([controllerLane([session()])])} />);
    const dot = screen.getByRole("img", { name: "Repomind: running" });
    expect(dot.className).toContain("bg-signal");
  });

  it("switches to the attention color when a controller wants the operator", () => {
    const lane = controllerLane([session(), session({ id: 2, session_id: "c2", status: "waiting" })]);
    render(() => <RepomindStateDot controller={controllerSummary([lane])} />);
    const dot = screen.getByRole("img", { name: "Repomind: needs you" });
    expect(dot.className).toContain("bg-attention");
  });

  it("uses the fault color for a stalled controller rather than flattening it to attention", () => {
    const lane = controllerLane([session({ stale: true })]);
    render(() => <RepomindStateDot controller={controllerSummary([lane])} />);
    const dot = screen.getByRole("img", { name: "Repomind: stalled" });
    expect(dot.className).toContain("bg-fault");
  });
});


describe("the Repomind row menu", () => {
  it("opens from the keyboard without selecting or starting the controller", () => {
    const onContextMenu = vi.fn();
    const onSelect = vi.fn();
    render(() => <RepomindRow controller={controllerSummary([controllerLane([])])} activePlans={0}
      home="/Users/pat/repomind" selected={false} onSelect={onSelect} onContextMenu={onContextMenu} />);
    const row = screen.getByRole("button");
    row.focus();
    fireEvent.keyDown(row, { key: "F10", shiftKey: true });
    expect(onContextMenu).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(row, { key: "ContextMenu" });
    expect(onContextMenu).toHaveBeenCalledTimes(2);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("focuses its actions, walks them with arrows, and returns focus on Escape", () => {
    const trigger = document.createElement("button");
    document.body.append(trigger);
    trigger.focus();
    const onClose = vi.fn();
    render(() => <RepomindRowMenu running={false} x={100} y={100} onAction={vi.fn()} onClose={onClose} />);
    const items = screen.getAllByRole("menuitem");
    expect(items[0]).toHaveFocus();
    fireEvent.keyDown(items[0], { key: "ArrowUp" });
    expect(items[2]).toHaveFocus();
    fireEvent.keyDown(items[2], { key: "ArrowDown" });
    expect(items[0]).toHaveFocus();
    fireEvent.keyDown(items[0], { key: "End" });
    expect(items[2]).toHaveFocus();
    fireEvent.keyDown(items[2], { key: "Home" });
    expect(items[0]).toHaveFocus();
    fireEvent.keyDown(items[0], { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(trigger).toHaveFocus();
    trigger.remove();
  });

  it("keeps the context menu inside the right viewport edge", () => {
    vi.stubGlobal("innerWidth", 1040);
    render(() => <RepomindRowMenu running={false} x={1038} y={100} onAction={vi.fn()} onClose={vi.fn()} />);
    const menu = screen.getByRole("menu", { name: "Repomind" });
    expect(Number.parseFloat(menu.style.left) + 224).toBeLessThanOrEqual(1032);
  });
});
