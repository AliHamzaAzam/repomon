export interface PaneTarget {
  laneId: number;
  window: string;
  label: string;
  shell: boolean;
  sessionId: string | null;
  agent?: string | null;
}

export function dedupe(targets: PaneTarget[]): PaneTarget[] {
  const seen = new Set<string>();
  return targets.filter((target) => {
    if (seen.has(target.window)) return false;
    seen.add(target.window);
    return true;
  });
}

/// Pick the panes to show in a multi-pane layout (split = 2, grid = 6), in a stable order.
///
/// The order is always the incoming (fleet) order — never "selected lane first". Selection has
/// to move focus and highlight only: deriving the arrangement from the selection made every grid
/// cell jump around each time the user clicked an agent, which made split/grid view unusable.
/// The one concession: when the newly selected pane is not visible at all (beyond the cap), it
/// swaps into the last slot so focusing an agent never silently shows you panes that exclude it.
export function stableVisibleTargets(
  all: PaneTarget[],
  activeWindow: string | null,
  layout: "split" | "grid",
): PaneTarget[] {
  const cap = layout === "split" ? 2 : 6;
  const stable = all.slice(0, cap);
  const active = all.find((target) => target.window === activeWindow);
  if (active && !stable.some((target) => target.window === active.window)) {
    stable[cap - 1] = active;
  }
  return stable;
}

/// Keep visible windows hot, retain recently viewed windows, then proactively warm unvisited
/// live windows up to `capacity`. Visible windows are ordered first so CSS can place them in the
/// active layout while the remaining panes stay mounted off-layout with their xterm state and
/// byte watches intact.
export function warmTargetWindows(
  previous: string[],
  visible: PaneTarget[],
  available: PaneTarget[],
  capacity = 6,
): string[] {
  const live = new Set(available.map((target) => target.window));
  const next: string[] = [];
  const append = (window: string) => {
    if (live.has(window) && !next.includes(window)) next.push(window);
  };
  visible.forEach((target) => append(target.window));
  previous.forEach(append);
  available.forEach((target) => append(target.window));
  return next.slice(0, Math.max(0, capacity));
}

/// Reconcile a freshly-built target list against a per-window cache, reusing the previous
/// object reference for any window that still exists. Solid's `<For>` is reference-keyed, so
/// returning stable references keeps each terminal pane mounted across the 1s fleet poll
/// instead of tearing it down and rebuilding it (which would restart the byte watch every
/// second). Mutable fields are copied onto the retained object so a window's pane survives a
/// label change; windows that disappear are pruned from the cache.
export function stabilizeTargets(
  cache: Map<string, PaneTarget>,
  fresh: PaneTarget[],
): PaneTarget[] {
  const live = new Set<string>();
  const next = fresh.map((target) => {
    live.add(target.window);
    const prev = cache.get(target.window);
    if (!prev) {
      cache.set(target.window, target);
      return target;
    }
    prev.laneId = target.laneId;
    prev.label = target.label;
    prev.shell = target.shell;
    prev.sessionId = target.sessionId;
    prev.agent = target.agent;
    return prev;
  });
  for (const window of [...cache.keys()]) {
    if (!live.has(window)) cache.delete(window);
  }
  return next;
}
