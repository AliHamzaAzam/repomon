import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ConfigView } from "../ipc/rpc";
import SettingsModal from "./SettingsModal";

const calls = vi.hoisted(() => ({ saved: [] as ConfigView[] }));
const state = vi.hoisted(() => ({ config: null as ConfigView | null }));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params?: ConfigView) => {
    if (method === "config.get") return Promise.resolve({ ...state.config });
    if (method === "agent.detect") return Promise.resolve([]);
    if (method === "config.set" && params) {
      calls.saved.push(params);
      state.config = { ...params };
      return Promise.resolve({ ...params });
    }
    return Promise.reject(new Error(`unexpected RPC ${method}`));
  },
}));

const config: ConfigView = {
  worktree_template: "../{repo}-{branch}",
  auto_continue: true,
  auto_continue_message: "continue",
  spawn_prompt: true,
  notify_enabled: true,
  notify_needs_you: true,
  notify_rate_limited: true,
  notify_resumed: true,
  notify_idle: true,
  notify_sound: true,
  notify_sound_volume: 0.25,
  notify_sound_unfocused_only: true,
  notify_sound_agent_needs_you: true,
  notify_sound_agent_finished: true,
  notify_sound_repomind_needs_you: true,
  notify_sound_error_or_stall: true,
  notify_sound_incoming_message: true,
  notify_sound_update_ready: true,
  notify_show_why: true,
  notify_coalesce: true,
  notify_click_focus: true,
  notify_desktop_fallback: true,
  notify_subagents: true,
  usage_probe: true,
  expand_agents: true,
  sort_repos_by_activity: true,
  embedded_pty: true,
};

afterEach(() => {
  cleanup();
  calls.saved = [];
  state.config = { ...config };
});

describe("Settings auto-save persistence", () => {
  it("automatically saves sound preferences immediately on change", async () => {
    state.config = { ...config };
    render(() => (
      <SettingsModal
        initialTab="notifications"
        onClose={() => undefined}
      />
    ));

    const focusOnly = await screen.findByRole("switch", { name: "Only while unfocused" });
    fireEvent.click(focusOnly);
    await waitFor(() => expect(calls.saved.length).toBe(1));

    fireEvent.click(screen.getByRole("switch", { name: "Agent needs you" }));
    await waitFor(() => expect(calls.saved.length).toBe(2));

    fireEvent.change(screen.getByRole("slider"), { target: { value: "0.6" } });
    await waitFor(() => expect(calls.saved.length).toBe(3));

    await screen.findByText("Saved");
    expect(calls.saved[calls.saved.length - 1]).toMatchObject({
      notify_sound: true,
      notify_sound_volume: 0.6,
      notify_sound_unfocused_only: false,
      notify_sound_agent_needs_you: false,
      notify_sound_agent_finished: true,
      notify_sound_repomind_needs_you: true,
      notify_sound_error_or_stall: true,
      notify_sound_incoming_message: true,
      notify_sound_update_ready: true,
    });
    await waitFor(() => expect(focusOnly).toHaveAttribute("aria-checked", "false"));
  });

  it("automatically saves custom agent icon overrides immediately on selection", async () => {
    state.config = { ...config, agent_icons: {} };
    render(() => (
      <SettingsModal
        initialTab="agents"
        onClose={() => undefined}
      />
    ));

    // Find the change icon button for codex
    await screen.findByText("Agent Icon Library & Overrides");
    const changeButtons = screen.getAllByRole("button", { name: "Change Icon…" });
    expect(changeButtons.length).toBeGreaterThan(0);

    // Open icon picker for the fourth agent (codex)
    fireEvent.click(changeButtons[3]);

    // Icon picker modal should open
    await screen.findByText("Icon for");
    const lightningButton = screen.getByRole("button", { name: /Lightning Bolt/i });
    fireEvent.click(lightningButton);

    await waitFor(() => expect(calls.saved.length).toBeGreaterThan(0));
    await screen.findByText("Saved");

    expect(calls.saved[calls.saved.length - 1].agent_icons).toMatchObject({
      codex: "bolt",
    });
  });
});
