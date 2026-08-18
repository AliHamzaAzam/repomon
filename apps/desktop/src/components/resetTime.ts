/// Local calendar-day difference (`to` minus `from`), ignoring time-of-day. Used to tell a
/// same-day reset from one that lands tomorrow or later, regardless of how many hours away it is.
function localDayDiff(from: Date, to: Date): number {
  const a = new Date(from.getFullYear(), from.getMonth(), from.getDate());
  const b = new Date(to.getFullYear(), to.getMonth(), to.getDate());
  return Math.round((b.getTime() - a.getTime()) / 86_400_000);
}

/// Format a reset_at ISO timestamp as a local reset phrase for E9/E10 quota tooltips, paired with
/// "resets " at the call site (e.g. "resets at 3:00 AM", "resets tomorrow at 3:00 AM", "resets
/// Sat, Aug 22 at 3:00 AM"). Weekly/monthly windows often reset days out, so a bare time misreads
/// as "tonight" (FleetSidebar backlog v0.7.1) - the date must show whenever the reset isn't today.
///
/// Provider precision check (crates/repomon-core/src/agent/usage.rs + agent/text.rs,
/// parse_reset_datetime and friends): every reset_at the daemon emits already carries a real
/// calendar date, for every provider. parse_dated resolves an explicitly stated date ("jun 21 at
/// 7:59pm", "04:00 on 19 jul"); parse_reset_at rolls a bare clock time ("resets 11:59pm") onto
/// today or tomorrow using GRACE_PAST_HOURS; parse_relative_duration ("refreshes in 4h 18m") adds
/// the duration to now. So there is no "time only, no date" case any provider can hand this - the
/// date component of the ISO string is always trustworthy, and no provider-specific fallback is
/// needed here.
///
/// `now` defaults to the real current time but takes an injectable epoch ms so callers (tests)
/// can pin "today" without mocking global Date.
///
/// Returns null when reset_at is absent or unparsable (omit the tooltip clause rather than
/// showing nothing useful).
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
