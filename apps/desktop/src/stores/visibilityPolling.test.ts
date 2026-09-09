import { afterEach, describe, expect, it, vi } from "vitest";
import { startVisibilityPolling } from "./visibilityPolling";

afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });

describe("visibility polling", () => {
  it("backs off in the background and refreshes immediately on return", async () => {
    vi.useFakeTimers();
    let focused = true;
    vi.spyOn(document, "hasFocus").mockImplementation(() => focused);
    const refresh = vi.fn();
    const stop = startVisibilityPolling(refresh, 1200);
    try {
      await vi.advanceTimersByTimeAsync(2400);
      expect(refresh).toHaveBeenCalledTimes(2);
      focused = false;
      window.dispatchEvent(new Event("blur"));
      await vi.advanceTimersByTimeAsync(29_999);
      expect(refresh).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(1);
      expect(refresh).toHaveBeenCalledTimes(3);
      focused = true;
      window.dispatchEvent(new Event("focus"));
      expect(refresh).toHaveBeenCalledTimes(4);
      await vi.advanceTimersByTimeAsync(1200);
      expect(refresh).toHaveBeenCalledTimes(5);
    } finally { stop(); }
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(60_000);
    expect(refresh).toHaveBeenCalledTimes(5);
  });
});
