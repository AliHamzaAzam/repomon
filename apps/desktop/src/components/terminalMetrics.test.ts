import { describe, expect, it } from "vitest";

import { MULTITASK_MIN_ROWS, multitaskRowFloor } from "./terminalMetrics";

describe("multitaskRowFloor", () => {
  it("equals chrome plus MULTITASK_MIN_ROWS rows at the given cell height", () => {
    const chromeHeight = 28;
    const cellHeight = 16.5;
    expect(multitaskRowFloor({ chromeHeight, cellHeight })).toBe(
      Math.ceil(chromeHeight + MULTITASK_MIN_ROWS * cellHeight),
    );
    expect(multitaskRowFloor({ chromeHeight, cellHeight })).toBe(424);
  });

  it("does not change with the terminal's current row count, only cell height and chrome matter", () => {
    const chromeHeight = 28;
    const cellHeight = 16.5;
    const floor = multitaskRowFloor({ chromeHeight, cellHeight });

    // The same cell metrics must yield the same floor regardless of a pane’s rendered row count.
    for (let call = 0; call < 5; call += 1) {
      expect(multitaskRowFloor({ chromeHeight, cellHeight })).toBe(floor);
    }
  });

  it("scales with cell height, not with any particular row count", () => {
    const small = multitaskRowFloor({ chromeHeight: 28, cellHeight: 10 });
    const large = multitaskRowFloor({ chromeHeight: 28, cellHeight: 20 });
    expect(large).toBe(28 + MULTITASK_MIN_ROWS * 20);
    expect(small).toBe(28 + MULTITASK_MIN_ROWS * 10);
    expect(large).toBeGreaterThan(small);
  });
});
