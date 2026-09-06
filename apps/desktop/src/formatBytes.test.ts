import { expect, it } from "vitest";
import { formatBytes } from "./formatBytes";

it("preserves viewer byte units, precision, and absent sizes", () => {
  expect([undefined, null, 0, 1023, 1024, 1536, 1024 * 1024, 1.5 * 1024 * 1024].map(formatBytes)).toEqual([
    "", "", "0 B", "1023 B", "1.0 KB", "1.5 KB", "1.00 MB", "1.50 MB",
  ]);
});
