import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SystemDoctorResult } from "../bindings";
import type { ConfigView } from "../ipc/rpc";
import SettingsModal from "./SettingsModal";

const calls = vi.hoisted(() => ({ saved: [] as ConfigView[] }));
const state = vi.hoisted(() => ({
  config: null as ConfigView | null,
  doctor: null as SystemDoctorResult | null,
  doctorCalls: 0,
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params?: ConfigView) => {
    if (method === "config.get") return Promise.resolve({ ...state.config });
    if (method === "agent.detect") return Promise.resolve([]);
    if (method === "system.doctor") {
      state.doctorCalls++;
      if (state.doctor) return Promise.resolve({ ...state.doctor });
      return Promise.resolve({
        tmux: { available: true, version: "tmux 3.4", source: "system", path: "/opt/homebrew/bin/tmux" },
        git: { available: true, version: "git version 2.44.0", path: "/usr/bin/git" },
        agents: [
          { kind: "claude-code", name: "Claude Code", command: "claude", detected: true },
          { kind: "cursor", name: "Cursor Agent", command: "cursor-agent", detected: false },
        ],
      });
    }
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

  it("renders theme presets and allows accent color selection in appearance tab", async () => {
    state.config = { ...config, accent: "cyan" };
    render(() => (
      <SettingsModal
        initialTab="appearance"
        onClose={() => undefined}
      />
    ));

    await screen.findByText("Color Themes & Presets");
    expect(screen.getByText("Midnight OLED")).toBeInTheDocument();
    expect(screen.getByText("Nord Arctic")).toBeInTheDocument();
    expect(screen.getByText("Dracula")).toBeInTheDocument();
    expect(screen.getByText("Warm Paper")).toBeInTheDocument();
    expect(screen.getByText("Modern Light")).toBeInTheDocument();

    // Click Nord Arctic theme
    const nordButton = screen.getByRole("button", { name: /Nord Arctic/i });
    fireEvent.click(nordButton);

    // Select an accent color swatch
    const emeraldButton = screen.getByRole("button", { name: /Emerald/i });
    fireEvent.click(emeraldButton);

    await waitFor(() => expect(calls.saved.length).toBeGreaterThan(0));
    expect(calls.saved[calls.saved.length - 1].accent).toBe("green");
  });

  it("renders and toggles auto-collapse empty lanes switch in general tab", async () => {
    render(() => (
      <SettingsModal
        initialTab="general"
        onClose={() => undefined}
      />
    ));

    const toggle = await screen.findByRole("switch", { name: "Auto-collapse lanes with no active agent" });
    expect(toggle).toBeInTheDocument();
    expect(toggle).toHaveAttribute("aria-checked", "true");

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(localStorage.getItem("repomon:auto-collapse-empty-lanes")).toBe("false");
  });
});

describe("System Health tab", () => {
  it("renders healthy system state with system tmux, git, and detected agents", async () => {
    state.config = { ...config };
    state.doctor = {
      tmux: { available: true, version: "tmux 3.4", source: "system", path: "/opt/homebrew/bin/tmux" },
      git: { available: true, version: "git version 2.44.0", path: "/usr/bin/git" },
      agents: [
        { kind: "claude-code", name: "Claude Code", command: "claude", detected: true },
        { kind: "cursor", name: "Cursor Agent", command: "cursor-agent", detected: false },
      ],
    };

    render(() => (
      <SettingsModal
        initialTab="system"
        onClose={() => undefined}
      />
    ));

    await screen.findByText("System Health");
    expect(screen.getByText("Core Runtime Dependencies")).toBeInTheDocument();
    expect(screen.getByText("/opt/homebrew/bin/tmux")).toBeInTheDocument();
    expect(screen.getByText("System PATH")).toBeInTheDocument();
    expect(screen.getByText("tmux 3.4")).toBeInTheDocument();
    expect(screen.getByText("/usr/bin/git")).toBeInTheDocument();
    expect(screen.getByText("git version 2.44.0")).toBeInTheDocument();
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText("✓ Detected")).toBeInTheDocument();
    expect(screen.getByText("Cursor Agent")).toBeInTheDocument();
    expect(screen.getByText("Not Found")).toBeInTheDocument();
  });

  it("renders reassuring badge when using bundled tmux", async () => {
    state.config = { ...config };
    state.doctor = {
      tmux: {
        available: true,
        version: "tmux 3.4",
        source: "bundled",
        path: "/Applications/Repomon.app/Contents/MacOS/tmux",
      },
      git: { available: true, version: "git version 2.44.0", path: "/usr/bin/git" },
      agents: [],
    };

    render(() => (
      <SettingsModal
        initialTab="system"
        onClose={() => undefined}
      />
    ));

    await screen.findByText("Repomon Built-in");
    expect(
      screen.getByText(/Using Repomon's built-in standalone tmux/i),
    ).toBeInTheDocument();
  });

  it("renders missing dependencies with actionable copy and copy buttons", async () => {
    state.config = { ...config };
    state.doctor = {
      tmux: { available: false, version: null, source: null, path: null },
      git: { available: false, version: null, path: null },
      agents: [
        { kind: "claude-code", name: "Claude Code", command: "claude", detected: false },
        { kind: "antigravity", name: "Antigravity", command: "agy", detected: false },
        { kind: "cursor", name: "Cursor Agent", command: "cursor-agent", detected: false },
        { kind: "opencode", name: "OpenCode", command: "opencode", detected: false },
      ],
    };

    // Mock navigator.clipboard
    const writeTextMock = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: writeTextMock },
      configurable: true,
    });

    render(() => (
      <SettingsModal
        initialTab="system"
        onClose={() => undefined}
      />
    ));

    await screen.findByText("System Health");
    const missingBadges = screen.getAllByText("Missing");
    expect(missingBadges.length).toBe(2);

    // Check tmux copy button
    const copyTmuxButton = screen.getByRole("button", { name: "Copy tmux install command" });
    expect(copyTmuxButton).toBeInTheDocument();
    fireEvent.click(copyTmuxButton);
    expect(writeTextMock).toHaveBeenCalledWith(expect.stringContaining("tmux"));

    // Check git copy button
    const copyGitButton = screen.getByRole("button", { name: "Copy git install command" });
    expect(copyGitButton).toBeInTheDocument();
    fireEvent.click(copyGitButton);
    expect(writeTextMock).toHaveBeenCalledWith(expect.stringContaining("git"));

    // Check Claude Code copy button
    const copyClaudeButton = screen.getByRole("button", { name: /Copy install command for Claude Code/i });
    expect(copyClaudeButton).toBeInTheDocument();
    fireEvent.click(copyClaudeButton);
    expect(writeTextMock).toHaveBeenCalledWith("npm install -g @anthropic-ai/claude-code");

    // Check Antigravity copy button
    const copyAgyButton = screen.getByRole("button", { name: /Copy install command for Antigravity/i });
    expect(copyAgyButton).toBeInTheDocument();
    fireEvent.click(copyAgyButton);
    expect(writeTextMock).toHaveBeenCalledWith("curl -fsSL https://antigravity.google/cli/install.sh | bash");

    // Check Cursor copy button
    const copyCursorButton = screen.getByRole("button", { name: /Copy install command for Cursor Agent/i });
    expect(copyCursorButton).toBeInTheDocument();
    fireEvent.click(copyCursorButton);
    expect(writeTextMock).toHaveBeenCalledWith("curl https://cursor.com/install -fsS | bash");

    // Check OpenCode copy button
    const copyOpenCodeButton = screen.getByRole("button", { name: /Copy install command for OpenCode/i });
    expect(copyOpenCodeButton).toBeInTheDocument();
    fireEvent.click(copyOpenCodeButton);
    expect(writeTextMock).toHaveBeenCalledWith("npm install -g opencode-ai");
  });

  it("re-runs system.doctor when refresh button is clicked", async () => {
    state.config = { ...config };
    state.doctorCalls = 0;
    state.doctor = {
      tmux: { available: true, version: "tmux 3.4", source: "system", path: "/opt/homebrew/bin/tmux" },
      git: { available: true, version: "git version 2.44.0", path: "/usr/bin/git" },
      agents: [],
    };

    render(() => (
      <SettingsModal
        initialTab="system"
        onClose={() => undefined}
      />
    ));

    await screen.findByText("System Health");
    expect(state.doctorCalls).toBe(1);

    const refreshButton = screen.getByRole("button", { name: "Refresh system health status" });
    fireEvent.click(refreshButton);
    await waitFor(() => expect(state.doctorCalls).toBe(2));
  });
});
