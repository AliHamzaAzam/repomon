import type { AgentSession } from "../bindings";

/// Keys managed agents by backend window and external agents by transcript ID so names and ordering
/// survive refreshes.
export function agentSessionTargetId(session: AgentSession): string | null {
  if (session.tmux_window) return `win:${session.tmux_window}`;
  return session.session_id;
}

/// Identifies authoritative ordering changes using only sessions eligible for reordering.
export function agentSessionOrderKey(sessions: readonly AgentSession[]): string {
  return sessions
    .map(agentSessionTargetId)
    .filter((id): id is string => id !== null)
    .join("\u0000");
}
