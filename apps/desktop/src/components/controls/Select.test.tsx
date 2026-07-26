import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";

import Select from "./Select";

afterEach(() => {
  cleanup();
});

describe("Select", () => {
  it("calls onChange with the chosen option's value when the user picks it", () => {
    const [value, setValue] = createSignal("claude-code");
    render(() => (
      <Select
        label="Default agent"
        value={value()}
        options={[
          { value: "claude-code", label: "claude-code" },
          { value: "codex", label: "codex" },
        ]}
        onChange={setValue}
      />
    ));

    const control = screen.getByRole("combobox", { name: "Default agent" });
    fireEvent.change(control, { target: { value: "codex" } });
    expect(value()).toBe("codex");
  });
});
