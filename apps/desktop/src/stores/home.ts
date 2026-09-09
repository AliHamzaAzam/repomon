import type { Lane } from "../bindings";
import { agentStateIn, agentStateReason, isUrgentState, laneState } from "./fleet";

export interface NeedsYouRow {
  lane: Lane;
  question: string | null;
}

function activityMs(lane: Lane): number {
  const ms = Date.parse(lane.last_activity_at);
  return Number.isNaN(ms) ? 0 : ms;
}

function byNewestActivity(a: Lane, b: Lane): number {
  return activityMs(b) - activityMs(a);
}

/// The dialog or question text for a lane's most urgent agent, straight from the daemon's status
/// reason - the same source `laneIndicatorTitle`'s tooltip reads, never invented here.
function needsYouQuestion(lane: Lane): string | null {
  const state = laneState(lane);
  if (state === null) return null;
  const agent = lane.agent_sessions.find((session) => agentStateIn(lane, session) === state);
  return agent ? agentStateReason(agent) : null;
}

/// Lanes needing the operator, any kind, newest activity first - the home screen's strips above
/// the hairline rule.
export function needsInputLanes(lanes: Lane[]): NeedsYouRow[] {
  return lanes
    .filter((lane) => {
      const state = laneState(lane);
      return state !== null && isUrgentState(state);
    })
    .sort(byNewestActivity)
    .map((lane) => ({ lane, question: needsYouQuestion(lane) }));
}

/// Every other lane, newest activity first - the plain one-line strips under the rule.
export function recentLanes(lanes: Lane[]): Lane[] {
  const urgent = new Set(needsInputLanes(lanes).map((row) => row.lane.id));
  return lanes.filter((lane) => !urgent.has(lane.id)).sort(byNewestActivity);
}

/// Lowercase, hyphenated, ascii-only, capped so a long task description makes a reasonable branch
/// segment. Never empty: a title with no latin/digit characters still gets a name.
export function slugify(text: string): string {
  const slug = text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/g, "");
  return slug || "task";
}

/// A branch name derived from the compose text, unique against the repo's existing lane
/// branches: the bare slug, or the slug with the smallest free `-n` suffix.
export function uniqueBranchName(headline: string, existingBranches: Iterable<string>): string {
  const base = slugify(headline);
  const taken = new Set(existingBranches);
  if (!taken.has(base)) return base;
  for (let n = 2; ; n += 1) {
    const candidate = `${base}-${n}`;
    if (!taken.has(candidate)) return candidate;
  }
}
