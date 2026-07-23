export interface PaneTarget {
  laneId: number;
  window: string;
  label: string;
  shell: boolean;
}

export function dedupe(targets: PaneTarget[]): PaneTarget[] {
  const seen = new Set<string>();
  return targets.filter((target) => {
    if (seen.has(target.window)) return false;
    seen.add(target.window);
    return true;
  });
}

/// Keep visible windows hot and retain the most recently viewed live windows up to `capacity`.
/// Visible windows are ordered first so CSS can place them in the active layout while the
/// remaining panes stay mounted off-layout with their xterm state and byte watches intact.
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
    return prev;
  });
  for (const window of [...cache.keys()]) {
    if (!live.has(window)) cache.delete(window);
  }
  return next;
}
