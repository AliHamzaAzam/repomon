import { describe, expect, it } from "vitest";

import { cueFrequencies, type SoundCue } from "./sound";

describe("desktop sound contours", () => {
  it.each([
    ["agent-needs-you", [440.00, 554.37]],
    ["agent-finished", [659.25, 493.88]],
    ["repomind-needs-you", [293.66, 440.00, 587.33]],
    ["error-or-stall", [220.00, 155.56]],
    ["incoming-message", [523.25, 659.25]],
    ["update-ready", [261.63, 329.63, 392.00]],
  ] satisfies Array<[SoundCue, number[]]>)("keeps the %s frequencies fixed", (cue, expected) => {
    expect(cueFrequencies(cue)).toEqual(expected);
  });
});
