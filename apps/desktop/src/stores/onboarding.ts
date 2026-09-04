/// The first-run setup wizard's step machine and its resume point.
///
/// The machine is deliberately pure and lives outside the component: the wizard is the one screen
/// a user sees exactly once, so it is also the one screen whose bugs nobody reports. Keeping
/// "which step comes next" as plain functions means the sequence, the bounds, and the resume
/// behaviour can be tested without rendering anything.

export type OnboardingStepId =
  | "welcome"
  | "system"
  | "repos"
  | "agent"
  | "notifications"
  | "repomind"
  | "done";

export interface OnboardingStep {
  id: OnboardingStepId;
  /// 1-based position, shown in the step rail.
  number: number;
  /// Rail label. Two words at most; the step's own heading carries the detail.
  label: string;
}

/// The sequence, in order. Welcome states the premise, the middle five each set one thing up, and
/// the last one says what happened and where to go next.
export const ONBOARDING_STEPS: readonly OnboardingStep[] = [
  { id: "welcome", number: 1, label: "Welcome" },
  { id: "system", number: 2, label: "System" },
  { id: "repos", number: 3, label: "Repos" },
  { id: "agent", number: 4, label: "Agent" },
  { id: "notifications", number: 5, label: "Alerts" },
  { id: "repomind", number: 6, label: "Repomind" },
  { id: "done", number: 7, label: "Done" },
];

export const FIRST_STEP: OnboardingStepId = ONBOARDING_STEPS[0]!.id;
export const LAST_STEP: OnboardingStepId = ONBOARDING_STEPS[ONBOARDING_STEPS.length - 1]!.id;

export function isOnboardingStepId(value: unknown): value is OnboardingStepId {
  return typeof value === "string" && ONBOARDING_STEPS.some((step) => step.id === value);
}

/// Position of a step in the sequence, or 0 for anything unrecognised. Callers that persisted a
/// step id from an older build get sent back to the start rather than off the end of the rail.
export function stepIndex(id: OnboardingStepId): number {
  const index = ONBOARDING_STEPS.findIndex((step) => step.id === id);
  return index < 0 ? 0 : index;
}

export function stepAt(index: number): OnboardingStepId {
  return (ONBOARDING_STEPS[index] ?? ONBOARDING_STEPS[0]!).id;
}

/// The step after `id`, or null at the end of the sequence. Null is the signal to finish, which is
/// why this returns it rather than clamping.
export function nextStep(id: OnboardingStepId): OnboardingStepId | null {
  const index = stepIndex(id);
  return index >= ONBOARDING_STEPS.length - 1 ? null : ONBOARDING_STEPS[index + 1]!.id;
}

/// The step before `id`, or null at the start.
export function prevStep(id: OnboardingStepId): OnboardingStepId | null {
  const index = stepIndex(id);
  return index <= 0 ? null : ONBOARDING_STEPS[index - 1]!.id;
}

/// Whether `target` may be jumped to directly from `current`. The rail lets you go back to a step
/// you have already been through; it does not let you skip ahead past work you have not done.
export function canJumpTo(current: OnboardingStepId, target: OnboardingStepId): boolean {
  return stepIndex(target) <= stepIndex(current);
}

export const ONBOARDING_STEP_KEY = "repomon:onboarding-step";

/// Where the wizard left off, or null if it has never run or the stored value is not a step this
/// build knows about. Quitting mid-wizard is the common case (a missing agent CLI to go install,
/// a repo to go clone), so the step is written on every move rather than only on exit.
export function readOnboardingStep(): OnboardingStepId | null {
  if (typeof window === "undefined" || typeof localStorage === "undefined") return null;
  const raw = localStorage.getItem(ONBOARDING_STEP_KEY);
  return isOnboardingStepId(raw) ? raw : null;
}

/// Records the resume point. Passing null clears it, which is what finishing or skipping does so
/// a wizard reopened later from Settings starts at the top instead of on the Done screen.
export function saveOnboardingStep(step: OnboardingStepId | null): void {
  if (typeof window === "undefined" || typeof localStorage === "undefined") return;
  if (step === null) localStorage.removeItem(ONBOARDING_STEP_KEY);
  else localStorage.setItem(ONBOARDING_STEP_KEY, step);
}
