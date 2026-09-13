import { cleanup, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import UsageMeter, { meterLevel } from "./UsageMeter";

afterEach(cleanup);

/// The filled part of the track, which only exists once there is something to fill it with.
function fill(bar: HTMLElement): HTMLElement | null {
  return bar.firstElementChild as HTMLElement | null;
}

describe("usage meter", () => {
  it("is a real progressbar that keeps the number next to the bar", () => {
    render(() => <UsageMeter label="Weekly Quota" pct={62} />);
    const bar = screen.getByRole("progressbar", { name: "Weekly Quota" });
    expect(bar).toHaveAttribute("aria-valuemin", "0");
    expect(bar).toHaveAttribute("aria-valuemax", "100");
    expect(bar).toHaveAttribute("aria-valuenow", "62");
    expect(bar).toHaveAttribute("aria-valuetext", "62% used");
    expect(screen.getByText("62%")).toBeInTheDocument();
    expect(fill(bar)?.style.width).toBe("62%");
  });

  it("draws an empty but present track at 0 percent", () => {
    render(() => <UsageMeter label="Model Quota" pct={0} />);
    const bar = screen.getByRole("progressbar", { name: "Model Quota" });
    expect(bar).toHaveAttribute("aria-valuenow", "0");
    expect(bar).toHaveAttribute("aria-valuetext", "0% used");
    expect(screen.getByText("0%")).toBeInTheDocument();
    // The track is drawn even with nothing in it, so an unused quota still looks like a quota.
    expect(bar.className).toMatch(/\bbg-line\b/);
    expect(bar.style.background).toBe("");
    expect(fill(bar)).toBeNull();
  });

  it("reads as no data rather than zero when the probe read no percentage", () => {
    render(() => <UsageMeter label="Weekly Quota" pct={null} />);
    const bar = screen.getByRole("progressbar", { name: "Weekly Quota" });
    // An indeterminate progressbar carries no aria-valuenow at all; zero would be a claim.
    expect(bar).not.toHaveAttribute("aria-valuenow");
    expect(bar).toHaveAttribute("aria-valuetext", "Weekly Quota: no data");
    expect(screen.getByText("no data")).toBeInTheDocument();
    expect(screen.queryByText("0%")).not.toBeInTheDocument();
    // Hatched, not solid: unreadable and unused cannot share a track.
    expect(bar.style.background).toMatch(/repeating-linear-gradient/);
    expect(bar.className).not.toMatch(/\bbg-line\b/);
  });

  it("fills the whole track and says at limit at 100 percent", () => {
    render(() => <UsageMeter label="5-Hour Quota" pct={100} />);
    const bar = screen.getByRole("progressbar", { name: "5-Hour Quota" });
    expect(bar).toHaveAttribute("aria-valuenow", "100");
    expect(bar).toHaveAttribute("aria-valuetext", "100% used, at limit");
    expect(fill(bar)?.style.width).toBe("100%");
    // The word carries the warning for anyone who cannot separate the hues.
    expect(screen.getByText("at limit")).toBeInTheDocument();
    expect(screen.getByText("100%")).toBeInTheDocument();
  });

  it("keeps the label, the state word, the bar and the number on one row", () => {
    render(() => <UsageMeter label="Weekly" name="Weekly Quota" pct={88} />);
    const row = screen.getByRole("progressbar", { name: "Weekly Quota" }).parentElement!;
    expect(row).toHaveTextContent("Weekly");
    expect(row).toHaveTextContent("tight");
    expect(row).toHaveTextContent("88%");
    // A short fixed track, not a full-width rule: four of these stack into a column.
    expect(screen.getByRole("progressbar", { name: "Weekly Quota" }).className).toMatch(/\bw-12\b/);
  });

  it("says more to a screen reader than the row has room to draw", () => {
    render(() => <UsageMeter label="Weekly" name="Weekly Quota" pct={0} />);
    expect(screen.getByRole("progressbar", { name: "Weekly Quota" })).toBeInTheDocument();
    expect(screen.getByText("Weekly")).toBeInTheDocument();
    expect(screen.queryByText("Weekly Quota")).not.toBeInTheDocument();
  });

  it("puts something on the track for a window barely used, so 1 percent is not 0 percent", () => {
    render(() => <UsageMeter label="5-Hour" pct={1} />);
    const one = fill(screen.getByRole("progressbar", { name: "5-Hour" }));
    expect(one?.style.width).toBe("1%");
    // At this track length one percent would round away to nothing without a floor.
    expect(one?.style.minWidth).toBe("3px");
  });

  it("warns in words as well as colour once a window is tight", () => {
    render(() => <UsageMeter label="Weekly Quota" pct={88} />);
    expect(screen.getByText("tight")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuetext", "88% used, tight");
    expect(meterLevel(74)).toBe("steady");
    expect(meterLevel(75)).toBe("tight");
    expect(meterLevel(95)).toBe("at-limit");
    expect(meterLevel(undefined)).toBe("unknown");
    expect(meterLevel(Number.NaN)).toBe("unknown");
  });
});
