import { For, Show, createSignal, onCleanup, onMount } from "solid-js";

import type { ActionsStore } from "../stores/actions";
import BrandMark from "./BrandMark";
import SystemHealthView from "./SystemHealthView";
import {
  IconCheck,
  IconChevronLeft,
  IconChevronRight,
  IconCommand,
  IconGitBranch,
  IconLayers,
  IconPlus,
  IconTerminal,
} from "./icons";

export type OnboardingStepId = "welcome" | "system" | "repository" | "done";

export interface OnboardingStep {
  id: OnboardingStepId;
  number: number;
  label: string;
}

export const ONBOARDING_STEPS: OnboardingStep[] = [
  { id: "welcome", number: 1, label: "Welcome" },
  { id: "system", number: 2, label: "System Check" },
  { id: "repository", number: 3, label: "Repositories" },
  { id: "done", number: 4, label: "Ready" },
];

export interface OnboardingProps {
  actions: ActionsStore;
  initialStep?: OnboardingStepId;
  onComplete: () => void;
  onSkip: () => void;
}

export default function Onboarding(props: OnboardingProps) {
  const initialIndex = () => {
    if (props.initialStep) {
      const idx = ONBOARDING_STEPS.findIndex((s) => s.id === props.initialStep);
      return idx >= 0 ? idx : 0;
    }
    return 0;
  };
  const [currentStepIndex, setCurrentStepIndex] = createSignal(initialIndex());
  const currentStep = () => ONBOARDING_STEPS[currentStepIndex()] ?? ONBOARDING_STEPS[0];
  const repos = () => props.actions.fleet.repos();

  function nextStep() {
    if (currentStepIndex() < ONBOARDING_STEPS.length - 1) {
      setCurrentStepIndex((prev) => prev + 1);
    } else {
      props.onComplete();
    }
  }

  function prevStep() {
    if (currentStepIndex() > 0) {
      setCurrentStepIndex((prev) => prev - 1);
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    // Deliberate: Escape does NOT dismiss onboarding (skipping is an explicit click)
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      return;
    }

    if (e.key === "Enter") {
      const activeEl = document.activeElement;
      // If an interactive button or input is explicitly focused, let it handle its own enter
      if (activeEl && (activeEl.tagName === "BUTTON" || activeEl.tagName === "INPUT" || activeEl.tagName === "A")) {
        return;
      }
      e.preventDefault();
      const step = currentStep().id;
      if (step === "welcome" || step === "system" || step === "done") {
        nextStep();
      } else if (step === "repository") {
        if (repos().length > 0) {
          nextStep();
        } else {
          void props.actions.addRepo();
        }
      }
    }
  }

  onMount(() => {
    window.addEventListener("keydown", handleKeyDown);
  });

  onCleanup(() => {
    window.removeEventListener("keydown", handleKeyDown);
  });

  return (
    <div
      class="relative flex h-full w-full flex-col bg-background text-foreground overflow-y-auto select-none"
      data-testid="onboarding-wizard"
    >
      {/* Subtle ambient lighting */}
      <div
        class="pointer-events-none absolute -top-24 left-1/2 -translate-x-1/2 w-[42rem] h-[24rem] rounded-full opacity-20 blur-3xl"
        style="background: radial-gradient(ellipse at center, var(--signal) 0%, transparent 70%);"
        aria-hidden="true"
      />

      {/* Top Header Bar */}
      <header class="relative z-10 flex items-center justify-between border-b border-line/70 px-6 py-3.5 backdrop-blur-sm bg-background/80">
        <div class="flex items-center gap-3">
          <BrandMark size={22} />
          <span class="font-mono text-xs font-semibold tracking-wider text-foreground">REPOMON</span>
          <span class="rounded bg-surface px-1.5 py-0.5 text-[10px] font-mono text-muted border border-line">
            SETUP WIZARD
          </span>
        </div>

        {/* Step Progress Pill Indicator */}
        <nav aria-label="Onboarding progress" class="flex items-center gap-1.5">
          <For each={ONBOARDING_STEPS}>
            {(step, index) => {
              const isPassed = () => index() < currentStepIndex();
              const isCurrent = () => index() === currentStepIndex();
              return (
                <div class="flex items-center gap-1.5">
                  <button
                    type="button"
                    class={`focus-ring flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-mono transition-all ${
                      isCurrent()
                        ? "bg-signal text-background font-semibold shadow-xs"
                        : isPassed()
                          ? "bg-surface-raised text-foreground border border-line hover:bg-surface cursor-pointer"
                          : "text-muted/60 opacity-60"
                    }`}
                    onClick={() => {
                      if (isPassed()) setCurrentStepIndex(index());
                    }}
                    disabled={!isPassed() && !isCurrent()}
                    aria-current={isCurrent() ? "step" : undefined}
                  >
                    <Show when={isPassed()} fallback={<span>{step.number}</span>}>
                      <IconCheck size={10} strokeWidth={3} class="text-signal" />
                    </Show>
                    <span class="hidden sm:inline">{step.label}</span>
                  </button>
                  <Show when={index() < ONBOARDING_STEPS.length - 1}>
                    <span class="text-muted/40 font-mono text-xs">/</span>
                  </Show>
                </div>
              );
            }}
          </For>
        </nav>

        {/* Skip Affordance */}
        <div>
          <button
            type="button"
            class="focus-ring rounded-md px-2.5 py-1 text-xs text-muted hover:text-foreground transition-colors cursor-pointer"
            onClick={props.onSkip}
            aria-label="Skip setup wizard"
          >
            Skip setup
          </button>
        </div>
      </header>

      {/* Main Wizard Content Area */}
      <main class="relative z-10 flex-1 flex flex-col items-center justify-center py-6 px-6 md:px-10 max-w-4xl w-full mx-auto my-auto">
        {/* STEP 1: WELCOME */}
        <Show when={currentStep().id === "welcome"}>
          <div class="w-full max-w-2xl space-y-8 animate-in fade-in duration-200">
            {/* Visual Hero */}
            <div class="flex flex-col items-center text-center space-y-4">
              <div class="relative flex size-20 items-center justify-center rounded-2xl border border-line bg-surface/80 shadow-lg shadow-black/10">
                <BrandMark size={48} />
              </div>
              <div class="space-y-1.5 max-w-lg">
                <h1 class="text-2xl font-bold tracking-tight text-foreground sm:text-3xl">
                  Orchestrate coding agents across git worktrees
                </h1>
                <p class="text-sm text-muted leading-relaxed">
                  Repomon provides an industrial-grade mission control for parallel AI software engineering.
                </p>
              </div>
            </div>

            {/* Core Capability Cards */}
            <div class="grid gap-3 sm:grid-cols-3">
              <div class="rounded-xl border border-line bg-surface/50 p-4 space-y-2 text-left">
                <div class="flex size-8 items-center justify-center rounded-lg border border-line bg-surface text-accent">
                  <IconGitBranch size={16} />
                </div>
                <h2 class="font-medium text-xs text-foreground">Isolated Worktrees</h2>
                <p class="text-[11px] text-muted leading-snug">
                  Every agent session gets a clean git worktree. No branch collisions or locked working trees.
                </p>
              </div>

              <div class="rounded-xl border border-line bg-surface/50 p-4 space-y-2 text-left">
                <div class="flex size-8 items-center justify-center rounded-lg border border-line bg-surface text-accent">
                  <IconLayers size={16} />
                </div>
                <h2 class="font-medium text-xs text-foreground">Multi-Agent Runtimes</h2>
                <p class="text-[11px] text-muted leading-snug">
                  Spawn Claude Code, Codex, Antigravity, Cursor Agent, OpenCode, and Aider side-by-side.
                </p>
              </div>

              <div class="rounded-xl border border-line bg-surface/50 p-4 space-y-2 text-left">
                <div class="flex size-8 items-center justify-center rounded-lg border border-line bg-surface text-accent">
                  <IconTerminal size={16} />
                </div>
                <h2 class="font-medium text-xs text-foreground">Mission Control</h2>
                <p class="text-[11px] text-muted leading-snug">
                  Live tmux terminals, inter-lane agent messaging, automated status tracking, and audio cues.
                </p>
              </div>
            </div>

            {/* CTA */}
            <div class="flex flex-col items-center justify-center gap-3 pt-2">
              <button
                type="button"
                class="focus-ring flex items-center justify-center gap-2 rounded-xl bg-signal px-7 py-3 text-sm font-semibold text-background shadow-md transition-all hover:bg-signal/90 cursor-pointer"
                onClick={nextStep}
                aria-label="Get started with setup"
              >
                <span>Get Started</span>
                <IconChevronRight size={16} strokeWidth={2.5} />
              </button>
              <span class="text-[11px] text-muted font-mono">Press Enter to continue</span>
            </div>
          </div>
        </Show>

        {/* STEP 2: SYSTEM CHECK */}
        <Show when={currentStep().id === "system"}>
          <div class="w-full max-w-3xl space-y-4 animate-in fade-in duration-200">
            <div class="space-y-0.5">
              <h1 class="text-xl font-bold tracking-tight text-foreground">System Health & Tooling</h1>
              <p class="text-xs text-muted">
                Repomon validates your multiplexer, git installation, and agent CLI executables.
              </p>
            </div>

            {/* Embed Reusable SystemHealthView */}
            <div class="rounded-xl border border-line/80 bg-surface/30 p-3.5 backdrop-blur-xs">
              <SystemHealthView
                showTitle={false}
                onConfigureCustomAgents={() => {
                  /* Non-blocking in onboarding */
                }}
              />
            </div>

            <div class="rounded-lg border border-line/60 bg-surface/40 p-2.5 text-[11px] text-muted flex items-center justify-between gap-3">
              <span>
                💡 Repomon runs with built-in bundled tmux. At least one agent CLI is recommended to launch tasks, but you can continue now and install CLIs anytime later.
              </span>
            </div>

            {/* Navigation Buttons */}
            <div class="flex items-center justify-between pt-2 border-t border-line/60">
              <button
                type="button"
                class="focus-ring flex items-center gap-1.5 rounded-lg border border-line bg-surface px-4 py-2 text-xs font-medium text-foreground hover:bg-raised transition-colors cursor-pointer"
                onClick={prevStep}
              >
                <IconChevronLeft size={14} />
                <span>Back</span>
              </button>
              <button
                type="button"
                class="focus-ring flex items-center gap-1.5 rounded-lg bg-signal px-5 py-2 text-xs font-semibold text-background hover:bg-signal/90 transition-colors shadow-xs cursor-pointer"
                onClick={nextStep}
              >
                <span>Continue</span>
                <IconChevronRight size={14} strokeWidth={2.5} />
              </button>
            </div>
          </div>
        </Show>

        {/* STEP 3: REPOSITORIES */}
        <Show when={currentStep().id === "repository"}>
          <div class="w-full max-w-2xl space-y-6 animate-in fade-in duration-200">
            <div class="space-y-1">
              <h1 class="text-xl font-bold tracking-tight text-foreground">Add Your First Repository</h1>
              <p class="text-xs text-muted">
                Repomon manages local git repositories and spins up isolated branch worktrees for each session.
              </p>
            </div>

            <Show
              when={repos().length > 0}
              fallback={
                /* Empty State: Prompt to add repo */
                <div class="flex flex-col items-center justify-center rounded-2xl border-2 border-dashed border-line bg-surface/20 p-8 text-center space-y-4">
                  <div class="flex size-12 items-center justify-center rounded-xl border border-line bg-surface text-accent">
                    <IconGitBranch size={22} />
                  </div>
                  <div class="space-y-1 max-w-sm">
                    <p class="text-sm font-semibold text-foreground">No repository tracked yet</p>
                    <p class="text-xs text-muted">
                      Select any existing git repository folder on your machine to start orchestrating.
                    </p>
                  </div>
                  <button
                    type="button"
                    class="focus-ring flex items-center gap-2 rounded-xl bg-signal px-5 py-2.5 text-xs font-semibold text-background shadow-xs hover:bg-signal/90 transition-all cursor-pointer"
                    onClick={() => void props.actions.addRepo()}
                  >
                    <IconPlus size={14} strokeWidth={2.5} />
                    <span>Choose Folder…</span>
                  </button>
                </div>
              }
            >
              {/* Repos exist */}
              <div class="space-y-3">
                <div class="rounded-xl border border-line bg-surface p-4 space-y-3">
                  <div class="flex items-center justify-between">
                    <span class="section-label">Tracked Repositories</span>
                    <span class="text-[11px] font-mono text-emerald-500 font-medium">
                      ✓ {repos().length} repo{repos().length !== 1 ? "s" : ""} added
                    </span>
                  </div>

                  <div class="divide-y divide-line/60 rounded-lg border border-line/70 bg-background/50">
                    <For each={repos()}>
                      {(repo) => (
                        <div class="flex items-center justify-between p-3 gap-3">
                          <div class="flex items-center gap-2.5 min-w-0">
                            <div class="flex size-7 items-center justify-center rounded-lg border border-line bg-surface text-accent shrink-0">
                              <IconGitBranch size={14} />
                            </div>
                            <div class="min-w-0">
                              <p class="font-semibold text-xs text-foreground truncate">{repo.name}</p>
                              <p class="font-mono text-[11px] text-muted truncate mt-0.5" title={repo.path}>
                                {repo.path}
                              </p>
                            </div>
                          </div>
                          <div class="flex items-center gap-2 shrink-0">
                            <span class="rounded bg-surface px-2 py-0.5 font-mono text-[10px] text-muted border border-line">
                              tracked
                            </span>
                          </div>
                        </div>
                      )}
                    </For>
                  </div>
                </div>

                <div class="flex justify-end">
                  <button
                    type="button"
                    class="focus-ring flex items-center gap-1.5 rounded-lg border border-line bg-surface px-3 py-1.5 text-xs font-medium text-foreground hover:bg-raised transition-colors cursor-pointer"
                    onClick={() => void props.actions.addRepo()}
                  >
                    <IconPlus size={12} />
                    <span>Add another repository</span>
                  </button>
                </div>
              </div>
            </Show>

            {/* Navigation Buttons */}
            <div class="flex items-center justify-between pt-2 border-t border-line/60">
              <button
                type="button"
                class="focus-ring flex items-center gap-1.5 rounded-lg border border-line bg-surface px-4 py-2 text-xs font-medium text-foreground hover:bg-raised transition-colors cursor-pointer"
                onClick={prevStep}
              >
                <IconChevronLeft size={14} />
                <span>Back</span>
              </button>
              <Show
                when={repos().length > 0}
                fallback={
                  <button
                    type="button"
                    class="focus-ring flex items-center gap-1.5 rounded-lg border border-line bg-surface px-4 py-2 text-xs font-medium text-muted hover:text-foreground transition-colors cursor-pointer"
                    onClick={nextStep}
                  >
                    <span>Continue without repository</span>
                    <IconChevronRight size={14} />
                  </button>
                }
              >
                <button
                  type="button"
                  class="focus-ring flex items-center gap-1.5 rounded-lg bg-signal px-5 py-2 text-xs font-semibold text-background hover:bg-signal/90 transition-colors shadow-xs cursor-pointer"
                  onClick={nextStep}
                >
                  <span>Continue</span>
                  <IconChevronRight size={14} strokeWidth={2.5} />
                </button>
              </Show>
            </div>
          </div>
        </Show>

        {/* STEP 4: DONE */}
        <Show when={currentStep().id === "done"}>
          <div class="w-full max-w-2xl space-y-8 animate-in fade-in duration-200 text-center">
            <div class="flex flex-col items-center space-y-3">
              <div class="flex size-16 items-center justify-center rounded-2xl border border-emerald-500/30 bg-emerald-500/10 text-emerald-500 shadow-sm">
                <IconCheck size={32} strokeWidth={2.5} />
              </div>
              <div class="space-y-1 max-w-md">
                <h1 class="text-2xl font-bold tracking-tight text-foreground">
                  You're ready to orchestrate!
                </h1>
                <p class="text-xs text-muted leading-relaxed">
                  Here is a quick reference to the core control surfaces in Repomon.
                </p>
              </div>
            </div>

            {/* Quick Tour Highlights */}
            <div class="grid gap-3 sm:grid-cols-3 text-left">
              <div class="rounded-xl border border-line bg-surface/50 p-4 space-y-2">
                <div class="flex size-7 items-center justify-center rounded-md border border-line bg-surface text-foreground">
                  <IconPlus size={14} />
                </div>
                <h2 class="font-medium text-xs text-foreground">1. Sidebar & Lanes</h2>
                <p class="text-[11px] text-muted leading-snug">
                  Click <span class="font-mono text-foreground">+ Lane</span> to create a fresh branch worktree for your task.
                </p>
              </div>

              <div class="rounded-xl border border-line bg-surface/50 p-4 space-y-2">
                <div class="flex size-7 items-center justify-center rounded-lg border border-line bg-surface text-foreground">
                  <IconTerminal size={14} />
                </div>
                <h2 class="font-medium text-xs text-foreground">2. Spawn Agent</h2>
                <p class="text-[11px] text-muted leading-snug">
                  Launch Claude, Codex, Antigravity, or Cursor in any lane with 1-click terminal attachment.
                </p>
              </div>

              <div class="rounded-xl border border-line bg-surface/50 p-4 space-y-2">
                <div class="flex size-7 items-center justify-center rounded-lg border border-line bg-surface text-foreground">
                  <IconCommand size={14} />
                </div>
                <h2 class="font-medium text-xs text-foreground">3. Command Center</h2>
                <p class="text-[11px] text-muted leading-snug">
                  Press <kbd class="font-mono text-foreground bg-surface px-1 py-0.5 rounded border border-line">⌘K</kbd> to search lanes and <kbd class="font-mono text-foreground bg-surface px-1 py-0.5 rounded border border-line">⌘,</kbd> for Settings.
                </p>
              </div>
            </div>

            {/* Finish Actions */}
            <div class="flex flex-col items-center justify-center gap-3 pt-2">
              <button
                type="button"
                class="focus-ring flex items-center justify-center gap-2 rounded-xl bg-signal px-8 py-3 text-sm font-semibold text-background shadow-md transition-all hover:bg-signal/90 cursor-pointer"
                onClick={props.onComplete}
                aria-label="Open Mission Control"
              >
                <span>Enter Mission Control</span>
                <IconChevronRight size={16} strokeWidth={2.5} />
              </button>
              <span class="text-[11px] text-muted font-mono">Press Enter to start</span>
            </div>
          </div>
        </Show>
      </main>
    </div>
  );
}
