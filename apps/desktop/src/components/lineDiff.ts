export type DiffChangeType = "added" | "modified" | "removed";

export interface DiffHunk {
  id: string;
  type: DiffChangeType;
  baseStartLine: number; // 1-indexed in base
  baseLineCount: number; // line count in base
  currentStartLine: number; // 1-indexed in current
  currentLineCount: number; // line count in current (0 for removed)
  originalLines: string[]; // lines from base
  currentLines: string[]; // lines from current
}

export interface LineMarker {
  type: DiffChangeType;
  hunk: DiffHunk;
  isRemovedIndicator?: boolean;
}

export interface LineDiffResult {
  hunks: DiffHunk[];
  markers: Map<number, LineMarker>;
}

export interface RevertChange {
  from: number;
  to: number;
  insert: string;
}

interface EditOp {
  kind: "equal" | "delete" | "insert";
  baseIndex?: number;
  currentIndex?: number;
}

/**
 * Computes shortest edit script using the Myers diff algorithm on lines.
 */
function myersDiff(a: string[], b: string[]): EditOp[] {
  const n = a.length;
  const m = b.length;

  if (n === 0 && m === 0) return [];
  if (n === 0) {
    return b.map((_, i) => ({ kind: "insert", currentIndex: i }));
  }
  if (m === 0) {
    return a.map((_, i) => ({ kind: "delete", baseIndex: i }));
  }

  // Fast-path common prefix
  let prefix = 0;
  while (prefix < n && prefix < m && a[prefix] === b[prefix]) {
    prefix++;
  }

  // Fast-path common suffix
  let suffix = 0;
  while (suffix < n - prefix && suffix < m - prefix && a[n - 1 - suffix] === b[m - 1 - suffix]) {
    suffix++;
  }

  const aSlice = a.slice(prefix, n - suffix);
  const bSlice = b.slice(prefix, m - suffix);
  const sliceN = aSlice.length;
  const sliceM = bSlice.length;

  const ops: EditOp[] = [];
  for (let i = 0; i < prefix; i++) {
    ops.push({ kind: "equal", baseIndex: i, currentIndex: i });
  }

  if (sliceN > 0 || sliceM > 0) {
    if (sliceN === 0) {
      for (let i = 0; i < sliceM; i++) {
        ops.push({ kind: "insert", currentIndex: prefix + i });
      }
    } else if (sliceM === 0) {
      for (let i = 0; i < sliceN; i++) {
        ops.push({ kind: "delete", baseIndex: prefix + i });
      }
    } else {
      const max = sliceN + sliceM;
      const vSize = 2 * max + 1;
      const v = new Int32Array(vSize);
      v[max + 1] = 0;
      const trace: Int32Array[] = [];

      let found = false;
      for (let d = 0; d <= max; d++) {
        const vCopy = new Int32Array(v);
        trace.push(vCopy);

        for (let k = -d; k <= d; k += 2) {
          let x: number;
          if (k === -d || (k !== d && v[max + k - 1] < v[max + k + 1])) {
            x = v[max + k + 1];
          } else {
            x = v[max + k - 1] + 1;
          }
          let y = x - k;

          while (x < sliceN && y < sliceM && aSlice[x] === bSlice[y]) {
            x++;
            y++;
          }

          v[max + k] = x;

          if (x >= sliceN && y >= sliceM) {
            found = true;
            break;
          }
        }
        if (found) break;
      }

      // Backtrack
      const middleOps: EditOp[] = [];
      let x = sliceN;
      let y = sliceM;

      for (let d = trace.length - 1; d >= 0; d--) {
        const vD = trace[d];
        const k = x - y;
        let prevK: number;
        if (k === -d || (k !== d && vD[max + k - 1] < vD[max + k + 1])) {
          prevK = k + 1;
        } else {
          prevK = k - 1;
        }

        const prevX = vD[max + prevK];
        const prevY = prevX - prevK;

        while (x > prevX && y > prevY) {
          x--;
          y--;
          middleOps.push({ kind: "equal", baseIndex: prefix + x, currentIndex: prefix + y });
        }

        if (d > 0) {
          if (x === prevX) {
            y--;
            middleOps.push({ kind: "insert", currentIndex: prefix + y });
          } else {
            x--;
            middleOps.push({ kind: "delete", baseIndex: prefix + x });
          }
        }
      }

      middleOps.reverse();
      ops.push(...middleOps);
    }
  }

  for (let i = 0; i < suffix; i++) {
    ops.push({
      kind: "equal",
      baseIndex: n - suffix + i,
      currentIndex: m - suffix + i,
    });
  }

  return ops;
}

/**
 * Computes line-level diff hunks and gutter marker positions comparing `baseContent` to `currentContent`.
 * If `baseContent` is null (file not in HEAD or binary), returns empty result.
 */
