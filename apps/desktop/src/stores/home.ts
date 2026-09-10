import type { Lane } from "../bindings";
import { agentStateIn, agentStateReason, isUrgentState, laneState, type AgentState, type LaneTone } from "./fleet";

export interface NeedsYouRow {
  lane: Lane;
  /// The strip's second line. `null` when the agent has neither a dialog/prompt nor a status
  /// reason to show.
  question: string | null;
  /// Whether `question` is the agent's actual question (`pending_dialog.question` or
  /// `pending_prompt`), which the strip quotes, as opposed to the daemon's `status_reason`
  /// (e.g. "no output for 4m", "pane unchanged for 4m") describing why it's waiting, which
  /// the strip shows plain - quoting a status description would misrepresent it as something
  /// the agent asked.
  isQuestion: boolean;
}

function activityMs(lane: Lane): number {
  const ms = Date.parse(lane.last_activity_at);
  return Number.isNaN(ms) ? 0 : ms;
}

function byNewestActivity(a: Lane, b: Lane): number {
  return activityMs(b) - activityMs(a);
}

/// The strip's second line for a lane's most urgent agent: its actual question when it has
/// one (the parsed dialog's `question`, else the compact `pending_prompt`), falling back to
/// the daemon's status reason - the same source `laneIndicatorTitle`'s tooltip reads - only
/// when there is no question to show.
function needsYouLine(lane: Lane): Pick<NeedsYouRow, "question" | "isQuestion"> {
  const state = laneState(lane);
  if (state === null) return { question: null, isQuestion: false };
  const agent = lane.agent_sessions.find((session) => agentStateIn(lane, session) === state);
  if (!agent) return { question: null, isQuestion: false };
  const asked = agent.pending_dialog?.question ?? agent.pending_prompt;
  if (asked) return { question: asked, isQuestion: true };
  return { question: agentStateReason(agent), isQuestion: false };
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
    .map((lane) => ({ lane, ...needsYouLine(lane) }));
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

/// A strip's title: the lane's transcript headline when the daemon has one cached, else the
/// branch name, else the worktree name for a detached lane. `headline` is `undefined` while the
/// `lane.headline` RPC is still in flight, in which case the fallback shows rather than a blank.
export function laneTitle(lane: Lane, headline: string | null | undefined): string {
  return headline || lane.worktree.branch || lane.worktree.name;
}

/// The strip board's compact age column: no "ago" suffix, so it fits the mock's narrow fixed
/// column. Distinct from `formatRelativeTime`, which is prose-length for use elsewhere.
export function formatStripAge(iso: string, now: number = Date.now()): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const diffSecs = Math.max(0, Math.round((now - then) / 1000));
  const MINUTE = 60;
  const HOUR = MINUTE * 60;
  const DAY = HOUR * 24;
  if (diffSecs < MINUTE) return "now";
  if (diffSecs < HOUR) return `${Math.floor(diffSecs / MINUTE)}m`;
  if (diffSecs < DAY) return `${Math.floor(diffSecs / HOUR)}h`;
  return `${Math.floor(diffSecs / DAY)}d`;
}

/// Which SVG mark and tone a strip's status column shows, from the same state vocabulary the
/// sidebar reads. A lane with no agents at all reads as idle, same as one that is merely quiet.
export function stripMark(state: AgentState | null): { icon: "bolt" | "play" | "stop" | "check"; tone: LaneTone } {
  if (state !== null && isUrgentState(state)) return { icon: "bolt", tone: "attention" };
  if (state === "running" || state === "inferred") return { icon: "play", tone: "signal" };
  if (state === "exited") return { icon: "check", tone: "muted" };
  return { icon: "stop", tone: "muted" };
}
