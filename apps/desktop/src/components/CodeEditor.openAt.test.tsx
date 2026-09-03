import { cleanup, render, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";

import CodeEditor from "./CodeEditor";

afterEach(cleanup);

describe("CodeEditor openAtTarget", () => {
  it("selects the right range and position for line and column", async () => {
    const text = "first line\nsecond line with search term\nthird line\n";
    const [cursor, setCursor] = createSignal(0);
    const [target, setTarget] = createSignal<{ line: number; column: number; token: number } | null>(null);

    render(() => (
      <CodeEditor
        value={text}
        path="src/main.rs"
        onCursorActivity={(c) => setCursor(c)}
        openAtTarget={target()}
      />
    ));

    // Initially cursor is at 0
    expect(cursor()).toBe(0);

    // Target line 2, column 8 (start of "line" in "second line with search term")
    // "first line\n" is 11 chars (indices 0..10).
    // line 2 starts at index 11.
    // column 8 (1-based) is offset 7 from line start -> index 18.
    setTarget({ line: 2, column: 8, token: 1 });

    await waitFor(() => {
      expect(cursor()).toBe(18);
    });

    // Target line 3, column 1
    // line 3 starts at index 11 + 29 = 40.
    // column 1 (1-based) is offset 0 -> index 40.
    setTarget({ line: 3, column: 1, token: 2 });

    await waitFor(() => {
      expect(cursor()).toBe(40);
    });
  });
});
