import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentChoice, Lane } from "../bindings";
import { resetAgentChoicesCacheForTests } from "../stores/agentChoices";
import SpawnModal from "./SpawnModal";

const EIGHT: AgentChoice[] = [
  { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
  { name: "claude-work", command: "claude-work", detected: true, default: false, custom: false },
  { name: "codex", command: "codex", detected: true, default: false, custom: false },
  { name: "hermes", command: "hermes", detected: true, default: false, custom: false },
  { name: "opencode", command: "opencode", detected: true, default: false, custom: false },
  { name: "antigravity", command: "antigravity", detected: true, default: false, custom: false },
  { name: "aider", command: "aider", detected: false, default: false, custom: false },
  { name: "cursor", command: "cursor-agent", detected: false, default: false, custom: false },
];

const state = vi.hoisted(() => ({
  agents: [] as AgentChoice[],
  spawnCalls: [] as Array<{ lane_id: number; agent: string; task?: string }>,
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params: unknown) => {
    if (method === "agent.detect") return Promise.resolve(state.agents);
    if (method === "agent.spawn") {
      state.spawnCalls.push(params as { lane_id: number; agent: string; task?: string });
      return Promise.resolve({ spawn_warnings: [] });
    }
    return Promise.resolve(null);
  },
  subscribeDaemon: () => Promise.resolve(() => undefined),
}));

const lane: Lane = {
  id: 1,
  pinned: false,
  role: null,
  last_activity_at: "2026-09-01T00:00:00Z",
  repo: { id: 1, name: "repomon", path: "/tmp/repo", added_at: "2026-09-01T00:00:00Z", worktree_root_template: null, hidden: false, position: null, label: null, accent: null },
  worktree: { id: 1, repo_id: 1, name: "main", branch: "main", path: "/tmp/repo", head: "abc", is_main: true },
  state: { worktree_id: 1, head: "abc", branch: "main", upstream: null, ahead: 0, behind: 0, dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, locked: false, prunable: false, last_change_at: null },
  agent_sessions: [],
};

afterEach(() => {
  cleanup();
  resetAgentChoicesCacheForTests();
  state.agents = EIGHT;
  state.spawnCalls = [];
  document.body.innerHTML = "";
});

state.agents = EIGHT;

/// Renders the dialog and waits for the grid to paint and claim focus, which is where every
/// keyboard journey starts.
async function openDialog(overrides: Partial<Parameters<typeof SpawnModal>[0]> = {}) {
  const onClose = overrides.onClose ?? vi.fn();
  const onDone = overrides.onDone ?? vi.fn().mockResolvedValue(undefined);
  const result = render(() => (
    <SpawnModal lane={lane} onClose={onClose} onDone={onDone} onOpenSettingsTab={overrides.onOpenSettingsTab} />
  ));
  const radios = await screen.findAllByRole("radio");
  await waitFor(() => expect(document.activeElement).toBe(radios[0]));
  return { ...result, radios, onClose, onDone };
}

