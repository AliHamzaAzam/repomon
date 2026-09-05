import { cleanup, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SystemDoctorResult } from "../bindings";
import SystemHealthView, { getAgentInstallInfo, getSystemInstallCommand } from "./SystemHealthView";

const daemonResult = vi.hoisted(() => ({ current: null as SystemDoctorResult | null }));

vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string) => {
    if (method === "system.doctor") return Promise.resolve(daemonResult.current);
    return Promise.resolve({});
  },
}));

afterEach(() => {
  cleanup();
});

function macDoctor(overrides: Partial<SystemDoctorResult> = {}): SystemDoctorResult {
  return {
    platform: "macos",
    tmux: {
      available: true,
      version: "tmux 3.4",
      source: "system",
      path: "/opt/homebrew/bin/tmux",
      not_applicable: false,
    },
    git: { available: true, version: "git version 2.44.0", path: "/usr/bin/git" },
    agent_host: null,
    agents: [],
    ...overrides,
  };
}

function windowsDoctor(overrides: Partial<SystemDoctorResult> = {}): SystemDoctorResult {
  return {
    platform: "windows",
    tmux: { available: false, version: null, source: null, path: null, not_applicable: true },
    git: {
      available: true,
      version: "git version 2.44.0.windows.1",
      path: "C:\\Program Files\\Git\\cmd\\git.exe",
    },
    agent_host: {
      available: true,
      version: null,
      path: "C:\\Program Files\\Repomon\\repomon-agent-host.exe",
      source: "bundled",
    },
    agents: [],
    ...overrides,
  };
}

describe("getSystemInstallCommand", () => {
  it("uses brew on mac, apt on linux, and winget on windows", () => {
    expect(getSystemInstallCommand("git", "mac")).toBe("brew install git");
    expect(getSystemInstallCommand("tmux", "mac")).toBe("brew install tmux");
    expect(getSystemInstallCommand("git", "other")).toBe("sudo apt install git");
    expect(getSystemInstallCommand("git", "windows")).toBe("winget install --id Git.Git -e");
    expect(getSystemInstallCommand("tmux", "windows")).toBe("winget install tmux");
  });
});

describe("getAgentInstallInfo", () => {
  it("installs npm-based agents the same way everywhere, Windows included", () => {
    expect(getAgentInstallInfo("claude-code", "claude", "windows")?.command).toBe(
      "npm install -g @anthropic-ai/claude-code",
    );
    expect(getAgentInstallInfo("claude-code", "claude", "mac")?.command).toBe(
      "npm install -g @anthropic-ai/claude-code",
    );
  });

  it("swaps the curl-pipe-bash Antigravity installer for the PowerShell one on Windows", () => {
    expect(getAgentInstallInfo("antigravity", "agy", "mac")?.command).toContain("curl");
    const windows = getAgentInstallInfo("antigravity", "agy", "windows");
    expect(windows?.command).toBe("irm https://antigravity.google/cli/install.ps1 | iex");
    expect(windows?.command).not.toContain("curl");
  });

  it("says plainly that Cursor has no Windows CLI installer, pointing at the download page instead", () => {
    expect(getAgentInstallInfo("cursor", "cursor-agent", "mac")?.command).toContain("curl");
    const windows = getAgentInstallInfo("cursor", "cursor-agent", "windows");
    expect(windows?.command).not.toContain("curl");
    expect(windows?.command).toBe("https://cursor.com/downloads");
    expect(windows?.guide).toMatch(/no Windows CLI installer/i);
  });
});

describe("SystemHealthView", () => {
  it("shows the tmux row and no agent host row on macOS", async () => {
    daemonResult.current = macDoctor();
    render(() => <SystemHealthView />);

    expect(await screen.findByText("tmux")).toBeInTheDocument();
    expect(screen.queryByText("Agent host")).not.toBeInTheDocument();
    expect(screen.getByText("Ready for sessions")).toBeInTheDocument();
  });

  it("shows the agent host row and no tmux row on Windows", async () => {
    daemonResult.current = windowsDoctor();
    render(() => <SystemHealthView />);

    expect(await screen.findByText("Agent host")).toBeInTheDocument();
    expect(screen.getByText("ConPTY Runtime")).toBeInTheDocument();
    expect(screen.queryByText("tmux")).not.toBeInTheDocument();
    expect(screen.getByText("Bundled")).toBeInTheDocument();
    expect(screen.getByText("Ready for sessions")).toBeInTheDocument();
  });

  it("does not count tmux's not_applicable probe against the Windows summary", async () => {
    // tmux.available is false here (meaningless on Windows), but agent_host and git are fine -
    // the summary must read "Ready for sessions", not "Attention needed".
    daemonResult.current = windowsDoctor({
      tmux: { available: false, version: null, source: null, path: null, not_applicable: true },
    });
    render(() => <SystemHealthView />);

    expect(await screen.findByText("Ready for sessions")).toBeInTheDocument();
  });

  it("flags Attention needed on Windows when the agent host is missing", async () => {
    daemonResult.current = windowsDoctor({
      agent_host: { available: false, version: null, path: null, source: "missing" },
    });
    render(() => <SystemHealthView />);

    expect(await screen.findByText("Attention needed")).toBeInTheDocument();
    expect(screen.getByText("repomon-agent-host.exe not found")).toBeInTheDocument();
    expect(
      screen.getByText(/Repomon needs the bundled repomon-agent-host\.exe/i),
    ).toBeInTheDocument();
  });
});
