import { describe, expect, it } from "vitest";
import {
  computeLineDiff as computeLineDiffRaw,
  computeRevertChange,
  type DiffHunk,
  type LineDiffResult,
} from "./lineDiff";

/// Narrows `computeLineDiffRaw`'s `LineDiffOutcome` union down to the plain `LineDiffResult` shape
/// every test below except the two too-large ones expects - those two call `computeLineDiffRaw`
/// directly instead.
function computeLineDiff(base: string | null, current: string): LineDiffResult {
  const result = computeLineDiffRaw(base, current);
  if ("kind" in result) {
    throw new Error("expected an ok diff result, got too-large");
  }
  return result;
}

function makeDoc(text: string) {
  const lines = text.split("\n");
  const lineOffsets: Array<{ from: number; to: number; length: number }> = [];
  let offset = 0;
  for (let i = 0; i < lines.length; i++) {
    const len = lines[i].length;
    lineOffsets.push({
      from: offset,
      to: offset + len,
      length: len,
    });
    offset += len + 1; // +1 for '\n'
  }

  return {
    text,
    lines: lines.length,
    length: text.length,
    line(n: number) {
      if (n < 1 || n > lineOffsets.length) {
        throw new Error(`Line ${n} out of bounds`);
      }
      return lineOffsets[n - 1];
    },
  };
}

function applyRevert(text: string, hunk: DiffHunk): string {
  const doc = makeDoc(text);
  const change = computeRevertChange(hunk, doc);
  return text.slice(0, change.from) + change.insert + text.slice(change.to);
}

