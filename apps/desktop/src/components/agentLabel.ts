import type { AgentSession } from "../bindings";

/// The tmux window a managed agent runs in encodes its spawn-order slot: `lane-7` is the lane's
/// first slot, `lane-7-3` its third (see `TmuxRuntime::window_name` / `lane_id_of`). Returns
/// `null` for anything that is not a lane window — GUI-opened shells use their own id scheme.
export function slotOf(window: string | null | undefined): number | null {
  if (!window) return null;
  const match = /^lane-\d+(?:-(\d+))?$/.exec(window);
  if (!match) return null;
  return match[1] ? Number(match[1]) : 1;
}

/// The display label for one agent session.
///
/// The number comes from the window slot, never from the session's position in
/// `lane.agent_sessions`: the daemon builds that array newest-transcript-first, so it re-sorts
/// itself every time the lane's agents take turns, while their windows stay put. Numbering off the
/// array index made two agents swap labels between two stationary panes on every turn.
export function agentLabel(session: AgentSession): string {
  return session.custom_label ?? session.title ?? `${session.agent} ${slotOf(session.tmux_window) ?? 1}`;
}

/// The session that names a lane: the one in the lowest window slot, i.e. the lane's first-spawned
/// managed agent. Falls back to the first session when none has a lane window (external and
/// inferred sessions carry no window), so a lane still gets a title.
export function primarySession(sessions: AgentSession[]): AgentSession | undefined {
  let best: AgentSession | undefined;
  let bestSlot = Infinity;
  for (const session of sessions) {
    const slot = slotOf(session.tmux_window);
    if (slot !== null && slot < bestSlot) {
      best = session;
      bestSlot = slot;
    }
  }
  return best ?? sessions[0];
}
