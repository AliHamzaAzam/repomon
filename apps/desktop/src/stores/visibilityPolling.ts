/// A visible but unfocused app uses the background policy too.
export const isForeground = () => typeof document === "undefined"
  || (!document.hidden && document.hasFocus());

/// Keep the foreground heartbeat while backing off disk-backed RPCs in a background window.
/// Returning to the app refreshes immediately, including changes deferred while in background.
export function startVisibilityPolling(refresh: () => void, foregroundMs: number) {
  let timer: ReturnType<typeof setInterval> | undefined;
  let wasForeground = isForeground();
  const schedule = () => {
    if (timer !== undefined) clearInterval(timer);
    timer = setInterval(refresh, isForeground() ? foregroundMs : 30_000);
  };
  const changed = () => {
    const active = isForeground();
    if (active && !wasForeground) refresh();
    wasForeground = active;
    schedule();
  };
  schedule();
  if (typeof window !== "undefined") {
    window.addEventListener("focus", changed);
    window.addEventListener("blur", changed);
    document.addEventListener("visibilitychange", changed);
  }
  return () => {
    if (timer !== undefined) clearInterval(timer);
    if (typeof window !== "undefined") {
      window.removeEventListener("focus", changed);
      window.removeEventListener("blur", changed);
      document.removeEventListener("visibilitychange", changed);
    }
  };
}
