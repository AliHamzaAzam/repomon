import { createSignal } from "solid-js";
export type AgentView = "terminal" | "conversation";
export const [agentViewDefaults, setAgentViewDefaults] = createSignal<Record<string, string>>({});
export function hasTranscriptSource(kind: string) { return kind === "claude-code" || kind === "codex"; }
export function resolveAgentView(override: string | null | undefined, kind: string | null | undefined, defaults: Record<string, string>): AgentView {
  const value = override ?? (kind ? defaults[kind] : undefined);
  return value === "conversation" ? "conversation" : "terminal";
}
