import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import UsageRangePicker from "./UsageRangePicker";

// Pin today mid-month so past and future date selections are deterministic.
const TODAY = new Date(2026, 8, 5, 12, 0, 0);

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(TODAY);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function open(onApply = vi.fn()) {
  render(() => <UsageRangePicker label="Custom" active={false} onApply={onApply} />);
  fireEvent.click(screen.getByText("Custom"));
  return onApply;
}

describe("UsageRangePicker", () => {
  it("returns focus to Custom after applying or dismissing from any popover control", () => {
    open();
    const trigger = screen.getByRole("button", { name: "Custom" });
    const apply = screen.getByRole("button", { name: "Apply" });
    apply.focus();
    fireEvent.keyDown(apply, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();

    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(trigger).toHaveFocus();
  });

  it("announces the keyboard day within six calendar rows and keeps it in the displayed month", () => {
    open();
    const grid = screen.getByRole("grid", { name: "Days" });
    const activeDay = () => document.getElementById(grid.getAttribute("aria-activedescendant")!);
    expect(screen.getAllByRole("row")).toHaveLength(6);
    expect(activeDay()).toHaveAttribute("aria-label", TODAY.toDateString());

    fireEvent.click(screen.getByRole("button", { name: "Previous month" }));
    grid.focus();
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    expect(activeDay()).toHaveAttribute("aria-label", new Date(2026, 7, 2).toDateString());
    expect(screen.getByText("August 2026")).toBeInTheDocument();
  });

  it("keeps keyboard movement out of future dates just like the disabled day buttons", () => {
    open();
    const grid = screen.getByRole("grid", { name: "Days" });
    for (const key of ["ArrowRight", "ArrowDown", "PageDown"]) {
      fireEvent.keyDown(grid, { key });
      expect(document.getElementById(grid.getAttribute("aria-activedescendant")!))
        .toHaveAttribute("aria-label", TODAY.toDateString());
    }
    expect(screen.getByRole("button", { name: "Next month" })).toBeDisabled();
  });

  it("reopens on today's month with its default range instead of a stale navigation month", () => {
    open();
    fireEvent.click(screen.getByRole("button", { name: "Previous month" }));
    fireEvent.keyDown(screen.getByRole("grid"), { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "Custom" }));
    expect(screen.getByText("September 2026")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next month" })).toBeDisabled();
  });

  it("opens already spanning the last 7 days ending today", () => {
    const onApply = open();
    const expectedFrom = new Date(TODAY);
    expectedFrom.setDate(expectedFrom.getDate() - 6);
    expect(
      screen.getByText(`${expectedFrom.toLocaleDateString()} to ${TODAY.toLocaleDateString()}`),
    ).toBeTruthy();
    // The Apply button reads enabled because a range is already selected on open.
    expect(screen.getByText("Apply")).not.toBeDisabled();
    fireEvent.click(screen.getByText("Apply"));
    expect(onApply).toHaveBeenCalledTimes(1);
    const [from, to] = onApply.mock.calls[0] as [Date, Date];
    expect(from.toDateString()).toBe(expectedFrom.toDateString());
    expect(to.toDateString()).toBe(TODAY.toDateString());
  });

  it("ignores a click on a day after today", () => {
    const onApply = open();
    const tomorrow = new Date(TODAY);
    tomorrow.setDate(tomorrow.getDate() + 1);
    const future = screen.getByLabelText(tomorrow.toDateString());
    expect(future).toBeDisabled();
    expect(future).toHaveAttribute("aria-disabled", "true");

    fireEvent.click(future);
    fireEvent.click(screen.getByText("Apply"));

    // The picker still applies its default last-7-days window, not anything touched by the
    // ignored click.
    expect(onApply).toHaveBeenCalledTimes(1);
    const [from, to] = onApply.mock.calls[0] as [Date, Date];
    const expectedFrom = new Date(TODAY);
    expectedFrom.setDate(expectedFrom.getDate() - 6);
    expect(to.toDateString()).toBe(TODAY.toDateString());
    expect(from.toDateString()).toBe(expectedFrom.toDateString());
  });

  it("still lets a day on or before today be picked", () => {
    const onApply = open();
    const twoDaysAgo = new Date(TODAY);
    twoDaysAgo.setDate(twoDaysAgo.getDate() - 2);
    const past = screen.getByLabelText(twoDaysAgo.toDateString());
    expect(past).not.toBeDisabled();
    fireEvent.click(past);
    fireEvent.click(screen.getByText("Apply"));
    expect(onApply).toHaveBeenCalledTimes(1);
    const [from, to] = onApply.mock.calls[0] as [Date, Date];
    expect(from.getDate()).toBe(3);
    expect(to.getDate()).toBe(3);
  });

  it("disables the next-month arrow while the current month is shown", () => {
    open();
    expect(screen.getByLabelText("Next month")).toBeDisabled();
  });

  it("re-enables the next-month arrow after stepping back a month, and disables it again on return", () => {
    open();
    fireEvent.click(screen.getByLabelText("Previous month"));
    expect(screen.getByLabelText("Next month")).not.toBeDisabled();
    fireEvent.click(screen.getByLabelText("Next month"));
    expect(screen.getByLabelText("Next month")).toBeDisabled();
  });

  it("clamps the 'This month' preset's end to today rather than the month's last day", () => {
    const onApply = open();
    fireEvent.click(screen.getByText("This month"));
    expect(onApply).toHaveBeenCalledTimes(1);
    const [, to, label] = onApply.mock.calls[0] as [Date, Date, string | undefined];
    expect(to.getTime()).toBe(TODAY.getTime());
    expect(to.getDate()).not.toBe(30); // September's last day, which "This month" must not reach.
    expect(label).toBe("This month");
  });
});
