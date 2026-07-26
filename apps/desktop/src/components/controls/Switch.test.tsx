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

  it("does not fire onChange when disabled", () => {
    let calls = 0;
    render(() => <Switch label="Locked" checked={false} disabled onChange={() => { calls += 1; }} />);

    const control = screen.getByRole("switch", { name: "Locked" });
    expect(control).toBeDisabled();
    fireEvent.click(control);
    expect(calls).toBe(0);
  });
});