describe("lineDiff", () => {
  it("returns empty result when baseContent is null", () => {
    const res = computeLineDiff(null, "some code\nhere\n");
    expect(res.hunks).toEqual([]);
    expect(res.markers.size).toBe(0);
  });

  it("returns empty result when baseContent equals currentContent", () => {
    const text = "function hello() {\n  return 42;\n}\n";
    const res = computeLineDiff(text, text);
    expect(res.hunks).toEqual([]);
    expect(res.markers.size).toBe(0);
  });

  it("detects added lines in the middle of a file", () => {
    const base = "line 1\nline 2\nline 3\n";
    const current = "line 1\nline 1.5\nline 2\nline 3\n";
    const res = computeLineDiff(base, current);

    expect(res.hunks.length).toBe(1);
    const hunk = res.hunks[0];
    expect(hunk.type).toBe("added");
    expect(hunk.currentStartLine).toBe(2);
    expect(hunk.currentLineCount).toBe(1);
    expect(hunk.currentLines).toEqual(["line 1.5"]);

    expect(res.markers.size).toBe(1);
    expect(res.markers.get(2)?.type).toBe("added");

    // Revert restores base
    expect(applyRevert(current, hunk)).toBe(base);
  });

  it("detects added lines at EOF", () => {
    const base = "line 1\nline 2";
    const current = "line 1\nline 2\nline 3\nline 4";
    const res = computeLineDiff(base, current);

    expect(res.hunks.length).toBe(1);
    const hunk = res.hunks[0];
    expect(hunk.type).toBe("added");
    expect(hunk.currentStartLine).toBe(3);
    expect(hunk.currentLineCount).toBe(2);

    expect(res.markers.get(3)?.type).toBe("added");
    expect(res.markers.get(4)?.type).toBe("added");

    expect(applyRevert(current, hunk)).toBe(base);
  });

  it("detects added lines at BOF", () => {
    const base = "line 2\nline 3\n";
    const current = "line 0\nline 1\nline 2\nline 3\n";
    const res = computeLineDiff(base, current);

    expect(res.hunks.length).toBe(1);
    const hunk = res.hunks[0];
    expect(hunk.type).toBe("added");
    expect(hunk.currentStartLine).toBe(1);
    expect(hunk.currentLineCount).toBe(2);

    expect(applyRevert(current, hunk)).toBe(base);
  });

  it("detects modified lines in the middle", () => {
    const base = "function add(a, b) {\n  return a - b;\n}\n";
    const current = "function add(a, b) {\n  return a + b;\n}\n";
    const res = computeLineDiff(base, current);

    expect(res.hunks.length).toBe(1);
    const hunk = res.hunks[0];
    expect(hunk.type).toBe("modified");
    expect(hunk.currentStartLine).toBe(2);
    expect(hunk.currentLineCount).toBe(1);
    expect(hunk.originalLines).toEqual(["  return a - b;"]);
    expect(hunk.currentLines).toEqual(["  return a + b;"]);

    expect(res.markers.get(2)?.type).toBe("modified");

    expect(applyRevert(current, hunk)).toBe(base);
  });

  it("detects multi-line replacement (modified with different line count)", () => {
    const base = "header\nold single line\nfooter\n";
    const current = "header\nnew line 1\nnew line 2\nnew line 3\nfooter\n";
    const res = computeLineDiff(base, current);

    expect(res.hunks.length).toBe(1);
    const hunk = res.hunks[0];
    expect(hunk.type).toBe("modified");
    expect(hunk.currentStartLine).toBe(2);
    expect(hunk.currentLineCount).toBe(3);
    expect(hunk.originalLines).toEqual(["old single line"]);
    expect(hunk.currentLines).toEqual(["new line 1", "new line 2", "new line 3"]);

    expect(res.markers.get(2)?.type).toBe("modified");
    expect(res.markers.get(3)?.type).toBe("modified");
    expect(res.markers.get(4)?.type).toBe("modified");

    expect(applyRevert(current, hunk)).toBe(base);
  });

  it("detects removed lines in the middle", () => {
    const base = "line 1\nremove me\nline 3\n";
    const current = "line 1\nline 3\n";
    const res = computeLineDiff(base, current);

    expect(res.hunks.length).toBe(1);
    const hunk = res.hunks[0];
    expect(hunk.type).toBe("removed");
    expect(hunk.currentStartLine).toBe(2);
    expect(hunk.currentLineCount).toBe(0);
    expect(hunk.originalLines).toEqual(["remove me"]);

    expect(res.markers.get(2)?.type).toBe("removed");
    expect(res.markers.get(2)?.isRemovedIndicator).toBe(true);

    expect(applyRevert(current, hunk)).toBe(base);
  });

  it("detects removed lines at EOF", () => {
    const base = "line 1\nline 2\nremove at end\n";
    const current = "line 1\nline 2\n";
    const res = computeLineDiff(base, current);

    expect(res.hunks.length).toBe(1);
    const hunk = res.hunks[0];
    expect(hunk.type).toBe("removed");

    expect(applyRevert(current, hunk)).toBe(base);
  });

  it("handles multiple separate hunks (added, modified, removed)", () => {
    const base = [
      "line 1",
      "line 2",
      "line 3",
      "line 4",
      "line 5",
      "line 6",
      "line 7",
    ].join("\n");

    const current = [
      "line 1",
      "line 1.5", // added
      "line 2",
      "line 3",
      "modified 4", // modified
      "line 5",
      // line 6 removed
      "line 7",
    ].join("\n");

    const res = computeLineDiff(base, current);
    expect(res.hunks.length).toBe(3);

    expect(res.hunks[0].type).toBe("added");
    expect(res.hunks[1].type).toBe("modified");
    expect(res.hunks[2].type).toBe("removed");

    // Reverting all hunks in reverse order restores base
    let text = current;
    for (let i = res.hunks.length - 1; i >= 0; i--) {
      // Recompute diff against base to get adjusted hunk positions
      const subDiff = computeLineDiff(base, text);
      text = applyRevert(text, subDiff.hunks[i]);
    }
    expect(text).toBe(base);
  });

  it("diffs a 3000-line document with sweeping changes under 200ms and returns hunks", () => {
    const lineCount = 3000;
    const baseLines: string[] = [];
    const currentLines: string[] = [];
    for (let i = 0; i < lineCount; i++) {
      baseLines.push(`const value${i} = ${i};`);
      // Every other line is rewritten - a large, realistic "sweeping edit" rather than a
      // maximally-disjoint rewrite (which would drive the Myers edit distance past the
      // MAX_EDIT_DISTANCE cap on its own and legitimately return too-large - see the next test).
      currentLines.push(i % 2 === 0 ? `const value${i} = ${i};` : `const value${i} = ${i} + 1;`);
    }
    const base = baseLines.join("\n");
    const current = currentLines.join("\n");

    const start = performance.now();
    const result = computeLineDiffRaw(base, current);
    const elapsedMs = performance.now() - start;

    expect(elapsedMs).toBeLessThan(200);
    if ("kind" in result) {
      throw new Error("expected an ok diff result, got too-large");
    }
    expect(result.hunks.length).toBeGreaterThan(0);
    expect(result.hunks.every((h) => h.type === "modified")).toBe(true);
  });

  it("returns too-large (and no markers) for a diff over the total-line cap", () => {
    const base = Array.from({ length: 11000 }, (_, i) => `base line ${i}`).join("\n");
    const current = Array.from({ length: 11000 }, (_, i) => `current line ${i}`).join("\n");

    const result = computeLineDiffRaw(base, current);

    expect(result).toEqual({ kind: "too-large" });
  });
});
