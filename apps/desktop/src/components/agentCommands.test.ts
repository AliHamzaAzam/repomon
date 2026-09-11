import { describe, expect, it } from "vitest";
import { isAgentCommand, nativeAgentCommand } from "./agentCommands";
describe("agent commands", () => {
  it("distinguishes commands from paths and multiline prompts", () => {
    for (const text of ["/model", "/model opus", "/help", " /new "]) expect(isAgentCommand(text)).toBe(true);
    for (const text of ["/tmp/file.md", "Read /model", "/model\nAttached file: x", "/"]) expect(isAgentCommand(text)).toBe(false);
  });
  it("uses OpenCode's native model command without rewriting its arguments", () => {
    expect(nativeAgentCommand("opencode", "/model")).toBe("/models");
    expect(nativeAgentCommand("codex", "/model")).toBe("/model");
    expect(nativeAgentCommand("opencode", "/model example")).toBe("/model example");
  });
});