describe("spawn dialog keyboard operation", () => {
  it("focuses the preselected runtime as soon as the grid paints", async () => {
    const { radios } = await openDialog();
    expect(radios[0]).toHaveAttribute("aria-label", "claude-code, default runtime");
    expect(radios[0]).toHaveAttribute("aria-checked", "true");
    expect(document.activeElement).toBe(radios[0]);
  });

  it("moves across columns with Left and Right and across rows with Up and Down", async () => {
    const { radios } = await openDialog();
    fireEvent.keyDown(radios[0], { key: "ArrowRight" });
    expect(document.activeElement).toBe(radios[1]);
    fireEvent.keyDown(radios[1], { key: "ArrowDown" });
    expect(document.activeElement).toBe(radios[3]);
    fireEvent.keyDown(radios[3], { key: "ArrowLeft" });
    expect(document.activeElement).toBe(radios[2]);
    fireEvent.keyDown(radios[2], { key: "ArrowUp" });
    expect(document.activeElement).toBe(radios[0]);
  });

  it("wraps the runtime grid at both ends and answers Home and End", async () => {
    const { radios } = await openDialog();
    fireEvent.keyDown(radios[0], { key: "ArrowLeft" });
    expect(document.activeElement).toBe(radios[7]);
    fireEvent.keyDown(radios[7], { key: "ArrowDown" });
    expect(document.activeElement).toBe(radios[1]);
    fireEvent.keyDown(radios[1], { key: "End" });
    expect(document.activeElement).toBe(radios[7]);
    fireEvent.keyDown(radios[7], { key: "Home" });
    expect(document.activeElement).toBe(radios[0]);
  });

  it("selects a runtime directly by the digit printed on its tile", async () => {
    const { radios } = await openDialog();
    expect(screen.getByText("Arrows move, 1 to 8 picks a runtime, Enter spawns.")).toBeInTheDocument();
    fireEvent.keyDown(radios[0], { key: "3" });
    expect(document.activeElement).toBe(radios[2]);
    expect(radios[2]).toHaveAttribute("aria-checked", "true");
    expect(radios[0]).toHaveAttribute("aria-checked", "false");
    expect(state.spawnCalls).toEqual([]);
  });

  it("spawns with Enter from the runtime grid", async () => {
    const { radios } = await openDialog();
    fireEvent.keyDown(radios[0], { key: "3" });
    fireEvent.keyDown(radios[2], { key: "Enter" });
    await waitFor(() => expect(state.spawnCalls).toHaveLength(1));
    expect(state.spawnCalls[0]).toMatchObject({ lane_id: 1, agent: "codex" });
  });

  it("spawns from the task description only with the platform chord", async () => {
    await openDialog();
    const textarea = screen.getByPlaceholderText("Describe what this agent should start on…");
    fireEvent.input(textarea, { target: { value: "ship the meter" } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(state.spawnCalls).toEqual([]);
    fireEvent.keyDown(textarea, { key: "Enter", metaKey: true });
    await waitFor(() => expect(state.spawnCalls).toHaveLength(1));
    expect(state.spawnCalls[0]).toMatchObject({ agent: "claude-code", task: "ship the meter" });
  });

  it("cancels with Escape and returns focus to whatever opened the dialog", async () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    opener.focus();
    const { onClose, unmount } = await openDialog();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    unmount();
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });

  it("keeps Tab inside the dialog at both ends", async () => {
    await openDialog();
    const close = screen.getByRole("button", { name: "Close Spawn agent" });
    const spawn = screen.getByRole("button", { name: "Spawn Agent" });
    spawn.focus();
    fireEvent.keyDown(window, { key: "Tab" });
    expect(document.activeElement).toBe(close);
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(spawn);
  });

  it("lets a missing runtime take focus but never the selection", async () => {
    const onOpenSettingsTab = vi.fn();
    const { radios } = await openDialog({ onOpenSettingsTab });
    const aider = radios[6];
    fireEvent.keyDown(radios[0], { key: "7" });
    expect(document.activeElement).toBe(aider);
    expect(aider).toHaveAttribute("aria-disabled", "true");
    expect(aider).toHaveAttribute("aria-checked", "false");
    // Selection follows focus, so standing on a runtime that cannot be spawned leaves the dialog
    // with nothing selected rather than a mark stranded on the runtime behind you.
    expect(radios.filter((tile) => tile.getAttribute("aria-checked") === "true")).toEqual([]);
    expect(screen.getByText("aider is not installed. Enter opens its install instructions.")).toBeInTheDocument();
    fireEvent.keyDown(aider, { key: " " });
    expect(aider).toHaveAttribute("aria-checked", "false");
    expect(onOpenSettingsTab).toHaveBeenCalledWith("system");
    expect(state.spawnCalls).toEqual([]);
  });

  it("refuses to spawn a runtime that is not installed", async () => {
    const onOpenSettingsTab = vi.fn();
    const { radios } = await openDialog({ onOpenSettingsTab });
    fireEvent.keyDown(radios[7], { key: "Enter" });
    expect(state.spawnCalls).toEqual([]);
    expect(onOpenSettingsTab).toHaveBeenCalledWith("system");
  });
});

/// Selection and focus are one state, so the grid carries exactly one mark and it is always the
/// runtime Enter spawns. The two can no longer name different runtimes, because there is only one
/// of them; the only way to see no mark is to stand on a runtime that cannot be spawned at all.
describe("spawn dialog selection follows focus", () => {
  // Anchored to whole class names, so a `hover:` variant of the same ground never counts as
  // the selected state.
  const SELECTION_CLASSES = /(^|\s)(bg-raised|border-muted)(\s|$)/;

  it("moves the mark with the arrows", async () => {
    const { container, radios } = await openDialog();
    expect(radios[0]).toHaveAttribute("data-selected", "");

    fireEvent.keyDown(radios[0], { key: "ArrowRight" });
    expect(document.activeElement).toBe(radios[1]);
    expect(radios[1]).toHaveAttribute("aria-checked", "true");
    expect(radios[0]).toHaveAttribute("aria-checked", "false");
    expect(radios[1].className).toMatch(/(^|\s)bg-raised(\s|$)/);
    expect(radios[1].className).toMatch(/(^|\s)border-muted(\s|$)/);
    expect(radios[0].className).not.toMatch(SELECTION_CLASSES);

    fireEvent.keyDown(radios[1], { key: "ArrowDown" });
    expect(document.activeElement).toBe(radios[3]);
    expect(radios[3]).toHaveAttribute("data-selected", "");
    // Never two marks to choose between, at any point in the journey.
    expect(container.querySelectorAll("[data-selected]")).toHaveLength(1);
    expect(state.spawnCalls).toEqual([]);
  });

  it("spawns the runtime that carries the mark", async () => {
    const { radios } = await openDialog();
    fireEvent.keyDown(radios[0], { key: "ArrowDown" });
    expect(radios[2]).toHaveAttribute("data-selected", "");
    expect(radios[0]).not.toHaveAttribute("data-selected");
    fireEvent.keyDown(radios[2], { key: "Enter" });
    await waitFor(() => expect(state.spawnCalls).toHaveLength(1));
    expect(state.spawnCalls[0]).toMatchObject({ agent: "codex" });
  });

  /// The operator's screenshot had the check on claude-code and the button about to spawn hermes.
  /// Mark hermes and the button must agree with it.
  it("spawns the marked runtime from the Spawn Agent button too", async () => {
    const { radios } = await openDialog();
    fireEvent.keyDown(radios[0], { key: "4" });
    expect(radios[3]).toHaveAttribute("data-selected", "");
    fireEvent.click(screen.getByRole("button", { name: "Spawn Agent" }));
    await waitFor(() => expect(state.spawnCalls).toHaveLength(1));
    expect(state.spawnCalls[0]).toMatchObject({ agent: "hermes" });
  });

  it("drops the mark rather than put it on a runtime that cannot be spawned", async () => {
    const { container, radios } = await openDialog({ onOpenSettingsTab: vi.fn() });
    fireEvent.keyDown(radios[0], { key: "7" });
    expect(document.activeElement).toBe(radios[6]);
    expect(radios[6]).toHaveAttribute("aria-checked", "false");
    expect(radios[6].className).toMatch(/\bborder-dashed\b/);
    expect(radios[6].className).not.toMatch(SELECTION_CLASSES);
    // The exception to selection-follows-focus, and it costs no mark anywhere: the dialog holds
    // no selection while the caret is somewhere Enter cannot spawn, and says so with the button.
    expect(container.querySelectorAll("[data-selected]")).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Spawn Agent" })).toBeDisabled();
    fireEvent.keyDown(radios[6], { key: "Enter" });
    expect(state.spawnCalls).toEqual([]);

    // Nothing was lost by looking: leaving the missing tile restores a selection immediately.
    fireEvent.keyDown(radios[6], { key: "ArrowUp" });
    expect(document.activeElement).toBe(radios[4]);
    expect(radios[4]).toHaveAttribute("data-selected", "");
    expect(container.querySelectorAll("[data-selected]")).toHaveLength(1);
  });

  it("keeps the mark after focus leaves the grid", async () => {
    const { container, radios } = await openDialog();
    fireEvent.keyDown(radios[0], { key: "5" });
    const textarea = screen.getByPlaceholderText("Describe what this agent should start on…");
    textarea.focus();
    expect(document.activeElement).toBe(textarea);
    // The mark is state, not the browser's focus ring, so it survives the grid losing focus and
    // still names what the chord will spawn.
    expect(radios[4]).toHaveAttribute("data-selected", "");
    expect(container.querySelectorAll("[data-selected]")).toHaveLength(1);
    fireEvent.keyDown(textarea, { key: "Enter", metaKey: true });
    await waitFor(() => expect(state.spawnCalls).toHaveLength(1));
    expect(state.spawnCalls[0]).toMatchObject({ agent: "opencode" });
  });
});
