import { describe, expect, it } from "vitest";
import { hasTranscriptSource, resolveAgentView, statusRowsFor } from "./agentViews";
describe("agent view resolution", () => {
  it("uses lane override, then kind default, then terminal, degrading unknown values", () => {
    const defaults = { codex:"conversation", "claude-code":"terminal" };
    expect(resolveAgentView("terminal", "codex", defaults)).toBe("terminal");
    expect(resolveAgentView("conversation", "opencode", defaults)).toBe("conversation");
    expect(resolveAgentView(null, "codex", defaults)).toBe("conversation");
    expect(resolveAgentView(undefined, "claude-code", defaults)).toBe("terminal");
    expect(resolveAgentView(undefined, "custom", defaults)).toBe("terminal");
    expect(resolveAgentView("unknown", "codex", defaults)).toBe("terminal");
    expect(resolveAgentView(null, null, defaults)).toBe("terminal");
  });
  it("enables transcript defaults for the daemon's four scanned SourceKind variants", () => {
    expect(["codex","claude-code","antigravity","opencode","hermes","aider"].every(hasTranscriptSource)).toBe(true);
    expect(["cursor","custom"].some(hasTranscriptSource)).toBe(false);
  });
});

it("lets detail choose status defaults, with an explicit per-kind override taking precedence", () => {
  expect(statusRowsFor("codex", "summary", {})).toEqual([]);
  expect(statusRowsFor("codex", "normal", {})).toEqual(["rate_limit", "usage_limit"]);
  expect(statusRowsFor("codex", "verbose", {})).toHaveLength(5);
  expect(statusRowsFor("codex", "verbose", {codex:[]})).toEqual([]);
  expect(statusRowsFor("codex", "summary", {codex:["turn_cost"]})).toEqual(["turn_cost"]);
});
