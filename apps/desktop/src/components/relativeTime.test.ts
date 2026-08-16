import { describe, expect, it } from "vitest";

import { formatRelativeTime } from "./relativeTime";

const NOW = Date.parse("2026-08-16T12:00:00Z");

describe("formatRelativeTime", () => {
  it("collapses sub-45s to just now", () => {
    expect(formatRelativeTime("2026-08-16T11:59:20Z", NOW)).toBe("just now");
  });

  it("renders minutes, hours, days, weeks, months, and years", () => {
    expect(formatRelativeTime("2026-08-16T11:55:00Z", NOW)).toBe("5m ago");
    expect(formatRelativeTime("2026-08-16T09:00:00Z", NOW)).toBe("3h ago");
    expect(formatRelativeTime("2026-08-14T12:00:00Z", NOW)).toBe("2d ago");
    expect(formatRelativeTime("2026-08-02T12:00:00Z", NOW)).toBe("2w ago");
    expect(formatRelativeTime("2026-05-16T12:00:00Z", NOW)).toBe("3mo ago");
    expect(formatRelativeTime("2024-08-16T12:00:00Z", NOW)).toBe("2y ago");
  });

  it("clamps a future timestamp to just now instead of going negative", () => {
    expect(formatRelativeTime("2026-08-16T12:05:00Z", NOW)).toBe("just now");
  });

  it("falls back to the raw string when it cannot parse", () => {
    expect(formatRelativeTime("not-a-date", NOW)).toBe("not-a-date");
  });
});
