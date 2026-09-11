// Match the daemon's command grammar. Paths and multiline prompts stay ordinary messages.
export function isAgentCommand(text: string): boolean {
  return /^\/[a-z][a-z0-9_-]*(?:[ \t]+[^\r\n]*)?$/i.test(text.trim());
}
export function nativeAgentCommand(kind: string, text: string): string {
  return kind === "opencode" && text.trim() === "/model" ? "/models" : text.trim();
}
export function modelCommand(kind: string): string | undefined {
  if (kind === "opencode") return "/models";
  return ["claude-code", "codex", "antigravity", "hermes", "aider", "cursor"].includes(kind) ? "/model" : undefined;
}
