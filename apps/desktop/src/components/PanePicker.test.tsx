import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, expect, it, vi } from "vitest";

import type { PaneSpan } from "../stores/workspace";
import type { PaneTarget } from "./terminalTargets";
import PanePicker from "./PanePicker";

const panes: PaneTarget[] = [
  { laneId: 1, window: "one", label: "Agent one", shell: false, sessionId: "s1", targetId: "s1" },
  { laneId: 2, window: "two", label: "Agent two", shell: false, sessionId: "s2", targetId: "s2" },
];

describe("PanePicker", () => {
  it("enters the portaled picker and returns focus on Escape from a footprint control", async () => {
    const { unmount } = render(() => (
      <PanePicker multitasking available={panes} selected={panes} onChange={vi.fn()} />
    ));
    const trigger = screen.getByRole("button", { name: "Configure multitasking panes" });
    trigger.focus();
    fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Choose multitasking panes" });
    await waitFor(() => expect(dialog).toHaveFocus());
    const footprint = within(dialog).getByRole("button", { name: "Agent one: toggle double height" });
    footprint.focus();
    fireEvent.keyDown(footprint, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(trigger).toHaveFocus();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    unmount();
  });

  it("exposes width and tall state while preserving both dimensions on changes", () => {
    const { unmount } = render(() => {
      const [spans, setSpans] = createSignal<Record<string, PaneSpan>>({ one: { columns: 1, rows: 1 } });
      return <PanePicker multitasking available={panes} selected={panes} spans={spans()}
        onChange={vi.fn()} onSpanChange={(_, span) => setSpans({ one: span })} />;
    });
    fireEvent.click(screen.getByRole("button", { name: "Configure multitasking panes" }));
    const narrow = screen.getByRole("button", { name: "Agent one: one column wide" });
    const wide = screen.getByRole("button", { name: "Agent one: two columns wide" });
    const tall = screen.getByRole("button", { name: "Agent one: toggle double height" });
    expect(narrow).toHaveAttribute("aria-pressed", "true");
    expect(wide).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(tall);
    fireEvent.click(wide);
    expect(wide).toHaveAttribute("aria-pressed", "true");
    expect(narrow).toHaveAttribute("aria-pressed", "false");
    expect(tall).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(tall);
    expect(tall).toHaveAttribute("aria-pressed", "false");
    expect(wide).toHaveAttribute("aria-pressed", "true");
    unmount();
  });

  it("disables hiding the last visible pane and enables it once another pane is selected", () => {
    const { unmount } = render(() => {
      const [selected, setSelected] = createSignal([panes[0]]);
      return <PanePicker available={panes} selected={selected()}
        onChange={(windows) => setSelected(panes.filter((pane) => windows.includes(pane.window)))} />;
    });
    fireEvent.click(screen.getByRole("button", { name: "Configure lane panes" }));
    expect(screen.getByRole("button", { name: "Hide Agent one" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Show Agent two" }));
    expect(screen.getByRole("button", { name: "Hide Agent one" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Hide Agent one" }));
    expect(screen.getByRole("button", { name: "Show Agent one" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Hide Agent two" })).toBeDisabled();
    unmount();
  });

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

  it("leaves the list unlabelled while every pane belongs to the fleet", () => {
    const { unmount } = render(() => (
      <PanePicker multitasking available={panes} selected={panes} onChange={vi.fn()} />
    ));

    fireEvent.click(screen.getByRole("button", { name: "Configure multitasking panes" }));
    const dialog = screen.getByRole("dialog", { name: "Choose multitasking panes" });
    expect(within(dialog).queryByText("Repomind")).toBeNull();
    expect(within(dialog).queryByText("Fleet")).toBeNull();
    unmount();
  });

  it("files a controller pane under Repomind rather than under a hidden repo group", () => {
    const controller: PaneTarget = {
      laneId: 90,
      window: "repomind-1",
      label: "Repomind primary",
      shell: false,
      sessionId: "c1",
      targetId: "c1",
      controller: true,
    };
    const { unmount } = render(() => (
      <PanePicker multitasking available={[...panes, controller]} selected={panes} onChange={vi.fn()} />
    ));

    fireEvent.click(screen.getByRole("button", { name: "Configure multitasking panes" }));
    const dialog = screen.getByRole("dialog", { name: "Choose multitasking panes" });
    expect(within(dialog).getByText("Repomind")).toBeInTheDocument();
    expect(within(dialog).getByText("Fleet")).toBeInTheDocument();

    expect(within(dialog).getByRole("button", { name: "Show Repomind primary" })).toBeInTheDocument();

    // The controller's group comes first, so it is found without scrolling past the fleet.
    const labels = within(dialog).getAllByText(/^(Repomind|Fleet)$/).map((node) => node.textContent);
    expect(labels).toEqual(["Repomind", "Fleet"]);
    unmount();
  });
});
