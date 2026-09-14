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
    // Windows readiness depends on the agent host and git even when tmux.available is false.
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

/// The operator's own panel: eight agents, six on PATH and two absent, with the two custom Claude
/// entries sharing a long boilerplate head. Six-versus-two is the case under test; a single row
/// proves nothing about how the panel reads.
const EIGHT_AGENTS = [
  { kind: "claude-code", name: "Claude Code", command: "claude", detected: true },
  { kind: "claude-code", name: "Claude Work", command: "env -u CLAUDE_CONFIG_DIR claude", detected: true },
  { kind: "claude-code", name: "Claude Alt Config", command: "CLAUDE_CONFIG_DIR='/Users/azaleas/.claude-alt' claude", detected: true },
  { kind: "codex", name: "Codex", command: "codex", detected: true },
  { kind: "hermes", name: "Hermes", command: "hermes", detected: true },
  { kind: "opencode", name: "OpenCode", command: "opencode", detected: true },
  { kind: "aider", name: "Aider", command: "aider", detected: false },
  { kind: "cursor", name: "Cursor", command: "cursor-agent", detected: false },
] as SystemDoctorResult["agents"];

/// The badge is an outer span carrying the state colors around an inner span that holds the word,
/// so the word's parent is the element whose classes are under test.
function badgeFor(word: "Detected" | "Not Found"): HTMLElement[] {
  return screen.getAllByText(word).map((label) => label.parentElement as HTMLElement);
}

describe("the Coding Agents panel's state emphasis", () => {
  it("keeps the six detected agents quiet and spends the accent on the two that want something", async () => {
    daemonResult.current = macDoctor({ agents: EIGHT_AGENTS });
    render(() => <SystemHealthView />);
    await screen.findByText("6 / 8 detected");

    const detected = badgeFor("Detected");
    const missing = badgeFor("Not Found");
    expect(detected).toHaveLength(6);
    expect(missing).toHaveLength(2);

    // Detected is the ordinary state: the panel's neutral chip, no state color at all.
    for (const badge of detected) {
      expect(badge.className).toContain("text-muted");
      expect(badge.className).not.toMatch(/signal|attention|fault/);
    }

    // Absence is what wants the operator, and it wears the same tone as the runtime rows' Missing.
    for (const badge of missing) {
      expect(badge.className).toMatch(/\battention\b/);
      expect(badge.className).not.toMatch(/signal|fault/);
    }
  });

  it("leaves signal free to mean NEEDS YOU by never marking a healthy agent with it", async () => {
    daemonResult.current = macDoctor({ agents: EIGHT_AGENTS });
    render(() => <SystemHealthView />);
    await screen.findByText("6 / 8 detected");

    const accented = [...badgeFor("Detected"), ...badgeFor("Not Found")]
      .filter((badge) => /signal/.test(badge.className));
    expect(accented).toEqual([]);
  });

  it("keeps the check glyph on a detected agent so the quiet badge still reads as good news", async () => {
    daemonResult.current = macDoctor({ agents: EIGHT_AGENTS });
    render(() => <SystemHealthView />);
    await screen.findByText("6 / 8 detected");

    expect(badgeFor("Detected")[0].querySelector("svg")).toBeInTheDocument();
  });
});

describe("the Coding Agents panel's command truncation", () => {
  it("clips the shared command head so the binary at the tail survives", async () => {
    daemonResult.current = macDoctor({ agents: EIGHT_AGENTS });
    render(() => <SystemHealthView />);

    const command = await screen.findByText("env -u CLAUDE_CONFIG_DIR claude");
    // .truncate-tail (pinned in index.css.fleet.test.ts) puts the ellipsis before the identifying
    // tail; Tailwind's own truncate would cut `claude` off the end and keep the boilerplate.
    expect(command.classList.contains("truncate-tail")).toBe(true);
    expect(command.classList.contains("truncate")).toBe(false);
  });

  it("keeps the whole probe command reachable even though the row clips it", async () => {
    daemonResult.current = macDoctor({ agents: EIGHT_AGENTS });
    render(() => <SystemHealthView />);

    const command = await screen.findByText("CLAUDE_CONFIG_DIR='/Users/azaleas/.claude-alt' claude");
    expect(command).toHaveAttribute("title", "CLAUDE_CONFIG_DIR='/Users/azaleas/.claude-alt' claude");
  });

  it("clips the install command from the start too, where the head is npm install -g", async () => {
    daemonResult.current = macDoctor({ agents: EIGHT_AGENTS });
    render(() => <SystemHealthView />);

    const install = await screen.findByText("pip install aider-chat");
    expect(install.classList.contains("truncate-tail")).toBe(true);
    expect(install.classList.contains("truncate")).toBe(false);
    expect(install).toHaveAttribute("title", "pip install aider-chat");
  });

  it("lets the install hint wrap instead of clipping the remedy off its end", async () => {
    daemonResult.current = macDoctor({ agents: EIGHT_AGENTS });
    render(() => <SystemHealthView />);

    // The hint is a sentence, not an identifier. Clipping either end loses it; the Windows Cursor
    // hint puts the whole remedy ("download the app instead") past the cut.
    const hint = await screen.findByText("Install Aider CLI via pip");
    expect(hint.classList.contains("truncate")).toBe(false);
    expect(hint.classList.contains("truncate-tail")).toBe(false);
  });
});

describe("SystemHealthView in the setup wizard", () => {
  it("names the ConPTY runtime in the Windows verdict instead of tmux", async () => {
    daemonResult.current = windowsDoctor();
    render(() => <SystemHealthView showTitle={false} showRefresh />);
    const verdict = await screen.findByRole("status");
    expect(verdict).toHaveTextContent("Agent host (ConPTY) and git ready, 0 of 0 agent CLIs found");
    expect(verdict).not.toHaveTextContent("tmux");
  });

  it("shares the re-check row with a one-line verdict instead of floating the button alone", async () => {
    daemonResult.current = macDoctor({
      agents: [
        { kind: "claude-code", name: "Claude Code", command: "claude", detected: true },
        { kind: "codex", name: "Codex", command: "codex", detected: false },
      ] as SystemDoctorResult["agents"],
    });
    render(() => <SystemHealthView showTitle={false} showRefresh />);
    const verdict = await screen.findByRole("status");
    expect(verdict.textContent).toBe("git and tmux ready, 1 of 2 agent CLIs found");
    expect(verdict.parentElement).toContainElement(screen.getByRole("button", { name: "Refresh system health status" }));
  });

  it("says something is missing when a core tool is absent", async () => {
    daemonResult.current = macDoctor({
      git: { available: false, version: null, path: null },
    });
    render(() => <SystemHealthView showTitle={false} showRefresh />);
    const verdict = await screen.findByRole("status");
    expect(verdict.textContent).toBe("Something is missing below, 0 of 0 agent CLIs found");
  });

  it("never reaches for a raw palette class: every state color is a theme token", async () => {
    daemonResult.current = macDoctor({
      agents: [{ kind: "codex", name: "Codex", command: "codex", detected: true }] as SystemDoctorResult["agents"],
    });
    const { container } = render(() => <SystemHealthView />);
    await screen.findByText("Ready for sessions");
    expect(container.innerHTML).not.toMatch(/emerald-|amber-|text-accent|bg-accent|surface-raised/);
  });
});
