import { describe, expect, it } from "vitest";
import { hasTranscriptSource, resolveAgentView } from "./agentViews";
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
  it("enables transcript defaults only for daemon-supported kinds", () => {
    expect(["codex","claude-code"].every(hasTranscriptSource)).toBe(true);
    expect(["opencode","cursor","aider","custom"].some(hasTranscriptSource)).toBe(false);
  });
});