export function computeLineDiff(
  baseContent: string | null,
  currentContent: string,
): LineDiffResult {
  if (baseContent === null) {
    return { hunks: [], markers: new Map() };
  }

  const baseLines = baseContent.split("\n");
  const currentLines = currentContent.split("\n");

  if (baseContent === currentContent) {
    return { hunks: [], markers: new Map() };
  }

  const ops = myersDiff(baseLines, currentLines);
  const hunks: DiffHunk[] = [];

  let opIndex = 0;
  let hunkCounter = 0;
  let currentLine = 1; // 1-indexed in current document
  let baseLine = 1; // 1-indexed in base document

  while (opIndex < ops.length) {
    const op = ops[opIndex];

    if (op.kind === "equal") {
      baseLine++;
      currentLine++;
      opIndex++;
      continue;
    }

    const hunkBaseStart = baseLine;
    const hunkCurrentStart = currentLine;
    const deletedLines: string[] = [];
    const insertedLines: string[] = [];

    while (opIndex < ops.length && ops[opIndex].kind !== "equal") {
      const nextOp = ops[opIndex];
      if (nextOp.kind === "delete") {
        deletedLines.push(baseLines[nextOp.baseIndex ?? baseLine - 1]);
        baseLine++;
      } else if (nextOp.kind === "insert") {
        insertedLines.push(currentLines[nextOp.currentIndex ?? currentLine - 1]);
        currentLine++;
      }
      opIndex++;
    }

    let type: DiffChangeType;
    if (deletedLines.length > 0 && insertedLines.length > 0) {
      type = "modified";
    } else if (insertedLines.length > 0) {
      type = "added";
    } else {
      type = "removed";
    }

    hunks.push({
      id: `hunk-${hunkCounter++}`,
      type,
      baseStartLine: hunkBaseStart,
      baseLineCount: deletedLines.length,
      currentStartLine: hunkCurrentStart,
      currentLineCount: insertedLines.length,
      originalLines: deletedLines,
      currentLines: insertedLines,
    });
  }

  // Build markers map
  const markers = new Map<number, LineMarker>();
  const totalCurrentLines = currentLines.length;

  for (const hunk of hunks) {
    if (hunk.type === "added" || hunk.type === "modified") {
      for (let i = 0; i < hunk.currentLineCount; i++) {
        const lineNum = hunk.currentStartLine + i;
        markers.set(lineNum, { type: hunk.type, hunk });
      }
    } else {
      // type === "removed"
      const targetLine =
        hunk.currentStartLine <= totalCurrentLines
          ? Math.max(1, hunk.currentStartLine)
          : Math.max(1, totalCurrentLines);

      markers.set(targetLine, {
        type: "removed",
        hunk,
        isRemovedIndicator: true,
      });
    }
  }

  return { hunks, markers };
}

/**
 * Computes the transaction change needed to revert a diff hunk in CodeMirror.
 */
export function computeRevertChange(
  hunk: DiffHunk,
  doc: {
    lines: number;
    length: number;
    line: (n: number) => { from: number; to: number; length: number };
  },
): RevertChange {
  if (hunk.type === "modified") {
    const startLine = doc.line(Math.min(hunk.currentStartLine, doc.lines));
    const endLine = doc.line(
      Math.min(hunk.currentStartLine + hunk.currentLineCount - 1, doc.lines),
    );
    return {
      from: startLine.from,
      to: endLine.to,
      insert: hunk.originalLines.join("\n"),
    };
  }

  if (hunk.type === "added") {
    // If entire document was added
    if (hunk.currentStartLine === 1 && hunk.currentLineCount >= doc.lines) {
      return {
        from: 0,
        to: doc.length,
        insert: "",
      };
    }

    // If added lines are not at EOF
    if (hunk.currentStartLine + hunk.currentLineCount <= doc.lines) {
      const startLine = doc.line(hunk.currentStartLine);
      const endLine = doc.line(hunk.currentStartLine + hunk.currentLineCount - 1);
      return {
        from: startLine.from,
        to: endLine.to + 1, // include trailing newline
        insert: "",
      };
    }

    // Added lines are at EOF
    if (hunk.currentStartLine > 1) {
      const prevLine = doc.line(hunk.currentStartLine - 1);
      const endLine = doc.line(doc.lines);
      return {
        from: prevLine.to, // include leading newline
        to: endLine.to,
        insert: "",
      };
    }

    return {
      from: 0,
      to: doc.length,
      insert: "",
    };
  }

  // hunk.type === "removed"
  if (doc.length === 0 || (doc.lines <= 1 && doc.line(1).length === 0)) {
    return {
      from: 0,
      to: doc.length,
      insert: hunk.originalLines.join("\n"),
    };
  }

  if (hunk.currentStartLine <= doc.lines) {
    const targetLine = doc.line(hunk.currentStartLine);
    return {
      from: targetLine.from,
      to: targetLine.from,
      insert: hunk.originalLines.join("\n") + "\n",
    };
  }

  // Deletion at EOF
  const lastLine = doc.line(doc.lines);
  return {
    from: lastLine.to,
    to: lastLine.to,
    insert: "\n" + hunk.originalLines.join("\n"),
  };
}
