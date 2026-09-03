import { describe, expect, it } from "vitest";

import type { FileSearchHit } from "../bindings";
import { groupSearchHits } from "./projectSearch";

describe("groupSearchHits", () => {
  it("groups empty hits array into empty groups", () => {
    expect(groupSearchHits([])).toEqual([]);
  });

  it("groups multiple hits across different files preserving order", () => {
    const hits: FileSearchHit[] = [
      { path: "src/main.rs", line: 10, column: 5, preview: "fn main() {" },
      { path: "src/lib.rs", line: 2, column: 1, preview: "pub mod foo;" },
      { path: "src/main.rs", line: 25, column: 8, preview: "println!();" },
      { path: "README.md", line: 1, column: 1, preview: "# Title" },
    ];

    const groups = groupSearchHits(hits);
    expect(groups.length).toBe(3);

    expect(groups[0].path).toBe("src/main.rs");
    expect(groups[0].hits.length).toBe(2);
    expect(groups[0].hits[0].line).toBe(10);
    expect(groups[0].hits[1].line).toBe(25);

    expect(groups[1].path).toBe("src/lib.rs");
    expect(groups[1].hits.length).toBe(1);

    expect(groups[2].path).toBe("README.md");
    expect(groups[2].hits.length).toBe(1);
  });
});
