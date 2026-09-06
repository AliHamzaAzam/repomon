/// Local calendar-day difference (`to` minus `from`), ignoring time-of-day. Used to tell a
/// same-day reset from one that lands tomorrow or later, regardless of how many hours away it is.
function localDayDiff(from: Date, to: Date): number {
  const a = new Date(from.getFullYear(), from.getMonth(), from.getDate());
  const b = new Date(to.getFullYear(), to.getMonth(), to.getDate());
  return Math.round((b.getTime() - a.getTime()) / 86_400_000);
}

/// Formats a valid reset instant in local time with a date when needed, returning null for missing
/// or invalid input.
export function formatResetAt(
  resetAt: string | null | undefined,
  now: number = Date.now(),
): string | null {
  if (!resetAt) return null;
  try {
    const d = new Date(resetAt);
    if (isNaN(d.getTime())) return null;
    const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    const dayDiff = localDayDiff(new Date(now), d);
    if (dayDiff === 0) return `at ${time}`;
    if (dayDiff === 1) return `tomorrow at ${time}`;
    const date = d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
    return `${date} at ${time}`;
  } catch {
    return null;
  }
}
