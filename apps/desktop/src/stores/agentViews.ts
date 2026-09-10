import { createSignal } from "solid-js";
export type AgentView = "terminal" | "conversation";
export const [agentViewDefaults, setAgentViewDefaults] = createSignal<Record<string, string>>({});
/// The daemon's four transcript-scanned SourceKind variants (Claude, Codex, Antigravity,
/// OpenCode). Every other kind, including Hermes, has no scanner and relies on the deliberate
/// terminal_block fallback built from agent.capture instead.
export function hasTranscriptSource(kind: string) {
  const raw = kind.toLowerCase().trim();
  return raw === "claude-code" || raw === "claude" || raw === "codex" || raw === "antigravity" || raw === "agy" || raw === "opencode";
}
export function agentKindDisplayName(agent?: string | null): string {
  const raw = agent?.toLowerCase().trim() ?? "";
  if (raw === "claude-code" || raw === "claude") return "Claude Code";
  if (raw === "antigravity" || raw === "agy") return "Antigravity";
  if (raw === "hermes" || raw === "hermes-agent") return "Hermes Agent";
  if (raw === "codex") return "Codex";
  if (raw === "opencode") return "OpenCode";
  if (raw === "cursor") return "Cursor";
  if (raw === "aider") return "Aider";
  if (!raw || raw === "unknown") return "Agent";
  return raw.charAt(0).toUpperCase() + raw.slice(1);
}
export function resolveAgentView(override: string | null | undefined, kind: string | null | undefined, defaults: Record<string, string>): AgentView {
  const value = override ?? (kind ? defaults[kind] : undefined);
  return value === "conversation" ? "conversation" : "terminal";
}

export const STATUS_ROWS = [
  ["turn_started", "Turn started"], ["turn_finished", "Turn finished"],
  ["turn_cost", "Turn cost"], ["rate_limit", "Rate limit"], ["usage_limit", "Usage limit"],
] as const;
export const [agentStatusRows, setAgentStatusRows] = createSignal<Record<string, string[]>>({});
export function statusRowsFor(kind: string, detail: string, overrides = agentStatusRows()): readonly string[] {
  return overrides[kind] ?? (detail === "verbose" ? STATUS_ROWS.map(([key]) => key) : detail === "summary" ? [] : ["rate_limit", "usage_limit"]);
}
