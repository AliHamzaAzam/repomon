import type { AgentChoice } from "../bindings";
import { daemonCall } from "./rpc";

/// Every spawnable agent choice, from `agent.detect`. Shared by `SpawnModal` and the home
/// screen's compose line so agent-kind picking behaves identically in both places.
export function fetchAgentChoices(): Promise<AgentChoice[]> {
  return daemonCall("agent.detect");
}

/// The choice a picker should preselect: the configured default if it's on PATH, else the first
/// detected choice, else whatever is first.
export function pickDefaultAgent(choices: AgentChoice[]): string {
  const preferred = choices.find((choice) => choice.default && choice.detected)
    ?? choices.find((choice) => choice.detected)
    ?? choices[0];
  return preferred?.name ?? "claude-code";
}

/// Agent kinds whose CLI takes a model override, with a short curated list of values worth
/// offering. `agent.spawn` accepts `model` for other kinds too (the daemon passes it through
/// verbatim); this only decides which kinds show a model picker, since offering one with no
/// known-good values would be worse than not offering it.
const MODEL_CHOICES: Record<string, string[]> = {
  "claude-code": ["opus", "sonnet", "haiku"],
  codex: ["gpt-5", "gpt-5-codex"],
};

export function modelChoicesFor(agentName: string): string[] {
  return MODEL_CHOICES[agentName] ?? [];
}
