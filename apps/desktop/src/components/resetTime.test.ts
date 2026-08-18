import { describe, expect, it } from "vitest";

import { formatResetAt } from "./resetTime";

const NOW = new Date(2026, 7, 18, 12, 0, 0).getTime(); // Tue Aug 18, 2026, 12:00 local

describe("formatResetAt (E10 dated resets)", () => {
  it("stays time-only when the reset lands later the same local day", () => {
    const resetAt = new Date(2026, 7, 18, 21, 15, 0); // 9:15pm, same local day
    const expectedTime = resetAt.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });

    expect(formatResetAt(resetAt.toISOString(), NOW)).toBe(`at ${expectedTime}`);
  });

  it("says 'tomorrow' when the reset lands the next local day", () => {
    const resetAt = new Date(2026, 7, 19, 3, 0, 0); // Wed Aug 19, next local day
    const expectedTime = resetAt.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });

    expect(formatResetAt(resetAt.toISOString(), NOW)).toBe(`tomorrow at ${expectedTime}`);
  });

  it("includes the weekday and date when the reset is multiple days away (weekly/monthly windows)", () => {
    const resetAt = new Date(2026, 7, 22, 3, 0, 0); // Sat Aug 22, 4 days out
    const expectedTime = resetAt.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    const expectedDate = resetAt.toLocaleDateString(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
    });

    const result = formatResetAt(resetAt.toISOString(), NOW);
    expect(result).toBe(`${expectedDate} at ${expectedTime}`);
    // Guards against silently collapsing into the same-day/tomorrow branches - the dated form
    // must not be hardcoded to a "weekday, month day" order/punctuation, since toLocaleDateString
    // renders that differently per locale (e.g. "Sat, Aug 22" in en-US vs "Sat 22 Aug" in en-GB).
    expect(result).not.toMatch(/^(at |tomorrow at )/);
  });

  it("rolls a reset into next month to the weekday/date form too (falls through same-day/tomorrow checks)", () => {
    const resetAt = new Date(2026, 8, 1, 4, 0, 0); // Sep 1, 2026 - crosses into next month
    const expectedTime = resetAt.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    const expectedDate = resetAt.toLocaleDateString(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
    });

    expect(formatResetAt(resetAt.toISOString(), NOW)).toBe(`${expectedDate} at ${expectedTime}`);
  });

  it("returns null for absent, undefined, or unparsable reset_at", () => {
    expect(formatResetAt(null, NOW)).toBeNull();
    expect(formatResetAt(undefined, NOW)).toBeNull();
    expect(formatResetAt("not-a-real-date", NOW)).toBeNull();
  });

  it("defaults `now` to the real current time when omitted", () => {
    const soon = new Date(Date.now() + 60_000); // 1 minute from now, almost certainly same local day
    const expectedTime = soon.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    expect(formatResetAt(soon.toISOString())).toBe(`at ${expectedTime}`);
  });
});
