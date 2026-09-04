import { cleanup, fireEvent, render, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";

import CodeEditor from "./CodeEditor";

afterEach(cleanup);

describe("CodeEditor git gutter", () => {
  it("renders added, modified, and removed markers based on diffBase", async () => {
    const base = "keep 1\nkeep 2\nmod 3\nkeep 4\ndel 5\nkeep 6\n";
    const current = "keep 1\nadd 1.5\nkeep 2\nmod 3 changed\nkeep 4\nkeep 6\n";

    const { container } = render(() => (
      <CodeEditor
        value={current}
        path="src/app.ts"
        diffBase={base}
      />
    ));

    await waitFor(() => {
      const addedMarker = container.querySelector(".cm-git-gutter-added");
      const modifiedMarker = container.querySelector(".cm-git-gutter-modified");
      const removedMarker = container.querySelector(".cm-git-gutter-removed");

      expect(addedMarker).not.toBeNull();
      expect(modifiedMarker).not.toBeNull();
      expect(removedMarker).not.toBeNull();
    });
  });

  it("clicking a marker opens the popover with original lines and reverts hunk on click", async () => {
    const base = "function calculate() {\n  return 100;\n}\n";
    const current = "function calculate() {\n  return 42;\n}\n";

    const [value, setValue] = createSignal(current);

    const { container } = render(() => (
      <CodeEditor
        value={value()}
        path="src/calc.ts"
        diffBase={base}
        onChange={(v) => setValue(v)}
      />
    ));

    await waitFor(() => {
      const modifiedMarker = container.querySelector<HTMLElement>(".cm-git-gutter-modified");
      expect(modifiedMarker).not.toBeNull();
    });

    const modifiedMarker = container.querySelector<HTMLElement>(".cm-git-gutter-modified")!;
    fireEvent.click(modifiedMarker);

    // Popover dialog is visible
    await waitFor(() => {
      const dialog = container.querySelector("[role=dialog]");
      expect(dialog).not.toBeNull();
      expect(dialog?.textContent).toContain("Modified lines");
      expect(dialog?.textContent).toContain("return 100;");
    });

    const buttons = Array.from(container.querySelectorAll("button"));
    const revert = buttons.find((b) => b.textContent?.includes("Revert hunk"));
    expect(revert).toBeDefined();

    fireEvent.click(revert!);

    // Revert restores original line
    await waitFor(() => {
      expect(value()).toBe(base);
    });
  });

  it("closes popover when Escape key is pressed", async () => {
    const base = "line 1\nline 2\n";
    const current = "line 1\nline 2\nline 3\n";

    const { container } = render(() => (
      <CodeEditor
        value={current}
        path="src/test.txt"
        diffBase={base}
      />
    ));

    await waitFor(() => {
      const marker = container.querySelector<HTMLElement>(".cm-git-gutter-added");
      expect(marker).not.toBeNull();
    });

    const marker = container.querySelector<HTMLElement>(".cm-git-gutter-added")!;
    fireEvent.click(marker);

    await waitFor(() => {
      expect(container.querySelector("[role=dialog]")).not.toBeNull();
    });

    fireEvent.keyDown(window, { key: "Escape" });

    await waitFor(() => {
      expect(container.querySelector("[role=dialog]")).toBeNull();
    });
  });

  it("does not render git gutter markers when disableGitGutter is true", async () => {
    const base = "line 1\n";
    const current = "line 1\nline 2\n";

    const { container } = render(() => (
      <CodeEditor
        value={current}
        path="src/large.txt"
        diffBase={base}
        disableGitGutter={true}
      />
    ));

    await waitFor(() => {
      const gutterEl = container.querySelector(".cm-git-diff-gutter");
      expect(gutterEl).toBeNull();
    });
  });
});
