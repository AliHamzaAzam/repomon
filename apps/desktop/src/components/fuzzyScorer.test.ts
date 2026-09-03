import { describe, expect, it } from "vitest";

import { filterAndRankPaths, scorePath } from "./fuzzyScorer";

describe("scorePath", () => {
  it("scores empty query with baseline score and empty indices", () => {
    const res = scorePath("src/main.rs", "");
    expect(res).not.toBeNull();
    expect(res!.indices).toEqual([]);
    expect(res!.score).toBeGreaterThan(0);
  });

  it("prioritizes exact basename match over prefix or substring matches", () => {
    const exact = scorePath("crates/daemon/src/files.rs", "files.rs");
    const prefix = scorePath("crates/daemon/src/files_test.rs", "files");
    const substring = scorePath("crates/daemon/src/my_files.rs", "files");

    expect(exact).not.toBeNull();
    expect(prefix).not.toBeNull();
    expect(substring).not.toBeNull();

    expect(exact!.score).toBeGreaterThan(prefix!.score);
    expect(prefix!.score).toBeGreaterThan(substring!.score);
  });

  it("prioritizes basename prefix over path matches", () => {
    const basenamePrefix = scorePath("crates/core/src/model.rs", "mod");
    const pathOnlyMatch = scorePath("modules/storage/src/index.ts", "mod");

    expect(basenamePrefix).not.toBeNull();
    expect(pathOnlyMatch).not.toBeNull();
    expect(basenamePrefix!.score).toBeGreaterThan(pathOnlyMatch!.score);
  });

  it("matches path segment prefixes", () => {
    const res = scorePath("crates/repomon-daemon/src/files.rs", "rep/src/fil");
    expect(res).not.toBeNull();
    expect(res!.indices.length).toBeGreaterThan(0);
  });

  it("performs subsequence matching with boundary bonuses", () => {
    const res = scorePath("src/components/FileFinder.tsx", "ffinder");
    expect(res).not.toBeNull();
    expect(res!.indices.length).toBe("ffinder".length);
  });

  it("returns null when characters are missing", () => {
    expect(scorePath("src/main.rs", "nomatch")).toBeNull();
    expect(scorePath("src/main.rs", "smr")).not.toBeNull(); // s-m-r in order
    expect(scorePath("src/main.rs", "smz")).toBeNull(); // 'z' not in path
  });

  it("highlights correct matched indices", () => {
    const res = scorePath("src/main.rs", "main");
    expect(res).not.toBeNull();
    const chars = res!.indices.map((idx) => "src/main.rs"[idx]).join("");
    expect(chars.toLowerCase()).toBe("main");
  });
});

describe("filterAndRankPaths", () => {
  const paths = [
    "crates/repomon-core/src/model.rs",
    "crates/repomon-daemon/src/files.rs",
    "crates/repomon-daemon/src/file_watch.rs",
    "apps/desktop/src/components/FileFinder.tsx",
    "apps/desktop/src/components/FileEditorPanel.tsx",
    "README.md",
  ];

  it("filters and ranks accurately", () => {
    const results = filterAndRankPaths(paths, "file");
    expect(results.length).toBeGreaterThan(0);
    expect(results[0].path).toMatch(/file/i);
  });

  it("caps results to limit", () => {
    const manyPaths = Array.from({ length: 100 }, (_, i) => `file_${i}.txt`);
    const results = filterAndRankPaths(manyPaths, "file", 50);
    expect(results.length).toBe(50);
  });

  it("maintains stable deterministic ordering", () => {
    const res1 = filterAndRankPaths(paths, "src");
    const res2 = filterAndRankPaths(paths, "src");
    expect(res1.map((r) => r.path)).toEqual(res2.map((r) => r.path));
  });
});
