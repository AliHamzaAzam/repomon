import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import ModelPanel from "./ModelPanel";
import type { CatalogModel } from "../../bindings";

afterEach(cleanup);

function anchor(): HTMLElement {
  const button = document.createElement("button");
  document.body.appendChild(button);
  return button;
}

const models: CatalogModel[] = [
  { id: "opus", label: "Claude Opus", current: false },
  { id: "sonnet", label: "Claude Sonnet", current: true },
  { id: "haiku", label: "Claude Haiku", current: false },
];

describe("ModelPanel keyboard navigation", () => {
  it("moves the highlighted row with arrows and selects it on Enter", () => {
    const onSelect = vi.fn();
    render(() => <ModelPanel models={models} modelCommand="/model" kind="claude-code" anchor={anchor()} onSelect={onSelect} onClose={vi.fn()} />);
    fireEvent.keyDown(window, { key: "ArrowDown" });
    fireEvent.keyDown(window, { key: "ArrowDown" });
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith("haiku");
  });

  it("wraps from the last row back to the first on ArrowDown", () => {
    const onSelect = vi.fn();
    render(() => <ModelPanel models={models} modelCommand="/model" kind="claude-code" anchor={anchor()} onSelect={onSelect} onClose={vi.fn()} />);
    fireEvent.keyDown(window, { key: "ArrowUp" });
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith("haiku");
  });

  it("Escape closes regardless of selectability", () => {
    const onClose = vi.fn();
    render(() => <ModelPanel models={models} modelCommand={null} kind="codex" anchor={anchor()} onSelect={vi.fn()} onClose={onClose} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("a digit key selects the matching model and does not fire for an unconfirmed one-shot kind", () => {
    const onSelect = vi.fn();
    render(() => <ModelPanel models={models} modelCommand="/model" kind="claude-code" anchor={anchor()} onSelect={onSelect} onClose={vi.fn()} />);
    fireEvent.keyDown(window, { key: "1" });
    expect(onSelect).toHaveBeenCalledWith("opus");
    onSelect.mockClear();
    cleanup();
    render(() => <ModelPanel models={models} modelCommand={null} kind="codex" anchor={anchor()} onSelect={onSelect} onClose={vi.fn()} />);
    fireEvent.keyDown(window, { key: "1" });
    expect(onSelect).not.toHaveBeenCalled();
  });
});

describe("ModelPanel disclosure", () => {
  const many: CatalogModel[] = Array.from({ length: 8 }, (_, i) => ({ id: `m${i}`, label: `Model ${i}`, current: i === 0 }));

  it("shows a 'More models' row only past the visible threshold, and Enter on it expands the list", () => {
    render(() => <ModelPanel models={many} modelCommand="/model" kind="claude-code" anchor={anchor()} onSelect={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText("More models")).toBeInTheDocument();
    expect(screen.getAllByRole("menuitemradio")).toHaveLength(5);
    for (let i = 0; i < 5; i++) fireEvent.keyDown(window, { key: "ArrowDown" });
    fireEvent.keyDown(window, { key: "Enter" });
    expect(screen.queryByText("More models")).not.toBeInTheDocument();
    expect(screen.getAllByRole("menuitemradio")).toHaveLength(8);
  });

  it("never shows 'More models' when every model already fits", () => {
    render(() => <ModelPanel models={models} modelCommand="/model" kind="claude-code" anchor={anchor()} onSelect={vi.fn()} onClose={vi.fn()} />);
    expect(screen.queryByText("More models")).not.toBeInTheDocument();
  });
});

describe("ModelPanel unconfirmed one-shot state", () => {
  it("lists models informationally, without selection, when model_command is null", () => {
    const onSelect = vi.fn();
    render(() => <ModelPanel models={models} modelCommand={null} kind="codex" anchor={anchor()} onSelect={onSelect} onClose={vi.fn()} />);
    expect(screen.getByText(/can't switch codex's model/i)).toBeInTheDocument();
    expect(screen.queryByRole("menuitemradio")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Claude Opus"));
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("still shows the current model with a check in the informational state", () => {
    render(() => <ModelPanel models={models} modelCommand={null} kind="codex" anchor={anchor()} onSelect={vi.fn()} onClose={vi.fn()} />);
    const sonnetRow = screen.getByText("Claude Sonnet").closest("div");
    expect(sonnetRow?.querySelector("svg")).toBeTruthy();
  });
});
