import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Repo, SystemDoctorResult } from "../bindings";
import type { ActionsStore } from "../stores/actions";
import { ONBOARDING_STEPS, ONBOARDING_STEP_KEY, type OnboardingStepId } from "../stores/onboarding";
import Onboarding from "./Onboarding";

const ALL_FOUND: SystemDoctorResult = {
  tmux: { available: true, version: "tmux 3.4", source: "bundled", path: "/bundled/tmux" },
  git: { available: true, version: "git 2.44.0", path: "/usr/bin/git" },
  agents: [
    { kind: "claude-code", name: "Claude Code", command: "claude", detected: true },
    { kind: "codex", name: "Codex", command: "codex", detected: true },
    { kind: "opencode", name: "OpenCode", command: "opencode", detected: false },
  ],
} as unknown as SystemDoctorResult;

const NOTHING_FOUND: SystemDoctorResult = {
  tmux: { available: false, version: null, source: null, path: null },
  git: { available: false, version: null, path: null },
  agents: [
    { kind: "claude-code", name: "Claude Code", command: "claude", detected: false },
    { kind: "codex", name: "Codex", command: "codex", detected: false },
  ],
} as unknown as SystemDoctorResult;

const BASE_CONFIG = {
  worktree_template: "",
  default_agent: null,
  notify_enabled: false,
  notify_needs_you: false,
};

/// One stub daemon for the whole file: the wizard reads `system.doctor`, `config.get` and
/// `repomind.status` on mount, and writes through `config.set` and `orchestrator.start`.
const daemon = {
  doctor: ALL_FOUND,
  config: { ...BASE_CONFIG } as Record<string, unknown>,
  repomind: { home: "/Users/dev/repomind", exists: true },
  calls: [] as Array<{ method: string; params: unknown }>,
  fail: null as string | null,
};

vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn(async (method: string, params?: unknown) => {
    daemon.calls.push({ method, params });
    if (daemon.fail === method) throw new Error("daemon refused");
    switch (method) {
      case "system.doctor":
        return daemon.doctor;
      case "config.get":
        return daemon.config;
      case "config.set":
        daemon.config = params as Record<string, unknown>;
        return daemon.config;
      case "repomind.status":
        return daemon.repomind;
      default:
        return {};
    }
  }),
}));

function createMockActions(repos: Repo[] = []): ActionsStore {
  return {
    fleet: { repos: () => repos },
    addRepo: vi.fn().mockResolvedValue(undefined),
    openSettingsTab: vi.fn(),
  } as unknown as ActionsStore;
}

function mountWizard(options: {
  step?: OnboardingStepId;
  repos?: Repo[];
  notifications?: { nativeEnabled: () => boolean; enableNative: () => Promise<boolean> };
} = {}) {
  const actions = createMockActions(options.repos);
  const onComplete = vi.fn();
  const onSkip = vi.fn();
  const result = render(() => (
    <Onboarding
      actions={actions}
      initialStep={options.step}
      notifications={options.notifications}
      onComplete={onComplete}
      onSkip={onSkip}
    />
  ));
  return { ...result, actions, onComplete, onSkip };
}

/// The most recent record sent to `config.set`.
function lastConfigWrite(): Record<string, unknown> {
  const writes = daemon.calls.filter((call) => call.method === "config.set");
  return (writes[writes.length - 1]?.params ?? {}) as Record<string, unknown>;
}

const stepBody = () => document.querySelector("[data-step-body]")?.getAttribute("data-step-body");

beforeEach(() => {
  daemon.doctor = ALL_FOUND;
  daemon.config = { ...BASE_CONFIG };
  daemon.repomind = { home: "/Users/dev/repomind", exists: true };
  daemon.calls = [];
  daemon.fail = null;
  localStorage.removeItem(ONBOARDING_STEP_KEY);
});

afterEach(cleanup);

