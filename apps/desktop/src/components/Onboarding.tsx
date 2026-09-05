import { For, Match, Show, Switch as SwitchBlock, createSignal, onCleanup, onMount } from "solid-js";

import type { AgentDoctorInfo, SystemDoctorResult } from "../bindings";
import { daemonCall, type ConfigView } from "../ipc/rpc";
import { BINDINGS, formatChord } from "../keymap";
import type { ActionsStore } from "../stores/actions";
import {
  FIRST_STEP,
  ONBOARDING_STEPS,
  canJumpTo,
  nextStep,
  prevStep,
  readOnboardingStep,
  saveOnboardingStep,
  stepIndex,
  type OnboardingStepId,
} from "../stores/onboarding";
import BrandLockup from "./BrandLockup";
import SystemHealthView from "./SystemHealthView";
import WindowChromeHeader from "./WindowChrome";
import Switch from "./controls/Switch";
import {
  AgentIcon,
  IconBrain,
  IconCheck,
  IconChevronLeft,
  IconChevronRight,
  IconCommand,
  IconCpu,
  IconGitBranch,
  IconPlus,
  IconRadar,
  IconTerminal,
} from "./icons";

export type { OnboardingStepId } from "../stores/onboarding";
export { ONBOARDING_STEPS } from "../stores/onboarding";

/// The slice of the notification store the wizard drives. Narrow on purpose: the wizard asks the
/// OS for permission and nothing else, and a narrow type is a cheap stub in tests.
export interface OnboardingNotifications {
  nativeEnabled: () => boolean;
  enableNative: () => Promise<boolean>;
}

export interface OnboardingProps {
  actions: ActionsStore;
  notifications?: OnboardingNotifications;
  /// Overrides the persisted resume point. Tests use it; the app lets the wizard resume itself.
  initialStep?: OnboardingStepId;
  onComplete: () => void;
  onSkip: () => void;
}

