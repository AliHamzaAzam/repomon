/// Render an ISO timestamp as a short relative label ("3m ago", "2h ago", "5d ago"). Repomon has
/// no existing relative-time helper (checked FleetSidebar, agentLabel, ansi — the closest is
/// FleetSidebar's inline `age_secs < 60 ? "just now" : ...` for usage pills), so this is a small,
/// dependency-free one for GitExplorerPanel's commit history rows rather than pulling in a date
/// library for one label.
const MINUTE = 60;
const HOUR = MINUTE * 60;
const DAY = HOUR * 24;
const WEEK = DAY * 7;
const MONTH = DAY * 30;
const YEAR = DAY * 365;

export function formatRelativeTime(iso: string, now: number = Date.now()): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return iso;
  const diffSecs = Math.max(0, Math.round((now - then) / 1000));

  if (diffSecs < 45) return "just now";
  if (diffSecs < HOUR) return `${Math.round(diffSecs / MINUTE)}m ago`;
  if (diffSecs < DAY) return `${Math.round(diffSecs / HOUR)}h ago`;
  if (diffSecs < WEEK) return `${Math.round(diffSecs / DAY)}d ago`;
  if (diffSecs < MONTH) return `${Math.round(diffSecs / WEEK)}w ago`;
  if (diffSecs < YEAR) return `${Math.round(diffSecs / MONTH)}mo ago`;
  return `${Math.round(diffSecs / YEAR)}y ago`;
}
