import { createSignal } from "solid-js";
export type AgentView = "terminal" | "conversation";
export const [agentViewDefaults, setAgentViewDefaults] = createSignal<Record<string, string>>({});
export function hasTranscriptSource(kind: string) { return kind === "claude-code" || kind === "codex"; }
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
