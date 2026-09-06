import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";

import Select from "./Select";

afterEach(() => {
  cleanup();
});

describe("Select custom dropdown", () => {
  it("opens listbox on trigger click and selects an option on click", async () => {
    const [value, setValue] = createSignal("claude-code");
    render(() => (
      <Select
        label="Default agent"
        value={value()}
        options={[
          { value: "claude-code", label: "claude-code" },
          { value: "codex", label: "codex" },
          { value: "antigravity", label: "antigravity" },
        ]}
        onChange={setValue}
      />
    ));

    const trigger = screen.getByRole("combobox", { name: "Default agent" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    const listbox = await screen.findByRole("listbox");
    expect(listbox).toBeInTheDocument();

    const codexOption = screen.getByRole("option", { name: "codex" });
    fireEvent.click(codexOption);

    expect(value()).toBe("codex");
    await waitFor(() => {
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
      expect(trigger).toHaveAttribute("aria-expanded", "false");
    });
  });

  it("navigates and selects options via keyboard (ArrowDown, Enter)", async () => {
    const [value, setValue] = createSignal("claude-code");
    render(() => (
      <Select
        label="Default agent"
        value={value()}
        options={[
          { value: "claude-code", label: "claude-code" },
          { value: "codex", label: "codex" },
          { value: "antigravity", label: "antigravity" },
        ]}
        onChange={setValue}
      />
    ));

    const trigger = screen.getByRole("combobox", { name: "Default agent" });

    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    expect(await screen.findByRole("listbox")).toBeInTheDocument();

    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    fireEvent.keyDown(trigger, { key: "Enter" });

    expect(value()).toBe("codex");
    await waitFor(() => expect(screen.queryByRole("listbox")).not.toBeInTheDocument());
  });

  it("closes on Escape key without changing selection", async () => {
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

    const trigger = screen.getByRole("combobox", { name: "Default agent" });
    fireEvent.click(trigger);
    expect(await screen.findByRole("listbox")).toBeInTheDocument();

    fireEvent.keyDown(trigger, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("listbox")).not.toBeInTheDocument());
    expect(value()).toBe("claude-code");
  });

  it("renders compact size variant with ariaLabel when label is omitted", () => {
    render(() => (
      <Select
        size="sm"
        ariaLabel="Layout mode"
        value="auto"
        options={[
          { value: "auto", label: "auto" },
          { value: "focused", label: "focused" },
          { value: "split", label: "split" },
          { value: "grid", label: "grid" },
        ]}
        onChange={() => undefined}
      />
    ));

    const trigger = screen.getByRole("combobox", { name: "Layout mode" });
    expect(trigger).toBeInTheDocument();
    expect(trigger).toHaveTextContent("auto");
  });

  it("portals the menu above ancestor stacking contexts and right-aligns it to the trigger", async () => {
    render(() => (
      <Select
        size="sm"
        align="right"
        ariaLabel="Layout mode"
        value="auto"
        options={[
          { value: "auto", label: "auto" },
          { value: "grid", label: "grid" },
        ]}
        onChange={() => undefined}
      />
    ));

    const trigger = screen.getByRole("combobox", { name: "Layout mode" });
    trigger.getBoundingClientRect = () => ({
      x: 700,
      y: 20,
      top: 20,
      left: 700,
      right: 800,
      bottom: 44,
      width: 100,
      height: 24,
      toJSON: () => undefined,
    });
    fireEvent.click(trigger);
    const listbox = await screen.findByRole("listbox");
    expect(trigger.parentElement).not.toContainElement(listbox);
    expect(listbox).toHaveClass("fixed", "z-[100]");
    expect(listbox).toHaveStyle({ top: "48px", right: `${window.innerWidth - 800}px` });
  });

  it("renders frameless variant without standard border/surface box", () => {
    render(() => (
      <Select
        size="sm"
        variant="frameless"
        ariaLabel="Layout mode"
        value="auto"
        options={[
          { value: "auto", label: "auto" },
          { value: "grid", label: "grid" },
        ]}
        onChange={() => undefined}
      />
    ));

    const trigger = screen.getByRole("combobox", { name: "Layout mode" });
    expect(trigger).toHaveClass("border-none");
    expect(trigger).toHaveClass("bg-transparent");
  });
});
