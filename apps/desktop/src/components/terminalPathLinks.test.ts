import { describe, expect, it } from "vitest";
import { findPathRefs } from "./terminalPathLinks";

describe("findPathRefs matcher", () => {
  it("matches path with line and column: src/foo.rs:12:4", () => {
    const line = "error: failed at src/foo.rs:12:4 in compile";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      raw: "src/foo.rs:12:4",
      path: "src/foo.rs",
      line: 12,
      column: 4,
      startIndex: 17,
      endIndex: 32,
    });
  });

  it("matches path with line only: src/foo.rs:12", () => {
    const line = "warning: src/foo.rs:12 unused variable";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      raw: "src/foo.rs:12",
      path: "src/foo.rs",
      line: 12,
      column: undefined,
      startIndex: 9,
      endIndex: 22,
    });
  });

  it("matches relative path with slash: apps/x/y.tsx", () => {
    const line = "Created new file at apps/x/y.tsx for layout";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      raw: "apps/x/y.tsx",
      path: "apps/x/y.tsx",
      line: undefined,
      column: undefined,
      startIndex: 20,
      endIndex: 32,
    });
  });

  it("matches dot-relative path: ./relative/path.ext", () => {
    const line = "read config from ./relative/path.ext successfully";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      raw: "./relative/path.ext",
      path: "./relative/path.ext",
      line: undefined,
      column: undefined,
      startIndex: 17,
      endIndex: 36,
    });
  });

  it("matches absolute Unix path under worktree", () => {
    const line = "Opening /private/tmp/worktree/src/main.rs:45:10";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      raw: "/private/tmp/worktree/src/main.rs:45:10",
      path: "/private/tmp/worktree/src/main.rs",
      line: 45,
      column: 10,
      startIndex: 8,
      endIndex: 47,
    });
  });

  it("matches Rust diagnostic arrow form: --> src/main.rs:10:5", () => {
    const line = "  --> src/main.rs:10:5";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      raw: "src/main.rs:10:5",
      path: "src/main.rs",
      line: 10,
      column: 5,
      startIndex: 6,
      endIndex: 22,
    });
  });

  it("matches TS diagnostic form: at src/app.ts:5:3", () => {
    const line = "    at src/app.ts:5:3";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      raw: "src/app.ts:5:3",
      path: "src/app.ts",
      line: 5,
      column: 3,
      startIndex: 7,
      endIndex: 21,
    });
  });

  it("matches TS diagnostic with function name: at Object.render (src/app.ts:5:3)", () => {
    const line = "    at Object.render (src/app.ts:5:3)";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      raw: "src/app.ts:5:3",
      path: "src/app.ts",
      line: 5,
      column: 3,
      startIndex: 22,
      endIndex: 36,
    });
  });

  it("strips trailing punctuation: periods, commas, colons, parens, quotes", () => {
    const line1 = "See src/foo.rs:12:4.";
    expect(findPathRefs(line1, { platform: "darwin" })[0].raw).toBe("src/foo.rs:12:4");

    const line2 = "Look at (src/foo.rs:12:4), next";
    expect(findPathRefs(line2, { platform: "darwin" })[0].raw).toBe("src/foo.rs:12:4");

    const line3 = "Found in 'apps/x/y.tsx', finished";
    expect(findPathRefs(line3, { platform: "darwin" })[0].raw).toBe("apps/x/y.tsx");

    const line4 = "--> src/main.rs:10:5:";
    const match4 = findPathRefs(line4, { platform: "darwin" })[0];
    expect(match4.raw).toBe("src/main.rs:10:5");
    expect(match4.line).toBe(10);
    expect(match4.column).toBe(5);
  });

  it("ignores Windows drive paths on macOS", () => {
    const line = "Error in C:\\projects\\repomon\\src\\main.rs:10:2";
    const macMatches = findPathRefs(line, { platform: "darwin" });
    expect(macMatches).toHaveLength(0);

    const winMatches = findPathRefs(line, { platform: "win32" });
    expect(winMatches).toHaveLength(1);
    expect(winMatches[0].path).toBe("C:\\projects\\repomon\\src\\main.rs");
    expect(winMatches[0].line).toBe(10);
    expect(winMatches[0].column).toBe(2);
  });

  it("ignores URLs containing paths", () => {
    const line = "Visit https://github.com/org/repo/blob/main/src/foo.rs:10 or http://localhost:3000/src/bar.ts";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(0);
  });

  it("finds multiple references in one line", () => {
    const line = "Moved src/old.ts to src/new.ts:20:1";
    const matches = findPathRefs(line, { platform: "darwin" });
    expect(matches).toHaveLength(2);
    expect(matches[0].path).toBe("src/old.ts");
    expect(matches[1].path).toBe("src/new.ts");
    expect(matches[1].line).toBe(20);
    expect(matches[1].column).toBe(1);
  });
});
