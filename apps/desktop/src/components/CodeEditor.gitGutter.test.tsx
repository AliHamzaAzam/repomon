import { cleanup, fireEvent, render, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

import CodeEditor from "./CodeEditor";

const diffBaseCallsMock = vi.hoisted(() => ({ list: [] as Array<{ lane_id: number; path: string }> }));

vi.mock("../ipc/rpc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/rpc")>();
  return {
    ...actual,
    daemonCall: (method: string, params?: unknown) => {
      if (method === "file.diff_base") {
        diffBaseCallsMock.list.push(params as { lane_id: number; path: string });
        return Promise.resolve({ content: "line 1\n", kind: "text" });
      }
      return Promise.resolve({});
    },
  };
});

afterEach(() => {
  cleanup();
  diffBaseCallsMock.list = [];
});

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

  it("refreshes the diff base exactly once when saveVersion changes, not just on mount", async () => {
    const [saveVersion, setSaveVersion] = createSignal(0);

    render(() => (
      <CodeEditor value="line 1\n" path="src/app.ts" laneId={7} saveVersion={saveVersion()} />
    ));

    await waitFor(() => expect(diffBaseCallsMock.list.length).toBeGreaterThan(0));
    const callsAfterMount = diffBaseCallsMock.list.length;

    // Simulates what editor.ts's saveFile does on a successful save, regardless of whether it was
    // triggered by CodeEditor's own Mod-s keymap or FileEditorPanel's rail Save button.
    setSaveVersion(1);

    await waitFor(() => expect(diffBaseCallsMock.list.length).toBe(callsAfterMount + 1));
    expect(diffBaseCallsMock.list[diffBaseCallsMock.list.length - 1]).toEqual({
      lane_id: 7,
      path: "src/app.ts",
    });

    // A no-op re-render (saveVersion unchanged) must not trigger another refresh.
    setSaveVersion(1);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(diffBaseCallsMock.list.length).toBe(callsAfterMount + 1);
  });

  it("clears markers and shows a muted note when the diff is too large", async () => {
    // Over lineDiff's MAX_TOTAL_LINES (20000) cap, so computeLineDiff short-circuits to
    // { kind: "too-large" } without running the Myers search at all.
    const base = Array.from({ length: 11000 }, (_, i) => `base line ${i}`).join("\n");
    const current = Array.from({ length: 11000 }, (_, i) => `current line ${i}`).join("\n");

    const { container, getByRole } = render(() => (
      <CodeEditor value={current} path="src/huge.ts" diffBase={base} />
    ));

    await waitFor(() => {
      expect(getByRole("status")).toHaveTextContent("Diff markers off: change too large");
    });

    expect(container.querySelector(".cm-git-gutter-added")).toBeNull();
    expect(container.querySelector(".cm-git-gutter-modified")).toBeNull();
    expect(container.querySelector(".cm-git-gutter-removed")).toBeNull();
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
