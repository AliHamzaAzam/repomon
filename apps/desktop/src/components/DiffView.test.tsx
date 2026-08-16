import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import DiffView, { parseDiff } from "./DiffView";

afterEach(() => {
  cleanup();
});

// Fixtures below are real `git diff HEAD` output captured from a scratch repo (same approach as
// GitExplorerPanel.test.tsx's parseStatFiles fixtures) rather than hand-typed guesses, since
// git's exact spacing/ordering around renames, binaries, and hunk headings is easy to get wrong
// by hand.

const MODIFY_PATCH = `diff --git a/a.txt b/a.txt
index 83db48f..e0c9b5e 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,4 @@
 line1
-line2
+CHANGED
 line3
+line4
`;

const ADD_PATCH = `diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..954185b
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+new content
+second line
`;

const DELETE_PATCH = `diff --git a/old.txt b/old.txt
deleted file mode 100644
index 75810ac..0000000
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-binarydata
\\ No newline at end of file
`;

const RENAME_WITH_CONTENT_PATCH = `diff --git a/a.txt b/renamed.txt
similarity index 75%
rename from a.txt
rename to renamed.txt
index 83db48f..0c53f61 100644
--- a/a.txt
+++ b/renamed.txt
@@ -1,3 +1,4 @@
 line1
 line2
 line3
+extra
`;

const RENAME_PURE_PATCH = `diff --git a/a.txt b/pure_renamed.txt
similarity index 100%
rename from a.txt
rename to pure_renamed.txt
`;

const BINARY_PATCH = `diff --git a/real.bin b/real.bin
index 91dc5fc..14cc868 100644
Binary files a/real.bin and b/real.bin differ
`;

const GIT_BINARY_PATCH_PATCH = `diff --git a/blob.dat b/blob.dat
index 91dc5fc..14cc868 100644
GIT binary patch
literal 12
zc-Vd^000000000000000000000

literal 8
zc-Vd^0000000000

`;

const MULTI_FILE_MULTI_HUNK_PATCH = `diff --git a/created.txt b/created.txt
new file mode 100644
index 0000000..a0a5f7d
--- /dev/null
+++ b/created.txt
@@ -0,0 +1 @@
+created file content
diff --git a/multi.txt b/multi.txt
index 86bba90..fcf3e55 100644
--- a/multi.txt
+++ b/multi.txt
@@ -1,5 +1,5 @@
 l1
-l2
+L2-CHANGED
 l3
 l4
 l5
@@ -16,5 +16,5 @@ l15
 l16
 l17
 l18
-l19
+L19-CHANGED
 l20
`;

describe("parseDiff", () => {
  it("parses a modified file: one hunk, context/add/remove lines, adds/dels counts", () => {
    const files = parseDiff(MODIFY_PATCH);
    expect(files).toHaveLength(1);
    const [f] = files;
    expect(f.path).toBe("a.txt");
    expect(f.changeType).toBe("modify");
    expect(f.binary).toBe(false);
    expect(f.adds).toBe(2);
    expect(f.dels).toBe(1);
    expect(f.hunks).toHaveLength(1);
    expect(f.hunks[0]).toMatchObject({ oldStart: 1, oldLines: 3, newStart: 1, newLines: 4, heading: "" });
    expect(f.hunks[0].lines).toEqual([
      { type: "context", text: "line1" },
      { type: "remove", text: "line2" },
      { type: "add", text: "CHANGED" },
      { type: "context", text: "line3" },
      { type: "add", text: "line4" },
    ]);
  });

  it("parses a new file (`new file mode` + `--- /dev/null`) as an add", () => {
    const files = parseDiff(ADD_PATCH);
    expect(files).toHaveLength(1);
    expect(files[0]).toMatchObject({ path: "new.txt", changeType: "add", binary: false, adds: 2, dels: 0 });
    expect(files[0].hunks[0]).toMatchObject({ oldStart: 0, oldLines: 0, newStart: 1, newLines: 2 });
  });

  it("parses a deleted file (`deleted file mode` + `+++ /dev/null`), dropping the no-newline marker", () => {
    const files = parseDiff(DELETE_PATCH);
    expect(files).toHaveLength(1);
    expect(files[0]).toMatchObject({ path: "old.txt", changeType: "delete", binary: false, adds: 0, dels: 1 });
    expect(files[0].hunks[0].lines).toEqual([{ type: "remove", text: "binarydata" }]);
  });

  it("parses a rename with content changes: renamedFrom set, hunk still parsed", () => {
    const files = parseDiff(RENAME_WITH_CONTENT_PATCH);
    expect(files).toHaveLength(1);
    expect(files[0]).toMatchObject({ path: "renamed.txt", renamedFrom: "a.txt", changeType: "rename", adds: 1, dels: 0 });
    expect(files[0].hunks).toHaveLength(1);
  });

  it("parses a pure rename (no content change): no hunks, no `---`/`+++` lines at all", () => {
    const files = parseDiff(RENAME_PURE_PATCH);
    expect(files).toHaveLength(1);
    expect(files[0]).toMatchObject({ path: "pure_renamed.txt", renamedFrom: "a.txt", changeType: "rename", hunks: [], adds: 0, dels: 0 });
  });

  it("marks a `Binary files ... differ` file as binary with no hunks", () => {
    const files = parseDiff(BINARY_PATCH);
    expect(files).toHaveLength(1);
    expect(files[0]).toMatchObject({ path: "real.bin", binary: true, changeType: "modify", hunks: [], adds: 0, dels: 0 });
  });

  it("marks a `GIT binary patch` file as binary and skips its base85 blob lines without misparsing them as hunk content", () => {
    const files = parseDiff(GIT_BINARY_PATCH_PATCH);
    expect(files).toHaveLength(1);
    expect(files[0]).toMatchObject({ path: "blob.dat", binary: true, hunks: [] });
  });

  it("parses multiple files each with multiple hunks, including a hunk's trailing section heading", () => {
    const files = parseDiff(MULTI_FILE_MULTI_HUNK_PATCH);
    expect(files).toHaveLength(2);
    expect(files[0]).toMatchObject({ path: "created.txt", changeType: "add", adds: 1 });
    expect(files[1]).toMatchObject({ path: "multi.txt", changeType: "modify", adds: 2, dels: 2 });
    expect(files[1].hunks).toHaveLength(2);
    expect(files[1].hunks[0].heading).toBe("");
    expect(files[1].hunks[1]).toMatchObject({ oldStart: 16, oldLines: 5, newStart: 16, newLines: 5, heading: "l15" });
  });

  it("never throws on a tail truncated mid hunk-line, and keeps whatever partial content it captured", () => {
    // Simulates `cap_chars` cutting the daemon's patch off mid-line (it's a character cap, not
    // hunk-aware) - here cutting `-l19` down to `-l`.
    const cutPoint = MULTI_FILE_MULTI_HUNK_PATCH.indexOf("-l19") + 2;
    const truncated = MULTI_FILE_MULTI_HUNK_PATCH.slice(0, cutPoint);
    expect(() => parseDiff(truncated)).not.toThrow();
    const files = parseDiff(truncated);
    expect(files).toHaveLength(2);
    const multi = files[1];
    expect(multi.hunks).toHaveLength(2);
    const lastLine = multi.hunks[1].lines[multi.hunks[1].lines.length - 1];
    expect(lastLine).toEqual({ type: "remove", text: "l" });
  });

  it("never throws on a tail truncated mid hunk-header, and drops the incomplete hunk entirely", () => {
    const cutPoint = MULTI_FILE_MULTI_HUNK_PATCH.indexOf("@@ -16,5") + 5;
    const truncated = MULTI_FILE_MULTI_HUNK_PATCH.slice(0, cutPoint);
    expect(() => parseDiff(truncated)).not.toThrow();
    const files = parseDiff(truncated);
    expect(files).toHaveLength(2);
    // Only the first (complete) hunk survives; the chopped-off `@@ -16,...` header never matched.
    expect(files[1].hunks).toHaveLength(1);
  });

  it("returns an empty list for empty input", () => {
    expect(parseDiff("")).toEqual([]);
  });

  it("returns an empty list for text with no `diff --git` header at all", () => {
    expect(parseDiff("not a diff\njust some text\n")).toEqual([]);
  });
});

