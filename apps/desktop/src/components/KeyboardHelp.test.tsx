import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import { BINDINGS } from "../keymap";
import KeyboardHelp from "./KeyboardHelp";

afterEach(() => {
  cleanup();
});

describe("keyboard reference", () => {
  it("lists every binding so help can never omit one", () => {
    render(() => <KeyboardHelp />);
    for (const binding of BINDINGS) {
      expect(screen.getByText(binding.label)).toBeInTheDocument();
    }
  });

  it("filters on the search box", () => {
    render(() => <KeyboardHelp />);
    fireEvent.input(screen.getByPlaceholderText("Search shortcuts"), { target: { value: "merge" } });
    expect(screen.getByText("Merge lane")).toBeInTheDocument();
    expect(screen.queryByText("Refresh")).not.toBeInTheDocument();
  });

  it("lists the two shortcuts that live outside BINDINGS", () => {
    render(() => <KeyboardHelp />);
    expect(screen.getByText("Open the control center")).toBeInTheDocument();
    expect(screen.getByText("Leave the terminal")).toBeInTheDocument();
  });

  it("filters the control center row on search", () => {
    render(() => <KeyboardHelp />);
    fireEvent.input(screen.getByPlaceholderText("Search shortcuts"), { target: { value: "control center" } });
    expect(screen.getByText("Open the control center")).toBeInTheDocument();
    expect(screen.queryByText("Leave the terminal")).not.toBeInTheDocument();
    expect(screen.queryByText("Merge lane")).not.toBeInTheDocument();
  });

  it("filters the leave-terminal row on search", () => {
    render(() => <KeyboardHelp />);
    fireEvent.input(screen.getByPlaceholderText("Search shortcuts"), { target: { value: "leave" } });
    expect(screen.getByText("Leave the terminal")).toBeInTheDocument();
    expect(screen.queryByText("Open the control center")).not.toBeInTheDocument();
  });
});
