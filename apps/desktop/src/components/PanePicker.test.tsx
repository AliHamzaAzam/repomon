import { fireEvent, render, screen, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";

import type { PaneTarget } from "./terminalTargets";
import PanePicker from "./PanePicker";

const panes: PaneTarget[] = [
  { laneId: 1, window: "one", label: "Agent one", shell: false, sessionId: "s1", targetId: "s1" },
  { laneId: 2, window: "two", label: "Agent two", shell: false, sessionId: "s2", targetId: "s2" },
];

describe("PanePicker", () => {
  it("portals the multitasking picker above the terminal stacking context and changes selection", () => {
    const onChange = vi.fn();
    const { container, unmount } = render(() => (
      <PanePicker multitasking available={panes} selected={[panes[0]]} onChange={onChange} />
    ));

    fireEvent.click(screen.getByRole("button", { name: "Configure multitasking panes" }));
    const dialog = screen.getByRole("dialog", { name: "Choose multitasking panes" });
    expect(dialog).toHaveClass("fixed");
    expect(dialog).toHaveClass("z-[100]");
    // A Portal must escape the toolbar's backdrop-filter stacking context entirely.
    expect(container.contains(dialog)).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "Show Agent two" }));
    expect(onChange).toHaveBeenCalledWith(["one", "two"]);
    unmount();
  });

  it("reorders selected lane panes without exposing multitasking footprint controls", () => {
    const onChange = vi.fn();
    const { unmount } = render(() => (
      <PanePicker available={panes} selected={panes} onChange={onChange} />
    ));

    fireEvent.click(screen.getByRole("button", { name: "Configure lane panes" }));
    const dialog = screen.getByRole("dialog", { name: "Choose lane panes" });
    fireEvent.click(screen.getByRole("button", { name: "Move Agent two earlier" }));

    expect(onChange).toHaveBeenCalledWith(["two", "one"]);
    expect(within(dialog).queryByText("Footprint")).toBeNull();
    expect(within(dialog).queryByRole("button", { name: "Agent one: two columns wide" })).toBeNull();
    unmount();
  });
});
