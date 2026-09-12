import { describe, expect, it } from "vitest";
import { bareAlias, isAgentCommand, resolveCommand } from "./agentCommands";
import type { CommandCatalog } from "../bindings";

describe("agent commands", () => {
  it("distinguishes commands from paths and multiline prompts", () => {
    for (const text of ["/model", "/model opus", "/help", " /new "]) expect(isAgentCommand(text)).toBe(true);
    for (const text of ["/tmp/file.md", "Read /model", "/model\nAttached file: x", "/"]) expect(isAgentCommand(text)).toBe(false);
  });
});

describe("bareAlias", () => {
  it("strips a plugin namespace, leaving an unnamespaced name untouched", () => {
    expect(bareAlias("myplugin:review")).toBe("review");
    expect(bareAlias("compact")).toBe("compact");
  });
});

const catalog = (commands: CommandCatalog["commands"]): CommandCatalog => ({ commands, models: [], model_command: null, efforts: [], effort_command: null });

describe("resolveCommand", () => {
  it("sends a one_shot command as a fully specified line, arguments included, never opening the terminal", () => {
    const result = resolveCommand("/model sonnet", catalog([{ name: "model", description: "", source: "builtin", one_shot: true }]));
    expect(result).toEqual({ oneShotLine: "/model sonnet" });
  });

  it("matches a plugin command by its bare alias, not just its full namespaced name", () => {
    const result = resolveCommand("/review", catalog([{ name: "myplugin:review", description: "", source: "plugin", one_shot: true }]));
    expect(result).toEqual({ oneShotLine: "/review" });
  });

  it("falls back to the terminal route for a command the catalog marks one_shot: false", () => {
    const result = resolveCommand("/vim", catalog([{ name: "vim", description: "", source: "builtin", one_shot: false }]));
    expect(result).toEqual({ terminalText: "/vim" });
  });

  it("falls back to the terminal route for a command the catalog does not recognize at all - not sure is not safe", () => {
    const result = resolveCommand("/mystery", catalog([]));
    expect(result).toEqual({ terminalText: "/mystery" });
  });

  it("preserves the operator's own arguments verbatim in the terminal fallback", () => {
    const result = resolveCommand("/mystery --flag value", catalog([]));
    expect(result).toEqual({ terminalText: "/mystery --flag value" });
  });

  it("routes anything that is not command-shaped straight to the terminal fallback text as-is", () => {
    expect(resolveCommand("not a command", catalog([]))).toEqual({ terminalText: "not a command" });
  });
});
