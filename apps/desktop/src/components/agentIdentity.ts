import type { AgentSession } from "../bindings";

/// Stable UI identity for a surfaced agent. Managed agents are keyed by their tmux window so
/// transcript-less backends (notably Codex) can still be renamed and manually reordered.
/// External agents have no window, so their durable transcript id remains the identity.
export function agentSessionTargetId(session: AgentSession): string | null {
  if (session.tmux_window) return `win:${session.tmux_window}`;
  return session.session_id;
}
