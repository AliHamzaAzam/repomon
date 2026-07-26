import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";

import ColorField from "./ColorField";

afterEach(() => {
  cleanup();
});

describe("ColorField", () => {
  it("calls onChange with the preset name when a swatch button is activated", () => {
    const [value, setValue] = createSignal("cyan");
    render(() => <ColorField label="Accent" value={value()} onChange={setValue} />);

    const cyan = screen.getByRole("button", { name: "cyan" });
    expect(cyan).toHaveAttribute("aria-pressed", "true");

    const violet = screen.getByRole("button", { name: "violet" });
    expect(violet).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(violet);
    expect(value()).toBe("violet");
  });

  it("calls onChange with mono when the mono swatch is activated", () => {
    const [value, setValue] = createSignal("cyan");
    render(() => <ColorField label="Accent" value={value()} onChange={setValue} />);

    fireEvent.click(screen.getByRole("button", { name: "mono" }));
    expect(value()).toBe("mono");
  });

  it("calls onChange with typed text from the custom accent input", () => {
    const [value, setValue] = createSignal("cyan");
    render(() => <ColorField label="Accent" value={value()} onChange={setValue} />);

    const custom = screen.getByLabelText("Custom accent");
    fireEvent.input(custom, { target: { value: "#ff00aa" } });
    expect(value()).toBe("#ff00aa");
  });
});