describe("setup wizard shell", () => {
  it("keeps its header clear of the macOS traffic lights and draggable", () => {
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
    const { container } = mountWizard();
    const header = container.querySelector("[data-window-chrome]")!;

    expect(header.className).toContain("pl-[78px]");
    expect(header.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(header.querySelector("[data-brand-lockup]")).toBeInTheDocument();
  });

  it("uses ordinary padding on platforms with no overlay buttons", () => {
    Object.defineProperty(navigator, "platform", { value: "Linux x86_64", configurable: true });
    const { container } = mountWizard();
    const header = container.querySelector("[data-window-chrome]")!;

    expect(header.className).toContain("px-3.5");
    expect(header.className).not.toContain("pl-[78px]");
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
  });

  it("shows where you are in the sequence and offers Back, Continue and Skip", () => {
    mountWizard({ step: "repos" });

    const rail = screen.getByRole("navigation", { name: "Setup progress" });
    expect(rail.querySelectorAll("li")).toHaveLength(ONBOARDING_STEPS.length);
    expect(rail.querySelector('[aria-current="step"]')!.textContent).toContain("Repos");

    expect(screen.getByRole("button", { name: "Back" })).toBeEnabled();
    expect(screen.getByRole("button", { name: /Continue/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Skip setup" })).toBeInTheDocument();
  });

  it("has nowhere to go back to on the first step", () => {
    mountWizard({ step: "welcome" });
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();
  });

  it("offers the rail as a way back, never as a way to skip ahead", () => {
    mountWizard({ step: "repos" });
    const rail = screen.getByRole("navigation", { name: "Setup progress" });
    const buttons = Array.from(rail.querySelectorAll("button"));

    expect(buttons[0]).toBeEnabled();
    expect(buttons[1]).toBeEnabled();
    expect(buttons[2]).toBeDisabled();
    expect(buttons[3]).toBeDisabled();

    fireEvent.click(buttons[0]!);
    expect(stepBody()).toBe("welcome");
  });
});

describe("setup wizard sequence", () => {
  it("walks welcome to done and finishes", async () => {
    const { onComplete } = mountWizard();
    const seen: string[] = [];

    for (let i = 0; i < ONBOARDING_STEPS.length; i += 1) {
      seen.push(stepBody()!);
      fireEvent.click(screen.getByRole("button", { name: /Continue|Open Repomon/ }));
    }

    expect(seen).toEqual(ONBOARDING_STEPS.map((step) => step.id));
    await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1));
  });

  it("goes back the way it came", () => {
    mountWizard({ step: "agent" });
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(stepBody()).toBe("repos");
  });

  it("labels the last step's action for what it does", () => {
    mountWizard({ step: "done" });
    expect(screen.getByRole("button", { name: /Open Repomon/ })).toBeInTheDocument();
  });
});

describe("setup wizard resume", () => {
  it("records the step on every move so quitting mid-way loses nothing", () => {
    mountWizard();
    fireEvent.click(screen.getByRole("button", { name: /Continue/ }));
    expect(localStorage.getItem(ONBOARDING_STEP_KEY)).toBe("system");

    fireEvent.click(screen.getByRole("button", { name: /Continue/ }));
    expect(localStorage.getItem(ONBOARDING_STEP_KEY)).toBe("repos");
  });

  it("resumes where it left off", () => {
    localStorage.setItem(ONBOARDING_STEP_KEY, "repomind");
    mountWizard();
    expect(stepBody()).toBe("repomind");
  });

  it("starts at the top when the stored step is not one this build has", () => {
    localStorage.setItem(ONBOARDING_STEP_KEY, "quick-tour");
    mountWizard();
    expect(stepBody()).toBe("welcome");
  });

  it("clears the resume point on skip, so reopening from Settings starts fresh", () => {
    localStorage.setItem(ONBOARDING_STEP_KEY, "agent");
    const { onSkip } = mountWizard();
    fireEvent.click(screen.getByRole("button", { name: "Skip setup" }));

    expect(onSkip).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem(ONBOARDING_STEP_KEY)).toBeNull();
  });

  it("clears the resume point on finish", async () => {
    const { onComplete } = mountWizard({ step: "done" });
    fireEvent.click(screen.getByRole("button", { name: /Open Repomon/ }));

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(localStorage.getItem(ONBOARDING_STEP_KEY)).toBeNull();
  });
});

describe("setup wizard keyboard", () => {
  it("continues on Enter", () => {
    mountWizard();
    fireEvent.keyDown(window, { key: "Enter" });
    expect(stepBody()).toBe("system");
  });

  it("leaves Enter to a focused control rather than doing both", () => {
    mountWizard({ step: "repos" });
    screen.getByRole("button", { name: /Choose folder/ }).focus();
    fireEvent.keyDown(window, { key: "Enter" });
    expect(stepBody()).toBe("repos");
  });

  it("skips on Escape", () => {
    const { onSkip } = mountWizard();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onSkip).toHaveBeenCalledTimes(1);
  });
});

