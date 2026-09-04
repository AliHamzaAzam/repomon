import { afterEach, describe, expect, it } from "vitest";

import {
  FIRST_STEP,
  LAST_STEP,
  ONBOARDING_STEPS,
  ONBOARDING_STEP_KEY,
  canJumpTo,
  isOnboardingStepId,
  nextStep,
  prevStep,
  readOnboardingStep,
  saveOnboardingStep,
  stepAt,
  stepIndex,
  type OnboardingStepId,
} from "./onboarding";

afterEach(() => {
  localStorage.removeItem(ONBOARDING_STEP_KEY);
});

describe("onboarding step machine", () => {
  it("runs welcome to done through every step the wizard advertises", () => {
    const visited: OnboardingStepId[] = [FIRST_STEP];
    let cursor: OnboardingStepId | null = FIRST_STEP;
    while ((cursor = nextStep(cursor!)) !== null) visited.push(cursor);

    expect(visited).toEqual(ONBOARDING_STEPS.map((step) => step.id));
    expect(visited[visited.length - 1]).toBe(LAST_STEP);
  });

  it("stops at both ends instead of wrapping", () => {
    expect(prevStep(FIRST_STEP)).toBeNull();
    expect(nextStep(LAST_STEP)).toBeNull();
  });

  it("returns to the step it came from", () => {
    for (const step of ONBOARDING_STEPS) {
      const forward = nextStep(step.id);
      if (forward) expect(prevStep(forward)).toBe(step.id);
    }
  });

  it("numbers the rail from one, in sequence", () => {
    expect(ONBOARDING_STEPS.map((step) => step.number)).toEqual([1, 2, 3, 4, 5, 6, 7]);
    ONBOARDING_STEPS.forEach((step, i) => expect(stepIndex(step.id)).toBe(i));
  });

  it("allows jumping back or to the current step, never forward", () => {
    expect(canJumpTo("agent", "welcome")).toBe(true);
    expect(canJumpTo("agent", "agent")).toBe(true);
    expect(canJumpTo("agent", "notifications")).toBe(false);
    expect(canJumpTo("welcome", "done")).toBe(false);
  });

  it("treats an unknown step id as the start rather than as a position off the rail", () => {
    expect(isOnboardingStepId("welcome")).toBe(true);
    expect(isOnboardingStepId("quick-tour")).toBe(false);
    expect(isOnboardingStepId(null)).toBe(false);
    expect(stepIndex("quick-tour" as OnboardingStepId)).toBe(0);
    expect(stepAt(99)).toBe(FIRST_STEP);
  });
});

describe("onboarding resume point", () => {
  it("reads back the step it stored", () => {
    saveOnboardingStep("repomind");
    expect(readOnboardingStep()).toBe("repomind");
  });

  it("reports no resume point before the wizard has ever run", () => {
    expect(readOnboardingStep()).toBeNull();
  });

  it("clears the resume point when passed null, so a reopened wizard starts at the top", () => {
    saveOnboardingStep("agent");
    saveOnboardingStep(null);
    expect(localStorage.getItem(ONBOARDING_STEP_KEY)).toBeNull();
    expect(readOnboardingStep()).toBeNull();
  });

  it("ignores a stored step this build does not have", () => {
    localStorage.setItem(ONBOARDING_STEP_KEY, "quick-tour");
    expect(readOnboardingStep()).toBeNull();
  });
});