describe("DiffView renderer", () => {
  it("renders a file card per file, collapsed by default with no focusPath", () => {
    render(() => <DiffView patch={MULTI_FILE_MULTI_HUNK_PATCH} />);

    expect(screen.getByText("created.txt")).toBeInTheDocument();
    expect(screen.getByText("multi.txt")).toBeInTheDocument();
    // Collapsed: hunk content isn't in the DOM yet.
    expect(screen.queryByText("l15")).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { expanded: false }).length).toBeGreaterThan(0);
  });

  it("auto-expands and reveals the focusPath file's lines, tinting add/remove rows", () => {
    render(() => <DiffView patch={MODIFY_PATCH} focusPath="a.txt" />);

    const toggle = screen.getByRole("button", { expanded: true });
    expect(toggle).toBeInTheDocument();

    const addLine = screen.getByText("CHANGED").closest("div");
    expect(addLine).toHaveClass("diff-line-add");
    const removeLine = screen.getByText("line2").closest("div");
    expect(removeLine).toHaveClass("diff-line-remove");
  });

  it("expands and collapses a file card on click", () => {
    render(() => <DiffView patch={MODIFY_PATCH} />);

    const toggle = screen.getByRole("button", { name: /a\.txt/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("CHANGED")).not.toBeInTheDocument();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("CHANGED")).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("CHANGED")).not.toBeInTheDocument();
  });

  it("shows a binary placeholder instead of hunk content, with a muted 'binary' badge in the header", () => {
    render(() => <DiffView patch={BINARY_PATCH} focusPath="real.bin" />);

    expect(screen.getByText("binary")).toBeInTheDocument();
    expect(screen.getByText("Binary file, no text diff to show.")).toBeInTheDocument();
  });

  it("shows the truncation banner when truncated is set, and not otherwise", () => {
    const { unmount } = render(() => <DiffView patch={MODIFY_PATCH} truncated />);
    expect(screen.getByRole("status")).toHaveTextContent(`Diff truncated at ${MODIFY_PATCH.length.toLocaleString()} chars`);
    unmount();

    render(() => <DiffView patch={MODIFY_PATCH} truncated={false} />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("shows an empty state for a patch with no files", () => {
    render(() => <DiffView patch="" />);
    expect(screen.getByText("No uncommitted changes to show.")).toBeInTheDocument();
  });

  it("calls onClose when the close button is clicked, and omits it when onClose isn't given", () => {
    let closed = false;
    render(() => <DiffView patch={MODIFY_PATCH} onClose={() => { closed = true; }} />);
    fireEvent.click(screen.getByRole("button", { name: "Close diff" }));
    expect(closed).toBe(true);

    cleanup();
    render(() => <DiffView patch={MODIFY_PATCH} />);
    expect(screen.queryByRole("button", { name: "Close diff" })).not.toBeInTheDocument();
  });
});
