import { cleanup, render, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";

import CodeEditor from "./CodeEditor";

afterEach(cleanup);

describe("CodeEditor Replace All with zero-width regex", () => {
  it("terminates on regex a* and replaces only non-empty matches", async () => {
    const text = "baac da";
    const [doc, setDoc] = createSignal(text);
    const [replaceReq, setReplaceReq] = createSignal<{
      query: string;
      replacement: string;
      all: boolean;
      regex: boolean;
      caseSensitive: boolean;
      token: number;
    } | null>(null);

    render(() => (
      <CodeEditor
        value={doc()}
        path="test.txt"
        onChange={(val) => setDoc(val)}
        replaceRequest={replaceReq()}
      />
    ));

    // Request replace all with regex "a*" replaced by "X"
    setReplaceReq({
      query: "a*",
      replacement: "X",
      all: true,
      regex: true,
      caseSensitive: true,
      token: 1,
    });

    await waitFor(() => {
      // In "baac da", "aa" at index 1 is replaced by "X", and "a" at index 6 is replaced by "X"
      // Result must be "bXc dX", terminating cleanly without hanging on empty matches
      expect(doc()).toBe("bXc dX");
    });
  });
});
