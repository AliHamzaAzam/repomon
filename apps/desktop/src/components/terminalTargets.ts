export interface PaneTarget {
  laneId: number;
  repoId?: number;
  repoName?: string;
  laneName?: string;
  branch?: string | null;
  window: string;
  label: string;
  shell: boolean;
  /// Durable transcript identity, used only by history/transcript views when available.
  sessionId: string | null;
  /// UI action identity; managed transcript-less sessions use their tmux window.
  targetId: string | null;
  agent?: string | null;
  /// True for a pane in the repomind home's controller lane. The pane picker groups these under
  /// "Repomind" rather than under a project, mirroring the sidebar's pinned row.
  controller?: boolean;
}

/// The eight `--pane-accent-N` tokens in index.css, so the stripes follow the theme's palette.
const PANE_ACCENTS = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => `var(--pane-accent-${n})`);

/// Stable lane color for the fleet-wide workspace. Repo id carries most of the grouping signal;
/// lane id breaks ties so sibling worktrees remain distinguishable without changing on refresh.
export function paneAccent(target: Pick<PaneTarget, "repoId" | "laneId">): string {
  const seed = (target.repoId ?? 0) * 31 + target.laneId * 17;
  return PANE_ACCENTS[Math.abs(seed) % PANE_ACCENTS.length];
}

export function dedupe(targets: PaneTarget[]): PaneTarget[] {
  const seen = new Set<string>();
  return targets.filter((target) => {
    if (seen.has(target.window)) return false;
    seen.add(target.window);
    return true;
  });
}

/// Keeps visible panes in fleet order, replacing only the final slot when necessary to include the
/// selected pane.
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

/// Keeps visible and recent windows mounted, then warms unvisited live windows within capacity.
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

/// Reuses cached per-window object references so Solid’s reference-keyed For retains terminal
/// mounts across polls and label changes.
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
    prev.repoId = target.repoId;
    prev.repoName = target.repoName;
    prev.laneName = target.laneName;
    prev.branch = target.branch;
    prev.label = target.label;
    prev.shell = target.shell;
    prev.sessionId = target.sessionId;
    prev.targetId = target.targetId;
    prev.agent = target.agent;
    prev.controller = target.controller;
    return prev;
  });
  for (const window of [...cache.keys()]) {
    if (!live.has(window)) cache.delete(window);
  }
  return next;
}