describe("setup wizard steps", () => {
  it("explains what Repomon does, and what a lane and a worktree are", () => {
    mountWizard({ step: "welcome" });
    expect(screen.getByRole("heading", { name: "Set up Repomon" })).toBeInTheDocument();
    expect(screen.getByText(/A lane is one task plus the git worktree/)).toBeInTheDocument();
    expect(screen.getByText(/second checkout of the same repository/)).toBeInTheDocument();
  });

  it("reports the tools it found, with a way to check again", async () => {
    mountWizard({ step: "system" });

    expect(await screen.findByText("Repomon Built-in")).toBeInTheDocument();
    expect(screen.getByText("git 2.44.0")).toBeInTheDocument();
    expect(screen.getByText("2 / 3 detected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Refresh system health status" })).toBeInTheDocument();
  });

  it("names what is missing and how to install it", async () => {
    daemon.doctor = NOTHING_FOUND;
    mountWizard({ step: "system" });

    await waitFor(() => expect(screen.getAllByText("Missing")).toHaveLength(2));
    expect(screen.getByText("brew install tmux")).toBeInTheDocument();
    expect(screen.getByText("npm install -g @anthropic-ai/claude-code")).toBeInTheDocument();
    expect(screen.getByText("0 / 2 detected")).toBeInTheDocument();
  });

  it("re-runs the probe when asked to check again", async () => {
    mountWizard({ step: "system" });
    await screen.findByText("Repomon Built-in");
    const before = daemon.calls.filter((call) => call.method === "system.doctor").length;

    fireEvent.click(screen.getByRole("button", { name: "Refresh system health status" }));

    await waitFor(() => {
      expect(daemon.calls.filter((call) => call.method === "system.doctor").length).toBe(before + 1);
    });
  });

  it("invites a first repository, then lists what was added", () => {
    const { actions, unmount } = mountWizard({ step: "repos" });
    expect(screen.getByText("No repositories yet")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Choose folder/ }));
    expect(actions.addRepo).toHaveBeenCalledTimes(1);
    unmount();

    const repo: Repo = {
      id: 1,
      name: "repomon",
      path: "/Users/dev/repomon",
      added_at: "2026-08-01T00:00:00Z",
      worktree_root_template: null,
      hidden: false,
      position: null,
      label: null,
    };
    mountWizard({ step: "repos", repos: [repo] });
    expect(screen.getByText("repomon")).toBeInTheDocument();
    expect(screen.getByText("/Users/dev/repomon")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Add another repository/ })).toBeInTheDocument();
  });

  it("offers only the agents that were actually found", async () => {
    mountWizard({ step: "agent" });

    const options = await screen.findAllByRole("radio");
    expect(options.map((option) => option.textContent)).toEqual([
      "Claude Codeclaude",
      "Codexcodex",
    ]);
    expect(screen.queryByText("OpenCode")).not.toBeInTheDocument();
  });

  it("saves the chosen agent as the default", async () => {
    mountWizard({ step: "agent" });
    const options = await screen.findAllByRole("radio");

    fireEvent.click(options[1]!);
    await waitFor(() => {
      expect(lastConfigWrite().default_agent).toBe("codex");
    });
    expect(options[1]!.getAttribute("aria-checked")).toBe("true");
  });

  it("says what to do when no agent CLI is installed", async () => {
    daemon.doctor = NOTHING_FOUND;
    mountWizard({ step: "agent" });

    expect(await screen.findByText("No agent CLIs found yet")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Back to the system check/ }));
    expect(stepBody()).toBe("system");
  });

  it("asks for the system permission and turns on the needs-you alert", async () => {
    let granted = false;
    const notifications = {
      nativeEnabled: () => granted,
      enableNative: vi.fn(async () => {
        granted = true;
        return true;
      }),
    };
    mountWizard({ step: "notifications", notifications });

    fireEvent.click(screen.getByRole("button", { name: "Allow notifications" }));
    expect(notifications.enableNative).toHaveBeenCalledTimes(1);

    await waitFor(() => expect(screen.getByRole("switch", { name: "Send alerts from Repomon" })).toBeEnabled());
    fireEvent.click(screen.getByRole("switch", { name: "Send alerts from Repomon" }));
    await waitFor(() => {
      expect(lastConfigWrite().notify_enabled).toBe(true);
    });

    fireEvent.click(screen.getByRole("switch", { name: "Tell me when an agent needs me" }));
    await waitFor(() => {
      expect(lastConfigWrite().notify_needs_you).toBe(true);
    });
  });

  it("shows the real state of the repomind home", async () => {
    mountWizard({ step: "repomind" });

    expect(await screen.findByText("/Users/dev/repomind")).toBeInTheDocument();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Start Repomind when setup finishes" })).toBeInTheDocument();
  });

  it("says so when the home is not there yet", async () => {
    daemon.repomind = { home: "/Users/dev/repomind", exists: false };
    mountWizard({ step: "repomind" });
    expect(await screen.findByText("Not created yet")).toBeInTheDocument();
  });

  it("starts repomind after setup only when asked to", async () => {
    const { onComplete, unmount } = mountWizard({ step: "repomind" });
    fireEvent.click(screen.getByRole("button", { name: /Continue/ }));
    fireEvent.click(screen.getByRole("button", { name: /Open Repomon/ }));
    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(daemon.calls.some((call) => call.method === "orchestrator.start")).toBe(false);
    unmount();

    const second = mountWizard({ step: "repomind" });
    fireEvent.click(screen.getByRole("switch", { name: "Start Repomind when setup finishes" }));
    fireEvent.click(screen.getByRole("button", { name: /Continue/ }));
    fireEvent.click(screen.getByRole("button", { name: /Open Repomon/ }));

    await waitFor(() => expect(second.onComplete).toHaveBeenCalled());
    expect(daemon.calls.some((call) => call.method === "orchestrator.start")).toBe(true);
  });

  it("keeps the wizard up and names the problem when the finishing call fails", async () => {
    daemon.fail = "orchestrator.start";
    const { onComplete } = mountWizard({ step: "repomind" });

    fireEvent.click(screen.getByRole("switch", { name: "Start Repomind when setup finishes" }));
    fireEvent.click(screen.getByRole("button", { name: /Continue/ }));
    fireEvent.click(screen.getByRole("button", { name: /Open Repomon/ }));

    expect(await screen.findByRole("alert")).toHaveTextContent("daemon refused");
    expect(onComplete).not.toHaveBeenCalled();
  });

  it("summarises what was set up and suggests three things to do next", async () => {
    const repo: Repo = {
      id: 1,
      name: "repomon",
      path: "/Users/dev/repomon",
      added_at: "2026-08-01T00:00:00Z",
      worktree_root_template: null,
      hidden: false,
      position: null,
      label: null,
    };
    daemon.config = { ...BASE_CONFIG, default_agent: "codex", notify_enabled: true, notify_needs_you: true };
    mountWizard({ step: "done", repos: [repo] });

    await waitFor(() => expect(screen.getByText("codex")).toBeInTheDocument());
    expect(screen.getByText("1 added")).toBeInTheDocument();
    expect(screen.getByText("On for agents that need you")).toBeInTheDocument();
    expect(screen.getByText("Not started")).toBeInTheDocument();

    expect(screen.getByText("Open a lane.")).toBeInTheDocument();
    expect(screen.getByText("Find a file fast.")).toBeInTheDocument();
    expect(screen.getByText("Start Repomind.")).toBeInTheDocument();
  });
});

describe("setup wizard copy", () => {
  // House rules: no emoji standing in for icons, and no em-dashes anywhere in product copy.
  it("draws its icons and writes its dashes plainly", () => {
    for (const step of ONBOARDING_STEPS) {
      const { container, unmount } = mountWizard({ step: step.id });
      const text = container.textContent ?? "";
      expect(text).not.toMatch(/\p{Extended_Pictographic}/u);
      expect(text).not.toContain("—");
      unmount();
    }
  });
});
