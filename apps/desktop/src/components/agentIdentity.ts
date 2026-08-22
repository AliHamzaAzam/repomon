import type { AgentSession } from "../bindings";

/// Stable UI identity for a surfaced agent. Managed agents are keyed by their tmux window so
/// transcript-less backends (notably Codex) can still be renamed and manually reordered.
/// External agents have no window, so their durable transcript id remains the identity.
export function agentSessionTargetId(session: AgentSession): string | null {
  if (session.tmux_window) return `win:${session.tmux_window}`;
  return session.session_id;
}

/// Order signature used by every reorder surface to notice an authoritative backend update.
/// Deliberately excludes sessions without an actionable identity; those rows cannot participate in
/// either drag surface and should not make an optimistic order look stale.
export function agentSessionOrderKey(sessions: readonly AgentSession[]): string {
  return sessions
    .map(agentSessionTargetId)
    .filter((id): id is string => id !== null)
    .join("\u0000");
}
