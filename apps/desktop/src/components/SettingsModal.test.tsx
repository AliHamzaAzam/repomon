import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SystemDoctorResult } from "../bindings";
import type { ConfigView } from "../ipc/rpc";
import SettingsModal from "./SettingsModal";

const calls = vi.hoisted(() => ({
  saved: [] as ConfigView[],
  addedAgents: [] as Array<{ name: string; command: string }>,
  removedAgents: [] as string[],
  defaultAgents: [] as Array<string | null>,
}));
const state = vi.hoisted(() => ({
  config: null as ConfigView | null,
  doctor: null as SystemDoctorResult | null,
  doctorCalls: 0,
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params?: any) => {
    if (method === "config.get") return Promise.resolve({ ...state.config });
    if (method === "agent.detect") {
      const list = [
        { kind: "claude-code", name: "claude-code", command: "claude", detected: true, custom: false, default: false },
        { kind: "cursor", name: "cursor", command: "cursor-agent", detected: false, custom: false, default: false },
        { kind: "aider", name: "aider", command: "aider", detected: true, custom: false, default: false },
        { kind: "codex", name: "codex", command: "codex", detected: true, custom: false, default: false },
        { kind: "antigravity", name: "antigravity", command: "agy", detected: true, custom: false, default: false },
        { kind: "opencode", name: "opencode", command: "opencode", detected: true, custom: false, default: false },
      ];
      if (state.config?.agents) {
        for (const [name, cmd] of Object.entries(state.config.agents)) {
          list.push({
            kind: name,
            name,
            command: cmd,
            detected: true,
            custom: true,
            default: state.config.default_agent === name,
          });
        }
      }
      return Promise.resolve(list);
    }
    if (method === "agent.add") {
      if (params.name === "claude-code") {
        return Promise.reject(new Error("'claude-code' is a built-in agent name; pick a different name"));
      }
      calls.addedAgents.push(params);
      if (state.config) {
        state.config = {
          ...state.config,
          agents: { ...(state.config.agents ?? {}), [params.name]: params.command },
        };
      }
      return Promise.resolve(null);
    }
    if (method === "agent.remove") {
      calls.removedAgents.push(params.name);
      if (state.config && state.config.agents) {
        const nextAgents: Record<string, string> = { ...state.config.agents };
        delete nextAgents[params.name];
        state.config = { ...state.config, agents: nextAgents };
      }
      return Promise.resolve(null);
    }
    if (method === "agent.set_default") {
      calls.defaultAgents.push(params.name ?? null);
      if (state.config) {
        state.config = { ...state.config, default_agent: params.name ?? null };
      }
      return Promise.resolve(null);
    }
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
  supervision: {
    enabled: false,
    nudge_text: "Repomon: checking in on this lane.",
    mail_mode: "nudge",
    stall_mins: 15,
    nudge_retries: 2,
    classes: {},
  },
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
    await screen.findByText("Built-in Runtime Icons & Overrides");
    const changeCodexButton = screen.getByRole("button", { name: "Change icon for codex" });
    expect(changeCodexButton).toBeInTheDocument();

    // Open icon picker for codex
    fireEvent.click(changeCodexButton);

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

  it("triggers replay onboarding from General settings tab", async () => {
    state.config = { ...config };
    const onClose = vi.fn();
    const onReplayOnboarding = vi.fn();

    render(() => (
      <SettingsModal
        initialTab="general"
        onClose={onClose}
        onReplayOnboarding={onReplayOnboarding}
      />
    ));

    await screen.findByText("General Configuration");
    const replayBtn = screen.getByRole("button", { name: "Replay Onboarding" });
    expect(replayBtn).toBeInTheDocument();
    fireEvent.click(replayBtn);

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onReplayOnboarding).toHaveBeenCalledTimes(1);
  });

  describe("Custom Agents in Settings", () => {
    it("renders custom agents list and allows registering a new custom agent", async () => {
      state.config = {
        ...config,
        agents: {
          "custom-runner": "python -m runner",
        },
      };
      calls.addedAgents = [];

      render(() => (
        <SettingsModal
          initialTab="agents"
          onClose={() => undefined}
        />
      ));

      await screen.findByText("Custom Agent Registrations");
      expect(screen.getByText("custom-runner")).toBeInTheDocument();
      expect(screen.getByText("python -m runner")).toBeInTheDocument();

      // Fill in new custom agent form
      const nameInput = screen.getByPlaceholderText("e.g. 'devin', 'gemini-cli', 'deepseek'");
      const cmdInput = screen.getByPlaceholderText("e.g. 'gemini --repomon', 'python run.py'");
      const registerButton = screen.getByRole("button", { name: "Register Agent" });

      fireEvent.input(nameInput, { target: { value: "devin" } });
      fireEvent.input(cmdInput, { target: { value: "devin-cli run" } });
      expect(registerButton).not.toBeDisabled();

      fireEvent.click(registerButton);

      await waitFor(() => {
        expect(calls.addedAgents).toContainEqual({
          name: "devin",
          command: "devin-cli run",
        });
      });

      await screen.findByText("Registered custom agent 'devin'");
      expect(screen.getByText("devin")).toBeInTheDocument();
    });

    it("displays friendly validation error when trying to add a built-in agent name", async () => {
      state.config = { ...config, agents: {} };
      calls.addedAgents = [];

      render(() => (
        <SettingsModal
          initialTab="agents"
          onClose={() => undefined}
        />
      ));

      await screen.findByText("Custom Agent Registrations");
      const nameInput = screen.getByPlaceholderText("e.g. 'devin', 'gemini-cli', 'deepseek'");
      const cmdInput = screen.getByPlaceholderText("e.g. 'gemini --repomon', 'python run.py'");
      const registerButton = screen.getByRole("button", { name: "Register Agent" });

      fireEvent.input(nameInput, { target: { value: "claude-code" } });
      fireEvent.input(cmdInput, { target: { value: "claude --danger" } });
      fireEvent.click(registerButton);

      const alert = await screen.findByRole("alert");
      expect(alert).toHaveTextContent("'claude-code' is a built-in agent name; pick a different name");
      expect(calls.addedAgents).toHaveLength(0);
    });

    it("sets a custom agent as default with agent.set_default RPC", async () => {
      state.config = {
        ...config,
        agents: {
          "custom-runner": "python -m runner",
        },
        default_agent: null,
      };
      calls.defaultAgents = [];

      render(() => (
        <SettingsModal
          initialTab="agents"
          onClose={() => undefined}
        />
      ));

      await screen.findByText("Custom Agent Registrations");
      const setDefaultButton = screen.getByRole("button", { name: "Set custom-runner as default agent" });
      fireEvent.click(setDefaultButton);

      await waitFor(() => {
        expect(calls.defaultAgents).toContain("custom-runner");
      });
    });

    it("removes a custom agent with confirmation and agent.remove RPC", async () => {
      state.config = {
        ...config,
        agents: {
          "to-delete": "echo 'delete me'",
        },
      };
      calls.removedAgents = [];

      const mockConfirm = vi.fn((opts: { onConfirm: () => void }) => {
        opts.onConfirm();
      });

      render(() => (
        <SettingsModal
          initialTab="agents"
          onClose={() => undefined}
          actions={{ confirm: mockConfirm } as any}
        />
      ));

      await screen.findByText("Custom Agent Registrations");
      expect(screen.getByText("to-delete")).toBeInTheDocument();

      const removeButton = screen.getByRole("button", { name: "Remove agent to-delete" });
      fireEvent.click(removeButton);

      expect(mockConfirm).toHaveBeenCalledTimes(1);
      expect(mockConfirm).toHaveBeenCalledWith(expect.objectContaining({
        title: "Remove agent 'to-delete'?",
        danger: true,
      }));

      await waitFor(() => {
        expect(calls.removedAgents).toContain("to-delete");
      });
    });
  });
});
