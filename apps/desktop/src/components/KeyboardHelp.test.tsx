import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import { BINDINGS, isMac, numberedPanelBindings } from "../keymap";
import KeyboardHelp from "./KeyboardHelp";

afterEach(() => {
  cleanup();
});

describe("keyboard reference", () => {
  it("lists every binding - global, editor, terminal, finder, and sidebar alike - so help can never omit one", () => {
    render(() => <KeyboardHelp />);
    for (const binding of BINDINGS) {
      expect(screen.getByText(binding.label)).toBeInTheDocument();
    }
  });

  it("lists the numbered panel chords in numeric order", () => {
    const { container } = render(() => <KeyboardHelp />);
    const text = container.textContent ?? "";
    const positions = numberedPanelBindings().map((binding) => text.indexOf(binding.label));
    expect(positions.every((index) => index >= 0)).toBe(true);
    expect([...positions].sort((a, b) => a - b)).toEqual(positions);
  });

  it("tags a non-global binding with its scope", () => {
    render(() => <KeyboardHelp />);
    const row = screen.getByText("Toggle line comment").closest("div");
    expect(row).not.toBeNull();
    expect(row).toHaveTextContent("Editor");
  });

  it("leaves a global binding without a scope tag", () => {
    render(() => <KeyboardHelp />);
    const row = screen.getByText("Merge lane (asks first)").closest("div");
    expect(row).not.toBeNull();
    expect(row?.textContent).not.toContain("Global");
  });

  it("filters on the search box", () => {
    render(() => <KeyboardHelp />);
    fireEvent.input(screen.getByPlaceholderText("Search shortcuts"), { target: { value: "merge" } });
    expect(screen.getByText("Merge lane (asks first)")).toBeInTheDocument();
    expect(screen.queryByText("Refresh")).not.toBeInTheDocument();
  });

  it("finds a local binding by searching its scope name", () => {
    render(() => <KeyboardHelp />);
    fireEvent.input(screen.getByPlaceholderText("Search shortcuts"), { target: { value: "terminal" } });
    expect(screen.getByText("Leave the terminal, back to the fleet list")).toBeInTheDocument();
    expect(screen.queryByText("Merge lane (asks first)")).not.toBeInTheDocument();
  });
});

describe("KeyboardHelp variant", () => {
  it("shows the print action in the full (default) variant", () => {
    render(() => <KeyboardHelp />);
    expect(screen.getByText("Print cheat sheet")).toBeInTheDocument();
  });

  it("hides the print action and the Ctrl caveat in the compact overlay variant", () => {
    render(() => <KeyboardHelp variant="compact" />);
    expect(screen.queryByText("Print cheat sheet")).not.toBeInTheDocument();
  });

  it("shows the Windows/Linux Ctrl caveat only off macOS, and only in the full variant", () => {
    render(() => <KeyboardHelp />);
    const caveat = screen.queryByText(/mod is Ctrl - the terminal's own control modifier/);
    if (isMac()) {
      expect(caveat).not.toBeInTheDocument();
    } else {
      expect(caveat).toBeInTheDocument();
    }
  });
});

describe("KeyboardHelp activeScope", () => {
  it("highlights rows in the active scope", () => {
    render(() => <KeyboardHelp activeScope="editor" />);
    const editorRow = screen.getByText("Toggle line comment").closest("div");
    const globalRow = screen.getByText("Merge lane (asks first)").closest("div");
    expect(editorRow?.className).toContain("border-signal/50");
    expect(globalRow?.className).not.toContain("border-signal/50");
  });
});
