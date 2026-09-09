/// Keep the foreground heartbeat while backing off disk-backed RPCs in a background window.
/// Push events remain active; returning to the app refreshes immediately.
export function startVisibilityPolling(refresh: () => void, foregroundMs: number) {
  let timer: ReturnType<typeof setInterval> | undefined;
  const foreground = () => typeof document === "undefined"
    || (!document.hidden && document.hasFocus());
  let wasForeground = foreground();
  const schedule = () => {
    if (timer !== undefined) clearInterval(timer);
    timer = setInterval(refresh, foreground() ? foregroundMs : 30_000);
  };
  const changed = () => {
    const active = foreground();
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
