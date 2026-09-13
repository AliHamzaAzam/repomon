import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";

import Switch from "./Switch";

afterEach(() => {
  cleanup();
});

describe("Switch", () => {
  it("is reachable as a role=switch control and calls onChange with the inverted value", () => {
    const [checked, setChecked] = createSignal(false);
    render(() => <Switch label="Enable notifications" checked={checked()} onChange={setChecked} />);

    const control = screen.getByRole("switch", { name: "Enable notifications" });
    expect(control).toHaveAttribute("aria-checked", "false");

    fireEvent.click(control);
    expect(checked()).toBe(true);
  });

  it("toggles back off on a second activation", () => {
    const [checked, setChecked] = createSignal(true);
    render(() => <Switch label="Play sound" checked={checked()} onChange={setChecked} />);

    const control = screen.getByRole("switch", { name: "Play sound" });
    expect(control).toHaveAttribute("aria-checked", "true");

    fireEvent.click(control);
    expect(checked()).toBe(false);
  });

  it("hands the knob's position to the sheet through aria-checked", () => {
    const [checked, setChecked] = createSignal(false);
    const { container } = render(() => <Switch label="Coalesce bursts" checked={checked()} onChange={setChecked} />);

    const control = screen.getByRole("switch", { name: "Coalesce bursts" });
    const knob = container.querySelector(".switch-knob");
    expect(control).toHaveClass("switch-track");
    expect(knob).not.toBeNull();
    // The selector that moves the knob is `.switch-track[aria-checked="true"] .switch-knob`, so
    // the knob has to be a descendant of the control that carries the state, and the state has to
    // be a real attribute rather than a class the markup sets separately.
    expect(knob?.parentElement).toBe(control);
    expect(control).toHaveAttribute("aria-checked", "false");

    fireEvent.click(control);
    expect(control).toHaveAttribute("aria-checked", "true");
  });

  it("does not fire onChange when disabled", () => {
    let calls = 0;
    render(() => <Switch label="Locked" checked={false} disabled onChange={() => { calls += 1; }} />);

    const control = screen.getByRole("switch", { name: "Locked" });
    expect(control).toBeDisabled();
    fireEvent.click(control);
    expect(calls).toBe(0);
  });
});
