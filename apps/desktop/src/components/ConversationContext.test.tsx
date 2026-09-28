import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane } from "../bindings";
import ConversationContext from "./ConversationContext";

vi.mock("../ipc/rpc", () => ({ daemonCall: () => Promise.resolve([]), subscribeDaemon: () => Promise.resolve(() => undefined) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

afterEach(() => cleanup());

function session(overrides: Partial<AgentSession>): AgentSession {
  return {
    id: 1,
    agent: "codex",
    repo_id: 1,
    worktree_id: 1,
    started_at: "2026-09-10T00:00:00Z",
    last_activity_at: "2026-09-10T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 0,
    title: null,
    status: "running",
    external: false,
    session_id: "s1",
    resume_at: null,
    inferred: false,
    tmux_window: "lane-1",
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
    status_reason: null,
    ...overrides,
  };
}

function lane(agentSessions: AgentSession[]): Lane {
  return {
    id: 1,
    pinned: false,
    role: null,
    last_activity_at: "2026-09-10T00:00:00Z",
    repo: { id: 1, name: "repomon", path: "/tmp/repo", added_at: "2026-08-01T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null, accent: null },
    worktree: { id: 1, repo_id: 1, name: "main", branch: "main", path: "/tmp/repo", head: "abc", is_main: true },
    state: { worktree_id: 1, head: "abc", branch: "main", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
    agent_sessions: agentSessions,
  };
}

describe("ConversationContext agent rows (round 6 item 6)", () => {
  it("makes a session with a tmux window a real focusable control that focuses its pane", () => {
    const onFocusAgent = vi.fn();
    const codex = session({ agent: "codex", tmux_window: "lane-1/2", status: "running", custom_label: "fix-auth" });
    render(() => <ConversationContext lane={lane([codex])} visible onFocusAgent={onFocusAgent} />);
    const row = screen.getByRole("button", { name: /Focus fix-auth's pane/ });
    expect(row.tagName).toBe("BUTTON");
    row.focus();
    expect(row).toHaveFocus();
    fireEvent.click(row);
    expect(onFocusAgent).toHaveBeenCalledWith("lane-1/2");
  });

  it("keeps a session with no tmux window inert - no button role, no click handler", () => {
    const onFocusAgent = vi.fn();
    const external = session({ agent: "aider", tmux_window: null, external: true, custom_label: "external-aider" });
    render(() => <ConversationContext lane={lane([external])} visible onFocusAgent={onFocusAgent} />);
    expect(screen.queryByRole("button", { name: /external-aider/ })).not.toBeInTheDocument();
    const row = screen.getByText("external-aider").closest(".context-agent")!;
    expect(row.tagName).toBe("DIV");
    fireEvent.click(row);
    expect(onFocusAgent).not.toHaveBeenCalled();
  });

  it("gives the row the same status dot tone the fleet sidebar uses for that agent state", () => {
    const running = session({ tmux_window: "lane-1/1", status: "running", custom_label: "running-agent" });
    const waiting = session({ id: 2, tmux_window: "lane-1/2", status: "waiting", pending_prompt: "Allow Bash?", custom_label: "waiting-agent" });
    const result = render(() => <ConversationContext lane={lane([running, waiting])} visible onFocusAgent={vi.fn()} />);
    const runningDot = result.container.querySelector('[aria-label="Focus running-agent\'s pane"] .context-agent-dot');
    const waitingDot = result.container.querySelector('[aria-label="Focus waiting-agent\'s pane"] .context-agent-dot');
    expect(runningDot).toHaveClass("bg-signal");
    expect(waitingDot).toHaveClass("bg-attention");
  });
});