/// The first-run setup wizard: seven steps that leave a new install with a repository, an agent,
/// alerts, and somewhere to put what its agents learn.
///
/// It is resumable by design. Half of these steps send you somewhere else (install a CLI, clone a
/// repo, answer a system permission prompt), and an install that has to be re-done from step one
/// after every detour does not get finished. Every move writes the step to local storage, so
/// quitting mid-way and relaunching lands back on the same step.
export default function Onboarding(props: OnboardingProps) {
  const [step, setStep] = createSignal<OnboardingStepId>(
    props.initialStep ?? readOnboardingStep() ?? FIRST_STEP,
  );
  const [doctor, setDoctor] = createSignal<SystemDoctorResult | null>(null);
  const [config, setConfig] = createSignal<ConfigView | null>(null);
  const [repomindHome, setRepomindHome] = createSignal<{ path: string; exists: boolean } | null>(null);
  const [startRepomind, setStartRepomind] = createSignal(false);
  const [failure, setFailure] = createSignal<string | null>(null);

  const repos = () => props.actions.fleet.repos();
  const detectedAgents = (): AgentDoctorInfo[] => doctor()?.agents.filter((a) => a.detected) ?? [];
  const defaultAgent = () => config()?.default_agent ?? null;
  const index = () => stepIndex(step());

  // One token for every daemon read this screen starts. The wizard is short-lived and the calls
  // below are not (a doctor probe walks PATH), so a user who skips out mid-probe must not have a
  // late promise write into a component that is already gone.
  let token = 0;
  let live = true;

  async function loadContext() {
    const mine = ++token;
    try {
      const [probe, cfg, mind] = await Promise.all([
        daemonCall("system.doctor"),
        daemonCall("config.get"),
        daemonCall("repomind.status"),
      ]);
      if (!live || mine !== token) return;
      setDoctor(probe);
      setConfig(cfg);
      setRepomindHome({ path: mind.home, exists: mind.exists });
    } catch {
      // Without this data every step reads as "nothing detected yet" and still lets the user
      // continue, so a daemon that is briefly unreachable costs a re-check, not the wizard.
    }
  }

  /// Merge one field into the daemon config. `config.set` takes the whole record, the way Settings
  /// sends it, so the local copy is the base rather than a bare patch.
  async function patchConfig(next: Partial<ConfigView>) {
    const current = config();
    if (!current) return;
    const merged = { ...current, ...next };
    setConfig(merged);
    const mine = ++token;
    try {
      const saved = await daemonCall("config.set", merged);
      if (live && mine === token) setConfig(saved);
    } catch (cause) {
      if (!live || mine !== token) return;
      setConfig(current);
      setFailure(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function goTo(id: OnboardingStepId) {
    setStep(id);
    saveOnboardingStep(id);
  }

  function goNext() {
    const following = nextStep(step());
    if (following) goTo(following);
    else void finish();
  }

  function goBack() {
    const preceding = prevStep(step());
    if (preceding) goTo(preceding);
  }

  async function finish() {
    if (startRepomind()) {
      try {
        // `orchestrator.start` is also what creates the home: it runs the daemon's ensure-home
        // before spawning the controller, so one call covers both promises the Repomind step made.
        await daemonCall("orchestrator.start", {});
      } catch (cause) {
        setFailure(cause instanceof Error ? cause.message : String(cause));
        return;
      }
    }
    saveOnboardingStep(null);
    props.onComplete();
  }

  function skip() {
    saveOnboardingStep(null);
    props.onSkip();
  }

  function handleKeyDown(event: KeyboardEvent) {
    if (event.defaultPrevented) return;
    const active = document.activeElement;
    const onControl = active instanceof HTMLElement
      && (active.tagName === "BUTTON" || active.tagName === "INPUT" || active.tagName === "A");

    if (event.key === "Escape") {
      event.preventDefault();
      skip();
      return;
    }
    // A focused control owns its own Enter, or the wizard would both press it and advance.
    if (event.key === "Enter" && !onControl) {
      event.preventDefault();
      goNext();
    }
  }

  onMount(() => {
    void loadContext();
    window.addEventListener("keydown", handleKeyDown);
  });

  onCleanup(() => {
    live = false;
    token += 1;
    window.removeEventListener("keydown", handleKeyDown);
  });

  return (
    <div
      class="flex h-full w-full flex-col bg-background text-foreground"
      data-testid="onboarding-wizard"
    >
      <WindowChromeHeader>
        <BrandLockup />
        <button
          type="button"
          class="focus-ring cursor-pointer rounded px-2 py-1 text-xs text-muted transition-colors hover:text-foreground"
          onClick={skip}
          aria-label="Skip setup"
        >
          Skip setup
        </button>
      </WindowChromeHeader>

      <StepRail current={step()} onJump={goTo} />

      <div class="min-h-0 flex-1 overflow-y-auto">
        <div class="mx-auto w-full max-w-[46rem] px-8 py-9">
          {/* Keyed on the step so the one authored motion in this screen (a short rise as the
              next step arrives) replays per step rather than once per mount. */}
          <div class="onboarding-step" data-step={step()}>
            <SwitchBlock>
              <Match when={step() === "welcome"}>
                <WelcomeStep />
              </Match>
              <Match when={step() === "system"}>
                <SystemStep />
              </Match>
              <Match when={step() === "repos"}>
                <ReposStep repos={repos()} onAdd={() => void props.actions.addRepo()} />
              </Match>
              <Match when={step() === "agent"}>
                <AgentStep
                  agents={detectedAgents()}
                  selected={defaultAgent()}
                  onSelect={(kind) => void patchConfig({ default_agent: kind })}
                  onBackToSystem={() => goTo("system")}
                />
              </Match>
              <Match when={step() === "notifications"}>
                <NotificationsStep
                  config={config()}
                  notifications={props.notifications}
                  onPatch={(next) => void patchConfig(next)}
                />
              </Match>
              <Match when={step() === "repomind"}>
                <RepomindStep
                  home={repomindHome()}
                  start={startRepomind()}
                  onStartChange={setStartRepomind}
                />
              </Match>
              <Match when={step() === "done"}>
                <DoneStep
                  repoCount={repos().length}
                  agent={defaultAgent()}
                  alerts={Boolean(config()?.notify_enabled && config()?.notify_needs_you)}
                  repomind={startRepomind()}
                  onOpenShortcuts={() => props.actions.openShortcutsGuide()}
                />
              </Match>
            </SwitchBlock>
          </div>
        </div>
      </div>

      <Show when={failure()}>
        {(message) => (
          <p role="alert" class="border-t border-fault/40 bg-fault/10 px-8 py-2 text-xs text-fault">
            {message()}
          </p>
        )}
      </Show>

      <footer class="flex shrink-0 items-center justify-between gap-4 border-t border-line bg-surface/60 px-8 py-3">
        <button
          type="button"
          class="focus-ring flex cursor-pointer items-center gap-1.5 rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised disabled:cursor-default disabled:opacity-40"
          onClick={goBack}
          disabled={index() === 0}
        >
          <IconChevronLeft size={13} />
          <span>Back</span>
        </button>

        <p class="truncate font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
          Enter continues, Esc skips
        </p>

        <button
          type="button"
          class="focus-ring flex cursor-pointer items-center gap-1.5 rounded-lg bg-signal px-4 py-1.5 text-xs font-semibold text-background shadow-xs transition-colors hover:bg-signal/90"
          onClick={goNext}
        >
          <span>{step() === "done" ? "Open Repomon" : "Continue"}</span>
          <IconChevronRight size={13} strokeWidth={2.5} />
        </button>
      </footer>
    </div>
  );
}

/// The step rail. Numbers are earned here: this is a sequence, and "where am I in it" is the whole
/// reason the rail exists. Steps already passed are buttons back to themselves; steps not reached
/// yet are inert, so the rail never offers a jump it would refuse.
function StepRail(props: { current: OnboardingStepId; onJump: (id: OnboardingStepId) => void }) {
  const currentIndex = () => stepIndex(props.current);
  return (
    <nav aria-label="Setup progress" class="shrink-0 border-b border-line bg-surface/40 px-8 py-2.5">
      <ol class="flex items-center gap-1">
        <For each={ONBOARDING_STEPS}>
          {(item, i) => {
            const done = () => i() < currentIndex();
            const current = () => i() === currentIndex();
            return (
              <li class="flex min-w-0 items-center gap-1">
                <button
                  type="button"
                  class="focus-ring flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] transition-colors disabled:cursor-default"
                  classList={{
                    "bg-signal/15 font-semibold text-foreground ring-1 ring-signal/50": current(),
                    "cursor-pointer text-foreground hover:bg-line/50": done(),
                    "text-muted": !done() && !current(),
                  }}
                  disabled={!canJumpTo(props.current, item.id) || current()}
                  onClick={() => props.onJump(item.id)}
                  aria-current={current() ? "step" : undefined}
                >
                  <Show
                    when={done()}
                    fallback={<span class="font-mono tabular-nums">{item.number}</span>}
                  >
                    <IconCheck size={11} strokeWidth={3} />
                  </Show>
                  <span class="truncate">{item.label}</span>
                </button>
                <Show when={i() < ONBOARDING_STEPS.length - 1}>
                  <span class="h-px w-2 bg-line" aria-hidden="true" />
                </Show>
              </li>
            );
          }}
        </For>
      </ol>
    </nav>
  );
}

/// One step's heading block. Every step wears the same frame so the eye lands in the same place
/// seven times running, which is most of what makes a wizard feel short.
function StepHead(props: { title: string; lede: string }) {
  return (
    <div class="mb-6">
      <h2 class="text-lg font-semibold tracking-tight text-foreground">{props.title}</h2>
      <p class="mt-1.5 max-w-[62ch] text-xs leading-relaxed text-muted">{props.lede}</p>
    </div>
  );
}

function WelcomeStep() {
  return (
    <div data-step-body="welcome">
      <StepHead
        title="Set up Repomon"
        lede="Three things to know, then five short steps. Everything set here can be changed later in Settings."
      />
      <dl class="divide-y divide-line/70 rounded-xl border border-line bg-surface/50">
        <div class="flex gap-3 p-4">
          <span class="mt-0.5 shrink-0 text-signal"><IconTerminal size={15} /></span>
          <div>
            <dt class="text-xs font-medium text-foreground">
              Repomon runs coding agents on your own repositories, from one window.
            </dt>
            <dd class="mt-1 text-[11px] leading-relaxed text-muted">
              An agent kind is the CLI Repomon launches, such as Claude Code or Codex. Repomon
              starts it, watches it, and gives you its terminal.
            </dd>
          </div>
        </div>
        <div class="flex gap-3 p-4">
          <span class="mt-0.5 shrink-0 text-signal"><IconGitBranch size={15} /></span>
          <div>
            <dt class="text-xs font-medium text-foreground">
              Each task runs in a lane, on a branch of its own.
            </dt>
            <dd class="mt-1 text-[11px] leading-relaxed text-muted">
              A lane is one task plus the git worktree it runs in. A worktree is a second checkout
              of the same repository, so two agents never edit one working directory at once.
            </dd>
          </div>
        </div>
        <div class="flex gap-3 p-4">
          <span class="mt-0.5 shrink-0 text-signal"><IconRadar size={15} /></span>
          <div>
            <dt class="text-xs font-medium text-foreground">
              Repomon tells you which lane is waiting on you.
            </dt>
            <dd class="mt-1 text-[11px] leading-relaxed text-muted">
              Lanes report their own state, so you read one list instead of ten terminals.
            </dd>
          </div>
        </div>
      </dl>
    </div>
  );
}

function SystemStep() {
  return (
    <div data-step-body="system">
      <StepHead
        title="Check your tools"
        lede="Repomon needs git, and tmux to hold the terminal sessions (it ships its own tmux on macOS and Linux). Agent CLIs are found on your PATH. Install anything missing, then check again."
      />
      <SystemHealthView showTitle={false} showRefresh />
    </div>
  );
}

function ReposStep(props: {
  repos: ReturnType<ActionsStore["fleet"]["repos"]>;
  onAdd: () => void;
}) {
  return (
    <div data-step-body="repos">
      <StepHead
        title="Add a repository"
        lede="Point Repomon at a local git repository. Lanes are cut inside it, each in its own worktree. Add as many as you work in."
      />
      <Show
        when={props.repos.length > 0}
        fallback={
          <div class="rounded-xl border border-dashed border-line bg-surface/30 p-8 text-center">
            <p class="text-xs font-medium text-foreground">No repositories yet</p>
            <p class="mx-auto mt-1 max-w-[46ch] text-[11px] leading-relaxed text-muted">
              Choose any folder that is already a git repository. Repomon reads it in place and
              works in worktrees, so your checked-out branch is left alone.
            </p>
            <button
              type="button"
              class="focus-ring mt-4 inline-flex cursor-pointer items-center gap-1.5 rounded-lg bg-signal px-4 py-2 text-xs font-semibold text-background shadow-xs transition-colors hover:bg-signal/90"
              onClick={props.onAdd}
            >
              <IconPlus size={13} strokeWidth={2.5} />
              <span>Choose folder</span>
            </button>
          </div>
        }
      >
        <div class="overflow-hidden rounded-xl border border-line bg-surface/50">
          <ul class="divide-y divide-line/70">
            <For each={props.repos}>
              {(repo) => (
                <li class="flex items-center gap-2.5 p-3">
                  <span class="shrink-0 text-signal"><IconGitBranch size={14} /></span>
                  <span class="min-w-0">
                    <span class="block truncate text-xs font-medium text-foreground">{repo.name}</span>
                    <span class="block truncate font-mono text-[10.5px] text-muted" title={repo.path}>
                      {repo.path}
                    </span>
                  </span>
                </li>
              )}
            </For>
          </ul>
          <div class="border-t border-line/70 p-2.5">
            <button
              type="button"
              class="focus-ring inline-flex cursor-pointer items-center gap-1.5 rounded-lg border border-line bg-surface px-3 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised"
              onClick={props.onAdd}
            >
              <IconPlus size={12} />
              <span>Add another repository</span>
            </button>
          </div>
        </div>
      </Show>
    </div>
  );
}

function AgentStep(props: {
  agents: AgentDoctorInfo[];
  selected: string | null;
  onSelect: (kind: string) => void;
  onBackToSystem: () => void;
}) {
  return (
    <div data-step-body="agent">
      <StepHead
        title="Choose a default agent"
        lede="Only the agent CLIs found on this machine are offered. New lanes start with this one unless you pick another when you create them."
      />
      <Show
        when={props.agents.length > 0}
        fallback={
          <div class="rounded-xl border border-dashed border-line bg-surface/30 p-8 text-center">
            <p class="text-xs font-medium text-foreground">No agent CLIs found yet</p>
            <p class="mx-auto mt-1 max-w-[48ch] text-[11px] leading-relaxed text-muted">
              Repomon only offers agents it can actually run. Install one, then set the default
              here or in Settings.
            </p>
            <button
              type="button"
              class="focus-ring mt-4 inline-flex cursor-pointer items-center gap-1.5 rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised"
              onClick={props.onBackToSystem}
            >
              <IconCpu size={13} />
              <span>Back to the system check</span>
            </button>
          </div>
        }
      >
        <ul class="grid gap-2 sm:grid-cols-2" role="radiogroup" aria-label="Default agent">
          <For each={props.agents}>
            {(agent) => {
              const chosen = () => props.selected === agent.kind || props.selected === agent.name;
              return (
                <li>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={chosen()}
                    class="focus-ring flex w-full cursor-pointer items-center gap-2.5 rounded-lg border p-3 text-left transition-colors"
                    classList={{
                      "border-signal bg-signal/10": chosen(),
                      "border-line bg-surface/50 hover:bg-surface": !chosen(),
                    }}
                    onClick={() => props.onSelect(agent.kind)}
                  >
                    <span class="shrink-0 text-foreground">
                      <AgentIcon agent={agent.kind} size={15} />
                    </span>
                    <span class="min-w-0 flex-1">
                      <span class="block truncate text-xs font-medium text-foreground">{agent.name}</span>
                      <span class="block truncate font-mono text-[10.5px] text-muted">{agent.command}</span>
                    </span>
                    <Show when={chosen()}>
                      <span class="shrink-0 text-signal"><IconCheck size={14} strokeWidth={3} /></span>
                    </Show>
                  </button>
                </li>
              );
            }}
          </For>
        </ul>
      </Show>
    </div>
  );
}

function NotificationsStep(props: {
  config: ConfigView | null;
  notifications?: OnboardingNotifications;
  onPatch: (next: Partial<ConfigView>) => void;
}) {
  const [asked, setAsked] = createSignal(false);
  const granted = () => props.notifications?.nativeEnabled() ?? false;
  return (
    <div data-step-body="notifications">
      <StepHead
        title="Turn on alerts"
        lede="An agent that stops to ask a question waits until you notice. A desktop notification is how you find out without watching the window."
      />
      <div class="space-y-2">
        <div class="flex items-center justify-between gap-4 rounded-lg border border-line bg-surface/60 px-3.5 py-2.5">
          <span class="min-w-0">
            <span class="block text-xs font-medium text-foreground">Desktop notifications</span>
            <span class="block text-[11px] text-muted">
              {granted()
                ? "Your system is letting Repomon post notifications."
                : "Your system has to allow this once, per app."}
            </span>
          </span>
          <Show
            when={!granted()}
            fallback={
              <span class="inline-flex shrink-0 items-center gap-1 rounded border border-signal/40 bg-signal/15 px-2 py-0.5 text-[10.5px] font-medium text-foreground">
                <span class="text-signal"><IconCheck size={11} strokeWidth={2.5} /></span>
                Allowed
              </span>
            }
          >
            <button
              type="button"
              class="focus-ring shrink-0 cursor-pointer rounded-lg border border-line bg-surface px-3 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised disabled:cursor-default disabled:opacity-40"
              disabled={!props.notifications}
              onClick={() => {
                setAsked(true);
                void props.notifications?.enableNative();
              }}
            >
              Allow notifications
            </button>
          </Show>
        </div>
        <Show when={asked() && !granted()}>
          <p class="rounded-lg border border-attention/40 bg-attention/10 px-3 py-2 text-[11px] leading-relaxed text-foreground">
            Your system did not grant permission. Turn Repomon on under Notifications in System
            Settings, then come back to this step.
          </p>
        </Show>

        <Switch
          label="Send alerts from Repomon"
          checked={Boolean(props.config?.notify_enabled)}
          disabled={!props.config}
          onChange={(value) => props.onPatch({ notify_enabled: value })}
        />
        <Switch
          label="Tell me when an agent needs me"
          checked={Boolean(props.config?.notify_needs_you)}
          disabled={!props.config || !props.config.notify_enabled}
          onChange={(value) => props.onPatch({ notify_needs_you: value })}
        />
        <p class="px-1 text-[11px] leading-relaxed text-muted">
          Sounds, the other alert kinds, and quiet-while-focused live in Settings, under
          Notifications.
        </p>
      </div>
    </div>
  );
}

function RepomindStep(props: {
  home: { path: string; exists: boolean } | null;
  start: boolean;
  onStartChange: (value: boolean) => void;
}) {
  return (
    <div data-step-body="repomind">
      <StepHead
        title="Repomind"
        lede="Repomind is a long-running agent that keeps what your fleet learns. It works out of a home directory, holding notes, playbooks and a journal as ordinary files, so that knowledge outlives any one lane and every agent can read it. The daemon creates the home the first time it starts."
      />
      <div class="space-y-2">
        <div class="flex items-center justify-between gap-4 rounded-lg border border-line bg-surface/60 px-3.5 py-2.5">
          <span class="min-w-0">
            <span class="block text-xs font-medium text-foreground">Home</span>
            <span class="block truncate font-mono text-[10.5px] text-muted">
              {props.home?.path ?? "~/repomind"}
            </span>
          </span>
          <Show
            when={props.home?.exists}
            fallback={
              <span class="shrink-0 rounded border border-line bg-surface px-2 py-0.5 text-[10.5px] font-medium text-muted">
                Not created yet
              </span>
            }
          >
            <span class="inline-flex shrink-0 items-center gap-1 rounded border border-signal/40 bg-signal/15 px-2 py-0.5 text-[10.5px] font-medium text-foreground">
              <span class="text-signal"><IconCheck size={11} strokeWidth={2.5} /></span>
              Ready
            </span>
          </Show>
        </div>
        <Switch
          label="Start Repomind when setup finishes"
          checked={props.start}
          onChange={props.onStartChange}
        />
        <p class="px-1 text-[11px] leading-relaxed text-muted">
          Starting it creates the home first if it is not there yet. You can start and stop it any
          time from the Repomind row at the top of the sidebar.
        </p>
      </div>
    </div>
  );
}

/// Look up a chord by binding id in keymap.ts's BINDINGS and format it for display. Mirrors
/// ControlCenter.tsx's and App.tsx's helper of the same name and purpose, so the Done step's
/// "keyboard shortcuts" hint can never drift out of sync with the real binding.
function chordFor(id: string): string | undefined {
  const binding = BINDINGS.find((entry) => entry.id === id);
  return binding ? formatChord(binding.chord) : undefined;
}

function DoneStep(props: {
  repoCount: number;
  agent: string | null;
  alerts: boolean;
  repomind: boolean;
  onOpenShortcuts: () => void;
}) {
  const summary = () => [
    { label: "Repositories", value: props.repoCount === 0 ? "None yet" : `${props.repoCount} added` },
    { label: "Default agent", value: props.agent ?? "Not set" },
    { label: "Alerts", value: props.alerts ? "On for agents that need you" : "Off" },
    { label: "Repomind", value: props.repomind ? "Starting after setup" : "Not started" },
  ];
  return (
    <div data-step-body="done">
      <StepHead
        title="Setup complete"
        lede="Here is what is configured now. All of it lives in Settings if you want to change it."
      />
      <dl class="mb-7 divide-y divide-line/70 overflow-hidden rounded-xl border border-line bg-surface/50">
        <For each={summary()}>
          {(row) => (
            <div class="flex items-baseline justify-between gap-4 px-4 py-2.5">
              <dt class="section-label">{row.label}</dt>
              <dd class="truncate text-xs text-foreground">{row.value}</dd>
            </div>
          )}
        </For>
      </dl>

      <h3 class="section-label">Try next</h3>
      <ul class="mt-2 space-y-2">
        <li class="flex gap-3 rounded-lg border border-line bg-surface/50 p-3">
          <span class="mt-0.5 shrink-0 text-signal"><IconPlus size={14} /></span>
          <p class="text-[11px] leading-relaxed text-muted">
            <span class="text-xs font-medium text-foreground">Open a lane.</span>{" "}
            Use New lane in the sidebar. Repomon cuts the branch and the worktree, then starts your
            agent inside it.
          </p>
        </li>
        <li class="flex gap-3 rounded-lg border border-line bg-surface/50 p-3">
          <span class="mt-0.5 shrink-0 text-signal"><IconCommand size={14} /></span>
          <p class="text-[11px] leading-relaxed text-muted">
            <span class="text-xs font-medium text-foreground">Find a file fast.</span>{" "}
            Press{" "}
            <kbd class="rounded border border-line bg-surface px-1 py-0.5 font-mono text-[10px] text-foreground">
              Cmd P
            </kbd>{" "}
            in the editor to jump to any file in the lane.
          </p>
        </li>
        <li class="flex gap-3 rounded-lg border border-line bg-surface/50 p-3">
          <span class="mt-0.5 shrink-0 text-signal"><IconBrain size={14} /></span>
          <p class="text-[11px] leading-relaxed text-muted">
            <span class="text-xs font-medium text-foreground">Start Repomind.</span>{" "}
            Its row sits at the top of the sidebar. Start it once and it keeps notes for every lane
            that follows.
          </p>
        </li>
        <li>
          <button
            type="button"
            class="focus-ring flex w-full cursor-pointer gap-3 rounded-lg border border-line bg-surface/50 p-3 text-left transition-colors hover:bg-raised"
            onClick={() => props.onOpenShortcuts()}
          >
            <span class="mt-0.5 shrink-0 text-signal"><IconCommand size={14} /></span>
            <p class="text-[11px] leading-relaxed text-muted">
              <span class="text-xs font-medium text-foreground">Learn the keyboard shortcuts.</span>{" "}
              Press{" "}
              <kbd class="rounded border border-line bg-surface px-1 py-0.5 font-mono text-[10px] text-foreground">
                {chordFor("help.open")}
              </kbd>{" "}
              anytime, or click here now, to open the full cheat sheet.
            </p>
          </button>
        </li>
      </ul>
    </div>
  );
}
